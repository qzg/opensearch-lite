use serde_json::Value;

use crate::storage::{StoreError, StoreResult};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RestoreRequest {
    pub indices: RestoreIndices,
    pub ignore_unavailable: bool,
    pub include_aliases: bool,
    pub rename_pattern: Option<String>,
    pub rename_replacement: Option<String>,
    pub index_settings: Option<Value>,
    pub ignore_index_settings: Vec<String>,
    pub wait_for_completion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestoreIndices {
    All,
    Names(Vec<String>),
}

pub(crate) fn parse_restore_request(
    body: &Value,
    query: &[(String, String)],
) -> StoreResult<RestoreRequest> {
    let object = body.as_object().ok_or_else(|| {
        StoreError::new(
            400,
            "parse_exception",
            "snapshot restore body must be a JSON object",
        )
    })?;

    let wait_for_completion = parse_restore_query(query)?;
    reject_unsupported_restore_options(body)?;

    Ok(RestoreRequest {
        indices: parse_indices(body)?,
        ignore_unavailable: parse_bool(body, "ignore_unavailable", false)?,
        include_aliases: parse_bool(body, "include_aliases", true)?,
        rename_pattern: parse_optional_string(body, "rename_pattern")?,
        rename_replacement: parse_optional_string(body, "rename_replacement")?,
        index_settings: match object.get("index_settings") {
            Some(value) if value.is_object() => Some(value.clone()),
            Some(_) => {
                return Err(StoreError::new(
                    400,
                    "parse_exception",
                    "snapshot restore index_settings must be an object",
                ));
            }
            None => None,
        },
        ignore_index_settings: parse_string_list(body, "ignore_index_settings")?,
        wait_for_completion,
    })
}

fn reject_unsupported_restore_options(body: &Value) -> StoreResult<()> {
    let Some(object) = body.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if !KNOWN_RESTORE_BODY_FIELDS.contains(&key.as_str()) {
            return Err(unsupported_restore_option(format!(
                "snapshot restore option [{key}] is not supported by mainstack-search"
            )));
        }
    }
    if parse_bool(body, "include_global_state", false)? {
        return Err(unsupported_restore_option(
            "include_global_state=true is not supported by mainstack-search snapshot restore",
        ));
    }
    if parse_bool(body, "partial", false)? {
        return Err(unsupported_restore_option(
            "partial snapshot restore is not supported by mainstack-search",
        ));
    }
    if let Some(storage_type) = body.get("storage_type") {
        let Some(storage_type) = storage_type.as_str() else {
            return Err(StoreError::new(
                400,
                "parse_exception",
                "snapshot restore storage_type must be a string",
            ));
        };
        if storage_type != "local" {
            return Err(unsupported_restore_option(format!(
                "snapshot restore storage_type [{storage_type}] is not supported by mainstack-search"
            )));
        }
    }
    for key in [
        "source_remote_store_repository",
        "source_remote_translog_repository",
        "rename_alias_pattern",
        "rename_alias_replacement",
    ] {
        if body.get(key).is_some() {
            return Err(unsupported_restore_option(format!(
                "snapshot restore option [{key}] is not supported by mainstack-search"
            )));
        }
    }
    Ok(())
}

fn parse_restore_query(query: &[(String, String)]) -> StoreResult<bool> {
    let mut wait_for_completion = None;
    let mut master_timeout_seen = false;
    let mut cluster_manager_timeout_seen = false;
    let mut pretty_seen = false;
    let mut human_seen = false;
    let mut error_trace_seen = false;
    let mut filter_path_seen = false;

    for (key, value) in query {
        match key.as_str() {
            "wait_for_completion" => {
                if wait_for_completion.is_some() {
                    return Err(duplicate_query_parameter("wait_for_completion"));
                }
                wait_for_completion = Some(parse_query_bool(value, "wait_for_completion")?);
            }
            "master_timeout" => {
                if master_timeout_seen {
                    return Err(duplicate_query_parameter("master_timeout"));
                }
                validate_time_query(value, "master_timeout")?;
                master_timeout_seen = true;
            }
            "cluster_manager_timeout" => {
                if cluster_manager_timeout_seen {
                    return Err(duplicate_query_parameter("cluster_manager_timeout"));
                }
                validate_time_query(value, "cluster_manager_timeout")?;
                cluster_manager_timeout_seen = true;
            }
            "pretty" => {
                if pretty_seen {
                    return Err(duplicate_query_parameter("pretty"));
                }
                parse_common_query_bool(value, "pretty")?;
                pretty_seen = true;
            }
            "human" => {
                if human_seen {
                    return Err(duplicate_query_parameter("human"));
                }
                parse_common_query_bool(value, "human")?;
                human_seen = true;
            }
            "error_trace" => {
                if error_trace_seen {
                    return Err(duplicate_query_parameter("error_trace"));
                }
                parse_common_query_bool(value, "error_trace")?;
                error_trace_seen = true;
            }
            "filter_path" => {
                if filter_path_seen {
                    return Err(duplicate_query_parameter("filter_path"));
                }
                filter_path_seen = true;
            }
            "source" => {
                return Err(unsupported_restore_option(
                    "snapshot restore query parameter [source] is not supported by mainstack-search",
                ));
            }
            "source_remote_store_repository" => {
                return Err(unsupported_restore_option(
                    "snapshot restore query parameter [source_remote_store_repository] is not supported by mainstack-search",
                ));
            }
            other => {
                return Err(StoreError::new(
                    400,
                    "parse_exception",
                    format!(
                        "snapshot restore query parameter [{other}] is not supported by mainstack-search"
                    ),
                ));
            }
        }
    }

    Ok(wait_for_completion.unwrap_or(false))
}

fn validate_time_query(value: &str, key: &'static str) -> StoreResult<()> {
    let raw = value.trim();
    if raw.is_empty() {
        return Err(StoreError::new(
            400,
            "parse_exception",
            format!("query parameter [{key}] must not be empty"),
        ));
    }
    if matches!(raw, "0" | "-1") {
        return Ok(());
    }
    let digits = raw
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return Err(StoreError::new(
            400,
            "parse_exception",
            format!("invalid time value [{raw}] for query parameter [{key}]"),
        ));
    }
    let value = digits.parse::<u64>().map_err(|_| {
        StoreError::new(
            400,
            "parse_exception",
            format!("invalid time value [{raw}] for query parameter [{key}]"),
        )
    })?;
    if value == 0 {
        return Err(StoreError::new(
            400,
            "parse_exception",
            format!("query parameter [{key}] must be positive or exactly [0]"),
        ));
    }
    let unit = &raw[digits.len()..];
    match unit {
        "" | "nanos" | "micros" | "ms" | "s" | "m" | "h" | "d" => Ok(()),
        _ => Err(StoreError::new(
            400,
            "parse_exception",
            format!("unsupported time unit [{unit}] for query parameter [{key}]"),
        )),
    }
}

fn parse_indices(body: &Value) -> StoreResult<RestoreIndices> {
    match body.get("indices") {
        None => Ok(RestoreIndices::All),
        Some(Value::String(raw)) => {
            let names = split_csv(raw);
            if names.is_empty() {
                Err(empty_indices_error())
            } else if names
                .iter()
                .any(|name| matches!(name.as_str(), "_all" | "*" | "all"))
            {
                Ok(RestoreIndices::All)
            } else {
                Ok(RestoreIndices::Names(names))
            }
        }
        Some(Value::Array(values)) => {
            if values.is_empty() {
                return Err(empty_indices_error());
            }
            let mut names = Vec::new();
            for value in values {
                let Some(value) = value.as_str() else {
                    return Err(StoreError::new(
                        400,
                        "parse_exception",
                        "snapshot restore indices array must contain only strings",
                    ));
                };
                names.extend(split_csv(value));
            }
            if names.is_empty() {
                Err(empty_indices_error())
            } else if names
                .iter()
                .any(|name| matches!(name.as_str(), "_all" | "*" | "all"))
            {
                Ok(RestoreIndices::All)
            } else {
                names.sort();
                names.dedup();
                Ok(RestoreIndices::Names(names))
            }
        }
        Some(_) => Err(StoreError::new(
            400,
            "parse_exception",
            "snapshot restore indices must be a string or array of strings",
        )),
    }
}

fn empty_indices_error() -> StoreError {
    StoreError::new(
        400,
        "parse_exception",
        "snapshot restore indices must name at least one index or use _all/*",
    )
}

fn parse_bool(body: &Value, key: &'static str, default: bool) -> StoreResult<bool> {
    match body.get(key) {
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(StoreError::new(
            400,
            "parse_exception",
            format!("snapshot restore [{key}] must be a boolean"),
        )),
        None => Ok(default),
    }
}

fn parse_optional_string(body: &Value, key: &'static str) -> StoreResult<Option<String>> {
    match body.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(StoreError::new(
            400,
            "parse_exception",
            format!("snapshot restore [{key}] must be a string"),
        )),
        None => Ok(None),
    }
}

fn parse_string_list(body: &Value, key: &'static str) -> StoreResult<Vec<String>> {
    match body.get(key) {
        None => Ok(Vec::new()),
        Some(Value::String(raw)) => Ok(split_csv(raw)),
        Some(Value::Array(values)) => {
            let mut parsed = Vec::new();
            for value in values {
                let Some(value) = value.as_str() else {
                    return Err(StoreError::new(
                        400,
                        "parse_exception",
                        format!("snapshot restore [{key}] array must contain only strings"),
                    ));
                };
                parsed.extend(split_csv(value));
            }
            parsed.sort();
            parsed.dedup();
            Ok(parsed)
        }
        Some(_) => Err(StoreError::new(
            400,
            "parse_exception",
            format!("snapshot restore [{key}] must be a string or array of strings"),
        )),
    }
}

fn parse_query_bool(value: &str, key: &'static str) -> StoreResult<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(StoreError::new(
            400,
            "parse_exception",
            format!("query parameter [{key}] must be true or false, got [{other}]"),
        )),
    }
}

fn parse_common_query_bool(value: &str, key: &'static str) -> StoreResult<bool> {
    match value {
        "" | "true" => Ok(true),
        "false" => Ok(false),
        other => Err(StoreError::new(
            400,
            "parse_exception",
            format!("query parameter [{key}] must be true or false, got [{other}]"),
        )),
    }
}

fn split_csv(raw: &str) -> Vec<String> {
    let mut values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn unsupported_restore_option(reason: impl Into<String>) -> StoreError {
    StoreError::new(501, "mainstack_search_unsupported_api_exception", reason)
}

fn duplicate_query_parameter(key: &'static str) -> StoreError {
    StoreError::new(
        400,
        "parse_exception",
        format!("duplicate query parameter [{key}]"),
    )
}

const KNOWN_RESTORE_BODY_FIELDS: &[&str] = &[
    "ignore_index_settings",
    "ignore_unavailable",
    "include_aliases",
    "include_global_state",
    "index_settings",
    "indices",
    "partial",
    "rename_pattern",
    "rename_replacement",
    "source_remote_store_repository",
    "source_remote_translog_repository",
    "storage_type",
    "rename_alias_pattern",
    "rename_alias_replacement",
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_restore_request, RestoreIndices};

    fn query(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn parse_restore_request_accepts_narrow_local_subset() {
        let request = parse_restore_request(
            &json!({
                "indices": ["orders", "customers"],
                "ignore_unavailable": true,
                "include_aliases": false,
                "rename_pattern": "(.+)",
                "rename_replacement": "restored-$1",
                "index_settings": { "index": { "refresh_interval": "1s" } },
                "ignore_index_settings": "index.uuid,index.provided_name"
            }),
            &query(&[
                ("wait_for_completion", "true"),
                ("master_timeout", "30s"),
                ("cluster_manager_timeout", "30s"),
                ("pretty", "true"),
                ("human", "false"),
                ("error_trace", ""),
                ("filter_path", "error.type,error.reason"),
            ]),
        )
        .unwrap();

        assert_eq!(
            request.indices,
            RestoreIndices::Names(vec!["customers".to_string(), "orders".to_string()])
        );
        assert!(request.ignore_unavailable);
        assert!(!request.include_aliases);
        assert_eq!(request.rename_pattern.as_deref(), Some("(.+)"));
        assert_eq!(request.rename_replacement.as_deref(), Some("restored-$1"));
        assert_eq!(
            request.index_settings.unwrap(),
            json!({ "index": { "refresh_interval": "1s" } })
        );
        assert_eq!(
            request.ignore_index_settings,
            vec!["index.provided_name".to_string(), "index.uuid".to_string()]
        );
        assert!(request.wait_for_completion);
    }

    #[test]
    fn parse_restore_request_rejects_unsupported_options() {
        for body in [
            json!({ "include_global_state": true }),
            json!({ "partial": true }),
            json!({ "storage_type": "remote_snapshot" }),
            json!({ "source_remote_store_repository": "remote" }),
            json!({ "rename_alias_pattern": "(.+)" }),
            json!({ "feature_states": ["security"] }),
        ] {
            let error = parse_restore_request(&body, &[]).unwrap_err();
            assert_eq!(error.status, 501);
            assert_eq!(
                error.error_type,
                "mainstack_search_unsupported_api_exception"
            );
        }

        let error = parse_restore_request(
            &json!({}),
            &query(&[("source_remote_store_repository", "remote")]),
        )
        .unwrap_err();
        assert_eq!(error.status, 501);
        assert_eq!(
            error.error_type,
            "mainstack_search_unsupported_api_exception"
        );

        let error = parse_restore_request(&json!({}), &query(&[("source", "{}")])).unwrap_err();
        assert_eq!(error.status, 501);
        assert_eq!(
            error.error_type,
            "mainstack_search_unsupported_api_exception"
        );
    }

    #[test]
    fn parse_restore_request_rejects_invalid_query_parameters() {
        for query in [
            query(&[("wait_for_completion", "eventually")]),
            query(&[
                ("wait_for_completion", "true"),
                ("wait_for_completion", "false"),
            ]),
            query(&[("master_timeout", "30s"), ("master_timeout", "60s")]),
            query(&[
                ("cluster_manager_timeout", "30s"),
                ("cluster_manager_timeout", "60s"),
            ]),
            query(&[("master_timeout", "banana")]),
            query(&[("cluster_manager_timeout", "0s")]),
            query(&[("pretty", "sometimes")]),
            query(&[("filter_path", "error.type"), ("filter_path", "status")]),
            query(&[("unknown", "value")]),
        ] {
            let error = parse_restore_request(&json!({}), &query).unwrap_err();
            assert_eq!(error.status, 400);
            assert_eq!(error.error_type, "parse_exception");
        }
    }

    #[test]
    fn parse_restore_request_accepts_opensearch_shaped_timeout_values() {
        for query in [
            query(&[("master_timeout", "0")]),
            query(&[("master_timeout", "-1")]),
            query(&[("master_timeout", "10micros")]),
            query(&[("cluster_manager_timeout", "10nanos")]),
        ] {
            parse_restore_request(&json!({}), &query).unwrap();
        }
    }

    #[test]
    fn parse_restore_request_rejects_invalid_body_shapes() {
        for body in [
            json!({ "indices": "" }),
            json!({ "indices": [] }),
            json!({ "indices": [","] }),
            json!({ "indices": ["orders", 1] }),
            json!({ "index_settings": "index.refresh_interval=1s" }),
            json!({ "rename_pattern": 1 }),
            json!({ "rename_replacement": false }),
            json!({ "ignore_index_settings": 1 }),
            json!({ "ignore_index_settings": ["index.uuid", 1] }),
            json!({ "storage_type": false }),
        ] {
            let error = parse_restore_request(&body, &[]).unwrap_err();
            assert_eq!(error.status, 400, "{body}");
            assert_eq!(error.error_type, "parse_exception", "{body}");
        }
    }
}
