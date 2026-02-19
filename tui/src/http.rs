use crux_http::protocol::{HttpHeader, HttpRequest, HttpResponse, HttpResult};

pub async fn execute(request: &HttpRequest) -> HttpResult {
    let client = reqwest::Client::new();

    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .unwrap_or(reqwest::Method::GET);

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
            let headers: Vec<HttpHeader> = response
                .headers()
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: name.to_string(),
                    value: value.to_str().unwrap_or("").to_string(),
                })
                .collect();
            match response.bytes().await {
                Ok(bytes) => HttpResult::Ok(HttpResponse {
                    status,
                    headers,
                    body: bytes.to_vec(),
                }),
                Err(e) => HttpResult::Err(crux_http::HttpError::Io(e.to_string())),
            }
        }
        Err(e) => HttpResult::Err(crux_http::HttpError::Io(e.to_string())),
    }
}
