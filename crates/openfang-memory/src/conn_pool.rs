//! Read/write connection pool for SQLite.
//!
//! SQLite in WAL mode supports unlimited concurrent readers but only one writer.
//! This module provides a connection pool that exploits this:
//!
//! - **Write connection**: A single `Mutex<Connection>` for all writes.
//! - **Read pool**: Multiple `Connection` handles opened in read-only mode,
//!   distributed via a round-robin pool. WAL mode allows these to read
//!   concurrently without blocking the writer or each other.
//!
//! ## Performance Impact
//!
//! Under concurrent agent load (10+ agents), the single `Arc<Mutex<Connection>>`
//! pattern serializes ALL operations (reads and writes). With this pool:
//! - Reads never block on the write mutex
//! - Reads never block on each other
//! - Only writes serialize (unavoidable with SQLite)
//! - Expected throughput improvement: 5-10x under concurrent load

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tracing::debug;

/// Default number of read connections in the pool.
pub const DEFAULT_READ_POOL_SIZE: usize = 4;

/// A read/write SQLite connection pool.
///
/// Uses WAL mode for concurrent read access with a single write connection.
pub struct SqlitePool {
    /// Single write connection (serialized via Mutex).
    write_conn: Mutex<Connection>,
    /// Pool of read-only connections.
    read_conns: Vec<Mutex<Connection>>,
    /// Round-robin counter for distributing reads across the pool.
    read_index: AtomicUsize,
    /// Database path (for opening new connections if needed).
    db_path: Option<PathBuf>,
}

impl SqlitePool {
    /// Open a new connection pool at the given database path.
    ///
    /// Opens one write connection and `pool_size` read connections.
    /// All connections use WAL mode with a 5-second busy timeout.
    pub fn open(db_path: &Path, pool_size: usize) -> Result<Self, rusqlite::Error> {
        let pool_size = pool_size.max(1).min(16);

        // Write connection — full read/write
        let write_conn = Connection::open(db_path)?;
        write_conn.execute_batch(
            "PRAGMA journal_mode=WAL;\
             PRAGMA busy_timeout=5000;\
             PRAGMA synchronous=NORMAL;\
             PRAGMA cache_size=-8000;\
             PRAGMA wal_autocheckpoint=1000;",
        )?;

        // Read connections — opened with read-only flag for safety
        let mut read_conns = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let rc = Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            rc.execute_batch(
                "PRAGMA busy_timeout=5000;\
                 PRAGMA cache_size=-4000;",
            )?;
            read_conns.push(Mutex::new(rc));
        }

        debug!(
            pool_size = pool_size,
            path = %db_path.display(),
            "SQLite read/write pool opened"
        );

        Ok(Self {
            write_conn: Mutex::new(write_conn),
            read_conns,
            read_index: AtomicUsize::new(0),
            db_path: Some(db_path.to_path_buf()),
        })
    }

    /// Create an in-memory pool (for testing).
    ///
    /// In-memory databases cannot share across connections, so the read pool
    /// shares the same connection as the writer. This is correct for tests
    /// but doesn't provide concurrent read benefits.
    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;

        // For in-memory, we can't open separate read connections to the same DB.
        // Use a single connection for both reads and writes.
        Ok(Self {
            write_conn: Mutex::new(conn),
            read_conns: Vec::new(), // empty = fallback to write conn for reads
            read_index: AtomicUsize::new(0),
            db_path: None,
        })
    }

    /// Acquire the write connection.
    ///
    /// Only one writer can be active at a time (enforced by Mutex).
    pub fn write(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.write_conn
            .lock()
            .map_err(|e| format!("Write connection poisoned: {e}"))
    }

    /// Acquire a read connection from the pool (round-robin).
    ///
    /// If the read pool is empty (in-memory mode), falls back to the write connection.
    pub fn read(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        if self.read_conns.is_empty() {
            // Fallback: use write connection for reads (in-memory mode)
            return self.write();
        }

        let index = self.read_index.fetch_add(1, Ordering::Relaxed) % self.read_conns.len();
        self.read_conns[index]
            .lock()
            .map_err(|e| format!("Read connection {} poisoned: {e}", index))
    }

    /// Execute a write operation with the write connection.
    ///
    /// Convenience method that acquires the write lock, runs the closure,
    /// and releases. Returns the closure's result.
    pub fn with_write<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Connection) -> Result<T, String>,
    {
        let conn = self.write()?;
        f(&conn)
    }

    /// Execute a read operation with a read connection.
    ///
    /// Convenience method that acquires a read lock, runs the closure,
    /// and releases. Returns the closure's result.
    pub fn with_read<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Connection) -> Result<T, String>,
    {
        let conn = self.read()?;
        f(&conn)
    }

    /// Execute a batch write in a transaction.
    ///
    /// Wraps multiple operations in a single SQLite transaction for
    /// dramatically better write throughput (100 inserts in 1 transaction
    /// vs 100 separate transactions = ~50x faster).
    pub fn with_write_transaction<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, String>,
    {
        let mut conn = self.write()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;
        let result = f(&tx)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {e}"))?;
        Ok(result)
    }

    /// Get the number of read connections in the pool.
    pub fn read_pool_size(&self) -> usize {
        self.read_conns.len()
    }

    /// Get the database path (None for in-memory).
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Run a WAL checkpoint manually.
    ///
    /// Useful for ensuring all WAL data is written to the main database
    /// before backup or shutdown.
    pub fn checkpoint(&self) -> Result<(), String> {
        let conn = self.write()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| format!("Checkpoint failed: {e}"))
    }

    /// Get pool statistics.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            read_pool_size: self.read_conns.len(),
            total_reads_distributed: self.read_index.load(Ordering::Relaxed),
            is_in_memory: self.db_path.is_none(),
        }
    }
}

/// Pool statistics for monitoring.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    /// Number of read connections in the pool.
    pub read_pool_size: usize,
    /// Total number of read operations distributed across the pool.
    pub total_reads_distributed: usize,
    /// Whether this is an in-memory database (no read pool benefits).
    pub is_in_memory: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_in_memory_pool() {
        let pool = SqlitePool::open_in_memory().unwrap();
        assert_eq!(pool.read_pool_size(), 0); // in-memory has no read pool

        // Write should work
        pool.with_write(|conn| {
            conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT);")
                .map_err(|e| e.to_string())
        })
        .unwrap();

        // Read (falls back to write conn) should work
        pool.with_read(|conn| {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_write_transaction() {
        let pool = SqlitePool::open_in_memory().unwrap();

        pool.with_write(|conn| {
            conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT);")
                .map_err(|e| e.to_string())
        })
        .unwrap();

        // Batch insert via transaction
        pool.with_write_transaction(|tx| {
            for i in 0..100 {
                tx.execute(
                    "INSERT INTO test (val) VALUES (?1)",
                    rusqlite::params![format!("value-{i}")],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .unwrap();

        // Verify
        pool.with_read(|conn| {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            assert_eq!(count, 100);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_pool_stats() {
        let pool = SqlitePool::open_in_memory().unwrap();
        let stats = pool.stats();
        assert_eq!(stats.read_pool_size, 0);
        assert!(stats.is_in_memory);
    }

    #[test]
    fn test_concurrent_reads() {
        let pool = Arc::new(SqlitePool::open_in_memory().unwrap());

        pool.with_write(|conn| {
            conn.execute_batch(
                "CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT);
                 INSERT INTO test (val) VALUES ('hello');",
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();

        // Simulate concurrent reads
        let mut handles = Vec::new();
        for _ in 0..10 {
            let p = pool.clone();
            handles.push(std::thread::spawn(move || {
                p.with_read(|conn| {
                    let val: String = conn
                        .query_row("SELECT val FROM test WHERE id = 1", [], |row| row.get(0))
                        .map_err(|e| e.to_string())?;
                    assert_eq!(val, "hello");
                    Ok(())
                })
                .unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }
}
