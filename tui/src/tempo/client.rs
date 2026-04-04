use anyhow::{anyhow, Result};
use log::debug;
use reqwest::Client;
use serde::Deserialize;
use shared::TempoTrace;

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    traces: Vec<TraceMetadata>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TraceMetadata {
    #[serde(rename = "traceID", default)]
    trace_id: String,
    #[serde(default)]
    root_service_name: String,
    #[serde(default)]
    root_trace_name: String,
    // Tempo encodes uint64 timestamps as JSON strings
    #[serde(default, deserialize_with = "de_u64_str")]
    start_time_unix_nano: u64,
    #[serde(default)]
    duration_ms: u32,
}

fn de_u64_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    use serde::de;
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = u64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "u64 or string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> { Ok(v) }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
            v.parse().map_err(de::Error::custom)
        }
    }
    d.deserialize_any(V)
}

pub struct TempoClient {
    http: Client,
    grafana_url: String,
    ds_id: u64,
    token: String,
}

impl TempoClient {
    pub fn new(grafana_url: String, ds_id: u64, token: String) -> Self {
        Self { http: Client::new(), grafana_url, ds_id, token }
    }

    pub async fn search(&self, query: &str, start_s: u64, end_s: u64) -> Result<Vec<TempoTrace>> {
        let url = format!(
            "{}/api/datasources/proxy/{}/api/search",
            self.grafana_url, self.ds_id
        );
        debug!("→ GET {url}?q={query}&start={start_s}&end={end_s}");

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .query(&[
                ("q", query),
                ("limit", "20"),
                ("start", &start_s.to_string()),
                ("end", &end_s.to_string()),
            ])
            .send()
            .await?;

        let status = resp.status();
        debug!("← {status}");
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("HTTP {status}: {body}"));
        }

        let data: SearchResponse = resp.json().await?;
        debug!("← {} traces", data.traces.len());

        Ok(data
            .traces
            .into_iter()
            .map(|t| TempoTrace {
                trace_id: t.trace_id,
                root_service_name: t.root_service_name,
                root_trace_name: t.root_trace_name,
                start_time_unix_nano: t.start_time_unix_nano,
                duration_ms: t.duration_ms,
            })
            .collect())
    }
}
