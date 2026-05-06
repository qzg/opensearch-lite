use crate::{
    api, api_spec,
    http::{body::ensure_body_limit, request::Request},
    responses::{open_search_error, Response},
    security,
    server::AppState,
};

pub async fn handle(state: AppState, request: Request) -> Response {
    if let Err(response) = ensure_body_limit(&request.body, state.config.max_body_bytes) {
        return response;
    }
    if let Err(response) = guard_request(&state, &request) {
        return response;
    }
    let route = api_spec::classify(&request.method, &request.path);
    if let Err(response) = security::authz::authorize(&request, &route) {
        return response;
    }
    api::handle_classified_request(state, request, route).await
}

fn guard_request(state: &AppState, request: &Request) -> Result<(), Response> {
    if !state.config.allow_nonlocal_listen {
        if let Some(host) = request.headers.get("host") {
            if !host_header_is_loopback(host) {
                return Err(open_search_error(
                    403,
                    "mainstack_search_host_rejected_exception",
                    "request Host is not loopback",
                    Some("Use localhost or 127.0.0.1 when calling this local-only server."),
                ));
            }
        }
    }

    if is_state_changing_method(&request.method) {
        if let Some(site) = request.headers.get("sec-fetch-site") {
            if matches!(site.as_str(), "cross-site" | "same-site") {
                return Err(cross_site_error());
            }
        }
        for header in ["origin", "referer"] {
            if let Some(value) = request.headers.get(header) {
                if !origin_like_is_loopback(value) {
                    return Err(cross_site_error());
                }
            }
        }
        if !request.body.is_empty() {
            let content_type = request
                .headers
                .get("content-type")
                .map(|value| value.split(';').next().unwrap_or(value).trim())
                .unwrap_or("");
            if !write_content_type_is_json(content_type) {
                return Err(open_search_error(
                    415,
                    "mainstack_search_content_type_exception",
                    "write requests with bodies must use a JSON or NDJSON content type",
                    Some("Set Content-Type to application/json or application/x-ndjson."),
                ));
            }
        }
    }

    Ok(())
}

fn is_state_changing_method(method: &http::Method) -> bool {
    matches!(
        *method,
        http::Method::POST | http::Method::PUT | http::Method::DELETE | http::Method::PATCH
    )
}

fn write_content_type_is_json(content_type: &str) -> bool {
    let media_type = content_type.to_ascii_lowercase();
    matches!(
        media_type.as_str(),
        "application/json"
            | "application/x-ndjson"
            | "application/vnd.elasticsearch+json"
            | "application/vnd.elasticsearch+x-ndjson"
            | "application/vnd.opensearch+json"
            | "application/vnd.opensearch+x-ndjson"
    )
}

fn cross_site_error() -> Response {
    open_search_error(
        403,
        "mainstack_search_cross_site_request_exception",
        "cross-site browser request rejected",
        Some("Call mainstack-search from local tooling, or remove browser cross-site headers."),
    )
}

fn host_header_is_loopback(value: &str) -> bool {
    let host = value
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| value.split(':').next().unwrap_or(value));
    host_is_loopback(host)
}

fn origin_like_is_loopback(value: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(host_is_loopback))
        .unwrap_or(false)
}

fn host_is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}
