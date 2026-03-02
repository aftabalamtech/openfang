//! Parallel tool execution engine.
//!
//! Provides `execute_tools_parallel()` — a drop-in replacement for the
//! sequential `for tool_call in tool_calls` loop in `agent_loop.rs`.
//!
//! ## Integration (one-line change in agent_loop.rs)
//!
//! Replace:
//! ```ignore
//! for tool_call in &response.tool_calls { ... }
//! ```
//! With:
//! ```ignore
//! let tool_result_blocks = parallel_tools::execute_tool_calls(
//!     &response.tool_calls, ...,
//! ).await;
//! ```
//!
//! ## Design
//!
//! - **Phase 1** (sequential): Loop guard checks + hook firing (mutable state).
//! - **Phase 2** (parallel): Allowed tools execute via `futures::future::join_all`.
//!   Single-tool calls skip the concurrency overhead entirely.
//! - **Phase 3** (sequential): Post-processing (truncation, hook after, warnings).
//!
//! Results are returned in the original call order for deterministic behavior.

use crate::context_budget::{truncate_tool_result_dynamic, ContextBudget};
use crate::hooks::HookRegistry;
use crate::kernel_handle::KernelHandle;
use crate::loop_guard::{LoopGuard, LoopGuardVerdict};
use crate::mcp::McpConnection;
use crate::tool_runner;
use crate::web_search::WebToolsContext;
use openfang_skills::registry::SkillRegistry;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::message::ContentBlock;
use openfang_types::tool::{ToolCall, ToolDefinition};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Timeout for individual tool executions (seconds).
const TOOL_TIMEOUT_SECS: u64 = 120;

/// Outcome from executing tool calls — either results or a circuit break error.
pub enum ToolBatchOutcome {
    /// All tool calls completed (some may have errors in their results).
    Results(Vec<ContentBlock>),
    /// A circuit breaker was triggered — the caller should abort the agent loop.
    CircuitBreak(OpenFangError),
}

/// Execute a batch of tool calls with parallel execution for independent tools.
///
/// This is a drop-in replacement for the sequential tool execution loop in
/// `agent_loop.rs`. It preserves all existing behavior (loop guard, hooks,
/// phase callbacks, truncation, approval denial detection) while adding
/// concurrent execution when multiple tools are returned by the LLM.
///
/// # Returns
///
/// - `ToolBatchOutcome::Results(blocks)` — tool result content blocks ready
///   to be appended as a user message.
/// - `ToolBatchOutcome::CircuitBreak(err)` — loop guard triggered circuit
///   break; the caller should save the session and return the error.
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls<'a>(
    tool_calls: &[ToolCall],
    loop_guard: &mut LoopGuard,
    kernel: Option<&'a Arc<dyn KernelHandle>>,
    available_tools: &[ToolDefinition],
    caller_id: &str,
    agent_name: &str,
    skill_registry: Option<&'a SkillRegistry>,
    mcp_connections: Option<&'a tokio::sync::Mutex<Vec<McpConnection>>>,
    web_ctx: Option<&'a WebToolsContext>,
    browser_ctx: Option<&'a crate::browser::BrowserManager>,
    hand_allowed_env: Option<&'a [String]>,
    workspace_root: Option<&'a Path>,
    media_engine: Option<&'a crate::media_understanding::MediaEngine>,
    exec_policy: Option<&'a openfang_types::config::ExecPolicy>,
    tts_engine: Option<&'a crate::tts::TtsEngine>,
    docker_config: Option<&'a openfang_types::config::DockerSandboxConfig>,
    process_manager: Option<&'a crate::process_manager::ProcessManager>,
    context_budget: &ContextBudget,
    hooks: Option<&'a HookRegistry>,
) -> ToolBatchOutcome {
    let allowed_tool_names: Vec<String> =
        available_tools.iter().map(|t| t.name.clone()).collect();

    // ── Phase 1: Sequential pre-checks ──
    struct PreChecked {
        tool_call: ToolCall,
        verdict: LoopGuardVerdict,
    }
    let mut allowed: Vec<PreChecked> = Vec::new();
    let mut result_blocks: Vec<ContentBlock> = Vec::new();

    for tool_call in tool_calls {
        let verdict = loop_guard.check(&tool_call.name, &tool_call.input);
        match &verdict {
            LoopGuardVerdict::CircuitBreak(msg) => {
                return ToolBatchOutcome::CircuitBreak(OpenFangError::Internal(msg.clone()));
            }
            LoopGuardVerdict::Block(msg) => {
                warn!(tool = %tool_call.name, "Tool call blocked by loop guard");
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content: msg.clone(),
                    is_error: true,
                });
                continue;
            }
            _ => {}
        }

        debug!(tool = %tool_call.name, id = %tool_call.id, "Executing tool");

        // Fire BeforeToolCall hook (can block execution)
        if let Some(hook_reg) = hooks {
            let ctx = crate::hooks::HookContext {
                agent_name,
                agent_id: caller_id,
                event: openfang_types::agent::HookEvent::BeforeToolCall,
                data: serde_json::json!({
                    "tool_name": &tool_call.name,
                    "input": &tool_call.input,
                }),
            };
            if let Err(reason) = hook_reg.fire(&ctx) {
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content: format!("Hook blocked tool '{}': {}", tool_call.name, reason),
                    is_error: true,
                });
                continue;
            }
        }

        allowed.push(PreChecked {
            tool_call: tool_call.clone(),
            verdict,
        });
    }

    // ── Phase 2: Execute (parallel when >1 tool, sequential otherwise) ──
    let executed: Vec<(openfang_types::tool::ToolResult, LoopGuardVerdict, String)> =
        if allowed.len() <= 1 {
            // 0 or 1 tool — run directly, no concurrency overhead
            let mut out = Vec::with_capacity(allowed.len());
            for pre in allowed {
                let result = execute_single(
                    &pre.tool_call, kernel, &allowed_tool_names, caller_id,
                    skill_registry, mcp_connections, web_ctx, browser_ctx,
                    hand_allowed_env, workspace_root, media_engine, exec_policy,
                    tts_engine, docker_config, process_manager,
                ).await;
                out.push((result, pre.verdict, pre.tool_call.name.clone()));
            }
            out
        } else {
            // Multiple tools — execute in parallel via join_all
            info!(count = allowed.len(), "Executing {} tool calls in parallel", allowed.len());

            let names_ref = &allowed_tool_names;
            let futures: Vec<_> = allowed
                .into_iter()
                .map(|pre| {
                    let tc = pre.tool_call;
                    let verdict = pre.verdict;
                    async move {
                        let result = execute_single(
                            &tc, kernel, names_ref, caller_id,
                            skill_registry, mcp_connections, web_ctx, browser_ctx,
                            hand_allowed_env, workspace_root, media_engine, exec_policy,
                            tts_engine, docker_config, process_manager,
                        ).await;
                        let name = tc.name.clone();
                        (result, verdict, name)
                    }
                })
                .collect();

            // join_all preserves input order → deterministic result ordering
            futures::future::join_all(futures).await
        };

    // ── Phase 3: Sequential post-processing ──
    for (result, verdict, tool_name) in executed {
        // Fire AfterToolCall hook
        if let Some(hook_reg) = hooks {
            let ctx = crate::hooks::HookContext {
                agent_name,
                agent_id: caller_id,
                event: openfang_types::agent::HookEvent::AfterToolCall,
                data: serde_json::json!({
                    "tool_name": &tool_name,
                    "result": &result.content,
                    "is_error": result.is_error,
                }),
            };
            let _ = hook_reg.fire(&ctx);
        }

        let content = truncate_tool_result_dynamic(&result.content, context_budget);

        let final_content = if let LoopGuardVerdict::Warn(ref warn_msg) = verdict {
            format!("{content}\n\n[LOOP GUARD] {warn_msg}")
        } else {
            content
        };

        result_blocks.push(ContentBlock::ToolResult {
            tool_use_id: result.tool_use_id,
            tool_name,
            content: final_content,
            is_error: result.is_error,
        });
    }

    // Detect approval denials
    let denial_count = result_blocks.iter().filter(|b| {
        matches!(b, ContentBlock::ToolResult { content, is_error: true, .. }
            if content.contains("requires human approval and was denied"))
    }).count();
    if denial_count > 0 {
        result_blocks.push(ContentBlock::Text {
            text: format!(
                "[System: {} tool call(s) were denied by approval policy. \
                 Do NOT retry denied tools. Explain to the user what you \
                 wanted to do and that it requires their approval.]",
                denial_count
            ),
        });
    }

    ToolBatchOutcome::Results(result_blocks)
}

/// Execute a single tool call with timeout.
#[allow(clippy::too_many_arguments)]
async fn execute_single<'a>(
    tool_call: &ToolCall,
    kernel: Option<&'a Arc<dyn KernelHandle>>,
    allowed_tool_names: &[String],
    caller_id: &str,
    skill_registry: Option<&'a SkillRegistry>,
    mcp_connections: Option<&'a tokio::sync::Mutex<Vec<McpConnection>>>,
    web_ctx: Option<&'a WebToolsContext>,
    browser_ctx: Option<&'a crate::browser::BrowserManager>,
    hand_allowed_env: Option<&'a [String]>,
    workspace_root: Option<&'a Path>,
    media_engine: Option<&'a crate::media_understanding::MediaEngine>,
    exec_policy: Option<&'a openfang_types::config::ExecPolicy>,
    tts_engine: Option<&'a crate::tts::TtsEngine>,
    docker_config: Option<&'a openfang_types::config::DockerSandboxConfig>,
    process_manager: Option<&'a crate::process_manager::ProcessManager>,
) -> openfang_types::tool::ToolResult {
    match tokio::time::timeout(
        Duration::from_secs(TOOL_TIMEOUT_SECS),
        tool_runner::execute_tool(
            &tool_call.id,
            &tool_call.name,
            &tool_call.input,
            kernel,
            Some(allowed_tool_names),
            Some(caller_id),
            skill_registry,
            mcp_connections,
            web_ctx,
            browser_ctx,
            hand_allowed_env,
            workspace_root,
            media_engine,
            exec_policy,
            tts_engine,
            docker_config,
            process_manager,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            warn!(
                tool = %tool_call.name,
                "Tool execution timed out after {}s", TOOL_TIMEOUT_SECS
            );
            openfang_types::tool::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: format!(
                    "Tool '{}' timed out after {}s.",
                    tool_call.name, TOOL_TIMEOUT_SECS
                ),
                is_error: true,
            }
        }
    }
}
