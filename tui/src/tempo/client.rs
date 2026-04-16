use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use log::debug;
use serde::Deserialize;
use serde_json::{json, Value};
use shared::TempoTrace;
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
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> { Ok(v) }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> { Ok(v as u64) }
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

    if let Some(err) = values.get(3).and_then(|a| a.get(0)).and_then(|s| s.as_str()) {
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
        .get(2).and_then(|a| a.get(0)).and_then(|s| s.as_str())
        .unwrap_or("").to_string();

    if let Some(err) = values.get(3).and_then(|a| a.get(0)).and_then(|s| s.as_str()) {
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
    debug!("  span frame: {} span→trace pairs, state={state}", map.len());
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
        Self { http: reqwest::Client::new(), grafana_url, uid, token }
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
            return Err(anyhow!("GET /api/frontend/settings returned {}", resp.status()));
        }
        let settings: FrontendSettings = resp.json().await?;
        debug!("  frontend/settings: liveNamespaced={} namespace={:?}",
            settings.live_namespaced, settings.namespace);

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
            "connect": { "name": "grafex", "version": "0.1.0" }
        }))?))
        .await?;

        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    if t.trim() == "{}" { ws.send(Message::Text("{}".into())).await?; continue; }
                    let v: Value = serde_json::from_str(&t)?;
                    if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
                        if let Some(err) = v.get("error") {
                            return Err(anyhow!("connect error: {err}"));
                        }
                        break;
                    }
                }
                Some(Ok(Message::Ping(d))) => { ws.send(Message::Pong(d)).await?; }
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
                        if line.is_empty() { continue; }
                        let raw: Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(e) => { debug!("  parse error: {e}"); continue; }
                        };

                        if !subscribed && raw.get("id").and_then(|i| i.as_u64()) == Some(2) {
                            if let Some(err) = raw.get("error") {
                                let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
                                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                                return Err(anyhow!("subscribe failed (code {code}): {msg}"));
                            }
                            subscribed = true;
                            continue;
                        }

                        let msg: ServerMsg = serde_json::from_value(raw).unwrap_or(ServerMsg { id: None, push: None });
                        if let Some(push) = msg.push {
                            if let Some(pub_msg) = push.publication {
                                let (pairs, state) = parse_span_frame(&pub_msg.data);
                                if !pairs.is_empty() {
                                    on_progress(pairs);
                                }
                                match state.as_str() {
                                    "done" => { done = true; break; }
                                    "error" => {
                                        let err = pub_msg.data.pointer("/data/values/3/0")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown");
                                        return Err(anyhow!("{err}"));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if done { break 'outer; }
                }
                Some(Ok(Message::Ping(d))) => { ws.send(Message::Pong(d)).await?; }
                Some(Ok(Message::Close(f))) => { debug!("← WS Close: {f:?}"); break; }
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
            "connect": { "name": "grafex", "version": "0.1.0" }
        }))?))
        .await?;
        debug!("→ Connect sent");

        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    debug!("← (connect phase): {t}");
                    if t.trim() == "{}" { ws.send(Message::Text("{}".into())).await?; continue; }
                    let v: Value = serde_json::from_str(&t)?;
                    if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
                        if let Some(err) = v.get("error") {
                            return Err(anyhow!("connect error: {err}"));
                        }
                        debug!("← ConnectResult ok");
                        break;
                    }
                }
                Some(Ok(Message::Ping(d))) => { ws.send(Message::Pong(d)).await?; }
                Some(Ok(other)) => { debug!("← unexpected: {other:?}"); }
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
                        if line.is_empty() { continue; }
                        let raw: Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(e) => { debug!("  parse error: {e}"); continue; }
                        };

                        // SubscribeResult (id=2)
                        if !subscribed && raw.get("id").and_then(|i| i.as_u64()) == Some(2) {
                            if let Some(err) = raw.get("error") {
                                let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
                                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                                return Err(anyhow!("subscribe failed (code {code}): {msg}"));
                            }
                            subscribed = true;
                            continue;
                        }

                        // Publication push
                        let msg: ServerMsg = serde_json::from_value(raw).unwrap_or(ServerMsg { id: None, push: None });
                        if let Some(push) = msg.push {
                            if let Some(pub_msg) = push.publication {
                                let (new_traces, state) = parse_frame(&pub_msg.data);
                                if !new_traces.is_empty() { traces = new_traces; }
                                match state.as_str() {
                                    "done" => { done = true; break; }
                                    "error" => {
                                        let err = pub_msg.data.pointer("/data/values/3/0")
                                            .and_then(|s| s.as_str()).unwrap_or("unknown");
                                        return Err(anyhow!("{err}"));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if done { break 'outer; }
                }
                Some(Ok(Message::Ping(d))) => { ws.send(Message::Pong(d)).await?; }
                Some(Ok(Message::Close(f))) => { debug!("← WS Close: {f:?}"); break; }
                Some(Err(e)) => return Err(anyhow!("WS error: {e}")),
                None => break,
                _ => {}
            }
        }

        let _ = ws.close(None).await;
        debug!("← {} traces total", traces.len());
        Ok(traces)
    }
}
