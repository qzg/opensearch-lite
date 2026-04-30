use serde_json::Value;

pub const QUERY_BODY_LIMIT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_QUERY_DEPTH: usize = 32;
pub const MAX_QUERY_CLAUSES: usize = 1024;
pub const MAX_TERMS_VALUES: usize = 4096;
pub const MAX_AGGREGATION_DEPTH: usize = 8;
pub const MAX_BUCKETS: usize = 10_000;

#[derive(Debug, Clone)]
pub struct SearchLimits {
    pub max_result_window: usize,
}

pub fn validate_body_bytes(len: usize) -> Result<(), String> {
    if len > QUERY_BODY_LIMIT_BYTES {
        return Err(format!(
            "search body is {len} bytes, exceeding the local query limit of {QUERY_BODY_LIMIT_BYTES} bytes"
        ));
    }
    Ok(())
}

pub fn validate_request(
    body: &Value,
    from: usize,
    size: usize,
    limits: SearchLimits,
) -> Result<(), String> {
    if from.saturating_add(size) > limits.max_result_window {
        return Err("from + size exceeds max result window".to_string());
    }
    if let Some(query) = body.get("query") {
        let mut counter = QueryCounter::default();
        validate_query(query, 0, &mut counter)?;
    }
    if let Some(aggs) = body.get("aggregations").or_else(|| body.get("aggs")) {
        validate_aggs(aggs, 0, &mut 0, &limits)?;
    }
    Ok(())
}

#[derive(Default)]
struct QueryCounter {
    clauses: usize,
}

fn validate_query(query: &Value, depth: usize, counter: &mut QueryCounter) -> Result<(), String> {
    if depth > MAX_QUERY_DEPTH {
        return Err(format!(
            "query depth exceeds local limit of {MAX_QUERY_DEPTH}"
        ));
    }
    let Some(object) = query.as_object() else {
        return Err("query must be an object".to_string());
    };
    counter.clauses = counter.clauses.saturating_add(1);
    if counter.clauses > MAX_QUERY_CLAUSES {
        return Err(format!(
            "query clause count exceeds local limit of {MAX_QUERY_CLAUSES}"
        ));
    }
    if let Some(terms) = object.get("terms") {
        let Some(terms_object) = terms.as_object() else {
            return Err("terms query must be an object".to_string());
        };
        for value in terms_object.values() {
            if value.as_array().map(|values| values.len()).unwrap_or(0) > MAX_TERMS_VALUES {
                return Err(format!(
                    "terms query value count exceeds local limit of {MAX_TERMS_VALUES}"
                ));
            }
        }
    }
    if let Some(bool_query) = object.get("bool") {
        for key in ["must", "filter", "should", "must_not"] {
            for clause in clauses(bool_query.get(key)) {
                validate_query(clause, depth + 1, counter)?;
            }
        }
    }
    if let Some(nested) = object.get("nested") {
        if let Some(query) = nested.get("query") {
            validate_query(query, depth + 1, counter)?;
        }
    }
    Ok(())
}

fn clauses(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(values)) => values.iter().collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    }
}

fn validate_aggs(
    aggs: &Value,
    depth: usize,
    buckets: &mut usize,
    limits: &SearchLimits,
) -> Result<(), String> {
    if depth > MAX_AGGREGATION_DEPTH {
        return Err(format!(
            "aggregation depth exceeds local limit of {MAX_AGGREGATION_DEPTH}"
        ));
    }
    let Some(object) = aggs.as_object() else {
        return Err("aggregations must be an object".to_string());
    };
    for aggregation in object.values() {
        if let Some(config) = aggregation.get("terms") {
            add_buckets(
                config.get("size").and_then(Value::as_u64).unwrap_or(10),
                buckets,
            )?;
        } else if let Some(config) = aggregation.get("filters") {
            let filter_count = match config.get("filters") {
                Some(Value::Object(filters)) => filters.len() as u64,
                Some(Value::Array(filters)) => filters.len() as u64,
                _ => 0,
            };
            add_buckets(filter_count, buckets)?;
        } else if let Some(config) = aggregation.get("range") {
            add_buckets(
                config
                    .get("ranges")
                    .and_then(Value::as_array)
                    .map(|ranges| ranges.len() as u64)
                    .unwrap_or(0),
                buckets,
            )?;
        } else if aggregation.get("missing").is_some() {
            add_buckets(1, buckets)?;
        } else if let Some(config) = aggregation.get("top_hits") {
            let from = config.get("from").and_then(Value::as_u64).unwrap_or(0);
            let size = config.get("size").and_then(Value::as_u64).unwrap_or(3);
            if (from as usize).saturating_add(size as usize) > limits.max_result_window {
                return Err("top_hits from + size exceeds max result window".to_string());
            }
        }
        if let Some(sub_aggs) = aggregation
            .get("aggregations")
            .or_else(|| aggregation.get("aggs"))
        {
            validate_aggs(sub_aggs, depth + 1, buckets, limits)?;
        }
    }
    Ok(())
}

fn add_buckets(count: u64, buckets: &mut usize) -> Result<(), String> {
    *buckets = buckets.saturating_add(count as usize);
    if *buckets > MAX_BUCKETS {
        return Err(format!(
            "aggregation bucket count exceeds local limit of {MAX_BUCKETS}"
        ));
    }
    Ok(())
}
