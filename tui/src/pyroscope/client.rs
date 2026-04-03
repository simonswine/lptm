use anyhow::{Result, anyhow};
use log::debug;
use reqwest::Client;
use shared::pyroscope::{FlameGraph, Level};

mod gen {
    #![allow(dead_code, non_camel_case_types, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/_connectrpc.rs"));
}

use gen::querier::v1::{
    HeatmapQueryType, ProfileFormat, SelectHeatmapRequest, SelectHeatmapResponse,
    SelectMergeStacktracesRequest, SelectMergeStacktracesResponse, SelectSeriesRequest,
    SelectSeriesResponse, SeriesRequest, SeriesResponse,
};
use gen::types::v1::ExemplarType;
use shared::pyroscope::{HeatmapSlot, TimelineSeries};

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

    pub async fn select_series(
        &self,
        profile_type_id: &str,
        service: &str,
        start: i64,
        end: i64,
        step_s: f64,
        span: bool,
    ) -> Result<Vec<TimelineSeries>> {
        let exemplar_type = if span {
            ExemplarType::EXEMPLAR_TYPE_SPAN
        } else {
            ExemplarType::EXEMPLAR_TYPE_INDIVIDUAL
        };
        let req = SelectSeriesRequest {
            profile_typeID: profile_type_id.into(),
            label_selector: format!("{{service_name=\"{service}\"}}"),
            start,
            end,
            step: step_s,
            exemplar_type: exemplar_type.into(),
            ..SelectSeriesRequest::default()
        };
        let resp: SelectSeriesResponse = self.post("SelectSeries", &req).await?;

        Ok(resp
            .series
            .into_iter()
            .map(|s| TimelineSeries {
                labels: s.labels.into_iter().map(|l| (l.name, l.value)).collect(),
                exemplars: s
                    .points
                    .iter()
                    .flat_map(|p| p.exemplars.iter())
                    .map(|e| shared::pyroscope::TimelineExemplar {
                        labels: e.labels.iter().map(|l| (l.name.clone(), l.value.clone())).collect(),
                        profile_id: e.profile_id.clone(),
                        span_id: e.span_id.clone(),
                        value: e.value,
                        timestamp_ms: e.timestamp,
                    })
                    .collect(),
                points: s.points.into_iter().map(|p| (p.timestamp as f64, p.value)).collect(),
            })
            .collect())
    }

    pub async fn select_heatmap(
        &self,
        profile_type_id: &str,
        service: &str,
        start: i64,
        end: i64,
        step_s: f64,
        span: bool,
    ) -> Result<Vec<HeatmapSlot>> {
        let query_type = if span {
            HeatmapQueryType::HEATMAP_QUERY_TYPE_SPAN
        } else {
            HeatmapQueryType::HEATMAP_QUERY_TYPE_INDIVIDUAL
        };
        let exemplar_type = if span {
            ExemplarType::EXEMPLAR_TYPE_SPAN
        } else {
            ExemplarType::EXEMPLAR_TYPE_INDIVIDUAL
        };
        let req = SelectHeatmapRequest {
            profile_typeID: profile_type_id.into(),
            label_selector: format!("{{service_name=\"{service}\"}}"),
            start,
            end,
            step: step_s,
            query_type: query_type.into(),
            exemplar_type: exemplar_type.into(),
            ..SelectHeatmapRequest::default()
        };
        let resp: SelectHeatmapResponse = self.post("SelectHeatmap", &req).await?;

        let step_ms = (step_s * 1000.0).floor() as i64;
        let slots: Vec<HeatmapSlot> = resp
            .series
            .into_iter()
            .flat_map(|s| s.slots.into_iter())
            .map(|slot| HeatmapSlot {
                timestamp_ms: slot.timestamp,
                step_ms,
                y_min: slot.y_min,
                counts: slot.counts,
                exemplars: slot
                    .exemplars
                    .iter()
                    .map(|e| shared::pyroscope::TimelineExemplar {
                        labels: e.labels.iter().map(|l| (l.name.clone(), l.value.clone())).collect(),
                        profile_id: e.profile_id.clone(),
                        span_id: e.span_id.clone(),
                        value: e.value,
                        timestamp_ms: e.timestamp,
                    })
                    .collect(),
            })
            .collect();
        Ok(slots)
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
