//! 工作助手流式事件。
//!
//! agent 模块用 `tokio::sync::mpsc` 传递这些事件，commands 层再桥接到
//! Tauri `ipc::Channel`。本文件不依赖 tauri，保持 agent 可单测。

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use work_review_core::database::MemorySearchItem;

/// 工作助手流式事件（经 Tauri `ipc::Channel` 推送给前端）。
///
/// 前端按 `type` 字段分发：`stepStart` / `stepResult` / `token` / `done` / `error`。
///
/// 注意：内部标签（`tag = "type"`）不支持包裹原始类型的 newtype 变体
/// （serde 会在序列化时报错），因此 Token / Error 必须用 struct 变体，
/// 恰好也与前端读取的 `event.token` / `event.error` 字段对齐。
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamEvent {
    /// 工具步骤开始（每个 tool_call 执行前推送）。
    StepStart { tool: String, label: String },
    /// 工具步骤完成，携带本次新增的引用记录与执行成败。
    ///
    /// `ok` 区分工具是真正成功还是返回错误（err 时 result 文本以"工具执行失败:"开头）。
    /// 这对前端"上一轮工具历史摘要"很关键：成功的工具即便 hits=0（如
    /// query_activities/web_search，它们不往 collected_references 写数据）
    /// 也不应被标记为"0 条 = 失败"，否则下一轮模型会误以为工具失效而避开它。
    ///
    /// `digest` 是工具结果的截断摘要（约 400 字）。前端把它存进消息记录，
    /// 追问时随历史回传，让模型无需重查工具就能引用上一轮的数据。
    StepResult {
        tool: String,
        ok: bool,
        hits: usize,
        references: Vec<MemorySearchItem>,
        digest: String,
    },
    /// 行动工具确认请求：executor 暂停等待用户在前端批准/拒绝。
    /// 前端渲染确认卡片，用户点击后调用 `confirm_assistant_action` 命令回传决定。
    #[serde(rename_all = "camelCase")]
    ConfirmRequest {
        confirm_id: String,
        tool: String,
        label: String,
        summary: String,
    },
    /// LLM 文本增量（token 流式）。后端做了小批量合并，一个事件可含多个字符。
    Token { token: String },
    /// 终态：完整答案 + 合并后的全部引用 + 用到的工具标签。
    /// 字段级 camelCase：枚举顶层的 rename_all 只作用于变体名，
    /// 不重命名字段；前端读取的是 event.toolLabels。
    #[serde(rename_all = "camelCase")]
    Done {
        answer: String,
        references: Vec<MemorySearchItem>,
        tool_labels: Vec<String>,
    },
    /// 错误终态。
    Error { error: String },
}

/// Agent 内部事件信封。
///
/// Token 不携带确认器，允许在背压时丢弃；控制事件携带一次性确认器，只有 commands
/// 层真正调用 Tauri Channel 成功后，发送方才会继续执行后续模型或工具步骤。
pub(crate) struct StreamEventEnvelope {
    pub(crate) event: StreamEvent,
    pub(crate) delivery_ack: Option<oneshot::Sender<Result<(), String>>>,
}

/// Agent 事件发送器：封装内部 mpsc，并区分可丢 Token 与需确认的控制事件。
#[derive(Clone)]
pub struct StreamEventSender {
    tx: mpsc::Sender<StreamEventEnvelope>,
}

impl StreamEventSender {
    /// 创建 Agent 内部事件通道。接收端仅供 commands 桥接层消费。
    pub(crate) fn channel(capacity: usize) -> (Self, mpsc::Receiver<StreamEventEnvelope>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, rx)
    }

    /// Token 仅影响流式观感；通道满或关闭时允许丢弃，完整回答由 Done 兜底。
    pub(crate) fn try_send_token(&self, event: StreamEvent) {
        let _ = self.tx.try_send(StreamEventEnvelope {
            event,
            delivery_ack: None,
        });
    }

    /// 控制事件必须等待 commands 桥接层确认实际 Tauri Channel 投递结果。
    pub(crate) async fn send_control(&self, event: StreamEvent) -> Result<(), String> {
        let (delivery_ack, delivery_result) = oneshot::channel();
        self.tx
            .send(StreamEventEnvelope {
                event,
                delivery_ack: Some(delivery_ack),
            })
            .await
            .map_err(|_| "Agent 事件桥接已关闭".to_string())?;

        delivery_result
            .await
            .map_err(|_| "Agent 事件投递确认通道已关闭".to_string())?
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// 等待桥接接收端关闭，用于取消正在等待的模型或工具 Future。
    pub(crate) async fn closed(&self) {
        self.tx.closed().await;
    }
}

/// 工具名 → 默认中文标签（前端可按 tool 名覆盖为 i18n 文案）。
///
/// 放在这里而不是前端，是因为 executor 推送 `StepStart` 时需要立即给一个 label，
/// 否则前端在 i18n 未命中时会空白。
pub fn default_tool_label(tool: &str) -> &'static str {
    match tool {
        "search_memory" => "记忆检索",
        "analyze_intents" => "意图分析",
        "aggregate_stats" => "统计聚合",
        "category_search" => "分类检索",
        "trend_comparison" => "趋势对比",
        "query_activities" => "活动查询",
        "fetch_url" => "读取网页",
        "web_search" => "联网搜索",
        "get_work_sessions" => "工作时段",
        "get_insights" => "工作洞察",
        "weekly_review" => "周期回顾",
        "extract_todos" => "待办提取",
        "get_daily_report" => "读取日报",
        "list_reports" => "日报列表",
        "get_reports_in_range" => "区间日报",
        "get_hourly_summaries" => "小时摘要",
        "project_stats" => "项目统计",
        "list_todos" => "待办列表",
        "get_current_context" => "实时上下文",
        "semantic_search" => "记忆检索(语义)",
        "create_todo" => "新建待办",
        "complete_todo" => "完成待办",
        "update_todo" => "更新待办",
        "set_app_category" => "设置分类",
        "pause_recording" => "暂停记录",
        "resume_recording" => "恢复记录",
        "open_timeline" => "打开时间线",
        "generate_daily_report" => "生成日报",
        "read_run_artifact" => "读取产物片段",
        "load_skill" => "加载个人技能",
        other if other.starts_with("mcp__") => "调用外部 MCP 工具",
        _ => "处理中",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 线格式契约：前端 Ask.svelte 按 event.type 分发、读 event.token / event.error 字段。
    /// 内部标签 + newtype(String) 会直接序列化失败，必须保持 struct 变体。
    #[test]
    fn 流式事件序列化应匹配前端读取的字段() {
        let token = serde_json::to_value(StreamEvent::Token {
            token: "你好".to_string(),
        })
        .expect("Token 事件必须可序列化");
        assert_eq!(token["type"], "token");
        assert_eq!(token["token"], "你好");

        let error = serde_json::to_value(StreamEvent::Error {
            error: "boom".to_string(),
        })
        .expect("Error 事件必须可序列化");
        assert_eq!(error["type"], "error");
        assert_eq!(error["error"], "boom");

        let step = serde_json::to_value(StreamEvent::StepStart {
            tool: "search_memory".to_string(),
            label: "记忆检索".to_string(),
        })
        .expect("StepStart 事件必须可序列化");
        assert_eq!(step["type"], "stepStart");
        assert_eq!(step["tool"], "search_memory");

        // StepResult 必须带 ok 字段（前端历史摘要靠它区分"成功但 0 引用"
        // 和"真正失败"，避免把 query_activities 等成功工具误标为 0 条）。
        let step_ok = serde_json::to_value(StreamEvent::StepResult {
            tool: "query_activities".to_string(),
            ok: true,
            hits: 0,
            references: vec![],
            digest: "Top应用: A(30分)".to_string(),
        })
        .expect("StepResult 事件必须可序列化");
        assert_eq!(step_ok["type"], "stepResult");
        assert_eq!(step_ok["tool"], "query_activities");
        assert_eq!(step_ok["ok"], true);
        assert_eq!(step_ok["hits"], 0);
        assert_eq!(step_ok["digest"], "Top应用: A(30分)");

        // ConfirmRequest：前端读 event.confirmId / event.summary 渲染确认卡片。
        let confirm = serde_json::to_value(StreamEvent::ConfirmRequest {
            confirm_id: "c1".to_string(),
            tool: "create_todo".to_string(),
            label: "新建待办".to_string(),
            summary: "新建待办：整理周报".to_string(),
        })
        .expect("ConfirmRequest 事件必须可序列化");
        assert_eq!(confirm["type"], "confirmRequest");
        assert_eq!(confirm["confirmId"], "c1");
        assert_eq!(confirm["summary"], "新建待办：整理周报");

        let done = serde_json::to_value(StreamEvent::Done {
            answer: "答案".to_string(),
            references: vec![],
            tool_labels: vec!["search_memory".to_string()],
        })
        .expect("Done 事件必须可序列化");
        assert_eq!(done["type"], "done");
        assert_eq!(done["answer"], "答案");
        assert_eq!(done["toolLabels"][0], "search_memory");
    }

    #[test]
    fn query_activities应有默认中文标签() {
        assert_eq!(default_tool_label("query_activities"), "活动查询");
    }
}
