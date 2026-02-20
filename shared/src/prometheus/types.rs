use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrometheusVectorItem {
    pub metric: HashMap<String, String>,
    pub value: (f64, String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrometheusData {
    #[serde(rename = "resultType")]
    pub result_type: String,
    pub result: Vec<PrometheusVectorItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrometheusResponse {
    pub status: String,
    pub data: PrometheusData,
}

/// Generic Prometheus API list response (`{"status":"success","data":[...]}`)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrometheusStringListResponse {
    pub status: String,
    pub data: Vec<String>,
}
