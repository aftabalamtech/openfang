# OpenFang Crate-by-Crate Architecture Guide

> **Generated:** 2026-03-02
>
> This document provides an in-depth technical reference for every crate in the OpenFang workspace.
> For a higher-level overview, see [architecture.md](./architecture.md).

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [openfang-types](#2-openfang-types) — Shared domain types
3. [openfang-memory](#3-openfang-memory) — SQLite-backed memory substrate
4. [openfang-runtime](#4-openfang-runtime) — Agent execution engine
5. [openfang-kernel](#5-openfang-kernel) — Central coordinator
6. [openfang-api](#6-openfang-api) — HTTP/WebSocket API server
7. [openfang-wire](#7-openfang-wire) — OFP peer-to-peer networking
8. [openfang-cli](#8-openfang-cli) — CLI binary and TUI dashboard
9. [openfang-channels](#9-openfang-channels) — 40+ messaging platform adapters
10. [openfang-skills](#10-openfang-skills) — Plugin skill system
11. [openfang-hands](#11-openfang-hands) — Autonomous capability packages
12. [openfang-extensions](#12-openfang-extensions) — MCP integration templates
13. [openfang-migrate](#13-openfang-migrate) — Framework migration engine
14. [openfang-desktop](#14-openfang-desktop) — Native desktop wrapper (Tauri)
15. [xtask](#15-xtask) — Build tooling
16. [Dependency Graph](#16-dependency-graph)
17. [Request Flow](#17-request-flow)

---

## 1. System Overview

OpenFang is an open-source **Agent Operating System** written in Rust, organized as a Cargo workspace of 14 crates (13 libraries + 1 binary stub). It runs as a long-lived daemon that orchestrates AI agents backed by **29 LLM providers**, exposes 80+ REST endpoints, serves an embedded Alpine.js dashboard, and supports 40+ messaging platform adapters.

### Boot Sequence

```
openfang start
  → OpenFangKernel::boot(config_path)
    → Read ~/.openfang/config.toml → KernelConfig
    → Open SQLite database (WAL mode)
    → Create LLM driver (primary + fallback chain)
    → Initialize MeteringEngine, SkillRegistry, HandRegistry, IntegrationRegistry
    → Auto-detect embedding driver (OpenAI → Ollama → text fallback)
    → Initialize browser, TTS, pairing, cron scheduler
  → build_router() → Axum router with 80+ endpoints
  → Start channel bridge (Telegram, Discord, Slack, etc.)
  → Bind TCP on 127.0.0.1:4200
  → Write ~/.openfang/daemon.json (PID, address, version)
```

### Workspace Members

| Crate | Role | Type |
|-------|------|------|
| `openfang-types` | Shared domain types, no business logic | Library |
| `openfang-memory` | SQLite-backed unified memory (KV, semantic, knowledge graph, sessions, usage) | Library |
| `openfang-runtime` | Agent execution loop, LLM drivers, tool runner, MCP client | Library |
| `openfang-kernel` | Central coordinator — assembles all subsystems | Library |
| `openfang-api` | Axum HTTP server, REST routes, WebSocket, dashboard SPA | Library |
| `openfang-wire` | OFP (OpenFang Protocol) — TCP peer networking | Library |
| `openfang-cli` | Binary entry point: CLI, Ratatui TUI, MCP server | Binary |
| `openfang-channels` | 40+ messaging platform adapters | Library |
| `openfang-skills` | Plugin skill system (Python, WASM, Node, prompt-only) | Library |
| `openfang-hands` | Curated autonomous capability packages | Library |
| `openfang-extensions` | One-click MCP integrations with OAuth2 and encrypted vault | Library |
| `openfang-migrate` | Migration from other AI agent frameworks | Library |
| `openfang-desktop` | Native desktop wrapper (Tauri 2.0) | Library |
| `xtask` | Build tooling scripts (stub) | Binary |

---

## 2. openfang-types

**Path:** `crates/openfang-types/`

The foundational data-model crate. Every other crate depends on it. Contains **no business logic** — pure types with `Serialize`/`Deserialize`, `Default` impls, and validation methods.

### Key Dependencies

`serde`, `serde_json`, `chrono`, `uuid`, `thiserror`, `dirs`, `toml`, `async-trait`, `ed25519-dalek`, `sha2`, `hex`

### Module Inventory (18 modules)

#### `agent` — Agent Identity, Manifests, State

The largest module. Core types:

- **`AgentId`**, **`UserId`**, **`SessionId`** — UUID newtypes with `Display`, `FromStr`, `Hash`, `Copy`
- **`AgentState`** — `Created | Running | Suspended | Terminated | Crashed`
- **`AgentMode`** — `Observe` (no tools) | `Assist` (read-only tools) | `Full` (all tools)
- **`ScheduleMode`** — `Reactive` (default) | `Periodic { cron }` | `Proactive { conditions }` | `Continuous { check_interval }`
- **`ResourceQuota`** — Per-agent limits: max memory (256MB), CPU time (30s), tool calls/min (60), tokens/hour (1M), cost/hour ($1), cost/day, cost/month
- **`ToolProfile`** — `Minimal | Coding | Research | Messaging | Automation | Full | Custom` — expands to concrete tool name lists
- **`ModelConfig`** — Provider, model, max tokens, temperature, system prompt, API key env, base URL
- **`ModelRoutingConfig`** — Auto-select model by complexity: simple/medium/complex thresholds
- **`AutonomousConfig`** — Max iterations (50), max restarts (10), heartbeat interval (30s)

**`AgentManifest`** — The central configuration object:
```
name, version, description, author, module,
schedule, model, fallback_models, resources, priority,
capabilities, profile, tools, skills, mcp_servers,
metadata, tags, routing, autonomous, pinned_model,
workspace, generate_identity_files, exec_policy,
tool_allowlist, tool_blocklist
```

**`AgentEntry`** — Registry entry: id, name, manifest, state, mode, timestamps, parent/children, session_id, tags, identity, onboarding status.

#### `approval` — Human-in-the-Loop

- **`ApprovalRequest`** — agent_id, tool_name, description, risk_level, timeout
- **`ApprovalPolicy`** — Which tools require approval (default: `["shell_exec"]`), timeout (60s), auto-approve flags
- **`RiskLevel`** — `Low | Medium | High | Critical`

#### `capability` — Capability-Based Security

Fine-grained permission system with 20+ capability variants:

```
FileRead/Write(glob), NetConnect(host_pattern), NetListen(port),
ToolInvoke(name), ToolAll, LlmQuery(model), LlmMaxTokens(n),
AgentSpawn, AgentMessage(id), AgentKill(id),
MemoryRead/Write(scope), ShellExec(pattern), EnvRead(var),
OfpDiscover, OfpConnect(peer), OfpAdvertise,
EconSpend(amount), EconEarn, EconTransfer(target)
```

`capability_matches()` handles glob patterns and numeric comparisons. `validate_capability_inheritance()` prevents privilege escalation in parent→child agent spawning.

#### `config` — Complete Kernel Configuration

`KernelConfig` — the entire deserialized `~/.openfang/config.toml`. Key sections:

| Section | Purpose |
|---------|---------|
| `default_model` | Provider, model, API key env var |
| `memory` | SQLite path, embedding model, decay rate |
| `network` | Listen addresses, bootstrap peers, shared secret |
| `channels` | 30+ messaging platform configs |
| `web` | Search provider (Brave/Tavily/Perplexity/DuckDuckGo/Auto) |
| `browser` | Headless Chromium, viewport, session limits |
| `approval` | Which tools require human approval |
| `exec_policy` | Shell security mode (Deny/Allowlist/Full) + command allowlist |
| `docker` | Container sandbox for shell_exec |
| `budget` | Global hourly/daily/monthly USD limits |
| `auth_profiles` | Multi-key rotation per provider |
| `a2a` | Agent-to-Agent protocol config |
| `mcp_servers` | External MCP tool server configs |
| `bindings` | Route channel/account patterns to agents |
| `oauth` | Google/GitHub/Microsoft/Slack OAuth client IDs |

Security: custom `Debug` impl redacts `api_key` and `shared_secret`.

#### `error` — Unified Error Type

```rust
pub enum OpenFangError {
    AgentNotFound, AgentAlreadyExists, CapabilityDenied,
    QuotaExceeded, InvalidState, SessionNotFound, Memory,
    ToolExecution, LlmDriver, Config, ManifestParse,
    Sandbox, Network, Serialization, MaxIterationsExceeded,
    ShuttingDown, Io, Internal, AuthDenied, MeteringError,
    InvalidInput,
}
```

#### `event` — Internal Event Bus Types

- **`EventPayload`** — `Message | ToolResult | MemoryUpdate | Lifecycle | Network | System | Custom`
- **`LifecycleEvent`** — `Spawned | Started | Suspended | Resumed | Terminated | Crashed`
- **`SystemEvent`** — `KernelStarted | KernelStopping | QuotaWarning | HealthCheck | QuotaEnforced | ModelRouted | UserAction`
- **`Event`** — Full envelope: id, source, target, payload, timestamp, correlation_id, ttl

#### `manifest_signing` — Supply Chain Integrity

Ed25519-based cryptographic signing. `SignedManifest` bundles manifest content, SHA-256 hash, signature, and public key. Verification checks both hash integrity and signature validity.

#### `memory` — Memory Substrate Types and Trait

- **`MemoryFragment`** — content, embedding vector, metadata, source, confidence, access tracking
- **`Entity`** / **`Relation`** — Knowledge graph types with 8 entity types and 10 relation types
- **`Memory` trait** — Async trait defining the full memory API: KV get/set/delete, semantic remember/recall/forget, knowledge graph add/query, consolidation, import/export

#### `message` — LLM Conversation Protocol

- **`Message`** — role + content
- **`ContentBlock`** — `Text | Image | ToolUse | ToolResult | Thinking | Unknown`
- **`StopReason`** — `EndTurn | ToolUse | MaxTokens | StopSequence`
- **`TokenUsage`** — input_tokens + output_tokens

#### `model_catalog` — LLM Provider Registry

Base URL constants for 27 providers. `ModelCatalogEntry` with pricing, context window, capabilities. `ProviderInfo` with auth status.

#### `tool` — Tool Definition and Schema Normalization

`ToolDefinition` (name, description, JSON Schema), `ToolCall`, `ToolResult`. `normalize_schema_for_provider()` adapts JSON Schema for Gemini/Groq compatibility (strips `$schema`, flattens `anyOf`).

#### `taint` — Information Flow Security

Lattice-based taint propagation with labels: `ExternalNetwork`, `UserInput`, `Pii`, `Secret`, `UntrustedAgent`. Pre-defined sinks block dangerous data flows (e.g., secrets cannot reach `net_fetch`, external data cannot reach `shell_exec`).

#### Other Modules

- **`comms`** — Agent communication topology and event types for the dashboard
- **`scheduler`** — `CronJob` with `CronSchedule` (At/Every/Cron), `CronAction`, `CronDelivery`
- **`media`** — Multimodal content types: `MediaAttachment`, `ImageGenRequest`
- **`webhook`** — `WakePayload` and `AgentHookPayload` for trigger endpoints
- **`tool_compat`** — OpenClaw→OpenFang tool name mapping (26 translations)
- **`serde_compat`** — Backward-compatible deserialization for schema migrations

### Design Patterns

- UUID newtypes everywhere for type safety
- Rich `Default` impls with production-safe defaults
- `#[serde(default)]` on all config fields for forward compatibility
- Validation methods return `Result<(), String>` for human-readable errors
- Internally-tagged enums for unambiguous JSON serialization

---

## 3. openfang-memory

**Path:** `crates/openfang-memory/`

Unified memory substrate composing five stores over a single SQLite connection (`Arc<Mutex<Connection>>`).

### Architecture

```
┌─────────────────────────────────┐
│        MemorySubstrate          │
│   (implements Memory trait)     │
└──────────────┬─────────────────┘
               │ Arc<Mutex<Connection>>
    ┌──────────┼──────────┬──────────┬──────────┐
    │          │          │          │          │
Structured  Semantic  Knowledge  Session   Usage
  Store      Store     Store     Store    Store
    │
ConsolidationEngine
```

### Database Schema (version 7)

| Table | Purpose | Key Fields |
|-------|---------|------------|
| `agents` | Agent registry persistence | id, name, manifest (msgpack BLOB), state (JSON) |
| `sessions` | Conversation sessions | agent_id, messages (msgpack BLOB), label |
| `canonical_sessions` | One per agent, cross-channel context | agent_id (PK), messages, compacted_summary |
| `kv_store` | Per-agent key-value storage | (agent_id, key) PK, value (JSON BLOB), version |
| `memories` | Semantic memories with embeddings | content, embedding (f32 LE BLOB), confidence, scope |
| `entities` | Knowledge graph nodes | entity_type, name, properties |
| `relations` | Knowledge graph edges | source_entity, relation_type, target_entity, confidence |
| `usage_events` | LLM call cost tracking | agent_id, model, tokens, cost_usd |
| `task_queue` | Inter-agent task delegation | title, description, assigned_to, status, priority |
| `paired_devices` | Mobile device pairing | device_id, platform, push_token |

### StructuredStore

Per-agent key-value storage with version tracking (auto-incrementing on upsert). Agent persistence uses **named MessagePack** for manifests, with auto-repair: on `load_all_agents()`, re-serialized manifests that differ from stored blobs are silently upgraded.

### SemanticStore

Two search modes:

- **Text search** (Phase 1) — `content LIKE '%query%'`, ordered by `accessed_at DESC`
- **Vector search** (Phase 2) — Fetches `max(limit*10, 100)` candidates, computes cosine similarity in Rust, re-ranks. Embeddings stored as raw `f32` LE byte arrays in SQLite BLOBs.

Access tracking: every recall updates `access_count` and `accessed_at`. Soft deletes only (`deleted = 1`).

### KnowledgeStore

Entity-relation graph with single-hop JOIN queries. Supports entity lookup by ID or name. `query_graph()` returns `Vec<GraphMatch { source, relation, target }>`.

### SessionStore

Two session types:
- **Regular sessions** — per-channel, store `Vec<Message>` as msgpack
- **Canonical sessions** — one per agent, cross-channel context with automatic compaction at 100 messages (keeps last 50, text-summarizes older ones). LLM-based summary replacement via `store_llm_summary()`.

JSONL mirroring: `write_jsonl_mirror()` writes human-readable session files to disk alongside SQLite.

### UsageStore

Records every LLM call's cost. Query methods: hourly/daily/monthly per-agent and global aggregates, per-model breakdown, daily cost trend. `cleanup_old(days)` prunes old records.

### ConsolidationEngine

Time-based memory decay: memories not accessed in 7 days have confidence reduced by `decay_rate` (multiplicative). Minimum confidence floor: 0.1 — memories never fully decay.

### MemorySubstrate

The unified facade. All async methods delegate via `tokio::task::spawn_blocking` to bridge sync SQLite with the async runtime. Also directly manages the `task_queue` and `paired_devices` tables.

---

## 4. openfang-runtime

**Path:** `crates/openfang-runtime/`

The execution engine. Contains 54 modules. Every agent message flows through this crate.

### The Agent Loop (`agent_loop.rs`)

The heart of OpenFang. `run_agent_loop()` takes 20+ parameters (all optional subsystems degrade gracefully).

**Constants:**
- `MAX_ITERATIONS = 50` — tool execution loop cap
- `MAX_RETRIES = 3` — LLM call retries with exponential backoff
- `TOOL_TIMEOUT_SECS = 120` — per-tool execution timeout
- `MAX_CONTINUATIONS = 5` — MaxTokens continuation cap
- `MAX_HISTORY_MESSAGES = 20` — emergency history trim
- `DEFAULT_CONTEXT_WINDOW = 200,000` tokens

**Execution Flow:**

1. **Memory recall** — vector similarity (if embedding driver present) or text LIKE search for top-5 relevant memories
2. **Hook: BeforePromptBuild** — plugin hooks observe/log
3. **System prompt assembly** — manifest prompt + identity files + memories + canonical context
4. **Session repair** — drop orphan tool results, merge consecutive same-role messages
5. **LoopGuard initialization** — repetition detection with SHA-256 hashing

6. **Main loop** (up to `max_iterations`):
   - Context overflow recovery (tiered: truncate tool results → remove old results → summarize)
   - Context guard (compact oldest tool results if total exceeds 75% of budget)
   - LLM call with circuit breaker and retry
   - Text tool call recovery (for Groq/Llama `<function=name>{json}</function>` format)

7. **StopReason dispatch:**
   - **EndTurn** — parse directives, save session, remember interaction, return result
   - **ToolUse** — capability check, approval gate, execute with timeout, truncate result, push to messages
   - **MaxTokens** — append "Please continue", loop (up to 5 continuations)

Returns `AgentLoopResult { response, total_usage, iterations, cost_usd, silent, directives }`.

### LLM Driver Abstraction

**`LlmDriver` trait:**
```rust
async fn complete(request: CompletionRequest) -> Result<CompletionResponse, LlmError>;
async fn stream(request: CompletionRequest, tx: Sender<StreamEvent>) -> Result<CompletionResponse, LlmError>;
```

**Driver implementations (covering 29 providers):**

| Driver | Providers | Protocol |
|--------|-----------|----------|
| `AnthropicDriver` | Anthropic | Messages API (native) |
| `GeminiDriver` | Google Gemini | generateContent API (native) |
| `OpenAIDriver` | OpenAI, Groq, DeepSeek, OpenRouter, Together, Mistral, Fireworks, Ollama, vLLM, LM Studio, Perplexity, Cohere, AI21, Cerebras, SambaNova, HuggingFace, xAI, Replicate, Moonshot, Qwen, MiniMax, Zhipu, Qianfan + custom | Chat Completions API (OpenAI-compatible) |
| `CopilotDriver` | GitHub Copilot | PAT → Copilot token exchange + OpenAI format |
| `ClaudeCodeDriver` | Claude Code CLI | Subprocess (`claude -p`) |
| `FallbackDriver` | Any chain | Tries N drivers in sequence |

API keys stored as `Zeroizing<String>` — automatically wiped from memory on drop.

### Tool Runner (`tool_runner.rs`)

Central dispatch with pre-execution gates: capability enforcement and approval (human-in-the-loop for dangerous tools).

**Taint tracking:** `check_taint_shell_exec()` flags injection patterns (curl piped to sh, base64 decode, eval). `check_taint_net_fetch()` flags URLs containing secrets in query params.

**Tool categories (40+ tools):**

| Category | Tools |
|----------|-------|
| Filesystem | `file_read`, `file_write`, `file_list`, `apply_patch` |
| Web | `web_fetch` (SSRF-protected), `web_search` (multi-provider) |
| Shell | `shell_exec` (allowlist + taint check) |
| Inter-agent | `agent_send`, `agent_spawn`, `agent_list`, `agent_kill`, `agent_find` |
| Memory | `memory_store`, `memory_recall` |
| Tasks | `task_post`, `task_claim`, `task_complete`, `task_list` |
| Knowledge | `knowledge_add_entity`, `knowledge_add_relation`, `knowledge_query` |
| Scheduling | `schedule_create/list/delete`, `cron_create/list/cancel` |
| Media | `image_analyze`, `media_describe`, `media_transcribe`, `image_generate`, `text_to_speech` |
| Browser | `browser_navigate/click/type/screenshot/read_page/close/scroll/wait/run_js/back` |
| Hands | `hand_list/activate/status/deactivate` |
| A2A | `a2a_discover`, `a2a_send` |
| Processes | `process_start/poll/write/kill/list` |
| Canvas | `canvas_present` |
| Channels | `channel_send` |
| Docker | `docker_exec` |

Fallback: `mcp_*` prefix → MCP server, then skill registry, then "Unknown tool" error.

`AGENT_CALL_DEPTH` task-local prevents infinite inter-agent recursion (cap: 5).

### LoopGuard — Anti-Loop Circuit Breaker

Prevents stuck agents via:
- **Hash counting** — warn at 3, block at 5 identical calls
- **Outcome-aware detection** — blocks on 3 identical (tool, params, result) triples
- **Ping-pong detection** — detects A-B-A-B alternating patterns (length 2 and 3)
- **Global circuit breaker** — total call cap (default 30, scaled for autonomous agents)
- **Poll tool relaxation** — status-check commands get 3x thresholds

### Model Router

Scores request complexity via heuristics (token count, tool count, code markers, conversation depth, system prompt length). Maps to `Simple | Medium | Complex` → selects cheap/balanced/expensive model tier.

### Context Budget

Two-layer management:
- **Per-result:** individual tool results capped at 30% of context window
- **Context guard:** total tool content across history capped at 75% of context window

### Other Key Modules

- **MCP Client** (`mcp.rs`) — JSON-RPC 2.0 over stdio/SSE, tool namespacing (`mcp_{server}_{tool}`)
- **A2A Protocol** (`a2a.rs`) — AgentCard at `/.well-known/agent.json`, task lifecycle, client for external agents
- **KernelHandle** (`kernel_handle.rs`) — The anti-circular-dependency boundary (trait defined here, implemented by kernel)
- **PromptBuilder** — Assembles 15+ sections into the system prompt
- **Compactor** — LLM-based conversation summarization
- **Embedding** — `EmbeddingDriver` trait for vector recall
- **Browser** — Chrome DevTools automation
- **Web tools** — SSRF-protected fetch, multi-provider search, HTML-to-markdown
- **Auth cooldown** — Per-provider circuit breaker with exponential backoff

---

## 5. openfang-kernel

**Path:** `crates/openfang-kernel/`

The central coordinator. Assembles all subsystems into `OpenFangKernel`.

### OpenFangKernel Struct

Holds every subsystem:

```
config: KernelConfig
registry: AgentRegistry              (DashMap-based concurrent agent registry)
capabilities: CapabilityManager      (RBAC/capability enforcement)
event_bus: EventBus                  (broadcast channel + history ring buffer)
scheduler: AgentScheduler
memory: Arc<MemorySubstrate>         (SQLite-backed unified memory)
supervisor: Supervisor               (graceful shutdown signals)
workflows: WorkflowEngine
triggers: TriggerEngine              (event-driven proactive agents)
background: BackgroundExecutor
audit_log: Arc<AuditLog>             (Merkle hash-chain audit trail)
metering: Arc<MeteringEngine>        (cost tracking + quota enforcement)
wasm_sandbox: WasmSandbox
mcp_connections: Mutex<Vec<McpConnection>>
a2a_task_store: A2aTaskStore
web_ctx: WebToolsContext
browser_ctx: BrowserManager
peer_registry: Option<PeerRegistry>  (OFP mesh networking)
peer_node: Option<Arc<PeerNode>>
hand_registry: HandRegistry
extension_registry: RwLock<IntegrationRegistry>
skill_registry: RwLock<SkillRegistry>
// ... +15 more fields
self_handle: OnceLock<Weak<OpenFangKernel>>
```

### Boot Sequence (`boot_with_config`)

1. Open/create SQLite database
2. Create LLM driver (primary + optional fallback chain)
3. Initialize MeteringEngine (wraps UsageStore)
4. Load bundled + user skills into SkillRegistry
5. Load HandRegistry and IntegrationRegistry
6. Auto-detect embedding driver (OpenAI → Ollama → text fallback)
7. Initialize browser automation, TTS engine, pairing manager, cron scheduler
8. Load existing agents from database

### AgentRegistry

`DashMap<AgentId, AgentEntry>` with secondary indexes on name and tags. Lock-free concurrent read/write. Methods: `register()`, `get()`, `list()`, `kill()`, `update_state()`.

### MeteringEngine

Wraps `UsageStore`. After every agent loop:
1. `record(UsageRecord)` — persists to SQLite
2. `check_quota(agent_id, ResourceQuota)` — enforces per-agent limits
3. `check_global_budget(BudgetConfig)` — enforces system-wide limits

Returns `QuotaExceeded` error when limits are hit.

### send_message — Core Message Flow

```
send_message(agent_id, user_message)
  → Check quota (MeteringEngine)
  → Load AgentManifest from registry
  → Open/resume Session from MemorySubstrate
  → Build tool list (builtin + MCP + skills)
  → Filter tools by AgentMode (Observe/Assist/Full)
  → run_agent_loop(...)
  → Record usage in MeteringEngine
  → Save session to SQLite
  → Return AgentLoopResult
```

### KernelHandle Implementation

The kernel implements the `KernelHandle` trait (defined in `openfang-runtime`) to provide inter-agent communication without circular dependencies. Methods cover agent lifecycle, shared memory, task queue, events, knowledge graph, scheduling, approval, hands, A2A, and channels.

`spawn_agent_checked()` enforces capability inheritance — child agent capabilities must be a strict subset of parent's.

### Other Subsystems

- **EventBus** — Broadcast channel with history ring buffer; supports subscription by agent ID, pattern, or all
- **CapabilityManager** — Validates tool invocations against agent's granted capabilities
- **WorkflowEngine** — Multi-step agent pipeline execution
- **TriggerEngine** — Event-driven proactive agent activation
- **Supervisor** — Graceful shutdown signal coordination
- **AuditLog** — Merkle hash-chain audit trail for integrity verification
- **Config reload** — Hot-reload via file polling (30s interval)

---

## 6. openfang-api

**Path:** `crates/openfang-api/`

Axum HTTP server with 80+ REST endpoints, WebSocket support, SSE streaming, and an embedded Alpine.js dashboard.

### AppState

```rust
pub struct AppState {
    pub kernel: Arc<OpenFangKernel>,
    pub started_at: Instant,
    pub peer_registry: Option<Arc<PeerRegistry>>,
    pub bridge_manager: Mutex<Option<BridgeManager>>,
    pub channels_config: RwLock<ChannelsConfig>,
    pub shutdown_notify: Arc<tokio::sync::Notify>,
}
```

### Middleware Stack (innermost to outermost)

1. **Auth** — Bearer token / X-API-Key / query param; constant-time comparison; loopback-only when no key configured
2. **Rate limiting** — GCRA per-IP, 500 tokens/min, operation-cost-weighted (e.g., POST message = 30, spawn = 50)
3. **Security headers** — CSP, X-Frame-Options, X-Content-Type-Options, Referrer-Policy
4. **Request logging** — UUID request ID, method, path, status, latency
5. **Compression** — gzip/brotli
6. **CORS** — Permissive for loopback; restricted when API key configured

### Route Groups (80+ endpoints)

| Group | Prefix | Examples |
|-------|--------|---------|
| System | `/api/health`, `/api/status` | Health check, metrics, shutdown |
| Agents | `/api/agents` | CRUD, message, stream, session, tools, skills, clone, files |
| Channels | `/api/channels` | List, configure, test, reload, WhatsApp QR |
| Workflows | `/api/workflows` | Create, run, list runs |
| Triggers | `/api/triggers` | Create, update, delete |
| Cron | `/api/cron/jobs` | Create, delete, enable/disable |
| Skills | `/api/skills` | Install, remove, create, marketplace search |
| Hands | `/api/hands` | List, activate, deactivate, settings, stats |
| MCP | `/api/mcp/servers`, `/mcp` | Server listing, MCP-over-HTTP |
| Budget | `/api/budget` | Global and per-agent budget status/update |
| Usage | `/api/usage` | Summary, by-model, daily breakdown |
| Config | `/api/config` | Get, set, schema, reload |
| Approvals | `/api/approvals` | List, approve, reject |
| Audit | `/api/audit` | Recent entries, Merkle chain verification |
| Security | `/api/security` | Security dashboard |
| Models | `/api/models`, `/api/providers` | Model catalog, provider management, OAuth |
| Memory | `/api/memory` | Per-agent KV store CRUD |
| Sessions | `/api/sessions` | List, delete, label |
| Peers | `/api/peers`, `/api/network` | OFP peer list, network status |
| Comms | `/api/comms` | Agent topology, events, direct messaging |
| A2A | `/api/a2a`, `/.well-known/agent.json` | External agent discovery, task delegation |
| OpenAI | `/v1/chat/completions`, `/v1/models` | OpenAI-compatible API |
| Webhooks | `/hooks/wake`, `/hooks/agent` | External trigger endpoints |
| Integrations | `/api/integrations` | Install, remove, health, marketplace |
| Pairing | `/api/pairing` | Device pairing, push notifications |
| Migration | `/api/migrate` | Detect, scan, run migration |

### WebSocket Handler (`ws.rs`)

**Endpoint:** `GET /api/agents/{id}/ws`

- Per-IP connection limit: 5 concurrent WebSocket connections
- Rate limiting: 10 messages per 60 seconds
- 64KB message size limit
- 30-minute idle timeout
- Streaming text debounce: 100ms deadline or 200 chars
- Verbose levels: Off → On → Full (cycled via client command)
- Context pressure reporting: low / medium (>50%) / high (>70%) / critical (>85%)
- Agent list change detection: 5-second polling with hash comparison

### Dashboard Embedding (`webchat.rs`)

Assembled at compile time via `include_str!()` — single binary, no CDN dependencies. All vendor libraries bundled: Alpine.js, marked.js, highlight.js.

**16 page modules:** Overview, Chat, Agents, Workflows, Workflow Builder, Channels, Skills, Hands, Scheduler, Settings, Usage, Sessions, Logs, Wizard, Approvals, Comms.

ETag versioned as `"openfang-{version}"` with 1-hour cache.

### OpenAI-Compatible API (`openai_compat.rs`)

The `model` field resolves to an OpenFang agent:
- `"openfang:<name>"` → find by name
- Valid UUID → find by ID
- Plain string → try as agent name
- Fallback → first registered agent

Supports both batch and streaming modes.

### Channel Bridge (`channel_bridge.rs`)

`KernelBridgeAdapter` wraps `Arc<OpenFangKernel>` and implements `ChannelBridgeHandle`. `start_channel_bridge()` reads config and instantiates adapters for all configured channels. `reload_channels_from_disk()` supports hot-reload.

---

## 7. openfang-wire

**Path:** `crates/openfang-wire/`

Custom TCP-based peer-to-peer protocol (OFP) for connecting multiple OpenFang instances into a mesh.

### Wire Format

Length-prefixed JSON over TCP:
```
┌──────────────────┬────────────────────────┐
│  4 bytes (BE u32) │    JSON payload (N)    │
│   length = N      │   (WireMessage)        │
└──────────────────┴────────────────────────┘
```

Max message size: 16 MB.

### Message Types

**Requests:** `Handshake`, `Discover`, `AgentMessage`, `Ping`

**Responses:** `HandshakeAck`, `DiscoverResult`, `AgentResponse`, `Pong`, `Error`

**Notifications (fire-and-forget):** `AgentSpawned`, `AgentTerminated`, `ShuttingDown`

### Authentication: HMAC-SHA256

Pre-shared key model. Both sides authenticate with independent nonce+HMAC pairs:

```
auth_data = nonce (UUID v4) + node_id
auth_hmac = hex(HMAC-SHA256(shared_secret, auth_data))
```

Constant-time comparison via `subtle::ConstantTimeEq`.

### Handshake Flow

```
Client                              Server
  |-- Handshake(node_id,            |
  |   agents[], nonce, hmac) ------>|
  |                                 | Verify HMAC, check version
  |<-- HandshakeAck(node_id,        |
  |    agents[], nonce, hmac) ------|
  | Verify ack HMAC                 |
  |== Connection loop begins =======|
```

First message MUST be Handshake — anything else gets `Error{code: 401}`.

### PeerRegistry

Thread-safe `Arc<RwLock<HashMap<String, PeerEntry>>>`. Each `PeerEntry` tracks: node_id, node_name, address, agents, state (Connected/Disconnected), protocol_version. Disconnected peers remain in registry for reconnection visibility.

`find_agents(query)` performs case-insensitive substring search across agent name, description, and tags for connected peers only.

### PeerHandle Trait

Kernel integration boundary (kernel implements this):
- `local_agents()` — list local agents for handshake
- `handle_agent_message()` — route message to named agent
- `discover_agents()` — search local agents
- `uptime_secs()` — daemon uptime

---

## 8. openfang-cli

**Path:** `crates/openfang-cli/`

The single user-facing binary. Serves three roles: traditional CLI, interactive TUI, and MCP server.

### CLI Commands (35 top-level + subcommand groups)

| Command | Purpose |
|---------|---------|
| `start` | Boot kernel + API server |
| `stop` | Graceful daemon shutdown |
| `tui` | Full 19-tab Ratatui dashboard |
| `chat [agent]` | Standalone chat TUI |
| `status` | Daemon status |
| `doctor [--repair]` | 15-point health check |
| `message <agent> <text>` | One-shot message |
| `mcp` | MCP server over stdio |
| `init` / `setup` | First-run wizard |
| `agent` | 6 subcommands: new, spawn, list, chat, kill, set |
| `config` | 8 subcommands: show, edit, get, set, unset, set-key, delete-key, test-key |
| `models` | 4 subcommands: list, aliases, providers, set |
| `skill` | 5 subcommands: install, list, remove, search, create |
| `channel` | 5 subcommands: list, setup, test, enable, disable |
| `hand` | 5 subcommands: list, active, activate, deactivate, info |
| `workflow` | 3 subcommands: list, create, run |
| `trigger` | 3 subcommands: list, create, delete |
| `security` | 3 subcommands: status, audit, verify |
| `memory` | 4 subcommands: list, get, set, delete |
| `cron` | 5 subcommands: list, create, delete, enable, disable |
| `vault` | 4 subcommands: init, set, list, remove |

### Dual-Mode Architecture

Every command tries the daemon first (HTTP call), falls back to in-process kernel boot:

```
find_daemon()  →  reads ~/.openfang/daemon.json
               →  health check with 1s+2s timeout
               →  returns base URL or None
```

### Full TUI (`tui/mod.rs`)

19-tab Ratatui application:
- Dashboard, Agents, Chat, Sessions, Workflows, Triggers, Memory, Channels, Skills, Hands, Extensions, Templates, Peers, Comms, Security, Audit, Usage, Settings, Logs

**Event system** (`event.rs`): 60+ `AppEvent` variants covering input, streaming, lifecycle, and per-tab data loading. Background threads use `BackendRef` (Daemon URL or `Arc<OpenFangKernel>`) for HTTP or in-process calls.

**Streaming:** In-process mode uses kernel's `send_message_streaming()` receiver. Daemon mode parses SSE from `/api/agents/{id}/message/stream`.

### MCP Server (`mcp.rs`)

Exposes agents as MCP tools over `Content-Length`-framed JSON-RPC 2.0 on stdio. Each agent becomes tool `openfang_agent_{name}`. 10MB message size limit.

### Bundled Agent Templates

30 embedded templates via `include_str!()`: analyst, architect, assistant, coder, code-reviewer, customer-support, data-scientist, debugger, devops-lead, doc-writer, email-assistant, health-tracker, hello-world, home-automation, legal-assistant, meeting-assistant, ops, orchestrator, personal-finance, planner, recruiter, researcher, sales-assistant, security-auditor, social-media, test-engineer, translator, travel-planner, tutor, writer.

### Doctor Command

15-point diagnostic: directory structure, file permissions, config syntax, daemon status, port availability, stale PID, database integrity, disk space, API keys, config deserialization, MCP validation, skill registry scan, extension registry, daemon health detail, runtime checks.

---

## 9. openfang-channels

**Path:** `crates/openfang-channels/`

40+ pluggable messaging platform adapters that normalize platform-specific messages into a unified `ChannelMessage` type.

### Core Types

- **`ChannelMessage`** — Normalized inbound event: channel type, sender, content, target agent, metadata
- **`ChannelContent`** — `Text | Image | File | Voice | Location | Command`
- **`AgentPhase`** — UI feedback: `Queued | Thinking | ToolUse | Streaming | Done | Error`
- **`ChannelAdapter` trait** — `start()` returns a message stream, `send()` delivers to user, `send_typing()` for indicators

### Bridge Architecture

- **`ChannelBridgeHandle` trait** — Kernel abstraction with 30+ methods (message routing, RBAC, slash commands)
- **`BridgeManager`** — Owns running adapters and dispatch tasks
- **`ChannelRateLimiter`** — Sliding-window per-user, keyed by `"{channel}:{platform_id}"`

### Message Dispatch Pipeline

1. Fetch per-channel overrides (output format, threading, DM/group policy)
2. Group policy filter (Ignore / CommandsOnly / MentionOnly / All)
3. DM policy filter (Ignore / AllowedOnly / Respond)
4. Rate limit check
5. Slash command dispatch (30 commands: `/help`, `/agents`, `/model`, `/compact`, etc.)
6. Broadcast routing check
7. RBAC authorization
8. Auto-reply check
9. Forward to agent

### Agent Router

Priority-ordered resolution: config bindings (most specific first) → direct routes → user defaults → global default agent.

Binding specificity scoring: `channel`=1, `account_id`=2, `guild_id`=4, `peer_id`=8, `roles`=2 each.

### Adapters by Category

| Category | Adapters |
|----------|----------|
| Messaging (12) | Telegram, Discord, Slack, WhatsApp (QR + Business), Signal, Matrix, Email, LINE, Viber, Messenger, Threema, Keybase |
| Social (5) | Reddit, Mastodon, Bluesky, LinkedIn, Nostr |
| Enterprise (10) | Teams, Mattermost, Google Chat, Webex, Feishu/Lark, DingTalk, Pumble, Flock, Twist, Zulip |
| Developer (9) | IRC, XMPP, Gitter, Discourse, Revolt, Guilded, Nextcloud Talk, Rocket.Chat, Twitch |
| Notifications (4) | ntfy, Gotify, Webhook (HMAC-signed), Mumble |

### Formatter

Per-platform markdown conversion:
- `Markdown` — passthrough
- `TelegramHtml` — `**text**` → `<b>text</b>`
- `SlackMrkdwn` — `**text**` → `*text*`, `[text](url)` → `<url|text>`
- `PlainText` — strips all formatting

---

## 10. openfang-skills

**Path:** `crates/openfang-skills/`

Plugin system extending agent capabilities with "tool bundles."

### Skill Runtime Types

| Runtime | Execution |
|---------|-----------|
| `PromptOnly` (default) | Injects markdown into LLM system prompt — no code execution |
| `Python` | Subprocess execution |
| `Wasm` | Sandbox execution |
| `Node` | OpenClaw compatibility |
| `Builtin` | Compiled in |

### SkillRegistry

HashMap-based registry with `freeze()` for Stable mode (blocks all subsequent loads after boot). Workspace skills override global skills.

Load pipeline: scan directory → find `skill.toml` (or convert `SKILL.md` via openclaw_compat) → run security scan → register.

### Security Verifier

Two-tier scanning:

**`security_scan(manifest)`** — Checks runtime (Node = Warning), capabilities (ShellExec = Critical), tools (shell_exec = Critical, file_write = Warning).

**`scan_prompt_content(content)`** — Pattern matching for prompt injection:
- **Critical** (10 patterns): "ignore previous instructions", "you are now", "system prompt override", etc.
- **Warning** (9 patterns): data exfiltration patterns, dangerous shell commands

Skills with Critical warnings are blocked at load time.

### ClawHub Client

Full API client for `clawhub.ai` marketplace:
- Search, browse (trending/updated/downloads/stars), detail, download
- 7-step security pipeline on install: download → detect format → convert manifest → security scan → prompt injection scan → dependency check → write manifest
- Critical findings block installation with `SecurityBlocked` error

### OpenClaw Compatibility

Converts SKILL.md (YAML frontmatter + Markdown) into OpenFang `skill.toml` + `prompt_context.md`. Tool name translation table (26 mappings).

---

## 11. openfang-hands

**Path:** `crates/openfang-hands/`

Pre-built, domain-complete autonomous agent configurations. Unlike regular agents (you chat with them), Hands work independently — you check in on them.

### Hand Definition

Declared in `HAND.toml` with companion `SKILL.md`:

```
id, name, description, category, icon,
tools (required tool names),
skills, mcp_servers (allowlists),
requires (Vec<HandRequirement>),
settings (Vec<HandSetting> with Select/Text/Toggle types),
agent (HandAgentConfig: provider, model, system prompt, temperature, max iterations),
dashboard (HandDashboard: metrics bound to memory keys)
```

### HandRegistry

- `definitions: HashMap<String, HandDefinition>` — all known hands
- `instances: DashMap<Uuid, HandInstance>` — active instances linked to kernel agents

Methods: `activate()`, `deactivate()`, `pause()`, `resume()`, `check_requirements()` (binary PATH + env var checks), `check_settings_availability()`.

### Bundled Hands (7)

| ID | Category | Purpose | Key Requirements |
|----|----------|---------|-----------------|
| clip | Content | Video/media processing | ffmpeg |
| lead | Data | Lead generation | — |
| collector | Data | Data collection | — |
| predictor | Data | Prediction/forecasting | — |
| researcher | Productivity | Autonomous 7-phase research | — |
| twitter | Communication | Social media management | TWITTER_BEARER_TOKEN |
| browser | Productivity | Web automation (Playwright) | python3, playwright |

The researcher hand implements a full 7-phase pipeline: question classification → query decomposition → CRAAP-scored source gathering → cross-reference synthesis → fact-checking → report generation → state persistence.

---

## 12. openfang-extensions

**Path:** `crates/openfang-extensions/`

Manages third-party MCP server integrations with 25 bundled templates, encrypted credential vault, and PKCE OAuth2 flows.

### IntegrationRegistry

25 bundled templates across 6 categories:
- **DevTools (6):** GitHub, GitLab, Linear, Jira, Bitbucket, Sentry
- **Productivity (6):** Google Calendar, Gmail, Notion, Todoist, Google Drive, Dropbox
- **Communication (3):** Slack, Discord MCP, Teams MCP
- **Data (5):** PostgreSQL, SQLite, MongoDB, Redis, Elasticsearch
- **Cloud (3):** AWS, GCP, Azure
- **AI & Search (2):** Brave Search, Exa Search

### Credential Vault

AES-256-GCM encrypted storage with Argon2id key derivation:
- Master key stored in OS keyring (XOR-obfuscated with machine fingerprint)
- File format: `OFV1` magic + JSON envelope with salt, nonce, ciphertext
- All key material uses `Zeroizing<T>` for automatic memory wiping

### Credential Resolver

Resolution chain: Vault → dotenv → process env → interactive prompt.

### OAuth2 PKCE Flow

1. Bind ephemeral local port
2. Generate PKCE verifier/challenge (SHA-256 S256)
3. Generate CSRF state
4. Open browser to authorization URL
5. Local axum callback server receives code
6. Exchange code at token endpoint

### Health Monitor

Per-integration health tracking with exponential backoff reconnection: 5 * 2^attempt seconds, capped at 300s, max 10 attempts.

---

## 13. openfang-migrate

**Path:** `crates/openfang-migrate/`

Migration engine for importing from other AI agent frameworks.

### Supported Sources

- **OpenClaw** — Fully implemented: config, agents, sessions, memory, skills, channels, secrets
- **LangChain** — Stub (returns `UnsupportedSource`)
- **AutoGPT** — Stub (returns `UnsupportedSource`)

### OpenClaw Migration

Reads `openclaw.json` (JSON5 with camelCase) and converts:
- Config → `~/.openfang/config.toml`
- Agents → `~/.openfang/agents/{name}/agent.toml`
- Memory → `~/.openfang/memory/`
- Sessions → `~/.openfang/sessions/`
- Skills → `~/.openfang/skills/`
- Channel configs → merged into config.toml
- Secrets → `~/.openfang/secrets.env`

Supports dry-run mode. Generates Markdown migration report with imported/skipped/warnings.

---

## 14. openfang-desktop

**Path:** `crates/openfang-desktop/`

Native desktop application using Tauri 2.0.

### Features

- Boots kernel and embedded API server on an OS-assigned ephemeral port
- Opens native window pointing at the local API
- System tray with status, agent count, uptime, launch-at-login toggle
- Global keyboard shortcuts: `Ctrl+Shift+O` (show), `Ctrl+Shift+N` (agents), `Ctrl+Shift+C` (chat)
- Silent auto-update (checks 10s after startup)
- Native OS notifications for critical events (agent crash, kernel stopping, quota enforced)
- Window close → hide to tray (not quit)
- Single-instance enforcement (second launch focuses existing window)

### Tauri IPC Commands (11)

`get_port`, `get_status`, `get_agent_count`, `import_agent_toml`, `import_skill_file`, `get_autostart`, `set_autostart`, `check_for_updates`, `install_update`, `open_config_dir`, `open_logs_dir`

---

## 15. xtask

**Path:** `xtask/`

Build tooling stub following the Rust `xtask` convention. Currently prints "no tasks defined yet" — infrastructure is in place for future automation tasks.

---

## 16. Dependency Graph

```
openfang-desktop ─── openfang-api ─── openfang-kernel ─── openfang-runtime
       │                   │                │                    │
       │                   │                ├── openfang-memory  │
       │                   │                ├── openfang-skills  │
       │                   │                ├── openfang-hands   │
       │                   │                ├── openfang-extensions
       │                   │                ├── openfang-wire    │
       │                   │                └── openfang-channels│
       │                   │                                     │
openfang-cli ──────────────┤                                     │
                           │                                     │
                    openfang-migrate                              │
                                                                 │
                           All crates depend on: openfang-types ─┘
```

Key anti-circular-dependency boundary: `KernelHandle` trait is defined in `openfang-runtime` and implemented by `openfang-kernel`.

---

## 17. Request Flow

A typical user message through the system:

```
User POST /api/agents/{id}/message
  │
  ├─ Auth middleware (Bearer token / API key / loopback check)
  ├─ Rate limiter (GCRA, 30 tokens for message)
  ├─ Security headers
  │
  ▼
routes::send_message(AppState, agent_id, MessageRequest)
  │
  ├─ Reject if > 64KB
  ├─ Resolve file attachments → base64 ContentBlocks
  │
  ▼
kernel.send_message_with_handle(agent_id, message, kernel_handle)
  │
  ├─ MeteringEngine.check_quota(agent_id, manifest.resources)
  ├─ MeteringEngine.check_global_budget(config.budget)
  ├─ Load AgentManifest from AgentRegistry
  ├─ Open/resume Session from MemorySubstrate
  ├─ Build tool list (builtin + MCP + skills)
  ├─ Filter tools by AgentMode
  │
  ▼
runtime::run_agent_loop(manifest, msg, session, memory, driver, tools, ...)
  │
  ├─ Memory recall (vector/text, top-5)
  ├─ Prompt build (identity files + memories + canonical context)
  ├─ Session repair (orphan removal, consecutive merge)
  │
  ▼ Main Loop (up to 50 iterations)
  │
  ├─ Context overflow recovery
  ├─ Context guard (compact old tool results)
  ├─ LlmDriver.complete() → Groq/Anthropic/OpenAI/etc.
  │   └─ Retry on rate limit (exponential backoff, max 3)
  ├─ StopReason::ToolUse?
  │   ├─ Capability check
  │   ├─ Approval gate (if tool requires it)
  │   ├─ execute_tool() with 120s timeout
  │   ├─ Truncate result (context budget)
  │   ├─ LoopGuard check (repetition detection)
  │   └─ Continue loop
  ├─ StopReason::MaxTokens?
  │   └─ Append "Please continue", loop (up to 5x)
  └─ StopReason::EndTurn?
      ├─ Parse directives ([[reply_to:]], [[silent]])
      ├─ Remember interaction (with embedding)
      └─ Return AgentLoopResult
  │
  ▼
kernel post-loop:
  ├─ MeteringEngine.record(usage)
  ├─ Session saved to SQLite
  └─ Return JSON response to client
```
