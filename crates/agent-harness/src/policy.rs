//! 统一权限分级、数据污点传播与审批策略引擎。
//!
//! 取代散落在多处的布尔标记与工具名数组，作为 Personal Agent Harness 的单一安全判定源：
//! - 每个工具显式声明风险等级（[`ToolRiskTier`]）与数据流属性（读本机 / 读记忆 / 外发数据 / 读外部不可信内容）。
//! - 运行期维护 [`DataTaint`]，一旦本轮接触过本机工作数据或用户长期记忆，外发数据的工具一律进入强审批或拒绝（无交互通道时 fail-closed）。

use serde::{Deserialize, Serialize};

/// 工具风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskTier {
    /// 只读查询（无副作用）。
    ReadOnly,
    /// 本机可控写操作（新建/修改待办、调整分类、打开页面、生成日报等）。
    LocalSafeWrite,
    /// 产生外部副作用的操作（访问外部链接、外发消息/请求）。
    ExternalSideEffect,
    /// 高危/不可逆写操作（永久删除长期记忆、暂停录制等）。
    Destructive,
}

impl ToolRiskTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::LocalSafeWrite => "local_safe_write",
            Self::ExternalSideEffect => "external_side_effect",
            Self::Destructive => "destructive",
        }
    }
}

/// 单次 Run 内的数据污点状态。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataTaint {
    /// 本轮已接触本机工作记录（窗口标题、OCR、活动统计、日报等）。
    pub local_work_data: bool,
    /// 本轮已接触用户结构化长期记忆。
    pub user_memory: bool,
    /// 本轮已读取来自外部网络的不可信内容（网页、搜索结果）。
    pub external_untrusted: bool,
}

impl DataTaint {
    /// 是否包含需要防外带的本机敏感上下文。
    pub const fn has_sensitive_local(self) -> bool {
        self.local_work_data || self.user_memory
    }

    /// 序列化为稳定的逗号分隔标记，供 SQLite 审计与前端 Trace 展示。
    pub fn to_flag_string(self) -> String {
        let mut flags = Vec::with_capacity(3);
        if self.local_work_data {
            flags.push("local_work_data");
        }
        if self.user_memory {
            flags.push("user_memory");
        }
        if self.external_untrusted {
            flags.push("external_untrusted");
        }
        flags.join(",")
    }
}

/// 单个工具的声明式能力与安全规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCapabilitySpec {
    pub name: &'static str,
    pub risk_tier: ToolRiskTier,
    /// 是否读取本机工作数据（调用成功后会标记 `local_work_data` 污点）。
    pub reads_local_data: bool,
    /// 是否读取用户长期记忆（调用成功后会标记 `user_memory` 污点）。
    pub reads_user_memory: bool,
    /// 是否读取第三方不可信外部内容（如网页正文、搜索摘要）。
    pub reads_external_untrusted: bool,
    /// 是否会把参数直接作为外部目标发出（如 `fetch_url` 的 URL，属于潜在数据外带通道）。
    pub sends_external_data: bool,
    /// 是否强制要求每次显式人工确认（不可被会话级免审绕过）。
    pub requires_explicit_approval: bool,
}

impl ToolCapabilitySpec {
    pub const fn local_readonly(name: &'static str) -> Self {
        Self {
            name,
            risk_tier: ToolRiskTier::ReadOnly,
            reads_local_data: true,
            reads_user_memory: false,
            reads_external_untrusted: false,
            sends_external_data: false,
            requires_explicit_approval: false,
        }
    }

    pub const fn harness_readonly(name: &'static str) -> Self {
        Self {
            name,
            risk_tier: ToolRiskTier::ReadOnly,
            reads_local_data: false,
            reads_user_memory: false,
            reads_external_untrusted: false,
            sends_external_data: false,
            requires_explicit_approval: false,
        }
    }

    pub const fn user_memory_readonly(name: &'static str) -> Self {
        Self {
            name,
            risk_tier: ToolRiskTier::ReadOnly,
            reads_local_data: false,
            reads_user_memory: true,
            reads_external_untrusted: false,
            sends_external_data: false,
            requires_explicit_approval: false,
        }
    }

    pub const fn local_write(name: &'static str, destructive: bool) -> Self {
        Self {
            name,
            risk_tier: if destructive {
                ToolRiskTier::Destructive
            } else {
                ToolRiskTier::LocalSafeWrite
            },
            reads_local_data: false,
            reads_user_memory: false,
            reads_external_untrusted: false,
            sends_external_data: false,
            requires_explicit_approval: true,
        }
    }

    pub const fn external_fetch(name: &'static str) -> Self {
        Self {
            name,
            risk_tier: ToolRiskTier::ExternalSideEffect,
            reads_local_data: false,
            reads_user_memory: false,
            reads_external_untrusted: true,
            sends_external_data: true,
            requires_explicit_approval: false,
        }
    }

    /// 联网搜索工具规格。
    ///
    /// 已知安全边界（与 `CHANGELOG.md` 及 `tools.rs` 回归测试一致）：
    /// `web_search` 会向用户显式配置的搜索服务发送查询词，但在工作复盘模式下若每次搜索都弹窗
    /// 审批会严重破坏可用性，因此这里将 `sends_external_data` 设为 `false`，仅把任意目标 URL
    /// 访问（`fetch_url`）纳入污点强审批；模型侧仍通过系统提示词与数据边界标签禁止把本机记录
    /// 拼入搜索查询词。
    pub const fn external_search(name: &'static str) -> Self {
        Self {
            name,
            risk_tier: ToolRiskTier::ReadOnly,
            reads_local_data: false,
            reads_user_memory: false,
            reads_external_untrusted: true,
            sends_external_data: false,
            requires_explicit_approval: false,
        }
    }

    /// 外部 MCP Server 暴露的工具规格（最高保守边界）。
    ///
    /// 外部 MCP 工具同时具备外发参数与读取不可信外部返回的双重风险，因此：
    /// - 归类为 `ExternalSideEffect` 且强制 `requires_explicit_approval = true`；
    /// - 在无交互审批通道（如 IM Bot 或后台自主任务）中始终 fail-closed 拒绝；
    /// - 在桌面端每次调用均必须经用户显式确认，返回内容按外部不可信数据边界包裹。
    pub const fn external_mcp(name: &'static str) -> Self {
        Self {
            name,
            risk_tier: ToolRiskTier::ExternalSideEffect,
            reads_local_data: false,
            reads_user_memory: false,
            reads_external_untrusted: true,
            sends_external_data: true,
            requires_explicit_approval: true,
        }
    }
}

/// 调用通道的能力边界（桌面端 / IM Bot / 后台定时任务）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelCapabilities {
    /// 当前通道是否具备实时人工确认桥（如桌面端确认卡片）。
    pub interactive_approval: bool,
    /// 当前通道是否允许执行写操作。
    pub allow_writes: bool,
}

impl ChannelCapabilities {
    pub const fn desktop(has_confirm_bridge: bool, has_action_bridge: bool) -> Self {
        Self {
            interactive_approval: has_confirm_bridge,
            allow_writes: has_action_bridge && has_confirm_bridge,
        }
    }

    pub const fn readonly_bot() -> Self {
        Self {
            interactive_approval: false,
            allow_writes: false,
        }
    }
}

/// 策略引擎对单次工具调用的裁决结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    RequireApproval {
        reason: String,
        taint_triggered: bool,
    },
    Deny {
        reason: String,
    },
}

impl PolicyDecision {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::RequireApproval { .. } => "require_approval",
            Self::Deny { .. } => "deny",
        }
    }
}

/// Personal Agent Harness 统一策略引擎。
pub struct HarnessPolicyEngine;

impl HarnessPolicyEngine {
    /// 在执行工具前进行权限与污点判定（fail-closed）。
    pub fn evaluate(
        spec: &ToolCapabilitySpec,
        taint: DataTaint,
        channel: ChannelCapabilities,
    ) -> PolicyDecision {
        // 规则 1：本轮一旦接触过本机敏感上下文，任何可外发目标参数的外部工具必须人工审批；
        // 无交互审批通道（如 Bot）直接拒绝，防提示词注入借 URL 外带数据。
        if taint.has_sensitive_local() && spec.sends_external_data {
            if !channel.interactive_approval {
                return PolicyDecision::Deny {
                    reason: "本轮已读取本机工作记录，出于隐私保护，未经用户确认不能访问外部链接。请直接基于已有信息回答。".to_string(),
                };
            }
            return PolicyDecision::RequireApproval {
                reason: "本轮已接触本机工作数据，访问外部链接需用户确认以防数据外带".to_string(),
                taint_triggered: true,
            };
        }

        // 规则 2：拆离「写权限门禁」与「显式审批门禁」，避免只读但需确认的工具在无写权限通道被误拒。
        let is_write = matches!(
            spec.risk_tier,
            ToolRiskTier::LocalSafeWrite | ToolRiskTier::Destructive
        );

        if is_write && !channel.allow_writes {
            return PolicyDecision::Deny {
                reason: format!("当前通道仅允许只读操作，禁止执行 {}", spec.name),
            };
        }

        if is_write || spec.requires_explicit_approval {
            if !channel.interactive_approval {
                return PolicyDecision::Deny {
                    reason: format!("{} 需要用户确认，但当前通道不支持交互审批", spec.name),
                };
            }
            return PolicyDecision::RequireApproval {
                reason: format!(
                    "{} 属于 {} 操作，执行前需要用户确认",
                    spec.name,
                    spec.risk_tier.as_str()
                ),
                taint_triggered: false,
            };
        }

        PolicyDecision::Allow
    }

    /// 工具执行完成后按结果传播数据污点。
    pub fn propagate_taint(taint: &mut DataTaint, spec: &ToolCapabilitySpec, ok: bool) {
        if !ok {
            return;
        }
        if spec.reads_local_data {
            taint.local_work_data = true;
        }
        if spec.reads_user_memory {
            taint.user_memory = true;
        }
        if spec.reads_external_untrusted {
            taint.external_untrusted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 接触本机数据后外发链接在有确认桥时强制审批无确认桥时拒绝() {
        let fetch = ToolCapabilitySpec::external_fetch("fetch_url");
        let taint = DataTaint {
            local_work_data: true,
            ..Default::default()
        };

        let desktop =
            HarnessPolicyEngine::evaluate(&fetch, taint, ChannelCapabilities::desktop(true, true));
        assert!(matches!(
            desktop,
            PolicyDecision::RequireApproval {
                taint_triggered: true,
                ..
            }
        ));

        let bot = HarnessPolicyEngine::evaluate(&fetch, taint, ChannelCapabilities::readonly_bot());
        assert!(matches!(bot, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn 未接触本机数据时普通聊天允许直接fetch_url() {
        let fetch = ToolCapabilitySpec::external_fetch("fetch_url");
        let decision = HarnessPolicyEngine::evaluate(
            &fetch,
            DataTaint::default(),
            ChannelCapabilities::desktop(false, false),
        );
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn 写操作在桌面通道要求审批在只读通道直接拒绝() {
        let create_todo = ToolCapabilitySpec::local_write("create_todo", false);
        assert!(matches!(
            HarnessPolicyEngine::evaluate(
                &create_todo,
                DataTaint::default(),
                ChannelCapabilities::desktop(true, true),
            ),
            PolicyDecision::RequireApproval {
                taint_triggered: false,
                ..
            }
        ));
        assert!(matches!(
            HarnessPolicyEngine::evaluate(
                &create_todo,
                DataTaint::default(),
                ChannelCapabilities::readonly_bot(),
            ),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn 污点仅在工具执行成功时传播并生成稳定标记() {
        let mut taint = DataTaint::default();
        let search = ToolCapabilitySpec::local_readonly("search_memory");
        let mem = ToolCapabilitySpec::user_memory_readonly("search_user_memories");
        let web = ToolCapabilitySpec::external_search("web_search");

        HarnessPolicyEngine::propagate_taint(&mut taint, &search, false);
        assert!(!taint.has_sensitive_local());

        HarnessPolicyEngine::propagate_taint(&mut taint, &search, true);
        HarnessPolicyEngine::propagate_taint(&mut taint, &mem, true);
        HarnessPolicyEngine::propagate_taint(&mut taint, &web, true);
        assert_eq!(
            taint.to_flag_string(),
            "local_work_data,user_memory,external_untrusted"
        );
    }

    #[test]
    fn 只读需审批工具在无写权限但有确认桥的通道应请求审批而非被写门禁拒绝() {
        let sensitive_read = ToolCapabilitySpec {
            name: "sensitive_read",
            risk_tier: ToolRiskTier::ReadOnly,
            reads_local_data: true,
            reads_user_memory: false,
            reads_external_untrusted: false,
            sends_external_data: false,
            requires_explicit_approval: true,
        };
        let decision = HarnessPolicyEngine::evaluate(
            &sensitive_read,
            DataTaint::default(),
            ChannelCapabilities::desktop(true, false),
        );
        assert!(matches!(
            decision,
            PolicyDecision::RequireApproval {
                taint_triggered: false,
                ..
            }
        ));
    }

    #[test]
    fn 外部mcp工具在桌面端始终强制审批在无交互通道始终拒绝() {
        let mcp = ToolCapabilitySpec::external_mcp("mcp_external_tool");

        // 无污点时桌面端也必须人工审批
        let desktop_clean = HarnessPolicyEngine::evaluate(
            &mcp,
            DataTaint::default(),
            ChannelCapabilities::desktop(true, false),
        );
        assert!(matches!(
            desktop_clean,
            PolicyDecision::RequireApproval {
                taint_triggered: false,
                ..
            }
        ));

        // 有本机污点时触发 taint_triggered = true 的强审批
        let tainted = DataTaint {
            local_work_data: true,
            ..Default::default()
        };
        let desktop_tainted =
            HarnessPolicyEngine::evaluate(&mcp, tainted, ChannelCapabilities::desktop(true, true));
        assert!(matches!(
            desktop_tainted,
            PolicyDecision::RequireApproval {
                taint_triggered: true,
                ..
            }
        ));

        // 只读 Bot / 后台自主任务通道无论有无污点一律拒绝
        assert!(matches!(
            HarnessPolicyEngine::evaluate(
                &mcp,
                DataTaint::default(),
                ChannelCapabilities::readonly_bot(),
            ),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            HarnessPolicyEngine::evaluate(&mcp, tainted, ChannelCapabilities::readonly_bot()),
            PolicyDecision::Deny { .. }
        ));
    }
}
