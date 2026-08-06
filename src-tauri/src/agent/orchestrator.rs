//! Stage 5: Orchestrator — Agent 的"指挥官"
//!
//! 路由决策：简单 → FastPath，复杂 → AgentPath
//! 降级策略：Agent 失败 → FastPath → FallbackPath
//!
//! 对应 Python: 05_orchestrator.py 里的 Orchestrator 类

use super::events::{StreamEvent, StreamEventSender};
use super::executor::{AgentExecutor, AgentRunError};
use super::model::Message;
use crate::config::ModelConfig;
use crate::database::Database;
use crate::error::AppError;
use work_review_core::database::MemorySearchItem;

// ══════════════════════════════════════════════════════════
// 路径类型
// ══════════════════════════════════════════════════════════

/// 查询路径
#[derive(Debug, Clone, PartialEq)]
pub enum QueryPath {
    /// 直接回答（闲聊/求助）
    Direct,
    /// 规则快速路径（简单时间查询）
    Fast,
    /// Agent 循环（复杂查询）
    Agent,
    /// 无模型兜底（模板回答）
    Fallback,
}

/// 路由决策结果
#[derive(Debug)]
pub struct RouteDecision {
    pub path: QueryPath,
    #[allow(dead_code)] // 路由原因：保留用于调试输出，业务逻辑未读取
    pub reason: String,
}

// ══════════════════════════════════════════════════════════
// 路由决策函数
// ══════════════════════════════════════════════════════════

/// 路由决策 — 根据问题内容判断走哪条路径
///
/// 对应 Python: route_query()
/// 面试核心：这个函数决定了每个请求的命运。
/// 规则越简单越好——复杂的判断交给 Agent 自己做。
pub fn route_query(question: &str, has_model: bool) -> RouteDecision {
    let q = question.trim().to_lowercase();

    // ── 有模型 → 始终交给 Agent（相信模型）──
    // 避免身份、问候和能力类问题被固定模板提前截断，确保使用当前选中的模型配置。
    if has_model {
        return RouteDecision {
            path: QueryPath::Agent,
            reason: "交给模型判断意图".to_string(),
        };
    }

    // ── 无模型：闲聊 / 身份 / 能力问答 → 直接回答 ──
    // 基础模板模式仍提供可用的固定回答，避免落入无法使用模型的兜底提示。
    let greetings = [
        "你好",
        "嗨",
        "hello",
        "hi",
        "你能做什么",
        "帮助",
        "help",
        // 身份类（issue 截图："你是谁"被误判）
        "你是谁",
        "你是什么",
        "你是干",
        "你叫什么",
        "介绍",
        "who are you",
        "what are you",
        // 能力类
        "你能干",
        "你能帮我",
        "你会什么",
        "你能做啥",
        "你能做什么",
        "what can you do",
    ];
    if greetings.iter().any(|g| q.contains(g)) && q.len() < 30 {
        return RouteDecision {
            path: QueryPath::Direct,
            reason: "身份/问候/能力问答".to_string(),
        };
    }

    // ── 无模型（基础模板模式）→ 完整覆盖工作查询的统计模板 ──
    // 没有模型可用，只能用规则兜底。fast_answer 对任何带时间范围的工作查询都给统一统计
    // （活动总览 / 分类分布 / Top 应用 / 相关记录），所以这里尽量放宽触发，让"我这周主要做了什么"
    // "今天怎么样""最近忙啥"等工作查询都能拿到统计；仅明显非工作领域（天气/股票/新闻…）放行到 Fallback。
    let non_work_signals = [
        "天气",
        "股票",
        "新闻",
        "笑话",
        "写诗",
        "算命",
        "星座",
        "汇率",
        "翻译成",
    ];
    if non_work_signals.iter().any(|p| q.contains(p)) {
        return RouteDecision {
            path: QueryPath::Fallback,
            reason: "无模型且明显非工作领域，模板兜底".to_string(),
        };
    }
    let work_signals = [
        // 时间词（工作查询常带）
        "今天",
        "昨天",
        "前天",
        "本周",
        "这周",
        "上周",
        "本月",
        "这个月",
        "上月",
        "上个月",
        "最近",
        "这几天",
        "近期",
        // 「做/干/搞」家族
        "做了什么",
        "做了哪些",
        "主要做了",
        "干了什么",
        "搞了什么",
        // 「忙」家族
        "忙什么",
        "忙啥",
        "忙不忙",
        "忙吗",
        // 「总结/小结」家族
        "总结",
        "小结",
        "汇总",
        // 工作通用
        "工作",
        "记录",
        "待办",
        "进度",
        "进展",
        "回顾",
        "复盘",
        "整理",
        // 「数据/情况/概览」家族
        "数据",
        "情况",
        "概况",
        "概览",
        "报告",
        "汇报",
        // 「专注/产出」家族
        "效率",
        "专注",
        "产出",
        "饱和",
        "摸鱼",
        "下班",
        "打卡",
        "休息",
        // 「app/软件/程序」家族
        "应用",
        "软件",
        "程序",
        "工具",
        "分类",
        "占比",
        "比例",
        // 「时长/多久」家族
        "时长",
        "时间",
        "多久",
        "小时",
        "多长",
        // 「会议」家族
        "会话",
        "开会",
        "会议",
        "session",
        // 统计通用
        "统计",
        // 「干嘛」家族
        "干嘛",
        "干什么",
    ];
    if work_signals.iter().any(|p| q.contains(p)) {
        return RouteDecision {
            path: QueryPath::Fast,
            reason: "无模型，工作查询走统计模板".to_string(),
        };
    }

    RouteDecision {
        path: QueryPath::Fallback,
        reason: "无模型，模板兜底".to_string(),
    }
}

// ══════════════════════════════════════════════════════════
// Orchestrator 结构体
// ══════════════════════════════════════════════════════════

/// Orchestrator 的处理结果
#[derive(Debug)]
pub struct OrchestratorResult {
    pub answer: String,
    pub used_ai: bool,
    pub tool_labels: Vec<String>,
    /// 工具执行收集的引用记录（Agent 路径来自 executor，其它路径为空）
    pub references: Vec<MemorySearchItem>,
}

/// Orchestrator — Agent 的"指挥官"
///
/// 把 Stage 1-4 的组件组装起来，加上路由决策。
pub struct Orchestrator;

impl Orchestrator {
    /// 处理用户请求的总入口
    ///
    /// 对应 Python: Orchestrator.handle()
    #[allow(clippy::too_many_arguments)]
    pub async fn handle(
        question: &str,
        model_config: Option<&ModelConfig>,
        database: &Database,
        history: &[Message],
        system_prompt: Option<&str>,
        ignored_apps: &[String],
        excluded_domains: &[String],
        web_tools: Option<super::tools::WebToolsConfig>,
        runtime: super::tools::AssistantRuntime,
        event_tx: Option<StreamEventSender>,
        timeouts: super::model::ModelTimeouts,
    ) -> Result<OrchestratorResult, AppError> {
        let has_model = model_config
            .map(|c| !c.endpoint.trim().is_empty() && !c.model.trim().is_empty())
            .unwrap_or(false);

        // ① 路由决策
        let decision = route_query(question, has_model);
        ensure_event_receiver_open(&event_tx)?;

        // ② 执行对应路径
        match decision.path {
            QueryPath::Direct => {
                let answer = direct_answer(question);
                let tool_labels = vec!["direct".to_string()];
                emit_done(&event_tx, &answer, &[], &tool_labels).await?;
                Ok(OrchestratorResult {
                    answer,
                    used_ai: false,
                    tool_labels,
                    references: vec![],
                })
            }

            QueryPath::Fast => {
                // FastPath：用规则查数据 + 简单格式化
                let answer = fast_answer(question, database, ignored_apps, excluded_domains)?;
                let tool_labels = vec!["rule-based".to_string()];
                emit_done(&event_tx, &answer, &[], &tool_labels).await?;
                Ok(OrchestratorResult {
                    answer,
                    used_ai: false,
                    tool_labels,
                    references: vec![],
                })
            }

            QueryPath::Agent => {
                let config = model_config
                    .ok_or_else(|| AppError::Analysis("Agent 路径需要模型配置".to_string()))?;

                // AgentPath：调用 Stage 3 的 AgentExecutor（透传事件通道）
                match AgentExecutor::run(
                    question,
                    config,
                    database,
                    system_prompt,
                    history,
                    None,
                    ignored_apps.to_vec(),
                    excluded_domains.to_vec(),
                    web_tools,
                    runtime,
                    event_tx.clone(),
                    timeouts,
                )
                .await
                {
                    Ok(agent_result) => {
                        // Agent 内部各 return 点已 emit Done，此处不重复（避免双 Done）。
                        Ok(OrchestratorResult {
                            answer: agent_result.answer,
                            used_ai: true,
                            tool_labels: agent_result.tool_labels,
                            references: agent_result.references,
                        })
                    }
                    Err(AgentRunError::EventReceiverClosed) => Err(event_receiver_closed_error()),
                    Err(AgentRunError::EventDeliveryFailed(message)) => {
                        Err(event_delivery_error(message))
                    }
                    Err(AgentRunError::Execution(e)) => {
                        // Agent 失败 → 降级到 FastPath。
                        // 降级必须对用户可见（此前静默换成规则模板，用户以为 AI 在回答）：
                        // 前置一行说明失败类别，完整错误进日志。
                        log::warn!("Agent 路径失败，降级到本地统计: {e}");
                        ensure_event_receiver_open(&event_tx)?;
                        let reason = degrade_reason_summary(&e.to_string());
                        let fast = fast_answer(question, database, ignored_apps, excluded_domains)?;
                        let answer = format!(
                            "⚠️ AI 模型调用失败（{reason}），已切换为本地统计模式。可稍后重试或到「设置 → AI 模型」检查配置。\n\n{fast}"
                        );
                        let tool_labels = vec!["降级查询".to_string()];
                        emit_done(&event_tx, &answer, &[], &tool_labels).await?;
                        Ok(OrchestratorResult {
                            answer,
                            used_ai: false,
                            tool_labels,
                            references: vec![],
                        })
                    }
                }
            }

            QueryPath::Fallback => {
                let answer = fallback_answer(question);
                let tool_labels = vec!["fallback".to_string()];
                emit_done(&event_tx, &answer, &[], &tool_labels).await?;
                Ok(OrchestratorResult {
                    answer,
                    used_ai: false,
                    tool_labels,
                    references: vec![],
                })
            }
        }
    }
}

/// 把内部错误串归类成一句用户能理解的失败原因（不泄漏细节，细节进日志）。
fn degrade_reason_summary(error: &str) -> &'static str {
    let lower = error.to_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") || error.contains("超时") {
        "请求超时"
    } else if lower.contains("401") || lower.contains("unauthorized") || lower.contains("api key") {
        "鉴权失败，请检查 API Key"
    } else if lower.contains("429") || lower.contains("rate") {
        "请求频率受限"
    } else if lower.contains("connect") || lower.contains("dns") || lower.contains("network") {
        "网络连接失败"
    } else {
        "服务暂不可用"
    }
}

fn event_receiver_closed_error() -> AppError {
    AppError::Analysis("Agent 事件接收端已关闭，已停止后续处理".to_string())
}

fn event_delivery_error(message: String) -> AppError {
    AppError::Analysis(format!("Agent 事件投递失败，已停止后续处理: {message}"))
}

fn ensure_event_receiver_open(tx: &Option<StreamEventSender>) -> Result<(), AppError> {
    if tx.as_ref().is_some_and(StreamEventSender::is_closed) {
        Err(event_receiver_closed_error())
    } else {
        Ok(())
    }
}

/// 推送终态 Done 事件：必须等 Tauri Channel 确认成功，失败时停止当前流程。
async fn emit_done(
    tx: &Option<StreamEventSender>,
    answer: &str,
    references: &[MemorySearchItem],
    tool_labels: &[String],
) -> Result<(), AppError> {
    if let Some(tx) = tx {
        tx.send_control(StreamEvent::Done {
            answer: answer.to_string(),
            references: references.to_vec(),
            tool_labels: tool_labels.to_vec(),
        })
        .await
        .map_err(event_delivery_error)?;
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════
// 各路径的实现
// ══════════════════════════════════════════════════════════

/// DirectPath：直接回答
pub fn direct_answer(question: &str) -> String {
    let q = question.to_lowercase();
    let is_chinese = prefers_chinese_answer(question);
    if q.contains("你好") || q.contains("hi") || q.contains("hello") {
        return (if is_chinese {
            "你好！我是你的工作助手，可以帮你分析工作时间、查看记录、对比效率等。请问你想了解什么？"
        } else {
            "Hello! I'm your work assistant — I can help you analyze work time, review records, compare efficiency, and more. What would you like to know?"
        })
        .to_string();
    }
    // 身份类：你是谁 / 你是什么 / 你叫什么 / who are you
    let is_identity = [
        "你是谁",
        "你是什么",
        "你是干",
        "你叫什么",
        "who are you",
        "what are you",
    ]
    .iter()
    .any(|p| q.contains(p));
    if is_identity {
        return (if is_chinese {
            "我是 Work Review 的内置工作助手，可以帮你回顾和分析每天的工作记录——包括时间分布、应用使用、工作会话等。\n\n当前是「基础模板」模式，会基于本地记录给出统计。如果你在设置里配置了 AI 模型，可以在下方切换到对应模型，获得更智能的问答能力。"
        } else {
            "I'm the built-in work assistant for Work Review. I help you review and analyze your daily work records — time distribution, app usage, work sessions, and more.\n\nCurrently in Basic Template mode, which gives you local stats. If you've configured an AI model in Settings, switch to it below for smarter Q&A."
        })
        .to_string();
    }
    // 能力类：你能做什么 / 帮助 / help / what can you do
    if q.contains("你能做什么")
        || q.contains("帮助")
        || q.contains("help")
        || q.contains("你能干")
        || q.contains("你能帮我")
        || q.contains("你会什么")
        || q.contains("你能做啥")
        || q.contains("介绍")
        || q.contains("what can you do")
    {
        return (if is_chinese {
            "我可以帮你：\n1. 查看某天/某周的工作记录\n2. 分析时间分布（编码/会议/文档占比）\n3. 对比不同时间段的效率变化\n4. 搜索特定的工作内容\n\n当前是「基础模板」模式。配置 AI 模型后还可以：智能总结、自由问答、联网搜索等。\n请告诉我你想了解什么？"
        } else {
            "I can help you:\n1. Review work records for a day/week\n2. Analyze time distribution (coding/meetings/docs)\n3. Compare efficiency across periods\n4. Search for specific work items\n\nCurrently in Basic Template mode. With an AI model configured, you also get: smart summaries, free-form Q&A, web search, and more.\nWhat would you like to know?"
        })
        .to_string();
    }
    (if is_chinese {
        "请告诉我你想了解的工作信息。"
    } else {
        "Tell me what work info you'd like to know."
    })
    .to_string()
}

/// FastPath：规则快速查询
pub fn fast_answer(
    question: &str,
    database: &Database,
    ignored_apps: &[String],
    excluded_domains: &[String],
) -> Result<String, AppError> {
    use work_review_core::categorize::{
        categorize_app, get_category_name, normalize_display_app_name,
    };

    // 复用 parse_temporal_range（你在 Stage 0 修复过的函数）
    let (mut date_from, mut date_to) = crate::commands::parse_temporal_range(question);

    // 无时间词时默认查"今天"：用户问"摸鱼了吗""数据如何""小结"通常指当天，
    // 查全量（10000 条）反而信息量过大不聚焦。parse_temporal_range 在没匹配到
    // 任何时间词时返回 (None, None)，这里兜底成今天的日期范围。
    if date_from.is_none() && date_to.is_none() {
        let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
        date_from = Some(today.clone());
        date_to = Some(today);
    }

    // 策略：先按时间范围加载活动，再按分类聚合
    let activities = database
        .get_activities_in_range(date_from.as_deref(), date_to.as_deref(), 10000)
        .map_err(|e| AppError::Analysis(format!("查询失败: {e}")))?;
    // 应用隐私过滤：fast_answer 结果会直接展示给用户，不应出现被"忽略应用"/
    // "排除域名"的窗口标题（与其它统计命令保持一致）。
    let activities =
        crate::commands::filter_activities_by_privacy(activities, ignored_apps, excluded_domains);

    if activities.is_empty() {
        let is_chinese = prefers_chinese_answer(question);
        return Ok(if is_chinese {
            format!(
                "在 {} ~ {} 范围内未找到活动记录。",
                date_from.as_deref().unwrap_or("全部"),
                date_to.as_deref().unwrap_or("今天")
            )
        } else {
            format!(
                "No activity records found in {} ~ {}.",
                date_from.as_deref().unwrap_or("All"),
                date_to.as_deref().unwrap_or("Today")
            )
        });
    }

    // 按分类聚合
    let mut category_durations: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    let mut app_durations: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();

    for a in &activities {
        let cat = categorize_app(&a.app_name, &a.window_title);
        *category_durations.entry(cat).or_insert(0) += a.duration;
        let display = normalize_display_app_name(&a.app_name);
        *app_durations.entry(display).or_insert(0) += a.duration;
    }

    let total: i64 = activities.iter().map(|a| a.duration).sum();
    let mut sorted_cats: Vec<_> = category_durations.into_iter().collect();
    sorted_cats.sort_by_key(|item| std::cmp::Reverse(item.1));

    let mut sorted_apps: Vec<_> = app_durations.into_iter().collect();
    sorted_apps.sort_by_key(|item| std::cmp::Reverse(item.1));
    sorted_apps.truncate(5);

    // 格式化时长
    let fmt_dur = |s: i64| -> String {
        let h = s / 3600;
        let m = (s % 3600) / 60;
        if h > 0 {
            format!("{h}h{m}m")
        } else if m > 0 {
            format!("{m}m")
        } else {
            format!("{s}s")
        }
    };

    // 跟随用户提问语言（CJK -> 中文，否则英文）
    let is_chinese = prefers_chinese_answer(question);
    let (lbl_overview, lbl_records, lbl_category, lbl_top_apps, lbl_related) = if is_chinese {
        (
            "活动总览",
            "条记录，总时长",
            "分类分布",
            "使用最多的应用",
            "相关记录",
        )
    } else {
        (
            "Activity overview",
            "records, total",
            "Category breakdown",
            "Top apps",
            "Related records",
        )
    };

    let mut lines = vec![format!(
        "{} ~ {} {}：",
        date_from
            .as_deref()
            .unwrap_or(if is_chinese { "全部" } else { "All" }),
        date_to
            .as_deref()
            .unwrap_or(if is_chinese { "今天" } else { "Today" }),
        lbl_overview
    )];
    lines.push(format!(
        "{} {} {}",
        activities.len(),
        lbl_records,
        fmt_dur(total)
    ));
    lines.push("".to_string());

    // 分类分布
    lines.push(format!("{lbl_category}："));
    for (cat_key, dur) in &sorted_cats {
        let cat_display = if is_chinese {
            get_category_name(cat_key).to_string()
        } else {
            match cat_key.as_str() {
                "development" => "Development",
                "browser" => "Browser",
                "communication" => "Communication",
                "office" => "Office",
                "design" => "Design",
                "entertainment" => "Leisure",
                "other" => "Other",
                _ => cat_key,
            }
            .to_string()
        };
        let pct = if total > 0 {
            *dur as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        lines.push(format!("  - {cat_display}: {} ({pct:.0}%)", fmt_dur(*dur)));
    }

    // Top 5 应用
    lines.push("".to_string());
    lines.push(format!("{lbl_top_apps}："));
    for (app, dur) in &sorted_apps {
        lines.push(format!("  - {app}: {}", fmt_dur(*dur)));
    }

    // 如果有 FTS 关键词命中的结果，也附上
    let fts_results = database
        .search_memory(question, date_from.as_deref(), date_to.as_deref(), 3)
        .unwrap_or_default();
    if !fts_results.is_empty() {
        lines.push("".to_string());
        lines.push(format!("{lbl_related}："));
        for r in &fts_results {
            lines.push(format!("- {} | {}", r.date, r.title));
        }
    }

    Ok(lines.join("\n"))
}

fn prefers_chinese_answer(question: &str) -> bool {
    question
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// FallbackPath：无模型时的模板回答
///
/// 走到这里的问题既不是身份/问候（DirectPath），也不含工作信号（FastPath），
/// 且没有 AI 模型可用。文案要诚实说明能力边界，并引导用户到能回答的方向，
/// 而不是机械地说"无法使用 AI 模型"——那对"你是谁"这类问题很荒谬。
fn fallback_answer(question: &str) -> String {
    if prefers_chinese_answer(question) {
        "这个问题我暂时需要 AI 模型才能回答好。你可以：\n\
         - 问我具体的工作记录（比如「今天做了什么」「这周的时间分布」）\n\
         - 在「设置 → AI 模型」里配置模型，就能自由问答了\n\
         - 或者在下方模型选择器切换到已配置的模型"
            .to_string()
    } else {
        "I need an AI model to answer this well. You can:\n\
         - Ask about specific work records (e.g. \"what did I do today\", \"this week's time breakdown\")\n\
         - Configure a model in Settings → AI Model for free-form Q&A\n\
         - Or switch to a configured model in the selector below"
            .to_string()
    }
}

// ══════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiProvider;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_database(name: &str) -> Database {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("work-review-{name}-{unique}.db"));
        Database::new(&path).expect("创建测试数据库失败")
    }


    #[test]
    fn test_route_greeting_without_model() {
        let d = route_query("你好", false);
        assert_eq!(d.path, QueryPath::Direct);
    }

    #[test]
    fn test_route_simple_time_query() {
        // 简化后：有模型即交给 Agent，由模型决定是否调用工作记录工具。
        let d = route_query("今天做了什么", true);
        assert_eq!(d.path, QueryPath::Agent);
    }

    #[test]
    fn test_route_simple_time_query_month() {
        let d = route_query("这个月的时间分布", true);
        assert_eq!(d.path, QueryPath::Agent);
    }

    #[test]
    fn test_route_non_work_weather_uses_agent() {
        // 非工作问题（天气）也交给模型，不再被时间词"今天"误判为工作查询。
        let d = route_query("今天天气怎么样", true);
        assert_eq!(d.path, QueryPath::Agent);
    }

    #[test]
    fn test_route_non_work_weather_no_model_fallback() {
        // 无模型时无法由模型判断，走模板兜底。
        let d = route_query("今天天气怎么样", false);
        assert_eq!(d.path, QueryPath::Fallback);
    }

    #[test]
    fn test_route_complex_comparison() {
        let d = route_query("对比上个月和这个月的工作效率", true);
        assert_eq!(d.path, QueryPath::Agent);
    }

    #[test]
    fn test_route_complex_why() {
        let d = route_query("为什么最近编码时间下降了", true);
        assert_eq!(d.path, QueryPath::Agent);
    }

    #[test]
    fn test_route_multi_time_periods() {
        // 这个问题同时命中"变化"（规则2）和"上月+这个月"（规则3）
        // 规则2先匹配，所以走 Agent 路径，理由是"复杂意图"
        let d = route_query("上个月和这个月有什么变化", true);
        assert_eq!(d.path, QueryPath::Agent);
        // 两个规则都可能命中，关键是走了 Agent 路径
    }

    #[test]
    fn test_route_pure_multi_time_periods() {
        // 简化后统一交给模型，不再按时间段数量分流。
        let d = route_query("上个月和这个月的工作记录", true);
        assert_eq!(d.path, QueryPath::Agent);
    }

    #[test]
    fn test_route_no_model_time_word_fast() {
        // 放宽后：含时间词的工作查询走 Fast（基础模板给统计），不再 Fallback。
        let d = route_query("对比上个月和这个月", false);
        assert_eq!(d.path, QueryPath::Fast);
    }

    #[test]
    fn test_route_no_model_work_query_fast() {
        // 无模型（基础模板）：明确工作查询走 FastPath 统计模板，得到有意义内容。
        let d = route_query("我这周主要做了什么", false);
        assert_eq!(d.path, QueryPath::Fast);
    }

    #[test]
    fn test_route_no_model_non_work_fallback() {
        // 无模型且非明确工作查询 → 模板兜底指引。
        let d = route_query("随便聊聊", false);
        assert_eq!(d.path, QueryPath::Fallback);
    }

    #[test]
    fn test_route_unknown_with_model() {
        let d = route_query("帮我看看效率情况", true);
        assert_eq!(d.path, QueryPath::Agent); // 兜底走 Agent
    }

    #[test]
    fn test_route_unknown_without_model() {
        // 放宽后："效率"是工作信号 → 无模型走 Fast（基础模板给统计）。
        let d = route_query("帮我看看效率情况", false);
        assert_eq!(d.path, QueryPath::Fast);
    }

    #[test]
    fn 扩展后常见工作问法应走统计模板而非兜底() {
        // 这些问法在扩展前都走 FallbackPath（给引导文案），扩展后应走 FastPath（给统计）。
        let cases = [
            "忙不忙",
            "干了什么",
            "搞了什么",
            "做了哪些事",
            "摸鱼了吗",
            "几点下班的",
            "写了多少小时代码",
            "开会开了多久",
            "数据怎么样",
            "情况如何",
            "小结",
            "在干嘛",
            "在干什么",
            "专注度怎么样",
            "哪个软件用得最多",
        ];
        for q in cases {
            assert_eq!(
                route_query(q, false).path,
                QueryPath::Fast,
                "「{q}」应走 FastPath"
            );
        }
    }

    #[test]
    fn 已选择模型时身份与问候问题应交给_agent() {
        assert_eq!(route_query("你是谁", true).path, QueryPath::Agent);
        assert_eq!(route_query("你好", true).path, QueryPath::Agent);
    }

    #[test]
    fn 基础模板模式下身份问题应走直接回答路径() {
        // "你是谁"在基础模板模式下曾被误判到 FallbackPath，
        // 回出"无法使用 AI 模型分析"的蠢回答（issue 截图）。
        assert_eq!(route_query("你是谁", false).path, QueryPath::Direct);
        assert_eq!(route_query("你是什么", false).path, QueryPath::Direct);
        assert_eq!(route_query("你叫什么名字", false).path, QueryPath::Direct);
        assert_eq!(route_query("who are you", false).path, QueryPath::Direct);
    }

    #[test]
    fn 能力类问题应走直接回答路径() {
        assert_eq!(route_query("你能干什么", false).path, QueryPath::Direct);
        assert_eq!(route_query("你会什么", false).path, QueryPath::Direct);
        assert_eq!(route_query("介绍一下你自己", false).path, QueryPath::Direct);
        assert_eq!(route_query("what can you do", false).path, QueryPath::Direct);
    }

    #[test]
    fn direct回答身份类问题应提及工作助手身份而非报错() {
        let answer = direct_answer("你是谁");
        assert!(
            answer.contains("工作助手") || answer.contains("Work Review"),
            "身份回答应说明自己是工作助手，got: {answer}"
        );
        assert!(
            !answer.contains("无法使用"),
            "身份回答不应出现'无法使用 AI 模型'，got: {answer}"
        );
    }

    #[test]
    fn direct回答能力类问题应列举具体能力() {
        let answer = direct_answer("你能做什么");
        assert!(
            answer.contains("查看") && answer.contains("分析"),
            "能力回答应列举具体能力，got: {answer}"
        );
    }

    #[test]
    fn fallback回答应引导而非机械报错() {
        let answer = fallback_answer("随便聊聊");
        // 不应再出现"无法使用 AI 模型进行分析"这种答非所问的文案
        assert!(
            !answer.contains("无法使用 AI 模型进行分析"),
            "fallback 不应再机械报'无法使用 AI 模型'，got: {answer}"
        );
        assert!(
            answer.contains("工作记录") || answer.contains("模型"),
            "fallback 应引导到工作记录或配模型，got: {answer}"
        );
    }

    #[test]
    fn test_direct_answer_greeting() {
        let answer = direct_answer("你好");
        assert!(answer.contains("工作助手"));
    }

    #[test]
    fn test_fallback_answer() {
        let answer = fallback_answer("随便聊聊");
        assert!(answer.contains("AI 模型"));
    }

    #[test]
    fn test_fallback_answer_follows_english_question() {
        let answer = fallback_answer("Can you chat with me?");
        assert!(answer.contains("AI model"));
        assert!(!answer.contains("我目前无法使用"));
    }

    #[tokio::test]
    async fn direct路径的done在通道背压时应等待投递确认() {
        let database = test_database("direct-done-backpressure");
        let (tx, mut rx) = StreamEventSender::channel(1);
        tx.try_send_token(StreamEvent::Token {
            token: "先占满通道".to_string(),
        });
        let history = Vec::new();
        let filters = Vec::new();
        let handle = Orchestrator::handle(
            "你好",
            None,
            &database,
            &history,
            None,
            &filters,
            &filters,
            None,
            Default::default(),
            Some(tx),
            Default::default(),
        );
        let receive = async {
            assert!(matches!(
                rx.recv().await.map(|envelope| envelope.event),
                Some(StreamEvent::Token { token }) if token == "先占满通道"
            ));
            let envelope = rx.recv().await.expect("应收到 Done 事件");
            let done = envelope.event.clone();
            envelope
                .delivery_ack
                .expect("Done 必须携带投递确认器")
                .send(Ok(()))
                .expect("发送投递确认应成功");
            done
        };

        let (result, done) = tokio::join!(handle, receive);
        result.expect("Direct 路径应正常完成");
        assert!(matches!(done, StreamEvent::Done { answer, .. } if !answer.is_empty()));
    }

    #[tokio::test]
    async fn direct路径投递失败时应返回错误() {
        let database = test_database("direct-done-delivery-failed");
        let (tx, mut rx) = StreamEventSender::channel(1);
        let history = Vec::new();
        let filters = Vec::new();
        let handle = Orchestrator::handle(
            "你好",
            None,
            &database,
            &history,
            None,
            &filters,
            &filters,
            None,
            Default::default(),
            Some(tx),
            Default::default(),
        );
        let reject = async {
            let envelope = rx.recv().await.expect("应收到 Done 事件");
            envelope
                .delivery_ack
                .expect("Done 必须携带投递确认器")
                .send(Err("Webview 已关闭".to_string()))
                .expect("发送失败确认应成功");
        };

        let (result, ()) = tokio::join!(handle, reject);
        let error = result.expect_err("实际投递失败时 Direct 路径必须返回错误");
        assert!(
            error.to_string().contains("Webview 已关闭"),
            "应保留外部投递失败原因，实际: {error}"
        );
    }

    #[tokio::test]
    async fn agent事件接收端关闭时不应误走模型失败降级() {
        let database = test_database("closed-agent-channel");
        let model_config = ModelConfig {
            provider: AiProvider::Ollama,
            endpoint: "not-a-valid-url".to_string(),
            api_key: None,
            model: "test-model".to_string(),
            enable_thinking: None,
            thinking_budget: None,
            max_output_tokens: None,
        };
        let (tx, rx) = StreamEventSender::channel(1);
        drop(rx);
        let history = Vec::new();
        let filters = Vec::new();
        let error = Orchestrator::handle(
            "分析一下我的工作",
            Some(&model_config),
            &database,
            &history,
            None,
            &filters,
            &filters,
            None,
            Default::default(),
            Some(tx),
            Default::default(),
        )
        .await
        .expect_err("接收端关闭时应终止，而不是继续模型或降级流程");

        assert!(
            error.to_string().contains("事件接收端已关闭"),
            "应保留关闭原因，实际: {error}"
        );
    }
}
