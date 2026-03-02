//! Agent telemetry — structured observability for agent loop execution.
//!
//! Provides production-grade telemetry for every agent loop execution:
//! - **Execution traces**: Full waterfall of memory recall → prompt build → LLM call → tool execution
//! - **Metrics counters**: Token usage, cost, latency, tool calls, errors — per agent and global
//! - **Export hooks**: Pluggable sinks for Prometheus, OTLP, or custom backends
//!
//! This module is designed to integrate with OpenTelemetry-compatible backends
//! via the `tracing` crate's span/event model, while also maintaining in-process
//! metrics for the `/api/metrics` endpoint.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{info_span, Span};

// ---------------------------------------------------------------------------
// Execution Trace — structured record of a single agent loop run
// ---------------------------------------------------------------------------

/// A complete execution trace for a single agent loop invocation.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionTrace {
    /// Unique trace identifier.
    pub trace_id: String,
    /// Agent that executed this trace.
    pub agent_id: String,
    /// Agent display name.
    pub agent_name: String,
    /// When the trace started.
    pub started_at: DateTime<Utc>,
    /// Total wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Ordered list of trace phases (waterfall).
    pub phases: Vec<TracePhase>,
    /// Total LLM input tokens consumed.
    pub total_input_tokens: u64,
    /// Total LLM output tokens consumed.
    pub total_output_tokens: u64,
    /// Estimated total cost in USD.
    pub total_cost_usd: f64,
    /// Number of loop iterations.
    pub iterations: u32,
    /// Final outcome.
    pub outcome: TraceOutcome,
}

/// Outcome of an agent loop execution.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    Success,
    Error { message: String },
    CircuitBreak { reason: String },
    MaxIterations,
}

/// A single phase within an execution trace.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TracePhase {
    /// Memory recall phase.
    MemoryRecall {
        query: String,
        results_count: usize,
        duration_ms: u64,
    },
    /// Prompt construction phase.
    PromptBuild {
        system_prompt_tokens: usize,
        history_tokens: usize,
        tool_definition_tokens: usize,
        total_tokens: usize,
        duration_ms: u64,
    },
    /// LLM API call.
    LlmCall {
        model: String,
        provider: String,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
        stop_reason: String,
        is_streaming: bool,
    },
    /// Tool execution.
    ToolExecution {
        tool_name: String,
        tool_id: String,
        duration_ms: u64,
        is_error: bool,
        result_preview: String,
        was_parallel: bool,
    },
    /// Session repair phase.
    SessionRepair {
        orphaned_removed: usize,
        empty_removed: usize,
        merged: usize,
        synthetic_inserted: usize,
        duration_ms: u64,
    },
    /// Context compaction phase.
    Compaction {
        before_messages: usize,
        after_messages: usize,
        chunks_used: u32,
        duration_ms: u64,
    },
}

// ---------------------------------------------------------------------------
// Trace Builder — accumulates phases during agent loop execution
// ---------------------------------------------------------------------------

/// Builder for constructing an `ExecutionTrace` incrementally during agent loop execution.
pub struct TraceBuilder {
    trace_id: String,
    agent_id: String,
    agent_name: String,
    started_at: DateTime<Utc>,
    start_instant: Instant,
    phases: Vec<TracePhase>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cost_usd: f64,
    iterations: u32,
    /// The tracing span for this execution (carries structured fields for export).
    span: Span,
}

impl TraceBuilder {
    /// Start a new trace for an agent loop execution.
    pub fn new(agent_id: &str, agent_name: &str) -> Self {
        let trace_id = uuid::Uuid::new_v4().to_string();
        let span = info_span!(
            "agent_loop",
            otel.name = "agent_loop",
            trace_id = %trace_id,
            agent.id = %agent_id,
            agent.name = %agent_name,
            agent.iterations = tracing::field::Empty,
            agent.input_tokens = tracing::field::Empty,
            agent.output_tokens = tracing::field::Empty,
            agent.cost_usd = tracing::field::Empty,
            agent.outcome = tracing::field::Empty,
        );
        Self {
            trace_id,
            agent_id: agent_id.to_string(),
            agent_name: agent_name.to_string(),
            started_at: Utc::now(),
            start_instant: Instant::now(),
            phases: Vec::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            iterations: 0,
            span,
        }
    }

    /// Get a reference to the tracing span (for entering in the agent loop).
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// Record a memory recall phase.
    pub fn record_memory_recall(&mut self, query: &str, results_count: usize, duration: std::time::Duration) {
        let _guard = self.span.enter();
        let duration_ms = duration.as_millis() as u64;
        tracing::debug!(
            query = %query,
            results = results_count,
            duration_ms = duration_ms,
            "memory.recall"
        );
        self.phases.push(TracePhase::MemoryRecall {
            query: query.chars().take(200).collect(),
            results_count,
            duration_ms,
        });
    }

    /// Record a prompt construction phase.
    pub fn record_prompt_build(
        &mut self,
        system_prompt_tokens: usize,
        history_tokens: usize,
        tool_definition_tokens: usize,
        duration: std::time::Duration,
    ) {
        let total = system_prompt_tokens + history_tokens + tool_definition_tokens;
        let duration_ms = duration.as_millis() as u64;
        self.phases.push(TracePhase::PromptBuild {
            system_prompt_tokens,
            history_tokens,
            tool_definition_tokens,
            total_tokens: total,
            duration_ms,
        });
    }

    /// Record an LLM API call.
    pub fn record_llm_call(
        &mut self,
        model: &str,
        provider: &str,
        input_tokens: u64,
        output_tokens: u64,
        duration: std::time::Duration,
        stop_reason: &str,
        is_streaming: bool,
    ) {
        let _guard = self.span.enter();
        let duration_ms = duration.as_millis() as u64;
        tracing::info!(
            model = %model,
            provider = %provider,
            input_tokens = input_tokens,
            output_tokens = output_tokens,
            duration_ms = duration_ms,
            stop_reason = %stop_reason,
            streaming = is_streaming,
            "llm.call"
        );
        self.total_input_tokens += input_tokens;
        self.total_output_tokens += output_tokens;
        self.iterations += 1;
        self.phases.push(TracePhase::LlmCall {
            model: model.to_string(),
            provider: provider.to_string(),
            input_tokens,
            output_tokens,
            duration_ms,
            stop_reason: stop_reason.to_string(),
            is_streaming,
        });
    }

    /// Record a tool execution.
    pub fn record_tool_execution(
        &mut self,
        tool_name: &str,
        tool_id: &str,
        duration: std::time::Duration,
        is_error: bool,
        result_preview: &str,
        was_parallel: bool,
    ) {
        let _guard = self.span.enter();
        let duration_ms = duration.as_millis() as u64;
        tracing::debug!(
            tool.name = %tool_name,
            tool.id = %tool_id,
            duration_ms = duration_ms,
            is_error = is_error,
            parallel = was_parallel,
            "tool.execute"
        );
        self.phases.push(TracePhase::ToolExecution {
            tool_name: tool_name.to_string(),
            tool_id: tool_id.to_string(),
            duration_ms,
            is_error,
            result_preview: result_preview.chars().take(200).collect(),
            was_parallel,
        });
    }

    /// Record a session repair phase.
    pub fn record_session_repair(
        &mut self,
        orphaned_removed: usize,
        empty_removed: usize,
        merged: usize,
        synthetic_inserted: usize,
        duration: std::time::Duration,
    ) {
        self.phases.push(TracePhase::SessionRepair {
            orphaned_removed,
            empty_removed,
            merged,
            synthetic_inserted,
            duration_ms: duration.as_millis() as u64,
        });
    }

    /// Record a compaction phase.
    pub fn record_compaction(
        &mut self,
        before_messages: usize,
        after_messages: usize,
        chunks_used: u32,
        duration: std::time::Duration,
    ) {
        self.phases.push(TracePhase::Compaction {
            before_messages,
            after_messages,
            chunks_used,
            duration_ms: duration.as_millis() as u64,
        });
    }

    /// Set the estimated cost.
    pub fn set_cost(&mut self, cost_usd: f64) {
        self.total_cost_usd = cost_usd;
    }

    /// Finalize the trace with the given outcome.
    pub fn finish(self, outcome: TraceOutcome) -> ExecutionTrace {
        let duration_ms = self.start_instant.elapsed().as_millis() as u64;

        // Record final fields on the tracing span
        self.span.record("agent.iterations", self.iterations);
        self.span.record("agent.input_tokens", self.total_input_tokens);
        self.span.record("agent.output_tokens", self.total_output_tokens);
        self.span.record("agent.cost_usd", self.total_cost_usd);
        let outcome_str = match &outcome {
            TraceOutcome::Success => "success",
            TraceOutcome::Error { .. } => "error",
            TraceOutcome::CircuitBreak { .. } => "circuit_break",
            TraceOutcome::MaxIterations => "max_iterations",
        };
        self.span.record("agent.outcome", outcome_str);

        ExecutionTrace {
            trace_id: self.trace_id,
            agent_id: self.agent_id,
            agent_name: self.agent_name,
            started_at: self.started_at,
            duration_ms,
            phases: self.phases,
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
            total_cost_usd: self.total_cost_usd,
            iterations: self.iterations,
            outcome,
        }
    }
}

// ---------------------------------------------------------------------------
// Metrics Registry — in-process counters for Prometheus / OTLP export
// ---------------------------------------------------------------------------

/// Global metrics registry for OpenFang observability.
///
/// All counters are atomic — safe for concurrent updates from multiple agent tasks.
/// Designed for export to Prometheus (`/api/metrics`) or OTLP.
pub struct MetricsRegistry {
    // ── Agent metrics ──
    pub agent_loop_total: AtomicU64,
    pub agent_loop_errors: AtomicU64,
    pub agent_loop_duration_ms: AtomicU64,

    // ── LLM metrics ──
    pub llm_calls_total: AtomicU64,
    pub llm_input_tokens_total: AtomicU64,
    pub llm_output_tokens_total: AtomicU64,
    pub llm_errors_total: AtomicU64,
    pub llm_cost_usd_micros: AtomicU64, // in micro-dollars for atomic precision

    // ── Tool metrics ──
    pub tool_calls_total: AtomicU64,
    pub tool_errors_total: AtomicU64,
    pub tool_parallel_batches: AtomicU64,
    pub tool_timeouts_total: AtomicU64,

    // ── Memory metrics ──
    pub memory_store_total: AtomicU64,
    pub memory_recall_total: AtomicU64,

    // ── Channel metrics ──
    pub channel_messages_received: AtomicU64,
    pub channel_messages_sent: AtomicU64,
    pub channel_delivery_errors: AtomicU64,

    // ── Security metrics ──
    pub loop_guard_blocks: AtomicU64,
    pub loop_guard_circuit_breaks: AtomicU64,
    pub taint_violations: AtomicU64,
    pub capability_denials: AtomicU64,

    // ── Per-agent counters (for /api/budget/agents ranking) ──
    per_agent: dashmap::DashMap<String, AgentMetrics>,

    // ── Per-model counters ──
    per_model: dashmap::DashMap<String, ModelMetrics>,
}

/// Per-agent metric counters.
#[derive(Debug, Default)]
pub struct AgentMetrics {
    pub loop_count: AtomicU64,
    pub input_tokens: AtomicU64,
    pub output_tokens: AtomicU64,
    pub cost_usd_micros: AtomicU64,
    pub tool_calls: AtomicU64,
    pub errors: AtomicU64,
}

/// Per-model metric counters.
#[derive(Debug, Default)]
pub struct ModelMetrics {
    pub calls: AtomicU64,
    pub input_tokens: AtomicU64,
    pub output_tokens: AtomicU64,
    pub total_duration_ms: AtomicU64,
    pub errors: AtomicU64,
}

impl MetricsRegistry {
    /// Create a new empty metrics registry.
    pub fn new() -> Self {
        Self {
            agent_loop_total: AtomicU64::new(0),
            agent_loop_errors: AtomicU64::new(0),
            agent_loop_duration_ms: AtomicU64::new(0),
            llm_calls_total: AtomicU64::new(0),
            llm_input_tokens_total: AtomicU64::new(0),
            llm_output_tokens_total: AtomicU64::new(0),
            llm_errors_total: AtomicU64::new(0),
            llm_cost_usd_micros: AtomicU64::new(0),
            tool_calls_total: AtomicU64::new(0),
            tool_errors_total: AtomicU64::new(0),
            tool_parallel_batches: AtomicU64::new(0),
            tool_timeouts_total: AtomicU64::new(0),
            memory_store_total: AtomicU64::new(0),
            memory_recall_total: AtomicU64::new(0),
            channel_messages_received: AtomicU64::new(0),
            channel_messages_sent: AtomicU64::new(0),
            channel_delivery_errors: AtomicU64::new(0),
            loop_guard_blocks: AtomicU64::new(0),
            loop_guard_circuit_breaks: AtomicU64::new(0),
            taint_violations: AtomicU64::new(0),
            capability_denials: AtomicU64::new(0),
            per_agent: dashmap::DashMap::new(),
            per_model: dashmap::DashMap::new(),
        }
    }

    /// Record completion of an agent loop execution.
    pub fn record_agent_loop(&self, agent_id: &str, duration_ms: u64, is_error: bool) {
        self.agent_loop_total.fetch_add(1, Ordering::Relaxed);
        self.agent_loop_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
        if is_error {
            self.agent_loop_errors.fetch_add(1, Ordering::Relaxed);
        }
        let agent = self.per_agent.entry(agent_id.to_string()).or_default();
        agent.loop_count.fetch_add(1, Ordering::Relaxed);
        if is_error {
            agent.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record an LLM call.
    pub fn record_llm_call(
        &self,
        agent_id: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
        cost_usd: f64,
        is_error: bool,
    ) {
        self.llm_calls_total.fetch_add(1, Ordering::Relaxed);
        self.llm_input_tokens_total.fetch_add(input_tokens, Ordering::Relaxed);
        self.llm_output_tokens_total.fetch_add(output_tokens, Ordering::Relaxed);
        let cost_micros = (cost_usd * 1_000_000.0) as u64;
        self.llm_cost_usd_micros.fetch_add(cost_micros, Ordering::Relaxed);
        if is_error {
            self.llm_errors_total.fetch_add(1, Ordering::Relaxed);
        }

        // Per-agent
        let agent = self.per_agent.entry(agent_id.to_string()).or_default();
        agent.input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        agent.output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        agent.cost_usd_micros.fetch_add(cost_micros, Ordering::Relaxed);

        // Per-model
        let model_entry = self.per_model.entry(model.to_string()).or_default();
        model_entry.calls.fetch_add(1, Ordering::Relaxed);
        model_entry.input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        model_entry.output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        model_entry.total_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
        if is_error {
            model_entry.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a tool execution.
    pub fn record_tool_call(&self, agent_id: &str, is_error: bool, is_timeout: bool) {
        self.tool_calls_total.fetch_add(1, Ordering::Relaxed);
        if is_error {
            self.tool_errors_total.fetch_add(1, Ordering::Relaxed);
        }
        if is_timeout {
            self.tool_timeouts_total.fetch_add(1, Ordering::Relaxed);
        }
        let agent = self.per_agent.entry(agent_id.to_string()).or_default();
        agent.tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a parallel tool batch.
    pub fn record_parallel_batch(&self) {
        self.tool_parallel_batches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a loop guard action.
    pub fn record_loop_guard_block(&self) {
        self.loop_guard_blocks.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a circuit break.
    pub fn record_circuit_break(&self) {
        self.loop_guard_circuit_breaks.fetch_add(1, Ordering::Relaxed);
    }

    /// Export all metrics in Prometheus text exposition format.
    pub fn export_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        // Agent metrics
        prom_counter(&mut out, "openfang_agent_loop_total", "Total agent loop executions", self.agent_loop_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_agent_loop_errors_total", "Agent loop errors", self.agent_loop_errors.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_agent_loop_duration_ms_total", "Cumulative agent loop duration in ms", self.agent_loop_duration_ms.load(Ordering::Relaxed));

        // LLM metrics
        prom_counter(&mut out, "openfang_llm_calls_total", "Total LLM API calls", self.llm_calls_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_llm_input_tokens_total", "Total LLM input tokens", self.llm_input_tokens_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_llm_output_tokens_total", "Total LLM output tokens", self.llm_output_tokens_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_llm_errors_total", "Total LLM errors", self.llm_errors_total.load(Ordering::Relaxed));
        let cost_usd = self.llm_cost_usd_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        out.push_str(&format!("# HELP openfang_llm_cost_usd_total Total estimated LLM cost in USD\n# TYPE openfang_llm_cost_usd_total counter\nopenfang_llm_cost_usd_total {:.6}\n", cost_usd));

        // Tool metrics
        prom_counter(&mut out, "openfang_tool_calls_total", "Total tool executions", self.tool_calls_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_tool_errors_total", "Tool execution errors", self.tool_errors_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_tool_timeouts_total", "Tool execution timeouts", self.tool_timeouts_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_tool_parallel_batches_total", "Parallel tool execution batches", self.tool_parallel_batches.load(Ordering::Relaxed));

        // Memory metrics
        prom_counter(&mut out, "openfang_memory_store_total", "Memory store operations", self.memory_store_total.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_memory_recall_total", "Memory recall operations", self.memory_recall_total.load(Ordering::Relaxed));

        // Channel metrics
        prom_counter(&mut out, "openfang_channel_messages_received_total", "Messages received from channels", self.channel_messages_received.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_channel_messages_sent_total", "Messages sent to channels", self.channel_messages_sent.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_channel_delivery_errors_total", "Channel delivery errors", self.channel_delivery_errors.load(Ordering::Relaxed));

        // Security metrics
        prom_counter(&mut out, "openfang_loop_guard_blocks_total", "Tool calls blocked by loop guard", self.loop_guard_blocks.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_loop_guard_circuit_breaks_total", "Circuit breaker activations", self.loop_guard_circuit_breaks.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_taint_violations_total", "Taint tracking violations", self.taint_violations.load(Ordering::Relaxed));
        prom_counter(&mut out, "openfang_capability_denials_total", "Capability check denials", self.capability_denials.load(Ordering::Relaxed));

        // Per-model metrics
        for entry in self.per_model.iter() {
            let model = entry.key();
            let m = entry.value();
            let calls = m.calls.load(Ordering::Relaxed);
            let input = m.input_tokens.load(Ordering::Relaxed);
            let output = m.output_tokens.load(Ordering::Relaxed);
            let dur = m.total_duration_ms.load(Ordering::Relaxed);
            let errs = m.errors.load(Ordering::Relaxed);
            out.push_str(&format!(
                "openfang_llm_calls_by_model{{model=\"{}\"}} {}\n\
                 openfang_llm_input_tokens_by_model{{model=\"{}\"}} {}\n\
                 openfang_llm_output_tokens_by_model{{model=\"{}\"}} {}\n\
                 openfang_llm_duration_ms_by_model{{model=\"{}\"}} {}\n\
                 openfang_llm_errors_by_model{{model=\"{}\"}} {}\n",
                model, calls, model, input, model, output, model, dur, model, errs
            ));
        }

        out
    }

    /// Export summary as a JSON-serializable snapshot.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            agent_loop_total: self.agent_loop_total.load(Ordering::Relaxed),
            agent_loop_errors: self.agent_loop_errors.load(Ordering::Relaxed),
            llm_calls_total: self.llm_calls_total.load(Ordering::Relaxed),
            llm_input_tokens: self.llm_input_tokens_total.load(Ordering::Relaxed),
            llm_output_tokens: self.llm_output_tokens_total.load(Ordering::Relaxed),
            llm_cost_usd: self.llm_cost_usd_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0,
            tool_calls_total: self.tool_calls_total.load(Ordering::Relaxed),
            tool_errors: self.tool_errors_total.load(Ordering::Relaxed),
            tool_parallel_batches: self.tool_parallel_batches.load(Ordering::Relaxed),
            channel_messages_in: self.channel_messages_received.load(Ordering::Relaxed),
            channel_messages_out: self.channel_messages_sent.load(Ordering::Relaxed),
            loop_guard_blocks: self.loop_guard_blocks.load(Ordering::Relaxed),
            circuit_breaks: self.loop_guard_circuit_breaks.load(Ordering::Relaxed),
        }
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// JSON-serializable metrics snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub agent_loop_total: u64,
    pub agent_loop_errors: u64,
    pub llm_calls_total: u64,
    pub llm_input_tokens: u64,
    pub llm_output_tokens: u64,
    pub llm_cost_usd: f64,
    pub tool_calls_total: u64,
    pub tool_errors: u64,
    pub tool_parallel_batches: u64,
    pub channel_messages_in: u64,
    pub channel_messages_out: u64,
    pub loop_guard_blocks: u64,
    pub circuit_breaks: u64,
}

// ---------------------------------------------------------------------------
// Trace Store — bounded ring buffer of recent execution traces
// ---------------------------------------------------------------------------

/// Bounded store for recent execution traces (for the API trace viewer).
pub struct TraceStore {
    traces: std::sync::Mutex<std::collections::VecDeque<ExecutionTrace>>,
    max_traces: usize,
}

impl TraceStore {
    /// Create a new trace store with the given capacity.
    pub fn new(max_traces: usize) -> Self {
        Self {
            traces: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(max_traces)),
            max_traces,
        }
    }

    /// Store a completed execution trace.
    pub fn record(&self, trace: ExecutionTrace) {
        if let Ok(mut store) = self.traces.lock() {
            if store.len() >= self.max_traces {
                store.pop_front();
            }
            store.push_back(trace);
        }
    }

    /// Get recent traces for an agent (newest first).
    pub fn get_agent_traces(&self, agent_id: &str, limit: usize) -> Vec<ExecutionTrace> {
        self.traces
            .lock()
            .ok()
            .map(|store| {
                store
                    .iter()
                    .rev()
                    .filter(|t| t.agent_id == agent_id)
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all recent traces (newest first).
    pub fn get_recent(&self, limit: usize) -> Vec<ExecutionTrace> {
        self.traces
            .lock()
            .ok()
            .map(|store| store.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// Get a specific trace by ID.
    pub fn get_trace(&self, trace_id: &str) -> Option<ExecutionTrace> {
        self.traces
            .lock()
            .ok()
            .and_then(|store| store.iter().find(|t| t.trace_id == trace_id).cloned())
    }
}

impl Default for TraceStore {
    fn default() -> Self {
        Self::new(500)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn prom_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {} {}\n# TYPE {} counter\n{} {}\n",
        name, help, name, name, value
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_builder_lifecycle() {
        let mut builder = TraceBuilder::new("agent-123", "TestAgent");
        builder.record_memory_recall("test query", 5, std::time::Duration::from_millis(10));
        builder.record_llm_call(
            "gpt-4o", "openai", 100, 50,
            std::time::Duration::from_millis(500), "end_turn", false,
        );
        builder.record_tool_execution(
            "web_search", "tool-1",
            std::time::Duration::from_millis(200), false, "results...", false,
        );
        builder.set_cost(0.005);
        let trace = builder.finish(TraceOutcome::Success);
        assert_eq!(trace.phases.len(), 3);
        assert_eq!(trace.total_input_tokens, 100);
        assert_eq!(trace.total_output_tokens, 50);
        assert_eq!(trace.iterations, 1);
    }

    #[test]
    fn test_metrics_registry_counters() {
        let registry = MetricsRegistry::new();
        registry.record_agent_loop("agent-1", 150, false);
        registry.record_llm_call("agent-1", "gpt-4o", 100, 50, 500, 0.005, false);
        registry.record_tool_call("agent-1", false, false);
        registry.record_parallel_batch();

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.agent_loop_total, 1);
        assert_eq!(snapshot.llm_calls_total, 1);
        assert_eq!(snapshot.llm_input_tokens, 100);
        assert_eq!(snapshot.tool_calls_total, 1);
        assert_eq!(snapshot.tool_parallel_batches, 1);
    }

    #[test]
    fn test_prometheus_export() {
        let registry = MetricsRegistry::new();
        registry.record_agent_loop("agent-1", 150, false);
        registry.record_llm_call("agent-1", "gpt-4o", 100, 50, 500, 0.005, false);

        let output = registry.export_prometheus();
        assert!(output.contains("openfang_agent_loop_total 1"));
        assert!(output.contains("openfang_llm_calls_total 1"));
        assert!(output.contains("openfang_llm_input_tokens_total 100"));
        assert!(output.contains("openfang_llm_calls_by_model{model=\"gpt-4o\"}"));
    }

    #[test]
    fn test_trace_store_bounded() {
        let store = TraceStore::new(3);
        for i in 0..5 {
            store.record(ExecutionTrace {
                trace_id: format!("trace-{i}"),
                agent_id: "agent-1".to_string(),
                agent_name: "Test".to_string(),
                started_at: Utc::now(),
                duration_ms: 100,
                phases: vec![],
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost_usd: 0.0,
                iterations: 1,
                outcome: TraceOutcome::Success,
            });
        }
        // Only 3 should be retained
        let recent = store.get_recent(10);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].trace_id, "trace-4"); // newest first
    }
}
