use std::cmp::Ordering;

use serde_json::{json, Value};

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

    for index_name in expand_indices(db, &request.indices) {
        let Some(index) = db.indexes.get(&index_name) else {
            continue;
        };
        for doc in index.documents.values() {
            if matches_query(doc, &query)? {
                total += 1;
                if sorted || (total > request.from && hits.len() < request.size) {
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
    let offset = if sorted { request.from } else { 0 };
    let paged = hits
        .iter()
        .skip(offset)
        .take(request.size)
        .map(|hit| {
            json!({
                "_index": hit.index,
                "_id": hit.doc.id,
                "_score": hit.score,
                "_version": hit.doc.version,
                "_seq_no": hit.doc.seq_no,
                "_primary_term": hit.doc.primary_term,
                "_source": filter_source(&hit.doc.source, request.body.get("_source"))
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "took": 0,
        "timed_out": false,
        "_shards": { "total": 1, "successful": 1, "skipped": 0, "failed": 0 },
        "hits": {
            "total": { "value": total, "relation": "eq" },
            "max_score": if total == 0 { Value::Null } else { json!(1.0) },
            "hits": paged
        }
    }))
}

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
    let source = &doc.source;
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
        return Ok(values.iter().any(|value| value.as_str() == Some(&doc.id)));
    }
    if let Some(term) = object.get("term") {
        let (field, expected) = single_field(term, "term")?;
        let expected = expected.get("value").unwrap_or(expected);
        return Ok(value_at(source, field)
            .map(|actual| values_equal(actual, expected))
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
        return matches_bool(doc, bool_query);
    }
    Err(format!(
        "unsupported query type [{}]",
        object.keys().next().cloned().unwrap_or_default()
    ))
}

fn matches_bool(doc: &StoredDocument, bool_query: &Value) -> Result<bool, String> {
    let must = clauses(bool_query.get("must"));
    let filter = clauses(bool_query.get("filter"));
    let should = clauses(bool_query.get("should"));
    let must_not = clauses(bool_query.get("must_not"));

    for clause in must.iter().chain(filter.iter()) {
        if !matches_query(doc, clause)? {
            return Ok(false);
        }
    }
    for clause in &must_not {
        if matches_query(doc, clause)? {
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
        .filter(|clause| matches_query(doc, clause).unwrap_or(false))
        .count();
    if should_matches < minimum_should_match {
        return Ok(false);
    }
    Ok(true)
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

fn filter_source(source: &Value, source_filter: Option<&Value>) -> Value {
    match source_filter {
        Some(Value::Bool(false)) => Value::Null,
        Some(Value::Array(fields)) => {
            let mut output = serde_json::Map::new();
            for field in fields.iter().filter_map(Value::as_str) {
                if let Some(value) = value_at(source, field) {
                    output.insert(field.to_string(), value.clone());
                }
            }
            Value::Object(output)
        }
        Some(Value::Object(config)) => {
            if let Some(includes) = config.get("includes").and_then(Value::as_array) {
                let mut output = serde_json::Map::new();
                for field in includes.iter().filter_map(Value::as_str) {
                    if let Some(value) = value_at(source, field) {
                        output.insert(field.to_string(), value.clone());
                    }
                }
                Value::Object(output)
            } else {
                source.clone()
            }
        }
        _ => source.clone(),
    }
}
