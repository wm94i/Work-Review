//! 未知应用与域名的后台 AI 实体分类学习与进程内缓存。

use crate::AppState;
use once_cell::sync::OnceCell;
use std::sync::{Arc, Mutex};
/// AI 实体分类缓存的进程内查询表（entity_key → (基础分类, 语义分类)）。
/// 启动时从数据库载入,entity_classify_task 学习到新实体后同步更新,
/// 采集循环里的 resolve_activity_classification 只读查询。
pub(crate) fn entity_category_cache(
) -> &'static std::sync::RwLock<std::collections::HashMap<String, (String, String)>> {
    static CACHE: OnceCell<std::sync::RwLock<std::collections::HashMap<String, (String, String)>>> =
        OnceCell::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn build_entity_classify_request_body(
    model: &work_review_core::config::ModelConfig,
    prompt: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model.model,
        "messages": [{ "role": "user", "content": prompt }],
        "stream": false,
    });
    if work_review_core::generation_params::uses_openai_reasoning_chat_params(
        model.provider,
        &model.model,
    ) {
        body["reasoning_effort"] = serde_json::json!("low");
    } else {
        body["temperature"] = serde_json::json!(0);
    }
    body
}

/// 调用文本模型对一批实体做归类,返回 (entity_key, 基础分类, 语义分类)。
pub(crate) async fn classify_entities_with_model(
    model: &work_review_core::config::ModelConfig,
    entities: &[String],
    category_options: &str,
    semantic_options: &str,
) -> Option<Vec<(String, String, String)>> {
    let list = entities
        .iter()
        .map(|e| format!("- {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "你是活动分类助手。以下实体来自个人时间追踪软件,格式为 kind:名称(app 表示桌面应用,domain 表示网站域名)。\n\
         请为每个实体选择最贴切的基础分类 key 与语义分类名,只能从给定选项中选。\n\
         判定必须保守:只有名称有明确证据(明显是视频/游戏/音乐/购物等)才可归入娱乐或消遣类分类;\n\
         看起来像开发工具、软件项目名、专有名词或含义不明的实体,宁可跳过也不要猜——\n\
         对没有把握的实体,base 填 \"skip\",本轮不学习该实体。\n\n\
         基础分类(key: 名称)：{category_options}\n语义分类：{semantic_options}\n\n实体：\n{list}\n\n\
         只输出 JSON 数组,不要任何其他文字。元素形如 {{\"key\":\"app:xxx\",\"base\":\"development\",\"semantic\":\"编码开发\"}}"
    );

    let endpoint = model.endpoint.trim().trim_end_matches('/');
    let url = if endpoint.ends_with("/chat/completions") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/chat/completions")
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let body = build_entity_classify_request_body(model, &prompt);
    let mut request = client.post(&url).json(&body);
    if let Some(key) = model.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {key}"));
    }

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        log::warn!("实体分类模型调用失败: HTTP {}", response.status());
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    let content = value["choices"][0]["message"]["content"].as_str()?;
    let content = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Vec<serde_json::Value> = serde_json::from_str(content).ok()?;
    Some(
        parsed
            .into_iter()
            .filter_map(|item| {
                Some((
                    item["key"].as_str()?.to_string(),
                    item["base"].as_str()?.to_string(),
                    item["semantic"].as_str()?.to_string(),
                ))
            })
            .collect(),
    )
}

/// AI 实体分类学习任务：定期把"仍未识别"的应用与域名交给文本模型归类一次,
/// 写入缓存供采集即时查询,并回溯修正近 14 天的历史记录——分类随使用越来越准,
/// 不再依赖用户手动加规则。未配置文本模型时静默空转;内置知识库与用户显式规则始终优先
/// （候选收集阶段已排除两者覆盖的实体）。
pub(crate) async fn entity_classify_task(state: Arc<Mutex<AppState>>) {
    const INITIAL_DELAY_SECS: u64 = 180;
    const SWEEP_INTERVAL_SECS: u64 = 30 * 60;
    const LOOKBACK_SECS: i64 = 3 * 24 * 60 * 60;
    const RETRO_SECS: i64 = 14 * 24 * 60 * 60;
    const MIN_TOTAL_DURATION_SECS: i64 = 600;
    const BATCH_LIMIT: usize = 12;

    tokio::time::sleep(tokio::time::Duration::from_secs(INITIAL_DELAY_SECS)).await;

    loop {
        let now = chrono::Local::now().timestamp();
        let (
            model,
            category_options,
            semantic_options,
            valid_bases,
            valid_semantics,
            app_rule_names,
            website_rule_domains,
            app_candidates,
            url_durations,
        ) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            let model = guard
                .config
                .resolve_model_for_task(work_review_core::config::AiTaskKind::Classification);
            let category_options = guard
                .config
                .custom_categories
                .iter()
                .filter(|c| c.key != "browser")
                .map(|c| format!("{}: {}", c.key, c.name))
                .collect::<Vec<_>>()
                .join("、");
            let semantic_options = guard
                .config
                .custom_semantic_categories
                .iter()
                .map(|c| c.key.clone())
                .collect::<Vec<_>>()
                .join("、");
            let valid_bases: std::collections::HashSet<String> = guard
                .config
                .custom_categories
                .iter()
                .map(|c| c.key.clone())
                .collect();
            let valid_semantics: std::collections::HashSet<String> = guard
                .config
                .custom_semantic_categories
                .iter()
                .map(|c| c.key.clone())
                .collect();
            let app_rule_names: Vec<String> = guard
                .config
                .app_category_rules
                .iter()
                .map(|r| r.app_name.to_lowercase())
                .filter(|r| !r.is_empty())
                .collect();
            let website_rule_domains: Vec<String> = guard
                .config
                .website_semantic_rules
                .iter()
                .map(|r| r.domain.to_lowercase())
                .filter(|r| !r.is_empty())
                .collect();
            let app_candidates = guard
                .database
                .get_unclassified_app_candidates(now - LOOKBACK_SECS, MIN_TOTAL_DURATION_SECS, 20)
                .unwrap_or_default();
            let url_durations = guard
                .database
                .get_browser_url_durations(now - LOOKBACK_SECS)
                .unwrap_or_default();
            (
                model,
                category_options,
                semantic_options,
                valid_bases,
                valid_semantics,
                app_rule_names,
                website_rule_domains,
                app_candidates,
                url_durations,
            )
        };

        if model.endpoint.trim().is_empty() || model.model.trim().is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
            continue;
        }

        let cached_keys: std::collections::HashSet<String> = entity_category_cache()
            .read()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        // 应用候选:排除已学过与用户规则已覆盖的
        let mut entities: Vec<String> = Vec::new();
        for app in app_candidates {
            let app_lower = app.to_lowercase();
            let key = format!("app:{app_lower}");
            if cached_keys.contains(&key)
                || app_rule_names
                    .iter()
                    .any(|r| app_lower.contains(r.as_str()))
            {
                continue;
            }
            entities.push(key);
            if entities.len() >= BATCH_LIMIT {
                break;
            }
        }

        // 域名候选:聚合累计时长,排除知识库/用户规则/已学过的
        let mut domain_totals: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for (url, duration) in url_durations {
            let domain = work_review_core::config::PrivacyConfig::extract_domain(&url);
            let domain = domain.split(':').next().unwrap_or("").to_string();
            if !domain.is_empty() {
                *domain_totals.entry(domain).or_insert(0) += duration;
            }
        }
        let mut domain_candidates: Vec<(String, i64)> = domain_totals
            .into_iter()
            .filter(|(domain, total)| {
                *total >= MIN_TOTAL_DURATION_SECS
                    && work_review_core::knowledge::builtin_domain_category(domain).is_none()
                    && !cached_keys.contains(&format!("domain:{domain}"))
                    && !website_rule_domains.iter().any(|rule| {
                        work_review_core::config::PrivacyConfig::domain_matches(domain, rule)
                    })
            })
            .collect();
        domain_candidates.sort_by_key(|item| std::cmp::Reverse(item.1));
        for (domain, _) in domain_candidates {
            if entities.len() >= BATCH_LIMIT {
                break;
            }
            entities.push(format!("domain:{domain}"));
        }

        if entities.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
            continue;
        }

        let Some(results) =
            classify_entities_with_model(&model, &entities, &category_options, &semantic_options)
                .await
        else {
            log::warn!("实体自动归类本轮失败,下轮重试");
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
            continue;
        };

        // 一轮模型结果只读取一次近 14 天浏览器 URL，避免每个域名重复扫描同一批历史。
        let retro_domain_rows = if results.iter().any(|(key, _, _)| key.starts_with("domain:")) {
            let database = {
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.database.clone()
            };
            database
                .get_browser_activity_urls(now - RETRO_SECS)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut learned = 0usize;
        for (key, base, semantic) in results {
            // 模型对没把握的实体返回 "skip":不学习、不缓存,留待将来有更多上下文再判
            if base == "skip" {
                continue;
            }
            // 只接受本批实体、且分类必须在现有集合内("browser" 不作为学习目标)
            if !entities.contains(&key)
                || base == "browser"
                || !valid_bases.contains(&base)
                || !valid_semantics.contains(&semantic)
            {
                continue;
            }

            let database = {
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.database.clone()
            };
            let persisted = database.upsert_entity_category(&key, &base, &semantic);
            if let Err(e) = persisted {
                log::warn!("写入实体分类缓存失败({key}): {e}");
                continue;
            }
            if let Ok(mut map) = entity_category_cache().write() {
                map.insert(key.clone(), (base.clone(), semantic.clone()));
            }

            // 回溯修正近 14 天历史记录,让统计"自己变准"
            let database = {
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.database.clone()
            };
            let updated = if let Some(app) = key.strip_prefix("app:") {
                database
                    .reclassify_app_activities(app, &base, &semantic, now - RETRO_SECS)
                    .unwrap_or(0)
            } else if let Some(domain) = key.strip_prefix("domain:") {
                let ids: Vec<i64> = retro_domain_rows
                    .iter()
                    .filter(|(_, url)| {
                        let row_domain =
                            work_review_core::config::PrivacyConfig::extract_domain(url);
                        row_domain.split(':').next().unwrap_or("") == domain
                    })
                    .map(|(id, _)| *id)
                    .collect();
                database
                    .reclassify_activities_by_ids(&ids, &base, &semantic)
                    .unwrap_or(0)
            } else {
                0
            };

            learned += 1;
            log::info!("实体自动归类: {key} → {base}/{semantic}（回溯修正 {updated} 条）");
        }
        if learned > 0 {
            log::info!("实体分类自动学习完成: 本轮学习 {learned} 个实体");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::build_entity_classify_request_body;
    use work_review_core::config::{AiProvider, ModelConfig};

    #[test]
    fn 实体分类载荷对openai推理模型省略temperature并设置低思考深度() {
        let mut cfg = ModelConfig::default_text();
        cfg.provider = AiProvider::OpenAI;
        cfg.model = "gpt-6.2-sol".to_string();
        let body = build_entity_classify_request_body(&cfg, "classify");
        assert!(body.get("temperature").is_none());
        assert_eq!(body["reasoning_effort"], "low");

        cfg.provider = AiProvider::DeepSeek;
        cfg.model = "deepseek-flash".to_string();
        let compat = build_entity_classify_request_body(&cfg, "classify");
        assert_eq!(compat["temperature"], 0);
        assert!(compat.get("reasoning_effort").is_none());
    }
}
