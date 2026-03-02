//! Route handlers for the OpenFang API.
//!
//! Each domain area is split into its own module. This `mod.rs` re-exports
//! all handlers and the shared `AppState` so that `server.rs` can continue
//! to reference them as `routes::handler_name`.

pub mod common;

mod a2a;
mod agents;
mod approvals;
mod audit;
mod bindings;
mod channels;
mod commands;
mod config;
mod cron;
mod hands;
mod health;
mod integrations;
mod mcp;
mod memory;
mod migrate;
mod models;
mod network;
mod pairing;
mod schedules;
mod sessions;
mod skills;
mod templates;
mod tools;
mod triggers;
mod usage;
mod webhooks;
mod workflows;

// Re-export AppState for server.rs
pub use common::AppState;

// ── Agent handlers ──────────────────────────────────────────────────
pub use agents::{
    clone_agent, get_agent, get_agent_deliveries, get_agent_file, get_agent_mcp_servers,
    get_agent_skills, kill_agent, list_agent_files, list_agents, patch_agent_config,
    send_message, send_message_stream, serve_upload, set_agent_file, set_agent_mcp_servers,
    set_agent_mode, set_agent_skills, set_model, spawn_agent, stop_agent, update_agent,
    update_agent_identity, upload_file, list_profiles,
};

// ── Session handlers ────────────────────────────────────────────────
pub use sessions::{
    compact_session, create_agent_session, delete_session, find_session_by_label,
    get_agent_session, list_agent_sessions, list_sessions, reset_session, set_session_label,
    switch_agent_session,
};

// ── Channel handlers ────────────────────────────────────────────────
pub use channels::{
    configure_channel, list_channels, reload_channels, remove_channel, test_channel,
    whatsapp_qr_start, whatsapp_qr_status,
};

// ── Workflow handlers ───────────────────────────────────────────────
pub use workflows::{create_workflow, list_workflow_runs, list_workflows, run_workflow};

// ── Trigger handlers ────────────────────────────────────────────────
pub use triggers::{create_trigger, delete_trigger, list_triggers, update_trigger};

// ── Template handlers ───────────────────────────────────────────────
pub use templates::{get_template, list_templates};

// ── Memory handlers ─────────────────────────────────────────────────
pub use memory::{delete_agent_kv_key, get_agent_kv, get_agent_kv_key, set_agent_kv_key};

// ── Health / status handlers ────────────────────────────────────────
pub use health::{
    health, health_detail, prometheus_metrics, security_status, shutdown, status, version,
};

// ── Skill handlers ──────────────────────────────────────────────────
pub use skills::{
    clawhub_browse, clawhub_install, clawhub_search, clawhub_skill_detail, create_skill,
    install_skill, list_skills, marketplace_search, uninstall_skill,
};

// ── Hand handlers ───────────────────────────────────────────────────
pub use hands::{
    activate_hand, check_hand_deps, deactivate_hand, get_hand, hand_instance_browser, hand_stats,
    install_hand_deps, list_active_hands, list_hands, pause_hand, resume_hand,
};

// ── MCP handlers ────────────────────────────────────────────────────
pub use mcp::{list_mcp_servers, mcp_http};

// ── Audit handlers ──────────────────────────────────────────────────
pub use audit::{audit_recent, audit_verify, logs_stream};

// ── Network handlers ────────────────────────────────────────────────
pub use network::{list_peers, network_status};

// ── Tool handlers ───────────────────────────────────────────────────
pub use tools::list_tools;

// ── Config handlers ─────────────────────────────────────────────────
pub use config::{config_reload, config_schema, config_set, get_config};

// ── Usage / budget handlers ─────────────────────────────────────────
pub use usage::{
    agent_budget_ranking, agent_budget_status, budget_status, update_budget, usage_by_model,
    usage_daily, usage_stats, usage_summary,
};

// ── Model catalog handlers ──────────────────────────────────────────
pub use models::{
    delete_provider_key, get_model, list_aliases, list_models, list_providers, set_provider_key,
    set_provider_url, test_provider,
};

// ── A2A handlers ────────────────────────────────────────────────────
pub use a2a::{
    a2a_agent_card, a2a_cancel_task, a2a_discover_external, a2a_external_task_status,
    a2a_get_task, a2a_list_agents, a2a_list_external_agents, a2a_send_external, a2a_send_task,
};

// ── Integration handlers ────────────────────────────────────────────
pub use integrations::{
    add_integration, integrations_health, list_available_integrations, list_integrations,
    reconnect_integration, reload_integrations, remove_integration,
};

// ── Schedule handlers ───────────────────────────────────────────────
pub use schedules::{
    create_schedule, delete_schedule, list_schedules, run_schedule, update_schedule,
};

// ── Approval handlers ───────────────────────────────────────────────
pub use approvals::{approve_request, create_approval, list_approvals, reject_request};

// ── Cron job handlers ───────────────────────────────────────────────
pub use cron::{
    create_cron_job, cron_job_status, delete_cron_job, list_cron_jobs, toggle_cron_job,
};

// ── Webhook handlers ────────────────────────────────────────────────
pub use webhooks::{webhook_agent, webhook_wake};

// ── Binding handlers ────────────────────────────────────────────────
pub use bindings::{add_binding, list_bindings, remove_binding};

// ── Pairing handlers ────────────────────────────────────────────────
pub use pairing::{
    pairing_complete, pairing_devices, pairing_notify, pairing_remove_device, pairing_request,
};

// ── Command handlers ────────────────────────────────────────────────
pub use commands::list_commands;

// ── Migration handlers ──────────────────────────────────────────────
pub use migrate::{migrate_detect, migrate_scan, run_migrate};
