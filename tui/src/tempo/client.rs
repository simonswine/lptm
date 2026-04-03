use anyhow::Result;
use futures::StreamExt;
use log::debug;
use shared::TempoTrace;
use tempo_api::tempopb::{streaming_querier_client::StreamingQuerierClient, SearchRequest};
use tonic::transport::Channel;

pub struct TempoClient {
    url: String,
}

impl TempoClient {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub async fn search(&self, query: &str, start_s: u64, end_s: u64) -> Result<Vec<TempoTrace>> {
        debug!("tempo gRPC search url={} query={query} start={start_s} end={end_s}", self.url);

        let channel = Channel::from_shared(self.url.clone())?.connect().await?;
        let mut client = StreamingQuerierClient::new(channel);

        let request = SearchRequest {
            query: query.to_string(),
            start: start_s as u32,
            end: end_s as u32,
            limit: 20,
            ..Default::default()
        };

        let mut stream = client.search(request).await?.into_inner();
        let mut traces = Vec::new();

        while let Some(result) = stream.next().await {
            let response = result?;
            for t in response.traces {
                traces.push(TempoTrace {
                    trace_id: t.trace_id,
                    root_service_name: t.root_service_name,
                    root_trace_name: t.root_trace_name,
                    start_time_unix_nano: t.start_time_unix_nano,
                    duration_ms: t.duration_ms,
                });
            }
        }

        debug!("tempo search returned {} traces", traces.len());
        Ok(traces)
    }
}
