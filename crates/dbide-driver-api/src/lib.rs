//! Shared execution contract across native database adapters.
pub mod stream;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static SESSION_IDS: AtomicU64 = AtomicU64::new(1);
pub fn next_session_id() -> u64 {
    SESSION_IDS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseEngine {
    #[default]
    PostgreSql,
    Sqlite,
    MySql,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExecutionKey {
    pub session: u64,
    pub generation: u64,
    pub execution: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CommitPolicy {
    Auto,
    Manual,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ServerTransactionState {
    Idle,
    Active,
    Failed,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
    Rejected,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Event {
    /// Begins one result set, including an empty SELECT. On successful EOF the
    /// adapter must emit its own CommandComplete before any following command
    /// or result set. A failed result set ends with Error, not a success count.
    ResultSetStarted {
        columns: Vec<String>,
    },
    Row {
        values: Vec<Option<String>>,
    },
    /// Completes exactly one command/result set. For a row-producing statement,
    /// this is its observed total row count (including rows omitted by retention
    /// limits). A following DML command must emit a separate completion event.
    CommandComplete {
        affected_rows: u64,
    },
    /// Native engines may not provide a meaningful affected-row count for DDL/control statements.
    CommandCompleteUnknown,
    Error {
        sqlstate: Option<String>,
        message: String,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub severity: String,
    pub sqlstate: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ServerError {
    pub sqlstate: Option<String>,
    pub message: String,
}
#[derive(Debug, Serialize)]
pub struct ExecutionReport {
    pub key: ExecutionKey,
    /// Manual begins the next ordinary execution when actual wire state is Idle.
    pub commit_policy: CommitPolicy,
    pub transaction_state: ServerTransactionState,
    pub outcome: Outcome,
    /// Bounded terminal error retained even when the row/event budget is full.
    pub terminal_error: Option<ServerError>,
    pub events: Vec<Event>,
    /// Separate async channel: ordering relative to rows is not promised.
    pub notices: Vec<Notice>,
    pub retained_bytes: usize,
    pub truncated: bool,
    pub connection_retired: bool,
    pub elapsed_micros: u128,
}
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub retained_bytes: usize,
    pub events: usize,
    pub sql_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            retained_bytes: 512 * 1024,
            events: 4096,
            sql_bytes: 64 * 1024,
        }
    }
}
