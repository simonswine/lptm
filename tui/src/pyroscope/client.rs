use anyhow::{Result, anyhow};
use log::debug;
use reqwest::Client;
use shared::pyroscope::{FlameGraph, Level};

mod gen {
    #![allow(dead_code, non_camel_case_types, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/_connectrpc.rs"));
}

use gen::querier::v1::{
    ProfileFormat, SelectMergeStacktracesRequest, SelectMergeStacktracesResponse, SeriesRequest,
    SeriesResponse,
};

pub struct PyroscopeClient {
    http: Client,
    grafana_url: String,
    ds_id: u64,
    token: String,
}

impl PyroscopeClient {
    pub fn new(grafana_url: String, ds_id: u64, token: String) -> Self {
        Self {
            http: Client::new(),
            grafana_url,
            ds_id,
            token,
        }
    }

    fn url(&self, method: &str) -> String {
        format!(
            "{}/api/datasources/proxy/{}/querier.v1.QuerierService/{}",
            self.grafana_url, self.ds_id, method
        )
    }

    async fn post<Req, Resp>(&self, method: &str, req: &Req) -> Result<Resp>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
    {
        let url = self.url(method);
        let body = serde_json::to_string(req)?;
        debug!("→ POST {url}");
        debug!("  body: {body}");

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;

        let status = resp.status();
        debug!("← {status} {url}");

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            debug!("  error body: {body}");
            return Err(anyhow!("HTTP {status}: {body}"));
        }

        let bytes = resp.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn series(&self, start: i64, end: i64) -> Result<Vec<(String, String)>> {
        let req = SeriesRequest {
            matchers: vec!["{}".into()],
            label_names: vec!["__profile_type__".into(), "service_name".into()],
            start,
            end,
            ..SeriesRequest::default()
        };
        let resp: SeriesResponse = self.post("Series", &req).await?;

        let mut items: Vec<(String, String)> = resp
            .labels_set
            .iter()
            .filter_map(|s| {
                let service = s
                    .labels
                    .iter()
                    .find(|l| l.name == "service_name" && !l.value.is_empty())?
                    .value
                    .clone();
                let profile_type = s
                    .labels
                    .iter()
                    .find(|l| l.name == "__profile_type__" && !l.value.is_empty())?
                    .value
                    .clone();
                Some((service, profile_type))
            })
            .collect();

        items.sort();
        items.dedup();
        Ok(items)
    }

    pub async fn select_merge_stacktraces(
        &self,
        profile_type_id: &str,
        service: &str,
        start: i64,
        end: i64,
    ) -> Result<Option<FlameGraph>> {
        let req = SelectMergeStacktracesRequest {
            profile_typeID: profile_type_id.into(),
            label_selector: format!("{{service_name=\"{service}\"}}"),
            start,
            end,
            max_nodes: None,
            format: ProfileFormat::PROFILE_FORMAT_FLAMEGRAPH.into(),
            ..SelectMergeStacktracesRequest::default()
        };
        let resp: SelectMergeStacktracesResponse =
            self.post("SelectMergeStacktraces", &req).await?;

        Ok(resp.flamegraph.into_option().map(|fg| FlameGraph {
            names: fg.names,
            levels: fg
                .levels
                .into_iter()
                .map(|l| Level { values: l.values })
                .collect(),
            total: fg.total,
            max_self: fg.max_self,
        }))
    }
}
