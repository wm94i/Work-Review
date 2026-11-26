//! 活动采集分类、合并回补与浏览器 URL 探测子模块。

pub(crate) mod browser_url;
pub(crate) mod merge;

pub(crate) use browser_url::*;
pub(crate) use merge::*;

use crate::entity_category_cache;
use work_review_core::config::AppConfig;
pub(crate) fn resolve_activity_classification(
    config: &AppConfig,
    app_name: &str,
    window_title: &str,
    browser_url: Option<&str>,
) -> work_review_core::activity_classifier::ActivityClassification {
    let mut base_category = work_review_core::categorize::categorize_collected_app_with_rules(
        &config.app_category_rules,
        app_name,
        window_title,
        &config.custom_categories,
    );
    // 分类被删除时回退到 "other"
    if base_category != "other"
        && !config
            .custom_categories
            .iter()
            .any(|c| c.key == base_category)
    {
        base_category = "other".to_string();
    }

    // ── 内置域名知识库:浏览器活动按站点内容细分基础分类 ──
    // 此前浏览器活动一律归 "browser",B 站和 GitHub 在统计里没有区别。
    // 用户显式规则仍优先:命中 app_category_rules 时 base 已不是 "browser",不会走到这里。
    let domain_knowledge =
        browser_url.and_then(work_review_core::knowledge::builtin_domain_category);
    if base_category == "browser" {
        if let Some((kb_base, _)) = domain_knowledge {
            if kb_base != "browser" && config.custom_categories.iter().any(|c| c.key == kb_base) {
                base_category = kb_base.to_string();
            }
        }
    }

    // ── AI 实体分类缓存:后台任务学习到的未知应用/域名归类(见 entity_classify_task) ──
    let mut cached_semantic: Option<String> = None;
    let cache_lookup = match base_category.as_str() {
        "other" => Some(format!("app:{}", app_name.trim().to_lowercase())),
        "browser" if domain_knowledge.is_none() => browser_url
            .map(work_review_core::config::PrivacyConfig::extract_domain)
            .map(|d| d.split(':').next().unwrap_or("").to_string())
            .filter(|d| !d.is_empty())
            .map(|d| format!("domain:{d}")),
        _ => None,
    };
    if let Some(cache_key) = cache_lookup {
        let cached = entity_category_cache()
            .read()
            .ok()
            .and_then(|m| m.get(&cache_key).cloned());
        if let Some((cached_base, semantic)) = cached {
            if config
                .custom_categories
                .iter()
                .any(|c| c.key == cached_base)
            {
                base_category = cached_base;
                cached_semantic = Some(semantic);
            }
        }
    }

    let mut classification =
        work_review_core::activity_classifier::classify_activity_with_base_category(
            app_name,
            window_title,
            browser_url,
            &base_category,
        );

    // 语义分类被删除时回退到 "未知活动"
    if classification.semantic_category != "未知活动"
        && !config
            .custom_semantic_categories
            .iter()
            .any(|c| c.key == classification.semantic_category)
    {
        classification.semantic_category = "未知活动".to_string();
    }

    // 知识库/AI 缓存的语义提示:仅在打分器置信度不高时兜底;
    // 用户 website_semantic_rules 在下方应用,仍拥有最终覆盖权
    let semantic_hint =
        cached_semantic.or_else(|| domain_knowledge.map(|(_, semantic)| semantic.to_string()));
    if let Some(hint) = semantic_hint {
        if classification.confidence < 70
            && config
                .custom_semantic_categories
                .iter()
                .any(|c| c.key == hint)
        {
            classification.semantic_category = hint;
            classification.confidence = 70;
        }
    }

    // 与网站统计、历史回填共用域名来源；没有地址栏 URL 时仍可按标题匹配规则。
    let browser_page_hint = if work_review_core::categorize::is_browser_app(app_name) {
        work_review_core::categorize::resolve_browser_page_hint(browser_url, window_title, None)
    } else {
        None
    };
    if let Some(semantic_category) = work_review_core::categorize::find_website_semantic_override(
        &config.website_semantic_rules,
        browser_page_hint.as_deref(),
    ) {
        classification.base_category =
            work_review_core::categorize::semantic_category_to_base_category(
                &semantic_category,
                &classification.base_category,
            );
        classification.semantic_category = semantic_category.clone();
        classification.confidence = classification.confidence.max(100);
        classification
            .evidence
            .push(format!("命中网站语义规则: {semantic_category}"));
    }

    classification
}
