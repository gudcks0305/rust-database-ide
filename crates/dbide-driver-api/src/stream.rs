//! One explicit independent read-only execution, delivered to a bounded sink.
//! Values are native textual representations, not a typed interchange format.
use std::{fmt, time::Duration};

#[derive(Clone, Copy, Debug)]
pub struct StreamLimits {
    pub max_rows: u64,
    pub max_row_bytes: usize,
    pub max_payload_bytes: u64,
    pub timeout: Duration,
}
impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            max_rows: 1_000_000,
            max_row_bytes: 1024 * 1024,
            max_payload_bytes: 64 * 1024 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}
pub enum StreamEvent<'a> {
    Columns(&'a [String]),
    Row(&'a [Option<String>]),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSummary {
    pub rows: u64,
    pub payload_bytes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamErrorKind {
    Configuration,
    Connection,
    Execution,
    Cancelled,
    Timeout,
    Limit,
    Sink,
}
#[derive(Debug)]
pub struct StreamError {
    pub kind: StreamErrorKind,
    pub message: String,
}
impl StreamError {
    pub fn new(kind: StreamErrorKind, message: impl ToString) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }
}
impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for StreamError {}
