use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use work_review_core::config::{
    RemoteStorageConfig, RemoteStorageProvider, S3Config, WebDavConfig,
};
use work_review_core::error::{AppError, Result};

type HmacSha256 = Hmac<Sha256>;

/// 上传专用 HTTP 客户端：**禁止自动跟随重定向**。
/// 默认策略下 301/302/303 会把 PUT 静默降级为 GET（丢弃请求体），服务器对
/// GET 返回 200,代码误判"上传成功"而远端并没有文件——这是 WebDAV 配置
/// http→https 跳转、域名迁移等场景下"配置了却不生效"的头号原因。
/// 主流客户端（rclone 等）的做法一致：遇到重定向直接报错,让用户改填最终地址。
static UPLOAD_CLIENT: OnceLock<Client> = OnceLock::new();

fn upload_client() -> Result<&'static Client> {
    if let Some(client) = UPLOAD_CLIENT.get() {
        return Ok(client);
    }
    let built = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| AppError::Unknown(format!("初始化上传客户端失败: {e}")))?;
    Ok(UPLOAD_CLIENT.get_or_init(|| built))
}

/// 网络抖动 / 5xx 瞬态失败自动重试一次（2 秒后），避免后台上传因一次抖动而丢失。
async fn send_with_one_retry(
    request: reqwest::RequestBuilder,
) -> std::result::Result<reqwest::Response, reqwest::Error> {
    let retry = request.try_clone();
    match request.send().await {
        Ok(resp) if resp.status().is_server_error() => match retry {
            Some(retry_request) => {
                log::warn!("远程上传返回 {}，2 秒后重试一次", resp.status());
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                retry_request.send().await
            }
            None => Ok(resp),
        },
        Ok(resp) => Ok(resp),
        Err(e) => match retry {
            Some(retry_request) => {
                log::warn!("远程上传请求失败（{e}），2 秒后重试一次");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                retry_request.send().await
            }
            None => Err(e),
        },
    }
}

/// 本进程内已确认存在的 WebDAV 目录缓存（MKCOL 成功或返回 405 视为已存在）。
/// 截图按天分目录，同一目录只需确认一次，不再每张截图逐级重发 MKCOL；
/// 若远端目录被外部删除，后续 PUT 会以 409 等状态失败并由补传机制兜底，
/// 进程重启后缓存自然重建。
static ENSURED_WEBDAV_DIRS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn ensured_webdav_dirs() -> &'static Mutex<HashSet<String>> {
    ENSURED_WEBDAV_DIRS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub async fn upload_screenshot(
    config: &RemoteStorageConfig,
    local_path: &Path,
    relative_path: &str,
) -> Result<String> {
    let client = upload_client()?;
    let file_bytes = tokio::fs::read(local_path)
        .await
        .map_err(|e| AppError::Screenshot(format!("读取截图文件失败: {e}")))?;

    match config.provider {
        RemoteStorageProvider::S3 => {
            upload_s3(client, &config.s3, &file_bytes, relative_path).await
        }
        RemoteStorageProvider::WebDav => {
            upload_webdav(client, &config.webdav, &file_bytes, relative_path).await
        }
        RemoteStorageProvider::None => Err(AppError::Config("远程存储未配置".into())),
    }
}

// --- S3 (MinIO compatible) with hand-crafted SigV4 ---

async fn upload_s3(
    client: &Client,
    config: &S3Config,
    file_bytes: &[u8],
    relative_path: &str,
) -> Result<String> {
    ensure_s3_endpoint_allowed(&config.endpoint)?;
    let endpoint = config.endpoint.trim_end_matches('/');
    let object_key = remote_object_path(&config.path_prefix, relative_path);

    let url = format!("{}/{}/{}", endpoint, config.bucket, object_key);
    let parsed =
        reqwest::Url::parse(&url).map_err(|e| AppError::Config(format!("S3 URL 解析失败: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Config("S3 endpoint 缺少 host".into()))?;
    let host_with_port = if let Some(port) = parsed.port() {
        format!("{host}:{port}")
    } else {
        host.to_string()
    };

    let now = chrono::Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();

    let payload_hash = hex::encode(Sha256::digest(file_bytes));
    // Content-Type 按扩展名推导；同时参与 SigV4 签名，签名与实际请求头必须一致
    let content_type = content_type_for_extension(&object_key);

    let canonical_uri = format!("/{}/{}", config.bucket, url_encode_path(&object_key));
    let canonical_querystring = "";

    let canonical_headers = format!(
        "content-type:{content_type}\nhost:{host_with_port}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date";

    let canonical_request = format!(
        "PUT\n{canonical_uri}\n{canonical_querystring}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let credential_scope = format!("{}/{}/s3/aws4_request", date_stamp, config.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date,
        credential_scope,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let signing_key = derive_signing_key(&config.secret_key, &date_stamp, &config.region, "s3");
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        config.access_key, credential_scope, signed_headers, signature
    );

    // SigV4 签名有 15 分钟有效窗口，2 秒后的重试可安全重放同一签名请求
    let request = client
        .put(&url)
        .header("Content-Type", content_type)
        .header("Host", &host_with_port)
        .header("x-amz-content-sha256", &payload_hash)
        .header("x-amz-date", &amz_date)
        .header("Authorization", &authorization)
        .body(file_bytes.to_vec());
    let resp = send_with_one_retry(request)
        .await
        .map_err(|e| AppError::Screenshot(format!("S3 PUT 请求失败: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let body_preview = body.chars().take(500).collect::<String>();
        return Err(AppError::Screenshot(format!(
            "S3 PUT 返回 {status}: {body_preview}"
        )));
    }

    let public_url = public_url_or_fallback(config.public_url_base.as_deref(), &object_key, &url);

    Ok(public_url)
}

fn derive_signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// 依据文件扩展名推导 Content-Type（截图上传目前只涉及 jpg/png）。
fn content_type_for_extension(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

fn url_encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            segment
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~'
                    {
                        String::from(b as char)
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

// --- WebDAV ---

/// 判断主机名是否为本机/内网地址（NAS 等本地部署允许走 http）。
fn is_private_or_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => v6.is_loopback(),
        };
    }
    false
}

/// 远程端点安全校验：远程端点必须使用 https，明文 http 仅允许本机/内网地址
/// （与模型端点策略一致），防止截图与凭据经明文链路发往远程服务器。
fn ensure_remote_endpoint_allowed(url: &str, service: &str) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let Some(rest) = lower.strip_prefix("http://") else {
        return Err(AppError::Config(format!(
            "{service} 地址必须以 http:// 或 https:// 开头"
        )));
    };

    // 提取主机名：截到 path/query/fragment 之前，剥离端口；IPv6 形如 [::1]:5005
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(inner) = host_port.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    if is_private_or_local_host(host) {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "远程 {service} 端点必须使用 https（本机/内网地址除外）"
    )))
}

fn ensure_s3_endpoint_allowed(url: &str) -> Result<()> {
    ensure_remote_endpoint_allowed(url, "S3")
}

fn ensure_webdav_endpoint_allowed(url: &str) -> Result<()> {
    ensure_remote_endpoint_allowed(url, "WebDAV")
}

async fn upload_webdav(
    client: &Client,
    config: &WebDavConfig,
    file_bytes: &[u8],
    relative_path: &str,
) -> Result<String> {
    ensure_webdav_endpoint_allowed(&config.url)?;
    let base = config.url.trim_end_matches('/');
    let object_path = remote_object_path(&config.path_prefix, relative_path);

    ensure_webdav_directories(
        client,
        base,
        &object_path,
        &config.username,
        &config.password,
    )
    .await?;

    let put_url = format!("{}/{}", base, object_path);
    let request = client
        .put(&put_url)
        .basic_auth(&config.username, Some(&config.password))
        .header("Content-Type", content_type_for_extension(&object_path))
        .body(file_bytes.to_vec());
    let resp = send_with_one_retry(request)
        .await
        .map_err(|e| AppError::Screenshot(format!("WebDAV PUT 失败: {e}")))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() && status != 201 && status != 204 {
        let body = resp.text().await.unwrap_or_default();
        let body_preview = body.chars().take(500).collect::<String>();
        return Err(AppError::Screenshot(format!(
            "WebDAV PUT 返回 {status}: {body_preview}"
        )));
    }

    let public_url =
        public_url_or_fallback(config.public_url_base.as_deref(), &object_path, &put_url);

    Ok(public_url)
}

pub const APP_CONFIG_OBJECT: &str = "work-review/app-config.json";

pub fn should_sync_app_config(config: &RemoteStorageConfig) -> bool {
    config.sync_app_config
        && config.provider == RemoteStorageProvider::WebDav
        && !config.webdav.url.trim().is_empty()
}

pub async fn push_app_config_bytes(config: &RemoteStorageConfig, bytes: Vec<u8>) -> Result<String> {
    upload_webdav(upload_client()?, &config.webdav, &bytes, APP_CONFIG_OBJECT).await
}

pub async fn pull_app_config_bytes(config: &RemoteStorageConfig) -> Result<Option<Vec<u8>>> {
    ensure_webdav_endpoint_allowed(&config.webdav.url)?;
    let base = config.webdav.url.trim_end_matches('/');
    let object_path = remote_object_path(&config.webdav.path_prefix, APP_CONFIG_OBJECT);
    let get_url = format!("{base}/{object_path}");
    let resp = upload_client()?
        .get(&get_url)
        .basic_auth(&config.webdav.username, Some(&config.webdav.password))
        .send()
        .await
        .map_err(|e| AppError::Unknown(format!("WebDAV GET 失败: {e}")))?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Unknown(format!(
            "WebDAV GET 返回 {status}: {}",
            body.chars().take(300).collect::<String>()
        )));
    }
    Ok(Some(
        resp.bytes()
            .await
            .map_err(|e| AppError::Unknown(format!("读取 WebDAV 配置失败: {e}")))?
            .to_vec(),
    ))
}

/// 逐级确保远程目录存在（带进程内缓存，已确认过的目录不再重复 MKCOL）。
async fn ensure_webdav_directories(
    client: &Client,
    base_url: &str,
    object_path: &str,
    username: &str,
    password: &str,
) -> Result<()> {
    let parts: Vec<&str> = object_path.split('/').collect();
    let dir_parts = &parts[..parts.len().saturating_sub(1)];

    let mut current = String::new();
    for part in dir_parts {
        if part.is_empty() {
            continue;
        }
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);

        let cache_key = format!("{}/{}", base_url, current);
        {
            let ensured = ensured_webdav_dirs()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if ensured.contains(&cache_key) {
                continue;
            }
        }

        let mkcol_url = format!("{}/{}/", base_url, current);
        let mkcol_method = reqwest::Method::from_bytes(b"MKCOL")
            .map_err(|e| AppError::Screenshot(format!("MKCOL method: {e}")))?;

        match client
            .request(mkcol_method, &mkcol_url)
            .basic_auth(username, Some(password))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() || r.status().as_u16() == 405 => {
                ensured_webdav_dirs()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(cache_key);
            }
            Ok(r) => log::debug!("MKCOL {} 返回 {}", mkcol_url, r.status()),
            Err(e) => log::debug!("MKCOL {mkcol_url} 失败: {e}"),
        }
    }
    Ok(())
}

fn remote_object_path(prefix: &str, relative_path: &str) -> String {
    let relative_path = relative_path.replace('\\', "/");
    let prefix = prefix.trim().trim_matches('/');
    if prefix.is_empty() {
        relative_path
    } else {
        format!("{prefix}/{relative_path}")
    }
}

fn public_url_or_fallback(base_url: Option<&str>, object_path: &str, fallback: &str) -> String {
    let Some(base_url) = base_url
        .map(str::trim)
        .filter(|base_url| !base_url.is_empty())
    else {
        return fallback.to_string();
    };
    format!("{}/{}", base_url.trim_end_matches('/'), object_path)
}

#[cfg(test)]
mod tests {
    use super::{
        content_type_for_extension, ensure_s3_endpoint_allowed, ensure_webdav_endpoint_allowed,
        public_url_or_fallback, remote_object_path,
    };

    #[test]
    fn 内容类型应按扩展名推导() {
        assert_eq!(content_type_for_extension("a/b/shot.jpg"), "image/jpeg");
        assert_eq!(content_type_for_extension("shot.JPEG"), "image/jpeg");
        assert_eq!(content_type_for_extension("shot.png"), "image/png");
        assert_eq!(
            content_type_for_extension("work-review/app-config.json"),
            "application/json"
        );
        assert_eq!(
            content_type_for_extension("shot.webp"),
            "application/octet-stream"
        );
        assert_eq!(
            content_type_for_extension("noext"),
            "application/octet-stream"
        );
    }

    #[test]
    fn webdav端点应拒绝远程明文http仅放行本机内网() {
        assert!(ensure_webdav_endpoint_allowed("https://dav.example.com/dav").is_ok());
        assert!(ensure_webdav_endpoint_allowed("http://localhost:5005/dav").is_ok());
        assert!(ensure_webdav_endpoint_allowed("http://127.0.0.1/dav").is_ok());
        assert!(ensure_webdav_endpoint_allowed("http://[::1]:5005/dav").is_ok());
        assert!(ensure_webdav_endpoint_allowed("http://192.168.1.20:5005/dav").is_ok());
        assert!(ensure_webdav_endpoint_allowed("http://10.0.0.8/dav").is_ok());
        assert!(ensure_webdav_endpoint_allowed("http://172.16.0.2/dav").is_ok());

        assert!(ensure_webdav_endpoint_allowed("http://dav.example.com/dav").is_err());
        assert!(ensure_webdav_endpoint_allowed("http://8.8.8.8/dav").is_err());
        assert!(ensure_webdav_endpoint_allowed("ftp://dav.example.com").is_err());
    }

    #[test]
    fn s3端点应拒绝远程明文http仅放行本机内网() {
        assert!(ensure_s3_endpoint_allowed("https://s3.example.com").is_ok());
        assert!(ensure_s3_endpoint_allowed("http://localhost:9000").is_ok());
        assert!(ensure_s3_endpoint_allowed("http://192.168.1.20:9000").is_ok());
        assert!(ensure_s3_endpoint_allowed("http://s3.example.com").is_err());
        assert!(ensure_s3_endpoint_allowed("ftp://s3.example.com").is_err());
    }

    #[test]
    fn 远程对象路径应包含路径前缀并统一分隔符() {
        assert_eq!(
            remote_object_path(" workreview/ ", r"screenshots\2026-05-22\shot.jpg"),
            "workreview/screenshots/2026-05-22/shot.jpg"
        );
        assert_eq!(
            remote_object_path("", "screenshots/2026-05-22/shot.jpg"),
            "screenshots/2026-05-22/shot.jpg"
        );
    }

    #[test]
    fn 公开访问地址应使用远程对象路径并忽略空前缀() {
        assert_eq!(
            public_url_or_fallback(
                Some(" https://cdn.example.com/workreview/ "),
                "archive/screenshots/shot.jpg",
                "https://webdav.example.com/archive/screenshots/shot.jpg",
            ),
            "https://cdn.example.com/workreview/archive/screenshots/shot.jpg"
        );
        assert_eq!(
            public_url_or_fallback(
                Some("   "),
                "archive/screenshots/shot.jpg",
                "https://webdav.example.com/archive/screenshots/shot.jpg",
            ),
            "https://webdav.example.com/archive/screenshots/shot.jpg"
        );
    }
}
