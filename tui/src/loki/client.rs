use anyhow::{anyhow, Result};
use log::debug;
use serde::Deserialize;
use shared::loki::{LokiEntry, LokiStream};

pub struct LokiClient {
    http: reqwest::Client,
    grafana_url: String,
    ds_id: u64,
    token: String,
}

#[derive(Deserialize)]
struct LokiResponse {
    status: String,
    data: LokiData,
}

#[derive(Deserialize)]
struct LokiData {
    #[serde(rename = "resultType")]
    result_type: String,
    result: Vec<LokiStreamRaw>,
}

#[derive(Deserialize)]
struct LokiStreamRaw {
    stream: std::collections::HashMap<String, String>,
    values: Vec<(String, String)>,
}

impl LokiClient {
    pub fn new(grafana_url: String, ds_id: u64, token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            grafana_url,
            ds_id,
            token,
        }
    }

    pub async fn query_range(
        &self,
        query: &str,
        start_ns: u64,
        end_ns: u64,
        limit: u32,
        direction: &str,
    ) -> Result<Vec<LokiStream>> {
        let url = format!(
            "{}/api/datasources/proxy/{}/loki/api/v1/query_range",
            self.grafana_url, self.ds_id
        );
        debug!("loki query_range: query={query} start={start_ns} end={end_ns} direction={direction}");

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .query(&[
                ("query", query),
                ("start", &start_ns.to_string()),
                ("end", &end_ns.to_string()),
                ("limit", &limit.to_string()),
                ("direction", direction),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("HTTP {status}: {body}"));
        }

        let body: LokiResponse = resp.json().await?;
        if body.status != "success" {
            return Err(anyhow!("Loki returned status: {}", body.status));
        }

        debug!(
            "loki response: resultType={} streams={}",
            body.data.result_type,
            body.data.result.len()
        );

        let streams = body
            .data
            .result
            .into_iter()
            .map(|raw| {
                let labels = format_labels(&raw.stream);
                let entries = raw
                    .values
                    .into_iter()
                    .map(|(ts_ns, line)| LokiEntry {
                        timestamp: format_nano_timestamp(&ts_ns),
                        timestamp_ns: ts_ns,
                        line,
                    })
                    .collect();
                LokiStream { labels, entries }
            })
            .collect();

        Ok(streams)
    }
}

fn format_labels(labels: &std::collections::HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = labels.iter().collect();
    pairs.sort_by_key(|(k, _)| k.clone());
    let inner: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v:?}"))
        .collect();
    format!("{{{}}}", inner.join(", "))
}

fn format_nano_timestamp(ns_str: &str) -> String {
    let ns: u64 = ns_str.parse().unwrap_or(0);
    let secs = ns / 1_000_000_000;
    let subsec_ms = (ns % 1_000_000_000) / 1_000_000;

    // Simple UTC timestamp formatting
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let z = secs / 86400 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}.{subsec_ms:03}")
}
