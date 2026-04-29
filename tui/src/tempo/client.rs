use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use log::debug;
use serde::Deserialize;
use serde_json::{json, Value};
use shared::{TempoSpan, TempoTrace};
use std::collections::HashMap;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};
use uuid::Uuid;

pub struct TempoClient {
    http: reqwest::Client,
    grafana_url: String,
    uid: String,
    token: String,
}

// GET /api/frontend/settings — we only need two fields
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrontendSettings {
    #[serde(default)]
    namespace: String,
    #[serde(default)]
    live_namespaced: bool,
}

// GET /api/user — fallback to get numeric orgId
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInfo {
    org_id: u64,
}

// Centrifuge server message
#[derive(Deserialize)]
struct ServerMsg {
    #[allow(dead_code)]
    id: Option<u32>,
    push: Option<PushMsg>,
}

#[derive(Deserialize)]
struct PushMsg {
    #[serde(rename = "pub")]
    publication: Option<PubMsg>,
}

#[derive(Deserialize)]
struct PubMsg {
    data: Value,
}

// tempopb.TraceSearchMetadata as marshalled by Go's encoding/json
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TraceMetadata {
    #[serde(rename = "traceID", default)]
    trace_id: String,
    #[serde(default)]
    root_service_name: String,
    #[serde(default)]
    root_trace_name: String,
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
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
            Ok(v as u64)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
            v.parse().map_err(de::Error::custom)
        }
    }
    d.deserialize_any(V)
}

/// Parse a Grafana DataFrameJSON publication.
/// Fields: [0]=result [1]=metrics [2]=state [3]=error
fn parse_frame(frame: &Value) -> (Vec<TempoTrace>, String) {
    let values = match frame.pointer("/data/values") {
        Some(v) => v,
        None => return (vec![], String::new()),
    };

    let state = values
        .get(2)
        .and_then(|a| a.get(0))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(err) = values
        .get(3)
        .and_then(|a| a.get(0))
        .and_then(|s| s.as_str())
    {
        if !err.is_empty() {
            debug!("  stream error field: {err}");
        }
    }

    let traces_val = match values.get(0).and_then(|a| a.get(0)) {
        Some(v) if !v.is_null() => v,
        _ => return (vec![], state),
    };

    let metas: Vec<TraceMetadata> = serde_json::from_value(traces_val.clone()).unwrap_or_default();
    let traces = metas
        .into_iter()
        .map(|t| TempoTrace {
            trace_id: t.trace_id,
            root_service_name: t.root_service_name,
            root_trace_name: t.root_trace_name,
            start_time_unix_nano: t.start_time_unix_nano,
            duration_ms: t.duration_ms,
        })
        .collect();

    (traces, state)
}

/// Parse a span search publication — same `data/values` envelope as `parse_frame`.
/// `values[0][0]` is a JSON blob: an array of trace objects each containing `spanSets`.
/// Returns (span_id → trace_id map, state string).
fn parse_span_frame(frame: &Value) -> (std::collections::HashMap<String, String>, String) {
    #[derive(Deserialize)]
    struct SpanTrace {
        #[serde(rename = "traceID", default)]
        trace_id: String,
        #[serde(rename = "spanSets", default)]
        span_sets: Vec<SpanSet>,
    }
    #[derive(Deserialize)]
    struct SpanSet {
        #[serde(default)]
        spans: Vec<Span>,
    }
    #[derive(Deserialize)]
    struct Span {
        #[serde(rename = "spanID", default)]
        span_id: String,
    }

    let values = match frame.pointer("/data/values") {
        Some(v) => v,
        None => return (Default::default(), String::new()),
    };

    let state = values
        .get(2)
        .and_then(|a| a.get(0))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(err) = values
        .get(3)
        .and_then(|a| a.get(0))
        .and_then(|s| s.as_str())
    {
        if !err.is_empty() {
            debug!("  span stream error: {err}");
        }
    }

    let result_val = match values.get(0).and_then(|a| a.get(0)) {
        Some(v) if !v.is_null() => v,
        _ => return (Default::default(), state),
    };

    let traces: Vec<SpanTrace> = serde_json::from_value(result_val.clone()).unwrap_or_default();
    let mut map = std::collections::HashMap::new();
    for trace in traces {
        for span_set in trace.span_sets {
            for span in span_set.spans {
                if !span.span_id.is_empty() {
                    map.insert(span.span_id, trace.trace_id.clone());
                }
            }
        }
    }
    debug!(
        "  span frame: {} span→trace pairs, state={state}",
        map.len()
    );
    (map, state)
}

fn unix_to_iso(secs: u64) -> String {
    // Euclidean algorithm for Gregorian calendar (Hinnant 2010)
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
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.000Z")
}

impl TempoClient {
    pub fn new(grafana_url: String, uid: String, token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            grafana_url,
            uid,
            token,
        }
    }

    /// Resolve the Centrifuge channel namespace:
    ///   - Grafana Cloud: `config.namespace` (e.g. "stacks-27821")
    ///   - Self-hosted:   numeric org ID      (e.g. "1")
    async fn live_namespace(&self) -> Result<String> {
        let url = format!("{}/api/frontend/settings", self.grafana_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "GET /api/frontend/settings returned {}",
                resp.status()
            ));
        }
        let settings: FrontendSettings = resp.json().await?;
        debug!(
            "  frontend/settings: liveNamespaced={} namespace={:?}",
            settings.live_namespaced, settings.namespace
        );

        if settings.live_namespaced && !settings.namespace.is_empty() {
            return Ok(settings.namespace);
        }

        // Fall back to numeric org ID
        let user_url = format!("{}/api/user", self.grafana_url);
        let user: UserInfo = self
            .http
            .get(&user_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?
            .json()
            .await?;
        Ok(user.org_id.to_string())
    }

    /// Stream trace IDs for a set of span IDs using Grafana Live (Centrifuge).
    /// `on_progress` is called with each intermediate batch as frames arrive.
    pub async fn traces_for_span_ids(
        &self,
        span_ids: &[String],
        start_s: u64,
        end_s: u64,
        on_progress: impl Fn(std::collections::HashMap<String, String>),
    ) -> Result<()> {
        let namespace = self.live_namespace().await?;

        let ws_base = self
            .grafana_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let ws_url = format!("{}/api/live/ws", ws_base);

        let mut request = ws_url.as_str().into_client_request()?;
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token))?,
        );

        let (mut ws, _) = connect_async(request).await?;

        // ── 1. Connect ────────────────────────────────────────────────────────
        ws.send(Message::Text(serde_json::to_string(&json!({
            "id": 1,
            "connect": { "name": "lptm", "version": "0.1.0" }
        }))?))
        .await?;

        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    if t.trim() == "{}" {
                        ws.send(Message::Text("{}".into())).await?;
                        continue;
                    }
                    let v: Value = serde_json::from_str(&t)?;
                    if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
                        if let Some(err) = v.get("error") {
                            return Err(anyhow!("connect error: {err}"));
                        }
                        break;
                    }
                }
                Some(Ok(Message::Ping(d))) => {
                    ws.send(Message::Pong(d)).await?;
                }
                Some(Err(e)) => return Err(anyhow!("WS error: {e}")),
                None => return Err(anyhow!("WS closed before ConnectResult")),
                _ => {}
            }
        }

        // ── 2. Subscribe ──────────────────────────────────────────────────────
        let query = format!(
            "{{{}}}",
            span_ids
                .iter()
                .map(|id| format!("span:id = \"{id}\""))
                .collect::<Vec<_>>()
                .join(" || ")
        );
        let channel = format!("{namespace}/ds/{}/search/{}", self.uid, Uuid::new_v4());
        let from_iso = unix_to_iso(start_s);
        let to_iso = unix_to_iso(end_s);
        debug!("→ span lookup channel={channel} query={query}");

        ws.send(Message::Text(serde_json::to_string(&json!({
            "id": 2,
            "subscribe": {
                "channel": channel,
                "flag": 1,
                "data": {
                    "refId": "A",
                    "datasource": { "type": "tempo", "uid": self.uid },
                    "queryType": "traceql",
                    "tableType": "spans",
                    "metricsQueryType": "range",
                    "limit": span_ids.len(),
                    "query": query,
                    "timeRange": { "from": from_iso, "to": to_iso }
                }
            }
        }))?))
        .await?;

        // ── 3. Stream ─────────────────────────────────────────────────────────
        let mut subscribed = false;
        let mut done = false;

        'outer: loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    if t.trim() == "{}" {
                        ws.send(Message::Text("{}".into())).await?;
                        continue;
                    }
                    for line in t.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let raw: Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(e) => {
                                debug!("  parse error: {e}");
                                continue;
                            }
                        };

                        if !subscribed && raw.get("id").and_then(|i| i.as_u64()) == Some(2) {
                            if let Some(err) = raw.get("error") {
                                let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
                                let msg = err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("unknown");
                                return Err(anyhow!("subscribe failed (code {code}): {msg}"));
                            }
                            subscribed = true;
                            continue;
                        }

                        let msg: ServerMsg = serde_json::from_value(raw).unwrap_or(ServerMsg {
                            id: None,
                            push: None,
                        });
                        if let Some(push) = msg.push {
                            if let Some(pub_msg) = push.publication {
                                let (pairs, state) = parse_span_frame(&pub_msg.data);
                                if !pairs.is_empty() {
                                    on_progress(pairs);
                                }
                                match state.as_str() {
                                    "done" => {
                                        done = true;
                                        break;
                                    }
                                    "error" => {
                                        let err = pub_msg
                                            .data
                                            .pointer("/data/values/3/0")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown");
                                        return Err(anyhow!("{err}"));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if done {
                        break 'outer;
                    }
                }
                Some(Ok(Message::Ping(d))) => {
                    ws.send(Message::Pong(d)).await?;
                }
                Some(Ok(Message::Close(f))) => {
                    debug!("← WS Close: {f:?}");
                    break;
                }
                Some(Err(e)) => return Err(anyhow!("WS error: {e}")),
                None => break,
                _ => {}
            }
        }

        let _ = ws.close(None).await;
        Ok(())
    }

    pub async fn search(&self, query: &str, start_s: u64, end_s: u64) -> Result<Vec<TempoTrace>> {
        let namespace = self.live_namespace().await?;

        let ws_base = self
            .grafana_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let ws_url = format!("{}/api/live/ws", ws_base);
        debug!("→ WebSocket {ws_url}  namespace={namespace}");

        let mut request = ws_url.as_str().into_client_request()?;
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token))?,
        );

        let (mut ws, _) = connect_async(request).await?;
        debug!("  WebSocket connected");

        // ── 1. Connect ────────────────────────────────────────────────────────
        ws.send(Message::Text(serde_json::to_string(&json!({
            "id": 1,
            "connect": { "name": "lptm", "version": "0.1.0" }
        }))?))
        .await?;
        debug!("→ Connect sent");

        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    debug!("← (connect phase): {t}");
                    if t.trim() == "{}" {
                        ws.send(Message::Text("{}".into())).await?;
                        continue;
                    }
                    let v: Value = serde_json::from_str(&t)?;
                    if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
                        if let Some(err) = v.get("error") {
                            return Err(anyhow!("connect error: {err}"));
                        }
                        debug!("← ConnectResult ok");
                        break;
                    }
                }
                Some(Ok(Message::Ping(d))) => {
                    ws.send(Message::Pong(d)).await?;
                }
                Some(Ok(other)) => {
                    debug!("← unexpected: {other:?}");
                }
                Some(Err(e)) => return Err(anyhow!("WS error: {e}")),
                None => return Err(anyhow!("WS closed before ConnectResult")),
            }
        }

        // ── 2. Subscribe ──────────────────────────────────────────────────────
        let channel = format!("{namespace}/ds/{}/search/{}", self.uid, Uuid::new_v4());
        let from_iso = unix_to_iso(start_s);
        let to_iso = unix_to_iso(end_s);
        debug!("  channel={channel}  from={from_iso}  to={to_iso}  query={query}");

        // A 32-char hex string is a raw trace ID — wrap it in a TraceQL filter.
        // The Grafana Live search channel only understands TraceQL queries.
        let is_trace_id = query.len() == 32 && query.chars().all(|c| c.is_ascii_hexdigit());
        let traceql_query = if is_trace_id {
            format!("{{ trace:id = \"{query}\" }}")
        } else {
            query.to_string()
        };

        ws.send(Message::Text(serde_json::to_string(&json!({
            "id": 2,
            "subscribe": {
                "channel": channel,
                "flag": 1,
                "data": {
                    "refId": "A",
                    "datasource": { "type": "tempo", "uid": self.uid },
                    "queryType": "traceql",
                    "limit": 20,
                    "tableType": "traces",
                    "metricsQueryType": "range",
                    "serviceMapUseNativeHistograms": false,
                    "query": traceql_query,
                    "SpansPerSpanSet": 3,
                    "timeRange": { "from": from_iso, "to": to_iso }
                }
            }
        }))?))
        .await?;
        debug!("→ Subscribe sent");

        // ── 3. Stream ─────────────────────────────────────────────────────────
        let mut subscribed = false;
        let mut traces = Vec::new();
        let mut done = false;

        'outer: loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    if t.trim() == "{}" {
                        ws.send(Message::Text("{}".into())).await?;
                        continue;
                    }
                    for line in t.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let raw: Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(e) => {
                                debug!("  parse error: {e}");
                                continue;
                            }
                        };

                        // SubscribeResult (id=2)
                        if !subscribed && raw.get("id").and_then(|i| i.as_u64()) == Some(2) {
                            if let Some(err) = raw.get("error") {
                                let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
                                let msg = err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("unknown");
                                return Err(anyhow!("subscribe failed (code {code}): {msg}"));
                            }
                            subscribed = true;
                            continue;
                        }

                        // Publication push
                        let msg: ServerMsg = serde_json::from_value(raw).unwrap_or(ServerMsg {
                            id: None,
                            push: None,
                        });
                        if let Some(push) = msg.push {
                            if let Some(pub_msg) = push.publication {
                                let (new_traces, state) = parse_frame(&pub_msg.data);
                                if !new_traces.is_empty() {
                                    traces = new_traces;
                                }
                                match state.as_str() {
                                    "done" => {
                                        done = true;
                                        break;
                                    }
                                    "error" => {
                                        let err = pub_msg
                                            .data
                                            .pointer("/data/values/3/0")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("unknown");
                                        return Err(anyhow!("{err}"));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if done {
                        break 'outer;
                    }
                }
                Some(Ok(Message::Ping(d))) => {
                    ws.send(Message::Pong(d)).await?;
                }
                Some(Ok(Message::Close(f))) => {
                    debug!("← WS Close: {f:?}");
                    break;
                }
                Some(Err(e)) => return Err(anyhow!("WS error: {e}")),
                None => break,
                _ => {}
            }
        }

        let _ = ws.close(None).await;
        debug!("← {} traces total", traces.len());
        Ok(traces)
    }

    /// Fetch a single trace by ID through Grafana.
    ///
    /// Tries two URL formats in order:
    /// 1. `/api/datasources/uid/{uid}/resources/api/traces/{id}` — plugin CallResource (Grafana 8+)
    /// 2. `/api/datasources/proxy/{numericId}/api/traces/{id}` — direct HTTP proxy (all versions)
    ///
    /// Returns all spans sorted by start time with tree depth computed.
    pub async fn fetch_trace(&self, trace_id: &str) -> Result<Vec<TempoSpan>> {
        // ── Attempt 1: resources endpoint (plugin CallResource) ───────────────
        let resources_url = format!(
            "{}/api/datasources/uid/{}/resources/api/traces/{}",
            self.grafana_url, self.uid, trace_id
        );
        debug!("→ fetch_trace resources {resources_url}");
        let resp = self
            .http
            .get(&resources_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status().is_success() {
            let body_text = resp.text().await?;
            debug!(
                "← resources response (first 400 chars): {}",
                &body_text[..body_text.len().min(400)]
            );
            let body: Value = serde_json::from_str(&body_text)
                .map_err(|e| anyhow!("resources response JSON parse error: {e}"))?;
            let spans = parse_otlp_trace(&body)?;
            debug!("← {} spans (resources) for {trace_id}", spans.len());
            return Ok(spans);
        }

        let first_status = resp.status();
        let first_body = resp.text().await.unwrap_or_default();
        debug!("  resources endpoint returned {first_status}: {first_body}");

        // Only fall through on 404/405 — other errors are real failures.
        if first_status != 404 && first_status != 405 {
            return Err(anyhow!(
                "GET {resources_url} returned {first_status}: {first_body}"
            ));
        }

        // ── Attempt 2: direct HTTP proxy (by numeric datasource ID) ──────────
        // Fetch the datasource info to get the numeric ID.
        let ds_info_url = format!("{}/api/datasources/uid/{}", self.grafana_url, self.uid);
        debug!("  looking up numeric id via {ds_info_url}");
        let ds_resp = self
            .http
            .get(&ds_info_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        if !ds_resp.status().is_success() {
            // Can't fall back — report the original error.
            return Err(anyhow!(
                "GET {resources_url} returned {first_status}: {first_body}"
            ));
        }

        let ds_info: Value = ds_resp.json().await?;
        let ds_id = ds_info
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("GET {resources_url} returned {first_status}: {first_body}"))?;

        let proxy_url = format!(
            "{}/api/datasources/proxy/{}/api/traces/{}",
            self.grafana_url, ds_id, trace_id
        );
        debug!("→ fetch_trace proxy {proxy_url}");
        let proxy_resp = self
            .http
            .get(&proxy_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .send()
            .await?;

        if !proxy_resp.status().is_success() {
            let proxy_status = proxy_resp.status();
            let proxy_body = proxy_resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "resources: {first_status} ({first_body}); \
                 proxy: {proxy_status} ({proxy_body})"
            ));
        }

        let body_text = proxy_resp.text().await?;
        debug!(
            "← proxy response (first 400 chars): {}",
            &body_text[..body_text.len().min(400)]
        );
        let body: Value = serde_json::from_str(&body_text)
            .map_err(|e| anyhow!("proxy response JSON parse error: {e}"))?;
        let spans = parse_otlp_trace(&body)?;
        debug!("← {} spans (proxy) for {trace_id}", spans.len());
        Ok(spans)
    }
}

// ── OTLP JSON deserialization ─────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct OtlpBatch {
    #[serde(default)]
    resource: Option<OtlpResource>,
    // Handle both scopeSpans (OTLP v1) and instrumentationLibrarySpans (legacy Tempo)
    #[serde(rename = "scopeSpans", alias = "instrumentationLibrarySpans", default)]
    scope_spans: Vec<OtlpScopeSpans>,
}

#[derive(Deserialize, Default)]
struct OtlpResource {
    #[serde(default)]
    attributes: Vec<OtlpAttr>,
}

#[derive(Deserialize, Default)]
struct OtlpScopeSpans {
    #[serde(default)]
    spans: Vec<OtlpSpan>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OtlpSpan {
    #[serde(default)]
    span_id: String,
    #[serde(default)]
    parent_span_id: Option<String>,
    #[serde(default)]
    name: String,
    /// In proto3 JSON this can be an integer (2) or a string ("SPAN_KIND_SERVER").
    #[serde(default)]
    kind: Option<Value>,
    /// camelCase (standard OTLP JSON) or snake_case (some Tempo versions).
    #[serde(alias = "start_time_unix_nano", default)]
    start_time_unix_nano: Option<Value>,
    #[serde(alias = "end_time_unix_nano", default)]
    end_time_unix_nano: Option<Value>,
    #[serde(default)]
    attributes: Vec<OtlpAttr>,
    #[serde(default)]
    status: Option<OtlpStatus>,
}

#[derive(Deserialize, Default)]
struct OtlpAttr {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: OtlpAnyValue,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OtlpAnyValue {
    string_value: Option<String>,
    int_value: Option<Value>,
    bool_value: Option<bool>,
    double_value: Option<f64>,
}

#[derive(Deserialize, Default)]
struct OtlpStatus {
    /// In proto3 JSON this can be an integer (2) or a string ("STATUS_CODE_ERROR").
    #[serde(default)]
    code: Option<Value>,
}

fn otlp_attr_to_string(v: &OtlpAnyValue) -> String {
    if let Some(ref s) = v.string_value {
        return s.clone();
    }
    if let Some(ref i) = v.int_value {
        return match i {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };
    }
    if let Some(b) = v.bool_value {
        return b.to_string();
    }
    if let Some(d) = v.double_value {
        return d.to_string();
    }
    String::new()
}

/// Convert proto3 JSON `kind` (integer or string enum) to an integer.
fn span_kind_to_int(v: &Option<Value>) -> i32 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) as i32,
        Some(Value::String(s)) => match s.as_str() {
            "SPAN_KIND_INTERNAL" => 1,
            "SPAN_KIND_SERVER" => 2,
            "SPAN_KIND_CLIENT" => 3,
            "SPAN_KIND_PRODUCER" => 4,
            "SPAN_KIND_CONSUMER" => 5,
            _ => 0,
        },
        _ => 0,
    }
}

/// Returns true when the proto3 JSON `status.code` indicates an error.
/// Accepts both integer (2) and string ("STATUS_CODE_ERROR") forms.
fn status_code_is_error(v: &Option<Value>) -> bool {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) == 2,
        Some(Value::String(s)) => s == "STATUS_CODE_ERROR",
        _ => false,
    }
}

fn parse_nano_ts(v: &Option<Value>) -> u64 {
    match v {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn parse_otlp_trace(body: &Value) -> Result<Vec<TempoSpan>> {
    // Tempo / OTLP returns one of:
    //   { "batches": [...] }        — old Tempo proto-JSON format
    //   { "resourceSpans": [...] }  — new OTLP JSON format (Tempo ≥ 2.x)
    //   [...]                       — bare array (unlikely but handle anyway)
    if let Some(obj) = body.as_object() {
        let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        debug!("  trace response top-level keys: {:?}", keys);
    }

    let batches_val = body
        .get("batches")
        .or_else(|| body.get("resourceSpans"))
        .unwrap_or(body);

    // Log raw keys of first batch element for diagnostics
    if let Some(first) = batches_val.as_array().and_then(|a| a.first()) {
        if let Some(obj) = first.as_object() {
            debug!("  first batch keys: {:?}", obj.keys().collect::<Vec<_>>());
            // Also log keys of first span
            let scope_key = ["scopeSpans", "instrumentationLibrarySpans"]
                .iter()
                .find(|k| obj.contains_key(**k))
                .copied();
            if let Some(sk) = scope_key {
                if let Some(span) = obj[sk]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|s| s.get("spans"))
                    .and_then(|s| s.as_array())
                    .and_then(|a| a.first())
                {
                    if let Some(sobj) = span.as_object() {
                        debug!("  first span keys: {:?}", sobj.keys().collect::<Vec<_>>());
                        debug!(
                            "  startTimeUnixNano = {:?}",
                            sobj.get("startTimeUnixNano")
                                .or_else(|| sobj.get("start_time_unix_nano"))
                        );
                    }
                }
            }
        }
    }

    let batches: Vec<OtlpBatch> = match serde_json::from_value(batches_val.clone()) {
        Ok(v) => v,
        Err(e) => {
            debug!("  failed to parse batches/resourceSpans: {e}");
            return Ok(vec![]);
        }
    };
    debug!("  parsed {} resource batches", batches.len());

    let mut spans: Vec<TempoSpan> = Vec::new();

    for batch in &batches {
        let resource_attrs: Vec<(String, String)> = batch
            .resource
            .as_ref()
            .map(|r| {
                r.attributes
                    .iter()
                    .map(|a| (a.key.clone(), otlp_attr_to_string(&a.value)))
                    .collect()
            })
            .unwrap_or_default();

        let service_name = resource_attrs
            .iter()
            .find(|(k, _)| k == "service.name")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();

        for scope in &batch.scope_spans {
            for s in &scope.spans {
                let start = parse_nano_ts(&s.start_time_unix_nano);
                let end = parse_nano_ts(&s.end_time_unix_nano);
                let duration_ns = end.saturating_sub(start);

                // Normalize empty parent_span_id to None
                let parent_span_id = match s.parent_span_id.as_deref() {
                    Some("") | None => None,
                    Some(p) => Some(p.to_string()),
                };

                let span_attrs: Vec<(String, String)> = s
                    .attributes
                    .iter()
                    .map(|a| (a.key.clone(), otlp_attr_to_string(&a.value)))
                    .collect();

                let error = s
                    .status
                    .as_ref()
                    .map(|st| status_code_is_error(&st.code))
                    .unwrap_or(false);

                let kind = span_kind_to_int(&s.kind);

                spans.push(TempoSpan {
                    span_id: s.span_id.clone(),
                    parent_span_id,
                    name: s.name.clone(),
                    service_name: service_name.clone(),
                    start_time_unix_nano: start,
                    duration_ns,
                    depth: 0, // computed below
                    resource_attrs: resource_attrs.clone(),
                    span_attrs,
                    kind,
                    error,
                });
            }
        }
    }

    debug!("  extracted {} spans before depth computation", spans.len());

    // Sort by start time ascending
    spans.sort_by_key(|s| s.start_time_unix_nano);

    // Compute depth iteratively (collect into a separate vec to satisfy borrow checker)
    let id_to_idx: HashMap<String, usize> = spans
        .iter()
        .enumerate()
        .map(|(i, s)| (s.span_id.clone(), i))
        .collect();

    let depths: Vec<usize> = (0..spans.len())
        .map(|i| {
            let mut depth = 0usize;
            let mut current_parent = spans[i].parent_span_id.clone();
            while let Some(pid) = current_parent {
                if let Some(&j) = id_to_idx.get(&pid) {
                    depth += 1;
                    current_parent = spans[j].parent_span_id.clone();
                } else {
                    break;
                }
            }
            depth
        })
        .collect();

    for (s, d) in spans.iter_mut().zip(depths) {
        s.depth = d;
    }

    Ok(spans)
}
