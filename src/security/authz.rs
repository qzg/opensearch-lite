use crate::{
    api_spec::{AccessClass, RouteMatch, Tier},
    http::request::Request,
    responses::{open_search_error, Response},
    rest_path::decode_path_param,
};

pub fn authorize(request: &Request, route: &RouteMatch) -> Result<(), Response> {
    if request.security.allows_all() {
        return Ok(());
    }

    if route.tier == Tier::AgentRead && control_like_path(&request.path) {
        return Err(open_search_error(
            501,
            "mainstack_search_unsupported_api_exception",
            format!("mainstack-search does not implement [{}] yet", route.api_name),
            Some("Use a supported local API or test this security/control API against real OpenSearch."),
        ));
    }

    let allowed = match route.access {
        AccessClass::Read => request.security.can_read(),
        AccessClass::Write => request.security.can_write(),
        AccessClass::Admin => request.security.is_admin(),
    };
    if allowed {
        return Ok(());
    }

    Err(open_search_error(
        403,
        "mainstack_search_authorization_exception",
        format!(
            "role does not permit [{}] access to [{}]",
            route.access.as_str(),
            route.api_name
        ),
        Some("Use credentials with a sufficient role, or change the request to a permitted read API."),
    ))
}

fn control_like_path(path: &str) -> bool {
    let decoded_segments = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(decode_path_param)
        .collect::<Vec<_>>();
    let segments: Vec<&str> = decoded_segments.iter().map(String::as_str).collect();
    matches!(
        segments.as_slice(),
        ["_plugins", "_security", ..]
            | ["_opendistro", "_security", ..]
            | ["_security", ..]
            | ["_snapshot", ..]
            | ["_tasks", ..]
            | ["_task", ..]
    )
}
