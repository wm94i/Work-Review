//! Work Review Personal Agent Harness — 纯 Rust、零 Tauri 依赖的核心运行时原语。
//!
//! 为桌面助手、四端 IM Bot、后台定时/事件触发器与未来的 MCP Client 提供统一的：
//! - 权限分级、数据污点传播与审批策略引擎（[`policy`]）
//! - 上下文预算、自动摘要压缩与超长工具输出 Artifact 分页（[`context`]）
//! - Run / Event / Audit 状态模型与脱敏摘要（[`ledger`]）
//! - 本机渐进式 `SKILL.md` 发现与按需加载（[`skills`]）

pub mod context;
pub mod ledger;
pub mod mcp_client;
pub mod policy;
pub mod routine;
pub mod skills;
