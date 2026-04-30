use std::cmp::Ordering;

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use serde_json::{json, Value};

use super::limits;
use crate::storage::{Database, StoredDocument};

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub indices: Vec<String>,
    pub body: Value,
    pub from: usize,
    pub size: usize,
}

pub fn search(db: &Database, request: SearchRequest) -> Result<Value, String> {
    let query = request
        .body
        .get("query")
        .cloned()
        .unwrap_or_else(|| json!({"match_all": {}}));
    let mut total = 0usize;
    let mut hits = Vec::new();
    let sorted = request.body.get("sort").is_some();
    let aggregations = request
        .body
        .get("aggregations")
        .or_else(|| request.body.get("aggs"));
    let needs_all_hits = sorted || aggregations.is_some();

    for index_name in expand_indices(db, &request.indices) {
        let Some(index) = db.indexes.get(&index_name) else {
            continue;
        };
        for doc in index.documents.values() {
            if matches_query(doc, &query)? {
                total += 1;
                if needs_all_hits || (total > request.from && hits.len() < request.size) {
                    hits.push(MatchedDocument {
                        index: index_name.clone(),
                        doc,
                        score: score(&doc.source, &query),
                    });
                }
            }
        }
    }

    apply_sort(&mut hits, request.body.get("sort"));
    let offset = if needs_all_hits { request.from } else { 0 };
    let paged = hits
        .iter()
        .skip(offset)
        .take(request.size)
        .map(|hit| {
            let mut response = json!({
                "_index": hit.index,
                "_id": hit.doc.id,
                "_score": hit.score,
                "_version": hit.doc.version,
                "_seq_no": hit.doc.seq_no,
                "_primary_term": hit.doc.primary_term
            });
            if request.body.get("_source") != Some(&Value::Bool(false)) {
                response["_source"] = filter_source(&hit.doc.source, request.body.get("_source"));
            }
            response
        })
        .collect::<Vec<_>>();

    let mut response = json!({
        "took": 0,
        "timed_out": false,
        "_shards": { "total": 1, "successful": 1, "skipped": 0, "failed": 0 },
        "hits": {
            "total": { "value": total, "relation": "eq" },
            "max_score": if total == 0 { Value::Null } else { json!(1.0) },
            "hits": paged
        }
    });
    if let Some(aggregations) = aggregations {
        response["aggregations"] = evaluate_aggregations(&hits, aggregations)?;
    }
    Ok(response)
}

#[derive(Clone)]
struct MatchedDocument<'a> {
    index: String,
    doc: &'a StoredDocument,
    score: f64,
}

fn expand_indices(db: &Database, requested: &[String]) -> Vec<String> {
    if requested.is_empty()
        || requested
            .iter()
            .any(|index| index == "_all" || index == "*")
    {
        return db.indexes.keys().cloned().collect();
    }
    requested
        .iter()
        .filter_map(|name| db.resolve_index(name))
        .collect()
}

fn matches_query(doc: &StoredDocument, query: &Value) -> Result<bool, String> {
    matches_query_source(&doc.source, &doc.id, query)
}

pub fn document_matches(doc: &StoredDocument, query: &Value) -> Result<bool, String> {
    matches_query(doc, query)
}

fn matches_query_source(source: &Value, id: &str, query: &Value) -> Result<bool, String> {
    let Some(object) = query.as_object() else {
        return Err("query must be an object".to_string());
    };
    if object.contains_key("match_all") {
        return Ok(true);
    }
    if let Some(ids) = object.get("ids") {
        let values = ids
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| "ids query requires values".to_string())?;
        return Ok(values.iter().any(|value| value.as_str() == Some(id)));
    }
    if let Some(term) = object.get("term") {
        let (field, expected) = single_field(term, "term")?;
        let expected = expected.get("value").unwrap_or(expected);
        return Ok(value_at(source, field)
            .map(|actual| value_matches_term(actual, expected))
            .unwrap_or(false));
    }
    if let Some(terms) = object.get("terms") {
        let (field, expected) = single_field(terms, "terms")?;
        let Some(values) = expected.as_array() else {
            return Err("terms query value must be an array".to_string());
        };
        return Ok(value_at(source, field)
            .map(|actual| value_matches_any(actual, values))
            .unwrap_or(false));
    }
    if let Some(range) = object.get("range") {
        let (field, bounds) = single_field(range, "range")?;
        return Ok(value_at(source, field)
            .map(|actual| range_matches(actual, bounds))
            .unwrap_or(false));
    }
    if let Some(exists) = object.get("exists") {
        let field = exists
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| "exists query requires field".to_string())?;
        return Ok(value_at(source, field).is_some());
    }
    if let Some(match_query) = object.get("match") {
        let (field, expected) = single_field(match_query, "match")?;
        let expected = expected.get("query").unwrap_or(expected);
        return Ok(value_at(source, field)
            .and_then(Value::as_str)
            .zip(expected.as_str())
            .map(|(actual, expected)| actual.to_lowercase().contains(&expected.to_lowercase()))
            .unwrap_or(false));
    }
    if let Some(match_phrase) = object.get("match_phrase") {
        let (field, expected) = single_field(match_phrase, "match_phrase")?;
        let expected = expected.get("query").unwrap_or(expected);
        return Ok(value_at(source, field)
            .and_then(Value::as_str)
            .zip(expected.as_str())
            .map(|(actual, expected)| actual.to_lowercase().contains(&expected.to_lowercase()))
            .unwrap_or(false));
    }
    if let Some(match_phrase_prefix) = object.get("match_phrase_prefix") {
        let (field, expected) = single_field(match_phrase_prefix, "match_phrase_prefix")?;
        let expected = expected.get("query").unwrap_or(expected);
        return Ok(value_at(source, field)
            .and_then(Value::as_str)
            .zip(expected.as_str())
            .map(|(actual, expected)| actual.to_lowercase().starts_with(&expected.to_lowercase()))
            .unwrap_or(false));
    }
    if let Some(simple_query_string) = object.get("simple_query_string") {
        return matches_simple_query_string(source, simple_query_string);
    }
    if let Some(nested) = object.get("nested") {
        return matches_nested_query(source, id, nested);
    }
    if let Some(prefix) = object.get("prefix") {
        let (field, expected) = single_field(prefix, "prefix")?;
        let expected = expected.get("value").unwrap_or(expected);
        return Ok(value_at(source, field)
            .and_then(Value::as_str)
            .zip(expected.as_str())
            .map(|(actual, expected)| actual.starts_with(expected))
            .unwrap_or(false));
    }
    if let Some(wildcard) = object.get("wildcard") {
        let (field, expected) = single_field(wildcard, "wildcard")?;
        let expected = expected.get("value").unwrap_or(expected);
        return Ok(value_at(source, field)
            .and_then(Value::as_str)
            .zip(expected.as_str())
            .map(|(actual, expected)| wildcard_matches(expected, actual))
            .unwrap_or(false));
    }
    if let Some(bool_query) = object.get("bool") {
        return matches_bool(source, id, bool_query);
    }
    Err(format!(
        "unsupported query type [{}]",
        object.keys().next().cloned().unwrap_or_default()
    ))
}

fn matches_bool(source: &Value, id: &str, bool_query: &Value) -> Result<bool, String> {
    let must = clauses(bool_query.get("must"));
    let filter = clauses(bool_query.get("filter"));
    let should = clauses(bool_query.get("should"));
    let must_not = clauses(bool_query.get("must_not"));

    for clause in must.iter().chain(filter.iter()) {
        if !matches_query_source(source, id, clause)? {
            return Ok(false);
        }
    }
    for clause in &must_not {
        if matches_query_source(source, id, clause)? {
            return Ok(false);
        }
    }
    let default_minimum_should_match =
        if should.is_empty() || !must.is_empty() || !filter.is_empty() {
            0
        } else {
            1
        };
    let minimum_should_match =
        minimum_should_match(bool_query).unwrap_or(default_minimum_should_match);
    let should_matches = should
        .iter()
        .filter(|clause| matches_query_source(source, id, clause).unwrap_or(false))
        .count();
    if should_matches < minimum_should_match {
        return Ok(false);
    }
    Ok(true)
}

fn matches_simple_query_string(source: &Value, query: &Value) -> Result<bool, String> {
    let query_text = query
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| "simple_query_string requires query".to_string())?;
    if query_text.trim().is_empty() || query_text.trim() == "*" {
        return Ok(true);
    }
    let fields = query
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| fields.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let haystack = if fields.is_empty() {
        let mut values = Vec::new();
        collect_string_values(source, &mut values);
        values.join(" ")
    } else {
        fields
            .iter()
            .filter_map(|field| value_at(source, field).and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
    .to_lowercase();
    let terms = query_text
        .split_whitespace()
        .map(|term| term.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '+' | '-' | '(' | ')')))
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    Ok(!terms.is_empty() && terms.iter().all(|term| haystack.contains(term)))
}

fn collect_string_values<'a>(value: &'a Value, output: &mut Vec<&'a str>) {
    match value {
        Value::String(value) => output.push(value),
        Value::Array(values) => {
            for value in values {
                collect_string_values(value, output);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_string_values(value, output);
            }
        }
        _ => {}
    }
}

fn matches_nested_query(source: &Value, id: &str, nested: &Value) -> Result<bool, String> {
    let path = nested
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "nested query requires path".to_string())?;
    let query = nested
        .get("query")
        .ok_or_else(|| "nested query requires query".to_string())?;
    let Some(value) = value_at(source, path) else {
        return Ok(false);
    };
    match value {
        Value::Array(values) => {
            values
                .iter()
                .filter(|value| value.is_object())
                .try_fold(false, |matched, value| {
                    if matched {
                        Ok(true)
                    } else {
                        matches_nested_query_source(value, path, id, query)
                    }
                })
        }
        Value::Object(_) => matches_nested_query_source(value, path, id, query),
        _ => Err("nested query path must resolve to an object or array of objects".to_string()),
    }
}

fn matches_nested_query_source(
    value: &Value,
    path: &str,
    id: &str,
    query: &Value,
) -> Result<bool, String> {
    let mut nested_source = value.clone();
    insert_path(&mut nested_source, path, value.clone());
    matches_query_source(&nested_source, id, query)
}

fn minimum_should_match(bool_query: &Value) -> Option<usize> {
    match bool_query.get("minimum_should_match")? {
        Value::Number(number) => number.as_u64().map(|value| value as usize),
        Value::String(value) => value.parse::<usize>().ok(),
        _ => None,
    }
}

fn clauses(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(values)) => values.clone(),
        Some(value) => vec![value.clone()],
        None => Vec::new(),
    }
}

fn single_field<'a>(value: &'a Value, kind: &str) -> Result<(&'a str, &'a Value), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{kind} query must be an object"))?;
    if object.len() != 1 {
        return Err(format!("{kind} query must contain exactly one field"));
    }
    let (field, value) = object.iter().next().expect("len checked");
    Ok((field.as_str(), value))
}

fn value_at<'a>(source: &'a Value, field: &str) -> Option<&'a Value> {
    field
        .split('.')
        .try_fold(source, |value, segment| value.get(segment))
}

fn values_equal(actual: &Value, expected: &Value) -> bool {
    actual == expected
        || actual
            .as_str()
            .zip(expected.as_str())
            .map(|(a, e)| a == e)
            .unwrap_or(false)
        || actual
            .as_f64()
            .zip(expected.as_f64())
            .map(|(a, e)| (a - e).abs() < f64::EPSILON)
            .unwrap_or(false)
}

fn value_matches_term(actual: &Value, expected: &Value) -> bool {
    match actual {
        Value::Array(values) => values.iter().any(|actual| values_equal(actual, expected)),
        actual => values_equal(actual, expected),
    }
}

fn value_matches_any(actual: &Value, expected: &[Value]) -> bool {
    match actual {
        Value::Array(values) => values.iter().any(|actual| {
            expected
                .iter()
                .any(|expected| values_equal(actual, expected))
        }),
        actual => expected
            .iter()
            .any(|expected| values_equal(actual, expected)),
    }
}

fn range_matches(actual: &Value, bounds: &Value) -> bool {
    let Some(bounds) = bounds.as_object() else {
        return false;
    };
    for (op, bound) in bounds {
        let Some(ordering) = compare_values(actual, bound) else {
            return false;
        };
        let ok = match op.as_str() {
            "gt" => ordering == Ordering::Greater,
            "gte" => matches!(ordering, Ordering::Greater | Ordering::Equal),
            "lt" => ordering == Ordering::Less,
            "lte" => matches!(ordering, Ordering::Less | Ordering::Equal),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn compare_values(actual: &Value, bound: &Value) -> Option<Ordering> {
    if let Some(ordering) = actual
        .as_f64()
        .zip(bound.as_f64())
        .and_then(|(actual, bound)| actual.partial_cmp(&bound))
    {
        return Some(ordering);
    }
    let actual = scalar_for_compare(actual)?;
    let bound = scalar_for_compare(bound)?;
    Some(actual.cmp(&bound))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let pattern = pattern.to_lowercase();
    let value = value.to_lowercase();
    let mut remaining = value.as_str();
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            first = false;
            continue;
        }
        if first && !pattern.starts_with('*') {
            let Some(stripped) = remaining.strip_prefix(part) else {
                return false;
            };
            remaining = stripped;
        } else if let Some(position) = remaining.find(part) {
            remaining = &remaining[position + part.len()..];
        } else {
            return false;
        }
        first = false;
    }
    pattern.ends_with('*') || remaining.is_empty()
}

fn scalar_for_compare(value: &Value) -> Option<String> {
    if let Some(number) = value.as_f64() {
        return Some(format!("{number:020.6}"));
    }
    value.as_str().map(ToString::to_string)
}

fn score(_source: &Value, _query: &Value) -> f64 {
    1.0
}

fn evaluate_aggregations(
    hits: &[MatchedDocument<'_>],
    aggregations: &Value,
) -> Result<Value, String> {
    let object = aggregations
        .as_object()
        .ok_or_else(|| "aggregations must be an object".to_string())?;
    let mut output = serde_json::Map::new();
    for (name, aggregation) in object {
        output.insert(name.clone(), evaluate_aggregation(hits, aggregation)?);
    }
    Ok(Value::Object(output))
}

fn evaluate_aggregation(
    hits: &[MatchedDocument<'_>],
    aggregation: &Value,
) -> Result<Value, String> {
    if let Some(terms) = aggregation.get("terms") {
        return terms_aggregation(hits, terms, sub_aggregations(aggregation));
    }
    if let Some(date_histogram) = aggregation.get("date_histogram") {
        return date_histogram_aggregation(hits, date_histogram, sub_aggregations(aggregation));
    }
    if let Some(histogram) = aggregation.get("histogram") {
        return histogram_aggregation(hits, histogram, sub_aggregations(aggregation));
    }
    if let Some(range) = aggregation.get("range") {
        return range_aggregation(hits, range, sub_aggregations(aggregation));
    }
    if let Some(filters) = aggregation.get("filters") {
        return filters_aggregation(hits, filters, sub_aggregations(aggregation));
    }
    if let Some(missing) = aggregation.get("missing") {
        return missing_aggregation(hits, missing, sub_aggregations(aggregation));
    }
    if let Some(top_hits) = aggregation.get("top_hits") {
        return top_hits_aggregation(hits, top_hits);
    }

    for kind in [
        "min",
        "max",
        "sum",
        "avg",
        "value_count",
        "cardinality",
        "stats",
    ] {
        if let Some(config) = aggregation.get(kind) {
            let field = config
                .get("field")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{kind} aggregation requires field"))?;
            return Ok(metric_aggregation(hits, field, kind));
        }
    }

    Err(format!(
        "unsupported aggregation type [{}]",
        aggregation
            .as_object()
            .and_then(|object| object.keys().next())
            .cloned()
            .unwrap_or_default()
    ))
}

fn terms_aggregation(
    hits: &[MatchedDocument<'_>],
    terms: &Value,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let field = terms
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| "terms aggregation requires field".to_string())?;
    let size = terms
        .get("size")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(10);
    let mut buckets =
        std::collections::BTreeMap::<String, (Value, Vec<MatchedDocument<'_>>)>::new();
    for hit in hits {
        if let Some(value) = value_at(&hit.doc.source, field) {
            for value in aggregation_values(value) {
                let key = aggregation_key(value);
                if !buckets.contains_key(&key) && buckets.len() >= limits::MAX_BUCKETS {
                    return Err(bucket_limit_error());
                }
                let entry = buckets
                    .entry(key)
                    .or_insert_with(|| (value.clone(), Vec::new()));
                entry.1.push(hit.clone());
            }
        }
    }
    let mut buckets = buckets
        .into_values()
        .map(|(key, docs)| bucket_response(key, docs, sub_aggs))
        .collect::<Result<Vec<_>, _>>()?;
    sort_buckets(&mut buckets);
    buckets.truncate(size);
    Ok(json!({
        "doc_count_error_upper_bound": 0,
        "sum_other_doc_count": 0,
        "buckets": buckets
    }))
}

fn date_histogram_aggregation(
    hits: &[MatchedDocument<'_>],
    config: &Value,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let field = config
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| "date_histogram aggregation requires field".to_string())?;
    let interval = config
        .get("calendar_interval")
        .or_else(|| config.get("fixed_interval"))
        .or_else(|| config.get("interval"))
        .and_then(Value::as_str)
        .unwrap_or("day");
    let mut buckets = std::collections::BTreeMap::<i64, (String, Vec<MatchedDocument<'_>>)>::new();
    for hit in hits {
        let Some(value) = value_at(&hit.doc.source, field) else {
            continue;
        };
        for value in aggregation_values(value) {
            let Some(bucket) = date_bucket(value, interval)? else {
                continue;
            };
            let key = bucket.timestamp_millis();
            if !buckets.contains_key(&key) && buckets.len() >= limits::MAX_BUCKETS {
                return Err(bucket_limit_error());
            }
            buckets
                .entry(key)
                .or_insert_with(|| (bucket.to_rfc3339(), Vec::new()))
                .1
                .push(hit.clone());
        }
    }
    let buckets = buckets
        .into_iter()
        .map(|(key, (key_as_string, docs))| {
            let mut bucket = bucket_response(json!(key), docs, sub_aggs)?;
            bucket["key_as_string"] = json!(key_as_string);
            Ok(bucket)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(json!({ "buckets": buckets }))
}

fn histogram_aggregation(
    hits: &[MatchedDocument<'_>],
    config: &Value,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let field = config
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| "histogram aggregation requires field".to_string())?;
    let interval = config
        .get("interval")
        .and_then(Value::as_f64)
        .ok_or_else(|| "histogram aggregation requires numeric interval".to_string())?;
    if interval <= 0.0 {
        return Err("histogram interval must be positive".to_string());
    }
    let mut buckets = std::collections::BTreeMap::<String, (f64, Vec<MatchedDocument<'_>>)>::new();
    for hit in hits {
        if let Some(value) = value_at(&hit.doc.source, field).and_then(Value::as_f64) {
            let key = (value / interval).floor() * interval;
            let bucket_key = format!("{key:020.6}");
            if !buckets.contains_key(&bucket_key) && buckets.len() >= limits::MAX_BUCKETS {
                return Err(bucket_limit_error());
            }
            buckets
                .entry(bucket_key)
                .or_insert_with(|| (key, Vec::new()))
                .1
                .push(hit.clone());
        }
    }
    let buckets = buckets
        .into_values()
        .map(|(key, docs)| bucket_response(json!(key), docs, sub_aggs))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "buckets": buckets }))
}

fn range_aggregation(
    hits: &[MatchedDocument<'_>],
    config: &Value,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let field = config
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| "range aggregation requires field".to_string())?;
    let ranges = config
        .get("ranges")
        .and_then(Value::as_array)
        .ok_or_else(|| "range aggregation requires ranges".to_string())?;
    let mut buckets = Vec::new();
    for range in ranges {
        let from = range.get("from").and_then(Value::as_f64);
        let to = range.get("to").and_then(Value::as_f64);
        let key = range
            .get("key")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| range_key(from, to));
        let docs = hits
            .iter()
            .filter(|hit| {
                value_at(&hit.doc.source, field)
                    .and_then(Value::as_f64)
                    .map(|value| {
                        from.map(|from| value >= from).unwrap_or(true)
                            && to.map(|to| value < to).unwrap_or(true)
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut bucket = bucket_response(json!(key), docs, sub_aggs)?;
        if let Some(from) = from {
            bucket["from"] = json!(from);
        }
        if let Some(to) = to {
            bucket["to"] = json!(to);
        }
        buckets.push(bucket);
    }
    Ok(json!({ "buckets": buckets }))
}

fn filters_aggregation(
    hits: &[MatchedDocument<'_>],
    config: &Value,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let filters = config
        .get("filters")
        .ok_or_else(|| "filters aggregation requires filters".to_string())?;
    if let Some(object) = filters.as_object() {
        ensure_bucket_count(object.len())?;
        let mut buckets = serde_json::Map::new();
        for (key, query) in object {
            let docs = matching_filter_docs(hits, query)?;
            buckets.insert(key.clone(), bucket_response(Value::Null, docs, sub_aggs)?);
        }
        return Ok(json!({ "buckets": buckets }));
    }
    if let Some(array) = filters.as_array() {
        ensure_bucket_count(array.len())?;
        let buckets = array
            .iter()
            .map(|query| {
                let docs = matching_filter_docs(hits, query)?;
                bucket_response(Value::Null, docs, sub_aggs)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(json!({ "buckets": buckets }));
    }
    Err("filters aggregation filters must be an object or array".to_string())
}

fn missing_aggregation(
    hits: &[MatchedDocument<'_>],
    config: &Value,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let field = config
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing aggregation requires field".to_string())?;
    let docs = hits
        .iter()
        .filter(|hit| value_at(&hit.doc.source, field).is_none())
        .cloned()
        .collect::<Vec<_>>();
    bucket_response(Value::Null, docs, sub_aggs)
}

fn top_hits_aggregation(hits: &[MatchedDocument<'_>], config: &Value) -> Result<Value, String> {
    let from = config
        .get("from")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0);
    let size = config
        .get("size")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(3);
    if from.saturating_add(size) > limits::MAX_BUCKETS {
        return Err(format!(
            "top_hits from + size exceeds local limit of {}",
            limits::MAX_BUCKETS
        ));
    }
    let paged = if config.get("sort").is_some() {
        let mut docs = hits.to_vec();
        apply_sort(&mut docs, config.get("sort"));
        docs.iter()
            .skip(from)
            .take(size)
            .map(|hit| top_hit_response(hit, config))
            .collect::<Vec<_>>()
    } else {
        hits.iter()
            .skip(from)
            .take(size)
            .map(|hit| top_hit_response(hit, config))
            .collect::<Vec<_>>()
    };
    Ok(json!({
        "hits": {
            "total": { "value": hits.len(), "relation": "eq" },
            "max_score": if hits.is_empty() { Value::Null } else { json!(1.0) },
            "hits": paged
        }
    }))
}

fn matching_filter_docs<'a>(
    hits: &[MatchedDocument<'a>],
    query: &Value,
) -> Result<Vec<MatchedDocument<'a>>, String> {
    let mut docs = Vec::new();
    for hit in hits {
        if matches_query(hit.doc, query)? {
            docs.push(hit.clone());
        }
    }
    Ok(docs)
}

fn top_hit_response(hit: &MatchedDocument<'_>, config: &Value) -> Value {
    let mut response = json!({
        "_index": hit.index,
        "_id": hit.doc.id,
        "_score": hit.score,
        "_version": hit.doc.version
    });
    if config.get("_source") != Some(&Value::Bool(false)) {
        response["_source"] = filter_source(&hit.doc.source, config.get("_source"));
    }
    response
}

fn ensure_bucket_count(count: usize) -> Result<(), String> {
    if count > limits::MAX_BUCKETS {
        Err(bucket_limit_error())
    } else {
        Ok(())
    }
}

fn bucket_limit_error() -> String {
    format!(
        "aggregation bucket count exceeds local limit of {}",
        limits::MAX_BUCKETS
    )
}

fn metric_aggregation(hits: &[MatchedDocument<'_>], field: &str, kind: &str) -> Value {
    let numeric_values = hits
        .iter()
        .filter_map(|hit| value_at(&hit.doc.source, field))
        .flat_map(numeric_aggregation_values)
        .collect::<Vec<_>>();
    let count = numeric_values.len() as u64;
    let sum = numeric_values.iter().sum::<f64>();
    let min = numeric_values.iter().copied().reduce(f64::min);
    let max = numeric_values.iter().copied().reduce(f64::max);
    match kind {
        "min" => json!({ "value": min }),
        "max" => json!({ "value": max }),
        "sum" => json!({ "value": sum }),
        "avg" => json!({ "value": if count == 0 { None } else { Some(sum / count as f64) } }),
        "value_count" => json!({ "value": value_count(hits, field) }),
        "cardinality" => json!({ "value": cardinality(hits, field) }),
        "stats" => json!({
            "count": count,
            "min": min,
            "max": max,
            "avg": if count == 0 { None } else { Some(sum / count as f64) },
            "sum": sum
        }),
        _ => json!({}),
    }
}

fn bucket_response(
    key: Value,
    docs: Vec<MatchedDocument<'_>>,
    sub_aggs: Option<&Value>,
) -> Result<Value, String> {
    let mut bucket = json!({ "doc_count": docs.len() });
    if !key.is_null() {
        bucket["key"] = key;
    }
    if let Some(sub_aggs) = sub_aggs {
        let sub_aggs = evaluate_aggregations(&docs, sub_aggs)?;
        if let Some(object) = sub_aggs.as_object() {
            for (name, value) in object {
                bucket[name] = value.clone();
            }
        }
    }
    Ok(bucket)
}

fn sub_aggregations(aggregation: &Value) -> Option<&Value> {
    aggregation
        .get("aggregations")
        .or_else(|| aggregation.get("aggs"))
}

fn sort_buckets(buckets: &mut [Value]) {
    buckets.sort_by(|left, right| {
        right["doc_count"]
            .as_u64()
            .cmp(&left["doc_count"].as_u64())
            .then_with(|| left["key"].to_string().cmp(&right["key"].to_string()))
    });
}

fn date_bucket(value: &Value, interval: &str) -> Result<Option<DateTime<Utc>>, String> {
    let Some(value) = value.as_str() else {
        return Ok(None);
    };
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| "date_histogram only supports RFC3339 date strings".to_string())?
        .with_timezone(&Utc);
    let bucket = match interval {
        "day" | "1d" => Utc
            .with_ymd_and_hms(parsed.year(), parsed.month(), parsed.day(), 0, 0, 0)
            .single(),
        "hour" | "1h" => Utc
            .with_ymd_and_hms(
                parsed.year(),
                parsed.month(),
                parsed.day(),
                parsed.hour(),
                0,
                0,
            )
            .single(),
        "month" | "1M" => Utc
            .with_ymd_and_hms(parsed.year(), parsed.month(), 1, 0, 0, 0)
            .single(),
        other => {
            return Err(format!(
                "date_histogram interval [{other}] is not supported by OpenSearch Lite"
            ));
        }
    };
    Ok(bucket)
}

fn range_key(from: Option<f64>, to: Option<f64>) -> String {
    match (from, to) {
        (Some(from), Some(to)) => format!("{from}-{to}"),
        (Some(from), None) => format!("{from}-*"),
        (None, Some(to)) => format!("-{to}"),
        (None, None) => "*".to_string(),
    }
}

fn value_count(hits: &[MatchedDocument<'_>], field: &str) -> u64 {
    hits.iter()
        .filter_map(|hit| value_at(&hit.doc.source, field))
        .map(|value| aggregation_values(value).len() as u64)
        .sum()
}

fn cardinality(hits: &[MatchedDocument<'_>], field: &str) -> u64 {
    hits.iter()
        .filter_map(|hit| value_at(&hit.doc.source, field))
        .flat_map(aggregation_values)
        .map(aggregation_key)
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u64
}

fn aggregation_values(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(values) => values.iter().collect(),
        value => vec![value],
    }
}

fn numeric_aggregation_values(value: &Value) -> Vec<f64> {
    aggregation_values(value)
        .into_iter()
        .filter_map(Value::as_f64)
        .collect()
}

fn aggregation_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        value => value.to_string(),
    }
}

fn apply_sort(hits: &mut [MatchedDocument<'_>], sort: Option<&Value>) {
    let Some(sort) = sort else {
        return;
    };
    let clauses = match sort {
        Value::Array(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let Some(first) = clauses.first() else {
        return;
    };
    let (field, desc) = match first {
        Value::String(field) => (field.as_str(), false),
        Value::Object(object) => {
            let Some((field, config)) = object.iter().next() else {
                return;
            };
            let order = config.get("order").and_then(Value::as_str).unwrap_or("asc");
            (field.as_str(), order == "desc")
        }
        _ => return,
    };
    hits.sort_by(|left, right| {
        let left = value_at(&left.doc.source, field).and_then(scalar_for_compare);
        let right = value_at(&right.doc.source, field).and_then(scalar_for_compare);
        let ordering = left.cmp(&right);
        if desc {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

pub(crate) fn filter_source(source: &Value, source_filter: Option<&Value>) -> Value {
    match source_filter {
        Some(Value::Bool(false)) => Value::Null,
        Some(Value::Bool(true)) | None => source.clone(),
        Some(Value::String(field)) => include_fields(source, std::slice::from_ref(field)),
        Some(Value::Array(fields)) => {
            let fields = fields
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            include_fields(source, &fields)
        }
        Some(Value::Object(config)) => {
            let includes = config
                .get("includes")
                .or_else(|| config.get("include"))
                .map(source_filter_list)
                .unwrap_or_default();
            let excludes = config
                .get("excludes")
                .or_else(|| config.get("exclude"))
                .map(source_filter_list)
                .unwrap_or_default();
            let mut output = if includes.is_empty() {
                source.clone()
            } else {
                include_fields(source, &includes)
            };
            exclude_fields(&mut output, &excludes);
            output
        }
        _ => source.clone(),
    }
}

fn source_filter_list(value: &Value) -> Vec<String> {
    match value {
        Value::String(field) => vec![field.clone()],
        Value::Array(fields) => fields
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn include_fields(source: &Value, fields: &[String]) -> Value {
    let mut output = Value::Object(serde_json::Map::new());
    for field in fields {
        if let Some(value) = value_at(source, field) {
            insert_path(&mut output, field, value.clone());
        }
    }
    output
}

fn insert_path(output: &mut Value, field: &str, value: Value) {
    let mut cursor = output;
    let mut segments = field.split('.').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            if let Some(object) = cursor.as_object_mut() {
                object.insert(segment.to_string(), value);
            }
            return;
        }
        let Some(object) = cursor.as_object_mut() else {
            return;
        };
        cursor = object
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
}

fn exclude_fields(output: &mut Value, excludes: &[String]) {
    for exclude in excludes {
        remove_path(output, exclude);
    }
}

fn remove_path(output: &mut Value, field: &str) {
    let segments = field.split('.').collect::<Vec<_>>();
    if segments.contains(&"*") {
        remove_wildcard_path(output, &segments);
        return;
    }
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    let mut cursor = output;
    for segment in parents {
        let Some(next) = cursor.get_mut(*segment) else {
            return;
        };
        cursor = next;
    }
    if let Some(object) = cursor.as_object_mut() {
        object.remove(*last);
    }
}

fn remove_wildcard_path(output: &mut Value, segments: &[&str]) {
    let Some((segment, rest)) = segments.split_first() else {
        return;
    };
    if rest.is_empty() {
        if let Some(object) = output.as_object_mut() {
            if *segment == "*" {
                object.clear();
            } else {
                object.remove(*segment);
            }
        }
        return;
    }
    let Some(object) = output.as_object_mut() else {
        return;
    };
    if *segment == "*" {
        for value in object.values_mut() {
            remove_wildcard_path(value, rest);
        }
    } else if let Some(value) = object.get_mut(*segment) {
        remove_wildcard_path(value, rest);
    }
}
