//! Workflow CRUD and execution.

use crate::routes::common::*;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use openfang_kernel::workflow::{
    ErrorMode, StepAgent, StepMode, Workflow, WorkflowId, WorkflowStep,
};
use std::sync::Arc;

pub async fn create_workflow(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = req["name"].as_str().unwrap_or("unnamed").to_string();
    let description = req["description"].as_str().unwrap_or("").to_string();

    let steps_json = match req["steps"].as_array() {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing 'steps' array"})),
            );
        }
    };

    let mut steps = Vec::new();
    for s in steps_json {
        let step_name = s["name"].as_str().unwrap_or("step").to_string();
        let agent = if let Some(id) = s["agent_id"].as_str() {
            StepAgent::ById { id: id.to_string() }
        } else if let Some(name) = s["agent_name"].as_str() {
            StepAgent::ByName {
                name: name.to_string(),
            }
        } else {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({"error": format!("Step '{}' needs 'agent_id' or 'agent_name'", step_name)}),
                ),
            );
        };

        let mode = match s["mode"].as_str().unwrap_or("sequential") {
            "fan_out" => StepMode::FanOut,
            "collect" => StepMode::Collect,
            "conditional" => StepMode::Conditional {
                condition: s["condition"].as_str().unwrap_or("").to_string(),
            },
            "loop" => StepMode::Loop {
                max_iterations: s["max_iterations"].as_u64().unwrap_or(5) as u32,
                until: s["until"].as_str().unwrap_or("").to_string(),
            },
            _ => StepMode::Sequential,
        };

        let error_mode = match s["error_mode"].as_str().unwrap_or("fail") {
            "skip" => ErrorMode::Skip,
            "retry" => ErrorMode::Retry {
                max_retries: s["max_retries"].as_u64().unwrap_or(3) as u32,
            },
            _ => ErrorMode::Fail,
        };

        steps.push(WorkflowStep {
            name: step_name,
            agent,
            prompt_template: s["prompt"].as_str().unwrap_or("{{input}}").to_string(),
            mode,
            timeout_secs: s["timeout_secs"].as_u64().unwrap_or(120),
            error_mode,
            output_var: s["output_var"].as_str().map(String::from),
        });
    }

    let workflow = Workflow {
        id: WorkflowId::new(),
        name,
        description,
        steps,
        created_at: chrono::Utc::now(),
    };

    let id = state.kernel.register_workflow(workflow).await;
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"workflow_id": id.to_string()})),
    )
}

/// GET /api/workflows — List all workflows.
pub async fn list_workflows(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let workflows = state.kernel.workflows.list_workflows().await;
    let list: Vec<serde_json::Value> = workflows
        .iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id.to_string(),
                "name": w.name,
                "description": w.description,
                "steps": w.steps.len(),
                "created_at": w.created_at.to_rfc3339(),
            })
        })
        .collect();
    Json(list)
}

/// POST /api/workflows/:id/run — Execute a workflow.
pub async fn run_workflow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let workflow_id = WorkflowId(match id.parse() {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Invalid workflow ID"})),
            );
        }
    });

    let input = req["input"].as_str().unwrap_or("").to_string();

    match state.kernel.run_workflow(workflow_id, input).await {
        Ok((run_id, output)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "run_id": run_id.to_string(),
                "output": output,
                "status": "completed",
            })),
        ),
        Err(e) => {
            tracing::warn!("Workflow run failed for {id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Workflow execution failed"})),
            )
        }
    }
}

/// GET /api/workflows/:id/runs — List runs for a workflow.
pub async fn list_workflow_runs(
    State(state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> impl IntoResponse {
    let runs = state.kernel.workflows.list_runs(None).await;
    let list: Vec<serde_json::Value> = runs
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id.to_string(),
                "workflow_name": r.workflow_name,
                "state": serde_json::to_value(&r.state).unwrap_or_default(),
                "steps_completed": r.step_results.len(),
                "started_at": r.started_at.to_rfc3339(),
                "completed_at": r.completed_at.map(|t| t.to_rfc3339()),
            })
        })
        .collect();
    Json(list)
}
