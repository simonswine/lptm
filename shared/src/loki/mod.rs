pub mod app;

use serde::{Deserialize, Serialize};

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
