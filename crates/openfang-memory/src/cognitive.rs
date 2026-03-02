//! Cognitive memory — episodic, semantic, and procedural memory layers.
//!
//! Implements the three-layer memory model from cognitive science:
//!
//! - **Episodic**: Time-indexed event sequences from conversations. "What happened?"
//!   Auto-extracted, timestamped, decays unless reinforced by access.
//!
//! - **Semantic**: Factual knowledge with confidence scoring. "What do I know?"
//!   Extracted assertions with source attribution and confidence decay.
//!
//! - **Procedural**: Learned procedures and user preferences. "How should I do things?"
//!   User workflows, style preferences, and operational patterns.
//!
//! Each layer stores entries in SQLite with a shared schema but distinct `memory_type`
//! discriminator. All layers support time-based decay with reinforcement on access.

use chrono::{DateTime, Utc};
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The three cognitive memory types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveType {
    /// Time-indexed event sequences from conversations.
    Episodic,
    /// Factual knowledge with confidence scoring.
    Semantic,
    /// Learned procedures and user preferences.
    Procedural,
}

impl CognitiveType {
    fn as_str(&self) -> &'static str {
        match self {
            CognitiveType::Episodic => "episodic",
            CognitiveType::Semantic => "semantic",
            CognitiveType::Procedural => "procedural",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "episodic" => Some(CognitiveType::Episodic),
            "semantic" => Some(CognitiveType::Semantic),
            "procedural" => Some(CognitiveType::Procedural),
            _ => None,
        }
    }
}

/// A single cognitive memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveEntry {
    /// Unique entry ID.
    pub id: String,
    /// Which agent owns this memory.
    pub agent_id: String,
    /// Memory type (episodic/semantic/procedural).
    pub memory_type: CognitiveType,
    /// The memory content.
    pub content: String,
    /// Source attribution (e.g., "conversation:session-123", "user_statement", "inferred").
    pub source: String,
    /// Confidence score (0.0 - 1.0). Decays over time unless reinforced.
    pub confidence: f64,
    /// Number of times this memory has been accessed (reinforcement counter).
    pub access_count: u64,
    /// When this memory was created.
    pub created_at: DateTime<Utc>,
    /// When this memory was last accessed or reinforced.
    pub last_accessed: DateTime<Utc>,
    /// Optional tags for categorization.
    pub tags: Vec<String>,
    /// For episodic: optional session ID this memory came from.
    pub session_id: Option<String>,
}

/// Parameters for querying cognitive memory.
#[derive(Debug, Clone, Default)]
pub struct CognitiveQuery {
    /// Filter by memory type (None = all types).
    pub memory_type: Option<CognitiveType>,
    /// Text search query (LIKE matching).
    pub query: Option<String>,
    /// Minimum confidence threshold.
    pub min_confidence: Option<f64>,
    /// Filter by tags (any match).
    pub tags: Vec<String>,
    /// Maximum number of results.
    pub limit: usize,
    /// Whether to boost recently accessed entries.
    pub recency_boost: bool,
}

// ---------------------------------------------------------------------------
// Cognitive Store
// ---------------------------------------------------------------------------

/// The cognitive memory store — manages all three memory layers in SQLite.
pub struct CognitiveStore {
    conn: Arc<Mutex<Connection>>,
    /// Decay rate per day (fraction of confidence lost per day without access).
    decay_rate: f64,
}

impl CognitiveStore {
    /// Create a new cognitive store using the shared SQLite connection.
    pub fn new(conn: Arc<Mutex<Connection>>, decay_rate: f64) -> Self {
        Self {
            conn,
            decay_rate: decay_rate.clamp(0.0, 1.0),
        }
    }

    /// Initialize the cognitive memory table (called during migration).
    pub fn create_table(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cognitive_memory (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                content TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT '',
                confidence REAL NOT NULL DEFAULT 1.0,
                access_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                last_accessed TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                session_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_cognitive_agent_type
                ON cognitive_memory(agent_id, memory_type);
            CREATE INDEX IF NOT EXISTS idx_cognitive_confidence
                ON cognitive_memory(confidence);
            CREATE INDEX IF NOT EXISTS idx_cognitive_last_accessed
                ON cognitive_memory(last_accessed);",
        )
    }

    /// Store a new cognitive memory entry.
    pub fn store(&self, entry: &CognitiveEntry) -> OpenFangResult<()> {
        let conn = self.conn.lock().map_err(|e| {
            OpenFangError::Memory(format!("Failed to lock connection: {e}"))
        })?;
        let tags_json = serde_json::to_string(&entry.tags).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT OR REPLACE INTO cognitive_memory
             (id, agent_id, memory_type, content, source, confidence,
              access_count, created_at, last_accessed, tags, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                entry.id,
                entry.agent_id,
                entry.memory_type.as_str(),
                entry.content,
                entry.source,
                entry.confidence,
                entry.access_count,
                entry.created_at.to_rfc3339(),
                entry.last_accessed.to_rfc3339(),
                tags_json,
                entry.session_id,
            ],
        )
        .map_err(|e| OpenFangError::Memory(format!("Failed to store cognitive memory: {e}")))?;
        Ok(())
    }

    /// Query cognitive memories with filtering, decay adjustment, and recency boost.
    pub fn query(
        &self,
        agent_id: AgentId,
        params: &CognitiveQuery,
    ) -> OpenFangResult<Vec<CognitiveEntry>> {
        let conn = self.conn.lock().map_err(|e| {
            OpenFangError::Memory(format!("Failed to lock connection: {e}"))
        })?;

        let mut sql = String::from(
            "SELECT id, agent_id, memory_type, content, source, confidence,
                    access_count, created_at, last_accessed, tags, session_id
             FROM cognitive_memory
             WHERE agent_id = ?1",
        );
        let mut param_idx = 2;

        if params.memory_type.is_some() {
            sql.push_str(&format!(" AND memory_type = ?{param_idx}"));
            param_idx += 1;
        }
        if params.query.is_some() {
            sql.push_str(&format!(" AND content LIKE ?{param_idx}"));
            param_idx += 1;
        }
        if params.min_confidence.is_some() {
            sql.push_str(&format!(" AND confidence >= ?{param_idx}"));
            #[allow(unused_assignments)]
            { param_idx += 1; }
        }

        // Order by relevance: confidence * recency factor
        if params.recency_boost {
            sql.push_str(" ORDER BY confidence * (1.0 / (1.0 + (julianday('now') - julianday(last_accessed)))) DESC");
        } else {
            sql.push_str(" ORDER BY confidence DESC, last_accessed DESC");
        }

        let limit = if params.limit == 0 { 20 } else { params.limit };
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn.prepare(&sql).map_err(|e| {
            OpenFangError::Memory(format!("Failed to prepare cognitive query: {e}"))
        })?;

        // Build dynamic parameter list
        let agent_str = agent_id.to_string();
        let mut boxed_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(agent_str)];
        if let Some(ref mt) = params.memory_type {
            boxed_params.push(Box::new(mt.as_str().to_string()));
        }
        if let Some(ref q) = params.query {
            boxed_params.push(Box::new(format!("%{q}%")));
        }
        if let Some(min_conf) = params.min_confidence {
            boxed_params.push(Box::new(min_conf));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            boxed_params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let tags_str: String = row.get(9)?;
                let tags: Vec<String> =
                    serde_json::from_str(&tags_str).unwrap_or_default();
                let created_str: String = row.get(7)?;
                let accessed_str: String = row.get(8)?;
                Ok(CognitiveEntry {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    memory_type: CognitiveType::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(CognitiveType::Episodic),
                    content: row.get(3)?,
                    source: row.get(4)?,
                    confidence: row.get(5)?,
                    access_count: row.get(6)?,
                    created_at: DateTime::parse_from_rfc3339(&created_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    last_accessed: DateTime::parse_from_rfc3339(&accessed_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    tags,
                    session_id: row.get(10)?,
                })
            })
            .map_err(|e| OpenFangError::Memory(format!("Failed to query cognitive memory: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            if let Ok(mut entry) = row {
                // Apply time-based confidence decay
                let days_since_access = (Utc::now() - entry.last_accessed).num_hours() as f64 / 24.0;
                let decay = (-self.decay_rate * days_since_access).exp();
                entry.confidence *= decay;
                entries.push(entry);
            }
        }

        // Update access timestamps for retrieved entries (reinforcement)
        let now_str = Utc::now().to_rfc3339();
        for entry in &entries {
            let _ = conn.execute(
                "UPDATE cognitive_memory SET last_accessed = ?1, access_count = access_count + 1 WHERE id = ?2",
                rusqlite::params![now_str, entry.id],
            );
        }

        Ok(entries)
    }

    /// Store an episodic memory from a conversation event.
    pub fn store_episodic(
        &self,
        agent_id: AgentId,
        content: &str,
        session_id: Option<&str>,
        source: &str,
    ) -> OpenFangResult<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let entry = CognitiveEntry {
            id: id.clone(),
            agent_id: agent_id.to_string(),
            memory_type: CognitiveType::Episodic,
            content: content.to_string(),
            source: source.to_string(),
            confidence: 1.0,
            access_count: 0,
            created_at: now,
            last_accessed: now,
            tags: vec![],
            session_id: session_id.map(|s| s.to_string()),
        };
        self.store(&entry)?;
        Ok(id)
    }

    /// Store a semantic (factual) memory.
    pub fn store_semantic(
        &self,
        agent_id: AgentId,
        content: &str,
        source: &str,
        confidence: f64,
        tags: Vec<String>,
    ) -> OpenFangResult<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let entry = CognitiveEntry {
            id: id.clone(),
            agent_id: agent_id.to_string(),
            memory_type: CognitiveType::Semantic,
            content: content.to_string(),
            source: source.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            access_count: 0,
            created_at: now,
            last_accessed: now,
            tags,
            session_id: None,
        };
        self.store(&entry)?;
        Ok(id)
    }

    /// Store a procedural memory (user preference or workflow pattern).
    pub fn store_procedural(
        &self,
        agent_id: AgentId,
        content: &str,
        source: &str,
        tags: Vec<String>,
    ) -> OpenFangResult<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let entry = CognitiveEntry {
            id: id.clone(),
            agent_id: agent_id.to_string(),
            memory_type: CognitiveType::Procedural,
            content: content.to_string(),
            source: source.to_string(),
            confidence: 1.0, // procedural memories start at full confidence
            access_count: 0,
            created_at: now,
            last_accessed: now,
            tags,
            session_id: None,
        };
        self.store(&entry)?;
        Ok(id)
    }

    /// Delete a specific cognitive memory entry.
    pub fn forget(&self, id: &str) -> OpenFangResult<bool> {
        let conn = self.conn.lock().map_err(|e| {
            OpenFangError::Memory(format!("Failed to lock connection: {e}"))
        })?;
        let affected = conn
            .execute("DELETE FROM cognitive_memory WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| OpenFangError::Memory(format!("Failed to delete: {e}")))?;
        Ok(affected > 0)
    }

    /// Delete all cognitive memories for an agent.
    pub fn forget_agent(&self, agent_id: AgentId) -> OpenFangResult<usize> {
        let conn = self.conn.lock().map_err(|e| {
            OpenFangError::Memory(format!("Failed to lock connection: {e}"))
        })?;
        let agent_str = agent_id.to_string();
        let affected = conn
            .execute(
                "DELETE FROM cognitive_memory WHERE agent_id = ?1",
                rusqlite::params![agent_str],
            )
            .map_err(|e| OpenFangError::Memory(format!("Failed to delete agent memories: {e}")))?;
        Ok(affected)
    }

    /// Run decay pass: reduce confidence of old memories and prune dead ones.
    ///
    /// Entries with confidence below `prune_threshold` are deleted.
    /// Returns the number of pruned entries.
    pub fn run_decay(&self, prune_threshold: f64) -> OpenFangResult<usize> {
        let conn = self.conn.lock().map_err(|e| {
            OpenFangError::Memory(format!("Failed to lock connection: {e}"))
        })?;

        // Update confidence based on time decay
        // confidence = confidence * exp(-decay_rate * days_since_access)
        conn.execute(
            "UPDATE cognitive_memory SET confidence = confidence * exp(?1 * (julianday('now') - julianday(last_accessed)))",
            rusqlite::params![-self.decay_rate],
        )
        .map_err(|e| OpenFangError::Memory(format!("Failed to run decay: {e}")))?;

        // Prune entries below threshold
        let pruned = conn
            .execute(
                "DELETE FROM cognitive_memory WHERE confidence < ?1",
                rusqlite::params![prune_threshold],
            )
            .map_err(|e| OpenFangError::Memory(format!("Failed to prune: {e}")))?;

        Ok(pruned)
    }

    /// Detect contradictions: find semantic memories that contradict new content.
    ///
    /// Simple heuristic: look for memories with overlapping keywords but
    /// containing negation words (not, no, never, false, incorrect, wrong).
    pub fn find_contradictions(
        &self,
        agent_id: AgentId,
        content: &str,
    ) -> OpenFangResult<Vec<CognitiveEntry>> {
        let words: Vec<&str> = content.split_whitespace().take(10).collect();
        if words.is_empty() {
            return Ok(vec![]);
        }

        // Search for semantic memories containing any of the key words
        let mut results = Vec::new();
        for word in &words {
            if word.len() < 4 {
                continue; // skip short words
            }
            let query = CognitiveQuery {
                memory_type: Some(CognitiveType::Semantic),
                query: Some(word.to_string()),
                limit: 5,
                ..Default::default()
            };
            let entries = self.query(agent_id, &query)?;
            for entry in entries {
                // Check for negation markers that suggest contradiction
                let negation_markers = ["not ", "no ", "never ", "false", "incorrect", "wrong", "isn't", "doesn't", "can't"];
                let has_negation = negation_markers.iter().any(|neg| {
                    (entry.content.to_lowercase().contains(neg) && !content.to_lowercase().contains(neg))
                        || (!entry.content.to_lowercase().contains(neg) && content.to_lowercase().contains(neg))
                });
                if has_negation && !results.iter().any(|r: &CognitiveEntry| r.id == entry.id) {
                    results.push(entry);
                }
            }
        }

        Ok(results)
    }

    /// Get memory statistics for an agent.
    pub fn stats(&self, agent_id: AgentId) -> OpenFangResult<CognitiveStats> {
        let conn = self.conn.lock().map_err(|e| {
            OpenFangError::Memory(format!("Failed to lock connection: {e}"))
        })?;
        let agent_str = agent_id.to_string();

        let mut stmt = conn
            .prepare(
                "SELECT memory_type, COUNT(*), AVG(confidence)
                 FROM cognitive_memory
                 WHERE agent_id = ?1
                 GROUP BY memory_type",
            )
            .map_err(|e| OpenFangError::Memory(format!("Failed to prepare stats: {e}")))?;

        let mut stats = CognitiveStats::default();
        let rows = stmt
            .query_map(rusqlite::params![agent_str], |row| {
                let mt: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                let avg_conf: f64 = row.get(2)?;
                Ok((mt, count, avg_conf))
            })
            .map_err(|e| OpenFangError::Memory(format!("Failed to query stats: {e}")))?;

        for row in rows {
            if let Ok((mt, count, avg_conf)) = row {
                match mt.as_str() {
                    "episodic" => {
                        stats.episodic_count = count;
                        stats.episodic_avg_confidence = avg_conf;
                    }
                    "semantic" => {
                        stats.semantic_count = count;
                        stats.semantic_avg_confidence = avg_conf;
                    }
                    "procedural" => {
                        stats.procedural_count = count;
                        stats.procedural_avg_confidence = avg_conf;
                    }
                    _ => {}
                }
            }
        }

        stats.total = stats.episodic_count + stats.semantic_count + stats.procedural_count;
        Ok(stats)
    }
}

/// Statistics about cognitive memory for an agent.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CognitiveStats {
    pub total: u64,
    pub episodic_count: u64,
    pub episodic_avg_confidence: f64,
    pub semantic_count: u64,
    pub semantic_avg_confidence: f64,
    pub procedural_count: u64,
    pub procedural_avg_confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        CognitiveStore::create_table(&conn).unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn test_store_and_query_episodic() {
        let conn = test_conn();
        let store = CognitiveStore::new(conn, 0.1);
        let agent_id = AgentId::new();

        let id = store
            .store_episodic(agent_id, "User asked about Kubernetes pods", Some("sess-1"), "conversation")
            .unwrap();
        assert!(!id.is_empty());

        let results = store
            .query(
                agent_id,
                &CognitiveQuery {
                    memory_type: Some(CognitiveType::Episodic),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Kubernetes"));
    }

    #[test]
    fn test_store_and_query_semantic() {
        let conn = test_conn();
        let store = CognitiveStore::new(conn, 0.1);
        let agent_id = AgentId::new();

        store
            .store_semantic(
                agent_id,
                "Kubernetes uses etcd for cluster state storage",
                "web_search",
                0.9,
                vec!["kubernetes".to_string(), "infrastructure".to_string()],
            )
            .unwrap();

        let results = store
            .query(
                agent_id,
                &CognitiveQuery {
                    memory_type: Some(CognitiveType::Semantic),
                    query: Some("etcd".to_string()),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].confidence > 0.8);
    }

    #[test]
    fn test_store_procedural() {
        let conn = test_conn();
        let store = CognitiveStore::new(conn, 0.1);
        let agent_id = AgentId::new();

        store
            .store_procedural(
                agent_id,
                "User prefers code examples in Python over JavaScript",
                "user_statement",
                vec!["preference".to_string(), "coding".to_string()],
            )
            .unwrap();

        let results = store
            .query(
                agent_id,
                &CognitiveQuery {
                    memory_type: Some(CognitiveType::Procedural),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].confidence, 1.0);
    }

    #[test]
    fn test_forget() {
        let conn = test_conn();
        let store = CognitiveStore::new(conn, 0.1);
        let agent_id = AgentId::new();

        let id = store
            .store_episodic(agent_id, "temporary memory", None, "test")
            .unwrap();
        assert!(store.forget(&id).unwrap());

        let results = store
            .query(agent_id, &CognitiveQuery { limit: 10, ..Default::default() })
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_stats() {
        let conn = test_conn();
        let store = CognitiveStore::new(conn, 0.1);
        let agent_id = AgentId::new();

        store.store_episodic(agent_id, "ep1", None, "test").unwrap();
        store.store_episodic(agent_id, "ep2", None, "test").unwrap();
        store.store_semantic(agent_id, "fact1", "test", 0.9, vec![]).unwrap();
        store.store_procedural(agent_id, "pref1", "test", vec![]).unwrap();

        let stats = store.stats(agent_id).unwrap();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.episodic_count, 2);
        assert_eq!(stats.semantic_count, 1);
        assert_eq!(stats.procedural_count, 1);
    }
}
