use crux_http::protocol::{HttpHeader, HttpRequest, HttpResponse, HttpResult};
use log::debug;

pub async fn execute(request: &HttpRequest) -> HttpResult {
    let client = reqwest::Client::new();

    let method =
        reqwest::Method::from_bytes(request.method.as_bytes()).unwrap_or(reqwest::Method::GET);

    debug!("→ {} {}", request.method, request.url);
    if !request.body.is_empty() {
        debug!("  body: {}", String::from_utf8_lossy(&request.body));
    }

    let mut builder = client.request(method, &request.url);

    for header in &request.headers {
        builder = builder.header(&header.name, &header.value);
    }

    if !request.body.is_empty() {
        builder = builder.body(request.body.clone());
    }

    match builder.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            debug!("← {} {}", status, request.url);
            let headers: Vec<HttpHeader> = response
                .headers()
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: name.to_string(),
                    value: value.to_str().unwrap_or("").to_string(),
                })
                .collect();
            match response.bytes().await {
                Ok(bytes) => {
                    if status >= 400 {
                        debug!("  error body: {}", String::from_utf8_lossy(&bytes));
                    }
                    HttpResult::Ok(HttpResponse {
                        status,
                        headers,
                        body: bytes.to_vec(),
                    })
                }
                Err(e) => HttpResult::Err(crux_http::HttpError::Io(e.to_string())),
            }
        }
        Err(e) => {
            debug!("← error {}: {}", request.url, e);
            HttpResult::Err(crux_http::HttpError::Io(e.to_string()))
        }
    }
}
