use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::path::Path;
use work_review_core::config::{RemoteStorageConfig, RemoteStorageProvider, S3Config, WebDavConfig};
use work_review_core::error::{AppError, Result};

type HmacSha256 = Hmac<Sha256>;

pub async fn upload_screenshot(
    client: &Client,
    config: &RemoteStorageConfig,
    local_path: &Path,
    relative_path: &str,
) -> Result<String> {
    let file_bytes = tokio::fs::read(local_path)
        .await
        .map_err(|e| AppError::Screenshot(format!("读取截图文件失败: {e}")))?;

    match config.provider {
        RemoteStorageProvider::S3 => upload_s3(client, &config.s3, &file_bytes, relative_path).await,
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
    let endpoint = config.endpoint.trim_end_matches('/');
    let object_key = remote_object_path(&config.path_prefix, relative_path);

    let url = format!("{}/{}/{}", endpoint, &config.bucket, &object_key);
    let parsed = reqwest::Url::parse(&url)
        .map_err(|e| AppError::Config(format!("S3 URL 解析失败: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Config("S3 endpoint 缺少 host".into()))?;
    let host_with_port = if let Some(port) = parsed.port() {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    let now = chrono::Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();

    let payload_hash = hex::encode(Sha256::digest(file_bytes));

    let canonical_uri = format!(
        "/{}/{}",
        &config.bucket,
        url_encode_path(&object_key)
    );
    let canonical_querystring = "";

    let canonical_headers = format!(
        "content-type:image/jpeg\nhost:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
        host_with_port, payload_hash, amz_date
    );
    let signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date";

    let canonical_request = format!(
        "PUT\n{}\n{}\n{}\n{}\n{}",
        canonical_uri, canonical_querystring, canonical_headers, signed_headers, payload_hash
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

    let resp = client
        .put(&url)
        .header("Content-Type", "image/jpeg")
        .header("Host", &host_with_port)
        .header("x-amz-content-sha256", &payload_hash)
        .header("x-amz-date", &amz_date)
        .header("Authorization", &authorization)
        .body(file_bytes.to_vec())
        .send()
        .await
        .map_err(|e| AppError::Screenshot(format!("S3 PUT 请求失败: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Screenshot(format!(
            "S3 PUT 返回 {}: {}",
            status,
            &body[..body.len().min(500)]
        )));
    }

    let public_url = public_url_or_fallback(config.public_url_base.as_deref(), &object_key, &url);

    Ok(public_url)
}

fn derive_signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
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
                        format!("%{:02X}", b)
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

// --- WebDAV ---

async fn upload_webdav(
    client: &Client,
    config: &WebDavConfig,
    file_bytes: &[u8],
    relative_path: &str,
) -> Result<String> {
    let base = config.url.trim_end_matches('/');
    let object_path = remote_object_path(&config.path_prefix, relative_path);

    ensure_webdav_directories(client, base, &object_path, &config.username, &config.password)
        .await?;

    let put_url = format!("{}/{}", base, &object_path);
    let resp = client
        .put(&put_url)
        .basic_auth(&config.username, Some(&config.password))
        .header("Content-Type", "image/jpeg")
        .body(file_bytes.to_vec())
        .send()
        .await
        .map_err(|e| AppError::Screenshot(format!("WebDAV PUT 失败: {e}")))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() && status != 201 && status != 204 {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Screenshot(format!(
            "WebDAV PUT 返回 {}: {}",
            status,
            &body[..body.len().min(500)]
        )));
    }

    let public_url = public_url_or_fallback(config.public_url_base.as_deref(), &object_path, &put_url);

    Ok(public_url)
}

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

        let mkcol_url = format!("{}/{}/", base_url, &current);
        let mkcol_method = reqwest::Method::from_bytes(b"MKCOL")
            .map_err(|e| AppError::Screenshot(format!("MKCOL method: {e}")))?;

        match client
            .request(mkcol_method, &mkcol_url)
            .basic_auth(username, Some(password))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() || r.status().as_u16() == 405 => {}
            Ok(r) => log::debug!("MKCOL {} 返回 {}", mkcol_url, r.status()),
            Err(e) => log::debug!("MKCOL {} 失败: {}", mkcol_url, e),
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
        format!("{}/{}", prefix, relative_path)
    }
}

fn public_url_or_fallback(base_url: Option<&str>, object_path: &str, fallback: &str) -> String {
    let Some(base_url) = base_url.map(str::trim).filter(|base_url| !base_url.is_empty()) else {
        return fallback.to_string();
    };
    format!("{}/{}", base_url.trim_end_matches('/'), object_path)
}

#[cfg(test)]
mod tests {
    use super::{public_url_or_fallback, remote_object_path};

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
