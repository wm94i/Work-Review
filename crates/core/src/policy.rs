use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 权限级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadActivities,
    ReadReports,
    ReadStats,
    ReadSessions,
    ReadConfig,
    WriteReport,
    WriteConfig,
    ExecuteAi,
    ExecuteSkill,
    ReadDeviceStatus,
}

/// 调用来源
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallSource {
    /// MCP Tool 调用
    McpTool {
        tool_name: String,
        client_id: Option<String>,
    },
    /// Skill 执行
    SkillExecution { skill_id: String },
    /// Tauri 前端调用
    Frontend { route: Option<String> },
    /// Localhost API 调用
    LocalhostApi { endpoint: String },
}

/// 策略决策
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyDecision {
    /// 允许
    Allow,
    /// 脱敏后允许
    AllowSanitized,
    /// 拒绝
    Deny,
}

/// 审计日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: u64,
    pub timestamp: u64,
    pub source: CallSource,
    pub permission: Permission,
    pub decision: PolicyDecision,
    pub denied_reason: Option<String>,
    pub duration_ms: u64,
}

static AUDIT_COUNTER: AtomicU64 = AtomicU64::new(1);

impl AuditEntry {
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn new(
        source: CallSource,
        permission: Permission,
        decision: PolicyDecision,
        denied_reason: Option<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            id: AUDIT_COUNTER.fetch_add(1, Ordering::Relaxed),
            timestamp: Self::now_secs(),
            source,
            permission,
            decision,
            denied_reason,
            duration_ms,
        }
    }
}

/// 策略执行器
pub struct PolicyEnforcer {
    /// 审计日志（内存，最近 N 条）
    audit_log: Vec<AuditEntry>,
    max_audit_entries: usize,
    /// MCP 客户端权限白名单
    mcp_permissions: Vec<Permission>,
    /// MCP 读取活动/会话时是否输出原文（OCR、截图路径等）；默认 false → 脱敏后允许
    mcp_raw_activities: bool,
    /// Localhost API 权限
    localhost_permissions: Vec<Permission>,
    /// Skill 权限映射
    skill_permissions: HashMap<String, Vec<Permission>>,
}

impl PolicyEnforcer {
    pub fn new(config: &AppConfig) -> Self {
        let mcp_permissions = vec![
            Permission::ReadActivities,
            Permission::ReadReports,
            Permission::ReadStats,
            Permission::ReadSessions,
            Permission::ReadConfig,
            Permission::WriteReport,
            Permission::ExecuteAi,
            Permission::ExecuteSkill,
            Permission::ReadDeviceStatus,
        ];

        let localhost_permissions = if config.localhost_api_enabled {
            vec![
                Permission::ReadActivities,
                Permission::ReadReports,
                Permission::ReadStats,
                Permission::ReadSessions,
                Permission::ReadDeviceStatus,
                Permission::WriteReport,
            ]
        } else {
            Vec::new()
        };

        Self {
            audit_log: Vec::new(),
            max_audit_entries: 1000,
            mcp_permissions,
            mcp_raw_activities: config.mcp_include_ocr,
            localhost_permissions,
            skill_permissions: HashMap::new(),
        }
    }

    /// 根据最新配置刷新策略开关（用于 MCP 热重载配置）
    pub fn update_from_config(&mut self, config: &AppConfig) {
        self.mcp_raw_activities = config.mcp_include_ocr;
    }

    /// 检查权限并记录审计日志
    pub fn check_permission(
        &mut self,
        source: &CallSource,
        permission: Permission,
    ) -> PolicyDecision {
        let start = std::time::Instant::now();
        let (decision, reason) = self.evaluate(source, permission);
        let entry = AuditEntry::new(
            source.clone(),
            permission,
            decision,
            reason,
            start.elapsed().as_millis() as u64,
        );
        self.record_audit(entry);
        decision
    }

    /// 注册 Skill 权限
    pub fn register_skill_permissions(&mut self, skill_id: &str, permissions: Vec<Permission>) {
        self.skill_permissions
            .insert(skill_id.to_string(), permissions);
    }

    /// 获取统计：按来源分组的调用次数
    pub fn get_call_stats(&self) -> HashMap<String, u64> {
        let mut stats = HashMap::new();
        for entry in &self.audit_log {
            let key = match &entry.source {
                CallSource::McpTool { tool_name, .. } => format!("mcp:{tool_name}"),
                CallSource::SkillExecution { skill_id } => format!("skill:{skill_id}"),
                CallSource::Frontend { route, .. } => {
                    format!("frontend:{}", route.as_deref().unwrap_or("unknown"))
                }
                CallSource::LocalhostApi { endpoint } => format!("api:{endpoint}"),
            };
            *stats.entry(key).or_insert(0) += 1;
        }
        stats
    }

    fn evaluate(
        &self,
        source: &CallSource,
        permission: Permission,
    ) -> (PolicyDecision, Option<String>) {
        match source {
            CallSource::McpTool { .. } => {
                if !self.mcp_permissions.contains(&permission) {
                    (
                        PolicyDecision::Deny,
                        Some(format!("MCP 客户端无 {permission:?} 权限")),
                    )
                } else if !self.mcp_raw_activities
                    && matches!(
                        permission,
                        Permission::ReadActivities
                            | Permission::ReadSessions
                            | Permission::ReadStats
                    )
                {
                    // 活动/会话/统计可能携带 OCR、可执行文件路径或远程截图 URL，默认脱敏后再交给外部 AI 客户端
                    (PolicyDecision::AllowSanitized, None)
                } else {
                    (PolicyDecision::Allow, None)
                }
            }
            CallSource::SkillExecution { skill_id } => {
                if let Some(perms) = self.skill_permissions.get(skill_id) {
                    if perms.contains(&permission) {
                        (PolicyDecision::Allow, None)
                    } else {
                        (
                            PolicyDecision::Deny,
                            Some(format!("技能 {skill_id} 无 {permission:?} 权限")),
                        )
                    }
                } else {
                    // 未注册权限的技能，只允许读操作
                    match permission {
                        Permission::ReadActivities
                        | Permission::ReadReports
                        | Permission::ReadStats
                        | Permission::ReadSessions
                        | Permission::ReadConfig
                        | Permission::ReadDeviceStatus => (PolicyDecision::Allow, None),
                        _ => (
                            PolicyDecision::Deny,
                            Some(format!("技能 {skill_id} 未注册 {permission:?} 权限")),
                        ),
                    }
                }
            }
            CallSource::Frontend { .. } => (PolicyDecision::Allow, None),
            CallSource::LocalhostApi { .. } => {
                if self.localhost_permissions.contains(&permission) {
                    (PolicyDecision::Allow, None)
                } else {
                    (
                        PolicyDecision::Deny,
                        Some(format!("Localhost API 无 {permission:?} 权限")),
                    )
                }
            }
        }
    }

    fn record_audit(&mut self, entry: AuditEntry) {
        self.audit_log.push(entry);
        if self.audit_log.len() > self.max_audit_entries {
            self.audit_log
                .drain(0..self.audit_log.len() - self.max_audit_entries);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    fn mcp_source() -> CallSource {
        CallSource::McpTool {
            tool_name: "query_timeline".to_string(),
            client_id: None,
        }
    }

    #[test]
    fn mcp读取活动默认应脱敏后允许() {
        let mut policy = PolicyEnforcer::new(&AppConfig::default());
        assert_eq!(
            policy.check_permission(&mcp_source(), Permission::ReadActivities),
            PolicyDecision::AllowSanitized
        );
        assert_eq!(
            policy.check_permission(&mcp_source(), Permission::ReadSessions),
            PolicyDecision::AllowSanitized
        );
        assert_eq!(
            policy.check_permission(&mcp_source(), Permission::ReadStats),
            PolicyDecision::AllowSanitized
        );
        assert_eq!(
            policy.check_permission(&mcp_source(), Permission::ReadReports),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn 显式开启原文后mcp读取活动应直接允许() {
        let mut config = AppConfig::default();
        config.mcp_include_ocr = true;
        let mut policy = PolicyEnforcer::new(&config);
        assert_eq!(
            policy.check_permission(&mcp_source(), Permission::ReadActivities),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn mcp不在白名单内的权限应拒绝() {
        let mut policy = PolicyEnforcer::new(&AppConfig::default());
        assert_eq!(
            policy.check_permission(&mcp_source(), Permission::WriteConfig),
            PolicyDecision::Deny
        );
    }
}
