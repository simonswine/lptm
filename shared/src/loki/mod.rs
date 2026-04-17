pub mod app;
pub mod context;
pub mod keywords;
pub mod tokenizer;

use serde::{Deserialize, Serialize};

pub use context::CompletionCtx;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiStream {
    pub labels: String,
    pub entries: Vec<LokiEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiEntry {
    pub timestamp: String,
    /// Raw nanosecond timestamp string from the Loki API.
    pub timestamp_ns: String,
    pub line: String,
}
