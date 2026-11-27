//! Agent 模块 — 第三代 Agentic 架构
//!
//! 五层结构：Tools → Model → Executor → Orchestrator
//! 当前进度：Stage 1-5 全部完成 ✅

pub mod deadline;
pub mod events;
pub mod executor;
pub mod mcp_bridge;
pub mod model;
pub mod orchestrator;
pub mod routine_runner;
pub mod tools;

/// 助手请求的能力边界。
///
/// 模式在命令入口只判定一次，并贯穿编排、执行和工具注册，避免普通聊天
/// 意外读取本机工作数据，也避免工作复盘失去真实记录优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantRequestMode {
    WorkReview,
    GeneralChat,
}

/// 命令入口生成并贯穿编排器的单次请求分类结果。
///
/// `mode` 决定是否允许访问本机工作数据，`requires_model_action` 决定无模型时
/// 是否必须安全失败。默认值保持普通聊天且不执行动作，确保未知输入 fail-closed。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssistantRequestClassification {
    pub mode: AssistantRequestMode,
    pub requires_model_action: bool,
}

impl AssistantRequestClassification {
    pub const fn general_chat() -> Self {
        Self {
            mode: AssistantRequestMode::GeneralChat,
            requires_model_action: false,
        }
    }

    pub const fn work_review(requires_model_action: bool) -> Self {
        Self {
            mode: AssistantRequestMode::WorkReview,
            requires_model_action,
        }
    }
}

impl Default for AssistantRequestClassification {
    fn default() -> Self {
        Self::general_chat()
    }
}

pub use events::{StreamEvent, StreamEventSender};
pub use model::Message;
pub use orchestrator::Orchestrator;
pub use tools::{
    ActionBridge, AssistantAction, AssistantRuntime, ConfirmBridge, ConfirmDecision, WebToolsConfig,
};
