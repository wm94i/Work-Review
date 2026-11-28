//! Tauri 命令集合（按领域拆分）。
//!
//! 历史上这些命令全部位于 `commands.rs` 单文件中，现已按功能领域拆分到各子模块。
//! 本文件仅做模块声明与重新导出，保持 `commands::xxx` 的访问路径不变，
//! 使 `main.rs` 的 `generate_handler![...]` 注册与 `localhost_api` / `agent` / `bot`
//! 对内部 helper 的引用完全无需改动。

mod agent_harness;
mod ai;
mod ask;
mod assistant_history;
mod avatar;
mod backup;
mod category;
mod config;
mod integration;
mod memory;
mod period_review;
mod project;
mod recording;
mod report;
mod semantic_memory;
mod shared;
mod stats;
mod system;
mod timeline;
mod updater;
mod user_memory;

// 所有 pub command + DTO（main.rs generate_handler 的 commands::xxx 不变）
pub use agent_harness::*;
pub use ai::*;
pub use ask::*;
pub use assistant_history::*;
pub use avatar::*;
pub use backup::*;
pub use category::*;
pub use config::*;
pub use integration::*;
pub use memory::*;
pub use period_review::*;
pub use project::*;
pub use recording::*;
pub use report::*;
pub use semantic_memory::*;
pub use stats::*;
pub use system::*;
pub use timeline::*;
pub use updater::*;
pub use user_memory::*;

// 被 main.rs / localhost_api / agent / bot 直接调用的 pub(crate) helper
// 通过子模块再次 re-export，保持 `commands::xxx_inner` / `commands::xxx` 路径不断。
pub(crate) use shared::{
    collect_privacy_filters, filter_activities_by_privacy, is_iso_date,
    load_filtered_activities_in_range, merge_manual_followups_into_todos, parse_temporal_range,
    persist_app_config, resolve_single_date, sanitize_activities_for_external, update_app_config,
    update_app_config_with_source, ConfigWriteSource,
};
pub(crate) use system::apply_dock_visibility;

/// 统一注册所有 Tauri IPC 命令（从 `main.rs` 迁入，保持命令名与签名完全不变）。
pub(crate) fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        crate::autostart::enable_autostart,
        crate::autostart::disable_autostart,
        crate::autostart::is_autostart_enabled,
        get_today_stats,
        get_overview_stats,
        get_focus_metrics,
        get_overview_domains,
        get_overview_domain_detail,
        get_range_daily_totals,
        get_daily_stats,
        get_timeline,
        get_hourly_app_breakdown,
        generate_report,
        get_git_commits_for_date,
        get_saved_report,
        update_report_content,
        set_report_block_preference,
        export_report_markdown,
        get_saved_period_review,
        generate_period_review,
        export_period_review_markdown,
        export_timeline_json,
        export_reports_range,
        export_activities,
        export_daily_stats,
        create_backup,
        restore_backup,
        get_projects,
        save_project,
        add_project_rule,
        delete_project,
        preview_project_rule,
        resolve_activity_project,
        get_localhost_api_status,
        get_node_gateway_status,
        get_telegram_bot_status,
        get_wecom_bot_status,
        generate_telegram_bot_bind_code,
        patch_config,
        set_app_privacy_level,
        get_bot_sender_binding_status,
        generate_bot_sender_bind_code,
        clear_bot_sender_bindings,
        generate_text_with_model,
        reveal_localhost_api_token,
        rotate_localhost_api_token,
        get_config,
        save_config,
        get_update_settings,
        save_update_settings,
        should_check_updates,
        update_last_check_time,
        pause_recording,
        resume_recording,
        get_recording_state,
        get_avatar_state,
        save_avatar_position,
        persist_avatar_position,
        set_avatar_window_expanded,
        set_avatar_interactive_regions,
        get_data_dir,
        get_default_data_dir,
        get_runtime_platform,
        get_linux_session_support,
        install_gnome_avatar_extension,
        change_data_dir,
        cleanup_old_data_dir,
        check_github_update,
        download_and_install_github_update,
        quit_app_for_update,
        open_data_dir,
        get_screenshot_thumbnail,
        get_screenshot_full,
        test_remote_storage,
        test_model,
        get_ai_providers,
        fetch_models,
        get_running_apps,
        get_recent_apps,
        set_app_category_rule,
        set_domain_semantic_rule,
        get_categories,
        save_custom_category,
        delete_custom_category,
        get_semantic_categories,
        save_custom_semantic_category,
        delete_custom_semantic_category,
        get_storage_stats,
        get_hourly_summaries,
        get_activity,
        chat_work_assistant,
        cancel_assistant_request,
        confirm_assistant_action,
        list_agent_runs,
        get_agent_run_trace,
        list_personal_skills,
        get_personal_skill,
        save_personal_skill,
        delete_personal_skill,
        open_personal_skills_dir,
        list_agent_routines,
        save_agent_routine,
        delete_agent_routine,
        run_agent_routine_now,
        list_mcp_client_servers,
        save_mcp_client_server,
        delete_mcp_client_server,
        test_mcp_client_server,
        list_assistant_conversations,
        create_assistant_conversation,
        get_assistant_messages,
        append_assistant_message,
        delete_assistant_conversation,
        list_user_memories,
        create_user_memory,
        update_user_memory,
        delete_user_memory,
        clear_user_memories,
        index_semantic_memory,
        semantic_memory_status,
        test_embedding_model,
        test_assistant_search,
        synthesize_insights,
        clear_old_activities,
        crate::tray::set_app_locale,
        delete_activity,
        delete_activities_by_date,
        delete_activities_by_range,
        delete_activities_by_app,
        check_permissions,
        open_permission_settings,
        set_dock_visibility,
        get_app_icon,
        save_background_image,
        get_background_image,
        clear_background_image,
        show_main_window,
        handle_avatar_followup_action,
        crate::crash::get_platform,
    ]
}
