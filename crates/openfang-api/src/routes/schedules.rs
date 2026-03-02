//! Cron-based scheduled job management.

use crate::routes::common::*;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use openfang_types::agent::AgentId;
use std::sync::Arc;

/// The well-known shared-memory agent ID used for cross-agent KV storage.
/// Must match the value in `openfang-kernel/src/kernel.rs::shared_memory_agent_id()`.
fn schedule_shared_agent_id() -> AgentId {
    AgentId(uuid::Uuid::from_bytes([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ]))
}

const SCHEDULES_KEY: &str = "__openfang_schedules";

/// GET /api/schedules — List all cron-based scheduled jobs.
pub async fn list_schedules(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let agent_id = schedule_shared_agent_id();
    match state.kernel.memory.structured_get(agent_id, SCHEDULES_KEY) {
        Ok(Some(serde_json::Value::Array(arr))) => {
            let total = arr.len();
            Json(serde_json::json!({"schedules": arr, "total": total}))
        }
        Ok(_) => Json(serde_json::json!({"schedules": [], "total": 0})),
        Err(e) => {
            tracing::warn!("Failed to load schedules: {e}");
            Json(serde_json::json!({"schedules": [], "total": 0, "error": format!("{e}")}))
        }
    }
}

/// POST /api/schedules — Create a new cron-based scheduled job.
pub async fn create_schedule(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = match req["name"].as_str() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing 'name' field"})),
            );
        }
    };

    let cron = match req["cron"].as_str() {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing 'cron' field"})),
            );
        }
    };

    // Validate cron expression: must be 5 space-separated fields
    let cron_parts: Vec<&str> = cron.split_whitespace().collect();
    if cron_parts.len() != 5 {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": "Invalid cron expression: must have 5 fields (min hour dom mon dow)"}),
            ),
        );
    }

    let agent_id_str = req["agent_id"].as_str().unwrap_or("").to_string();
    let message = req["message"].as_str().unwrap_or("").to_string();
    let enabled = req.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

    let schedule_id = uuid::Uuid::new_v4().to_string();
    let entry = serde_json::json!({
        "id": schedule_id,
        "name": name,
        "cron": cron,
        "agent_id": agent_id_str,
        "message": message,
        "enabled": enabled,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "last_run": null,
        "run_count": 0,
    });

    let shared_id = schedule_shared_agent_id();
    let mut schedules: Vec<serde_json::Value> =
        match state.kernel.memory.structured_get(shared_id, SCHEDULES_KEY) {
            Ok(Some(serde_json::Value::Array(arr))) => arr,
            _ => Vec::new(),
        };

    schedules.push(entry.clone());
    if let Err(e) = state.kernel.memory.structured_set(
        shared_id,
        SCHEDULES_KEY,
        serde_json::Value::Array(schedules),
    ) {
        tracing::warn!("Failed to save schedule: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to save schedule: {e}")})),
        );
    }

    (StatusCode::CREATED, Json(entry))
}

/// PUT /api/schedules/:id — Update a scheduled job (toggle enabled, edit fields).
pub async fn update_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let shared_id = schedule_shared_agent_id();
    let mut schedules: Vec<serde_json::Value> =
        match state.kernel.memory.structured_get(shared_id, SCHEDULES_KEY) {
            Ok(Some(serde_json::Value::Array(arr))) => arr,
            _ => Vec::new(),
        };

    let mut found = false;
    for s in schedules.iter_mut() {
        if s["id"].as_str() == Some(&id) {
            found = true;
            if let Some(enabled) = req.get("enabled").and_then(|v| v.as_bool()) {
                s["enabled"] = serde_json::Value::Bool(enabled);
            }
            if let Some(name) = req.get("name").and_then(|v| v.as_str()) {
                s["name"] = serde_json::Value::String(name.to_string());
            }
            if let Some(cron) = req.get("cron").and_then(|v| v.as_str()) {
                let cron_parts: Vec<&str> = cron.split_whitespace().collect();
                if cron_parts.len() != 5 {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": "Invalid cron expression"})),
                    );
                }
                s["cron"] = serde_json::Value::String(cron.to_string());
            }
            if let Some(agent_id) = req.get("agent_id").and_then(|v| v.as_str()) {
                s["agent_id"] = serde_json::Value::String(agent_id.to_string());
            }
            if let Some(message) = req.get("message").and_then(|v| v.as_str()) {
                s["message"] = serde_json::Value::String(message.to_string());
            }
            break;
        }
    }

    if !found {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Schedule not found"})),
        );
    }

    if let Err(e) = state.kernel.memory.structured_set(
        shared_id,
        SCHEDULES_KEY,
        serde_json::Value::Array(schedules),
    ) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to update schedule: {e}")})),
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "updated", "schedule_id": id})),
    )
}

/// DELETE /api/schedules/:id — Remove a scheduled job.
pub async fn delete_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let shared_id = schedule_shared_agent_id();
    let mut schedules: Vec<serde_json::Value> =
        match state.kernel.memory.structured_get(shared_id, SCHEDULES_KEY) {
            Ok(Some(serde_json::Value::Array(arr))) => arr,
            _ => Vec::new(),
        };

    let before = schedules.len();
    schedules.retain(|s| s["id"].as_str() != Some(&id));

    if schedules.len() == before {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Schedule not found"})),
        );
    }

    if let Err(e) = state.kernel.memory.structured_set(
        shared_id,
        SCHEDULES_KEY,
        serde_json::Value::Array(schedules),
    ) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to delete schedule: {e}")})),
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "removed", "schedule_id": id})),
    )
}

/// POST /api/schedules/:id/run — Manually run a scheduled job now.
pub async fn run_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let shared_id = schedule_shared_agent_id();
    let schedules: Vec<serde_json::Value> =
        match state.kernel.memory.structured_get(shared_id, SCHEDULES_KEY) {
            Ok(Some(serde_json::Value::Array(arr))) => arr,
            _ => Vec::new(),
        };

    let schedule = match schedules.iter().find(|s| s["id"].as_str() == Some(&id)) {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Schedule not found"})),
            );
        }
    };

    let agent_id_str = schedule["agent_id"].as_str().unwrap_or("");
    let message = schedule["message"]
        .as_str()
        .unwrap_or("Scheduled task triggered manually.");
    let name = schedule["name"].as_str().unwrap_or("(unnamed)");

    // Find the target agent
    let target_agent = if !agent_id_str.is_empty() {
        if let Ok(aid) = agent_id_str.parse::<AgentId>() {
            Some(aid)
        } else {
            state
                .kernel
                .registry
                .list()
                .iter()
                .find(|a| a.name == agent_id_str)
                .map(|a| a.id)
        }
    } else {
        state.kernel.registry.list().first().map(|a| a.id)
    };

    let target_agent = match target_agent {
        Some(a) => a,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(
                    serde_json::json!({"error": "No target agent found. Specify an agent_id or start an agent first."}),
                ),
            );
        }
    };

    let run_message = if message.is_empty() {
        format!("[Scheduled task '{}' triggered manually]", name)
    } else {
        message.to_string()
    };

    // Update last_run and run_count
    let mut schedules_updated: Vec<serde_json::Value> =
        match state.kernel.memory.structured_get(shared_id, SCHEDULES_KEY) {
            Ok(Some(serde_json::Value::Array(arr))) => arr,
            _ => Vec::new(),
        };
    for s in schedules_updated.iter_mut() {
        if s["id"].as_str() == Some(&id) {
            s["last_run"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
            let count = s["run_count"].as_u64().unwrap_or(0);
            s["run_count"] = serde_json::json!(count + 1);
            break;
        }
    }
    let _ = state.kernel.memory.structured_set(
        shared_id,
        SCHEDULES_KEY,
        serde_json::Value::Array(schedules_updated),
    );

    match state.kernel.send_message(target_agent, &run_message).await {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "completed",
                "schedule_id": id,
                "agent_id": target_agent.to_string(),
                "response": result.response,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "status": "failed",
                "schedule_id": id,
                "error": format!("{e}"),
            })),
        ),
    }
}
