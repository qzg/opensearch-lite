use std::{collections::BTreeSet, path::Path};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Uri};
use opensearch_lite::{http::request::Request, http::router, responses::Response};
use serde::Deserialize;
use serde_json::{json, Value};
use url::form_urlencoded;

use crate::support::ephemeral_state;

pub async fn run_selected_yaml_tests(path: impl AsRef<Path>, selected_tests: &[&str]) {
    let path = path.as_ref();
    let fixture = Fixture::load(path);
    let selected = selected_tests.iter().copied().collect::<BTreeSet<_>>();

    for test in fixture
        .tests
        .iter()
        .filter(|test| selected.is_empty() || selected.contains(test.name.as_str()))
    {
        let mut runner = Runner::new(path, &test.name);
        runner.run_steps("setup", &fixture.setup).await;
        runner.run_steps("test", &test.steps).await;
        runner.run_steps("teardown", &fixture.teardown).await;
    }
}

#[derive(Debug)]
struct Fixture {
    setup: Vec<YamlValue>,
    teardown: Vec<YamlValue>,
    tests: Vec<YamlTest>,
}

impl Fixture {
    fn load(path: &Path) -> Self {
        let contents = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let mut fixture = Self {
            setup: Vec::new(),
            teardown: Vec::new(),
            tests: Vec::new(),
        };

        for document in serde_yaml::Deserializer::from_str(&contents) {
            let document = YamlValue::deserialize(document)
                .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
            let Some(entries) = document.as_mapping() else {
                continue;
            };
            for (key, value) in entries {
                let key = yaml_string(key).unwrap_or_else(|| {
                    panic!("{} contains a non-string top-level key", path.display())
                });
                let steps = yaml_sequence(value).unwrap_or_else(|| {
                    panic!(
                        "{} top-level entry {key:?} should contain a step sequence",
                        path.display()
                    )
                });
                match key.as_str() {
                    "setup" => fixture.setup.extend(steps),
                    "teardown" => fixture.teardown.extend(steps),
                    name => fixture.tests.push(YamlTest {
                        name: name.to_string(),
                        steps,
                    }),
                }
            }
        }

        fixture
    }
}

#[derive(Debug)]
struct YamlTest {
    name: String,
    steps: Vec<YamlValue>,
}

struct Runner<'a> {
    fixture_path: &'a Path,
    test_name: &'a str,
    state: opensearch_lite::server::AppState,
    last_response: Option<Response>,
    skip_reason: Option<String>,
}

impl<'a> Runner<'a> {
    fn new(fixture_path: &'a Path, test_name: &'a str) -> Self {
        Self {
            fixture_path,
            test_name,
            state: ephemeral_state(),
            last_response: None,
            skip_reason: None,
        }
    }

    async fn run_steps(&mut self, phase: &str, steps: &[YamlValue]) {
        for (index, step) in steps.iter().enumerate() {
            if self.skip_reason.is_some() {
                break;
            }
            self.run_step(phase, index, step).await;
        }
    }

    async fn run_step(&mut self, phase: &str, index: usize, step: &YamlValue) {
        let action = match single_entry(step) {
            Some(action) => action,
            None => self.fail(format!("{phase} step {index} should contain one action")),
        };
        match action.0.as_str() {
            "skip" => {
                if let Some(reason) = skip_reason(action.1) {
                    self.skip_reason = Some(reason);
                }
            }
            "do" => {
                let request = RestCall::from_do(action.1)
                    .unwrap_or_else(|error| self.fail(format!("{phase} step {index}: {error}")));
                let response = request.execute(&self.state).await;
                assert_response_status(&request, &response)
                    .unwrap_or_else(|error| self.fail(format!("{phase} step {index}: {error}")));
                self.last_response = Some(response);
            }
            "match" => {
                let (path, expected) = assertion_entry(action.1).unwrap_or_else(|| {
                    self.fail(format!("{phase} step {index} match assertion is malformed"))
                });
                let actual = if path == "status" {
                    self.last_response
                        .as_ref()
                        .map(|response| json!(response.status))
                } else {
                    self.response_path(&path).cloned()
                };
                let expected = yaml_to_json(&expected);
                if actual.as_ref() != Some(&expected) {
                    self.fail(format!(
                        "{phase} step {index}: expected {path} to be {expected}, got {:?}",
                        actual
                    ));
                }
            }
            "length" => {
                let (path, expected) = assertion_entry(action.1).unwrap_or_else(|| {
                    self.fail(format!(
                        "{phase} step {index} length assertion is malformed"
                    ))
                });
                let expected = expected.as_u64().unwrap_or_else(|| {
                    self.fail(format!(
                        "{phase} step {index}: length assertion expected value must be a number"
                    ))
                }) as usize;
                let actual = self
                    .response_path(&path)
                    .and_then(json_len)
                    .unwrap_or_else(|| {
                        self.fail(format!(
                            "{phase} step {index}: {path} has no measurable length"
                        ))
                    });
                if actual != expected {
                    self.fail(format!(
                        "{phase} step {index}: expected {path} length {expected}, got {actual}"
                    ));
                }
            }
            "is_true" => {
                let path = yaml_string(action.1).unwrap_or_else(|| {
                    self.fail(format!("{phase} step {index} is_true path is malformed"))
                });
                if !self.is_truthy_path(&path) {
                    self.fail(format!(
                        "{phase} step {index}: expected {path} to be present/truthy"
                    ));
                }
            }
            "is_false" => {
                let path = yaml_string(action.1).unwrap_or_else(|| {
                    self.fail(format!("{phase} step {index} is_false path is malformed"))
                });
                if self.is_truthy_path(&path) {
                    self.fail(format!(
                        "{phase} step {index}: expected {path} to be absent/falsey"
                    ));
                }
            }
            other => self.fail(format!(
                "{phase} step {index}: unsupported YAML action {other}"
            )),
        }
    }

    fn response_path(&self, path: &str) -> Option<&Value> {
        let response = self
            .last_response
            .as_ref()
            .unwrap_or_else(|| self.fail("assertion ran before any do step".to_string()));
        response
            .body
            .as_ref()
            .and_then(|body| value_path(body, path))
    }

    fn is_truthy_path(&self, path: &str) -> bool {
        if path.is_empty() {
            return self
                .last_response
                .as_ref()
                .map(|response| response.status < 400)
                .unwrap_or(false);
        }
        is_truthy(self.response_path(path))
    }

    fn fail(&self, message: String) -> ! {
        panic!(
            "{} [{}]: {message}",
            self.fixture_path.display(),
            self.test_name
        )
    }
}

struct RestCall {
    api: String,
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    body: Body,
    catch: Option<String>,
    ignore: BTreeSet<u16>,
}

impl RestCall {
    fn from_do(value: &YamlValue) -> Result<Self, String> {
        let entries = value
            .as_mapping()
            .ok_or_else(|| "do step should be a mapping".to_string())?;
        let catch = entries
            .get(YamlValue::String("catch".to_string()))
            .and_then(yaml_string);
        let (api, params) = entries
            .iter()
            .find_map(|(key, value)| {
                let key = yaml_string(key)?;
                (!matches!(
                    key.as_str(),
                    "catch" | "headers" | "warnings" | "allowed_warnings"
                ))
                .then_some((key, value))
            })
            .ok_or_else(|| "do step should contain an API call".to_string())?;
        let params = params.as_mapping().cloned().unwrap_or_else(YamlMap::new);
        let ignore = ignore_statuses(&params);

        let mut call = match api.as_str() {
            "bulk" => Self::bulk(&api, &params),
            "cat.plugins" => Self::cat_plugins(&api, &params),
            "cat.templates" => Self::cat_templates(&api, &params),
            "create" => Self::create(&api, &params),
            "delete" => Self::delete(&api, &params),
            "cluster.health" => Self::cluster_health(&api, &params),
            "cluster.stats" => Self::cluster_stats(&api, &params),
            "count" => Self::count(&api, &params),
            "field_caps" => Self::field_caps(&api, &params),
            "get" => Self::get(&api, &params),
            "get_source" => Self::get_source(&api, &params),
            "index" => Self::index(&api, &params),
            "indices.create" => Self::indices_create(&api, &params),
            "indices.delete_index_template" => Self::indices_delete_index_template(&api, &params),
            "indices.delete" => Self::indices_delete(&api, &params),
            "indices.exists" => Self::indices_exists(&api, &params),
            "indices.exists_alias" => Self::indices_exists_alias(&api, &params),
            "indices.get" => Self::indices_get(&api, &params),
            "indices.get_alias" => Self::indices_get_alias(&api, &params),
            "indices.get_field_mapping" => Self::indices_get_field_mapping(&api, &params),
            "indices.get_index_template" => Self::indices_get_index_template(&api, &params),
            "indices.put_alias" => Self::indices_put_alias(&api, &params),
            "indices.put_index_template" => Self::indices_put_index_template(&api, &params),
            "indices.refresh" => Self::indices_refresh(&api, &params),
            "indices.stats" => Self::indices_stats(&api, &params),
            "indices.update_aliases" => Self::indices_update_aliases(&api, &params),
            "mget" => Self::mget(&api, &params),
            "search" => Self::search(&api, &params),
            "update" => Self::update(&api, &params),
            other => Err(format!("unsupported YAML API [{other}]")),
        }?;
        call.catch = catch;
        call.ignore = ignore;
        Ok(call)
    }

    async fn execute(&self, state: &opensearch_lite::server::AppState) -> Response {
        let mut headers = HeaderMap::new();
        let body = match &self.body {
            Body::Empty => Bytes::new(),
            Body::Json(value) => {
                headers.insert("content-type", HeaderValue::from_static("application/json"));
                Bytes::from(serde_json::to_vec(value).expect("JSON request body serializes"))
            }
            Body::Ndjson(value) => {
                headers.insert(
                    "content-type",
                    HeaderValue::from_static("application/x-ndjson"),
                );
                Bytes::from(value.clone())
            }
        };
        let uri = uri_with_query(&self.path, &self.query);
        let request = Request::from_parts(self.method.clone(), uri, headers, body);
        router::handle(state.clone(), request).await
    }

    fn bulk(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        Ok(Self::new(
            api,
            Method::POST,
            index_path(index.as_deref(), "_bulk"),
            query_params(params, &["refresh", "require_alias"]),
            Body::Ndjson(required_ndjson(params, "body")?),
        ))
    }

    fn cat_plugins(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::GET,
            "/_cat/plugins".to_string(),
            query_params(params, &["format", "h", "s", "v"]),
            Body::Empty,
        ))
    }

    fn cat_templates(api: &str, params: &YamlMap) -> Result<Self, String> {
        let name = path_list(params.get(yaml_key("name")));
        Ok(Self::new(
            api,
            Method::GET,
            match name {
                Some(name) => format!("/_cat/templates/{name}"),
                None => "/_cat/templates".to_string(),
            },
            query_params(params, &["format", "h", "s", "v"]),
            Body::Empty,
        ))
    }

    fn create(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::PUT,
            format!(
                "/{}/_create/{}",
                required_path(params, "index")?,
                required_path(params, "id")?
            ),
            query_params(params, &["refresh", "routing"]),
            Body::Json(required_json(params, "body")?),
        ))
    }

    fn delete(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::DELETE,
            format!(
                "/{}/_doc/{}",
                required_path(params, "index")?,
                required_path(params, "id")?
            ),
            query_params(params, &["refresh", "routing"]),
            Body::Empty,
        ))
    }

    fn cluster_health(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::GET,
            "/_cluster/health".to_string(),
            query_params(params, &["wait_for_status"]),
            Body::Empty,
        ))
    }

    fn cluster_stats(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::GET,
            "/_cluster/stats".to_string(),
            query_params(params, &["timeout"]),
            Body::Empty,
        ))
    }

    fn count(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        Ok(Self::new(
            api,
            Method::POST,
            index_path(index.as_deref(), "_count"),
            Vec::new(),
            json_body(params),
        ))
    }

    fn field_caps(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        Ok(Self::new(
            api,
            Method::POST,
            index_path(index.as_deref(), "_field_caps"),
            query_params(
                params,
                &[
                    "fields",
                    "ignore_unavailable",
                    "allow_no_indices",
                    "include_unmapped",
                ],
            ),
            json_body(params),
        ))
    }

    fn get(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::GET,
            format!(
                "/{}/_doc/{}",
                required_path(params, "index")?,
                required_path(params, "id")?
            ),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn get_source(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::GET,
            format!(
                "/{}/_source/{}",
                required_path(params, "index")?,
                required_path(params, "id")?
            ),
            query_params(params, &["_source", "_source_includes", "_source_excludes"]),
            Body::Empty,
        ))
    }

    fn index(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = required_path(params, "index")?;
        let path = if let Some(id) = params.get(yaml_key("id")).and_then(scalar_string) {
            format!("/{index}/_doc/{id}")
        } else {
            format!("/{index}/_doc")
        };
        Ok(Self::new(
            api,
            Method::PUT,
            path,
            query_params(params, &["refresh", "routing"]),
            Body::Json(required_json(params, "body")?),
        ))
    }

    fn indices_create(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::PUT,
            format!("/{}", required_path(params, "index")?),
            Vec::new(),
            json_body(params),
        ))
    }

    fn indices_delete(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::DELETE,
            format!("/{}", required_path(params, "index")?),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn indices_exists(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::HEAD,
            format!("/{}", required_path(params, "index")?),
            query_params(params, &["local", "ignore_unavailable", "allow_no_indices"]),
            Body::Empty,
        ))
    }

    fn indices_delete_index_template(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::DELETE,
            format!("/_index_template/{}", required_path(params, "name")?),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn indices_exists_alias(api: &str, params: &YamlMap) -> Result<Self, String> {
        let name = required_path(params, "name")?;
        let index = path_list(params.get(yaml_key("index")));
        Ok(Self::new(
            api,
            Method::HEAD,
            index_path(index.as_deref(), &format!("_alias/{name}")),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn indices_get(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::GET,
            format!("/{}", required_path(params, "index")?),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn indices_get_alias(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        let name = path_list(params.get(yaml_key("name")));
        let suffix = match name {
            Some(name) => format!("_alias/{name}"),
            None => "_alias".to_string(),
        };
        Ok(Self::new(
            api,
            Method::GET,
            index_path(index.as_deref(), &suffix),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn indices_get_field_mapping(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        let fields = path_list(params.get(yaml_key("fields")))
            .ok_or_else(|| "indices.get_field_mapping requires [fields]".to_string())?;
        Ok(Self::new(
            api,
            Method::GET,
            index_path(index.as_deref(), &format!("_mapping/field/{fields}")),
            query_params(params, &["include_defaults"]),
            Body::Empty,
        ))
    }

    fn indices_get_index_template(api: &str, params: &YamlMap) -> Result<Self, String> {
        let name = path_list(params.get(yaml_key("name")));
        Ok(Self::new(
            api,
            Method::GET,
            match name {
                Some(name) => format!("/_index_template/{name}"),
                None => "/_index_template".to_string(),
            },
            query_params(params, &["local"]),
            Body::Empty,
        ))
    }

    fn indices_put_alias(api: &str, params: &YamlMap) -> Result<Self, String> {
        let body = params
            .get(yaml_key("body"))
            .map(yaml_to_json)
            .unwrap_or_else(|| json!({}));
        let index = body
            .get("index")
            .and_then(json_scalar_string)
            .or_else(|| path_list(params.get(yaml_key("index"))))
            .ok_or_else(|| "indices.put_alias requires [index]".to_string())?;
        let name = body
            .get("alias")
            .and_then(json_scalar_string)
            .or_else(|| path_list(params.get(yaml_key("name"))))
            .ok_or_else(|| "indices.put_alias requires [name]".to_string())?;
        Ok(Self::new(
            api,
            Method::PUT,
            format!("/{index}/_alias/{name}"),
            Vec::new(),
            Body::Json(body),
        ))
    }

    fn indices_put_index_template(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::PUT,
            format!("/_index_template/{}", required_path(params, "name")?),
            query_params(params, &["create"]),
            Body::Json(required_json(params, "body")?),
        ))
    }

    fn indices_refresh(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        let path = match index.as_deref() {
            None | Some("_all") => "/_refresh".to_string(),
            Some(index) => format!("/{index}/_refresh"),
        };
        Ok(Self::new(api, Method::POST, path, Vec::new(), Body::Empty))
    }

    fn indices_stats(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        let metric = path_list(params.get(yaml_key("metric")));
        let suffix = match metric {
            Some(metric) => format!("_stats/{metric}"),
            None => "_stats".to_string(),
        };
        Ok(Self::new(
            api,
            Method::GET,
            index_path(index.as_deref(), &suffix),
            Vec::new(),
            Body::Empty,
        ))
    }

    fn indices_update_aliases(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::POST,
            "/_aliases".to_string(),
            Vec::new(),
            Body::Json(required_json(params, "body")?),
        ))
    }

    fn mget(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        Ok(Self::new(
            api,
            Method::POST,
            index_path(index.as_deref(), "_mget"),
            query_params(params, &["_source", "_source_includes", "_source_excludes"]),
            Body::Json(required_json(params, "body")?),
        ))
    }

    fn search(api: &str, params: &YamlMap) -> Result<Self, String> {
        let index = path_list(params.get(yaml_key("index")));
        Ok(Self::new(
            api,
            Method::POST,
            index_path(index.as_deref(), "_search"),
            query_params(
                params,
                &[
                    "rest_total_hits_as_int",
                    "_source",
                    "_source_includes",
                    "_source_excludes",
                    "from",
                    "size",
                    "track_total_hits",
                ],
            ),
            json_body(params),
        ))
    }

    fn update(api: &str, params: &YamlMap) -> Result<Self, String> {
        Ok(Self::new(
            api,
            Method::POST,
            format!(
                "/{}/_update/{}",
                required_path(params, "index")?,
                required_path(params, "id")?
            ),
            query_params(params, &["_source", "refresh"]),
            Body::Json(required_json(params, "body")?),
        ))
    }

    fn new(
        api: &str,
        method: Method,
        path: String,
        query: Vec<(String, String)>,
        body: Body,
    ) -> Self {
        Self {
            api: api.to_string(),
            method,
            path,
            query,
            body,
            catch: None,
            ignore: BTreeSet::new(),
        }
    }
}

enum Body {
    Empty,
    Json(Value),
    Ndjson(String),
}

fn assert_response_status(call: &RestCall, response: &Response) -> Result<(), String> {
    if let Some(catch) = call.catch.as_deref() {
        let expected = catch_status(catch)?;
        if response.status != expected {
            return Err(format!(
                "{} expected catch [{catch}] status {expected}, got {} with body {:?}",
                call.api, response.status, response.body
            ));
        }
        return Ok(());
    }

    if call.method == Method::HEAD && response.status == 404 {
        return Ok(());
    }

    if response.status >= 400 && !call.ignore.contains(&response.status) {
        return Err(format!(
            "{} {} {} failed with status {} and body {:?}",
            call.method, call.path, call.api, response.status, response.body
        ));
    }
    Ok(())
}

fn catch_status(catch: &str) -> Result<u16, String> {
    match catch {
        "bad_request" | "request" => Ok(400),
        "conflict" => Ok(409),
        "missing" => Ok(404),
        "param" => Ok(400),
        catch if catch.starts_with('/') => Ok(400),
        other => Err(format!("unsupported catch type [{other}]")),
    }
}

type YamlValue = serde_yaml::Value;
type YamlMap = serde_yaml::Mapping;

fn single_entry(value: &YamlValue) -> Option<(String, &YamlValue)> {
    let mapping = value.as_mapping()?;
    if mapping.len() != 1 {
        return None;
    }
    let (key, value) = mapping.iter().next()?;
    Some((yaml_string(key)?, value))
}

fn assertion_entry(value: &YamlValue) -> Option<(String, YamlValue)> {
    let mapping = value.as_mapping()?;
    if mapping.len() != 1 {
        return None;
    }
    let (key, value) = mapping.iter().next()?;
    Some((yaml_string(key)?, value.clone()))
}

fn yaml_string(value: &YamlValue) -> Option<String> {
    scalar_string(value)
}

fn yaml_sequence(value: &YamlValue) -> Option<Vec<YamlValue>> {
    value.as_sequence().cloned()
}

fn yaml_key(key: &str) -> YamlValue {
    YamlValue::String(key.to_string())
}

fn skip_reason(value: &YamlValue) -> Option<String> {
    let skip = value.as_mapping()?;
    if let Some(version) = skip.get(yaml_key("version")).and_then(scalar_string) {
        if version_matches_current(&version) {
            return Some(format!("version range {version} matches local runner"));
        }
    }
    let unsupported_features = skip
        .get(yaml_key("features"))
        .into_iter()
        .flat_map(feature_values)
        .filter(|feature| !supported_feature(feature))
        .collect::<Vec<_>>();
    if unsupported_features.is_empty() {
        None
    } else {
        Some(format!(
            "unsupported YAML runner features: {}",
            unsupported_features.join(", ")
        ))
    }
}

fn feature_values(value: &YamlValue) -> Vec<String> {
    match value {
        YamlValue::Sequence(values) => values.iter().filter_map(scalar_string).collect(),
        value => scalar_string(value).into_iter().collect(),
    }
}

fn supported_feature(feature: &str) -> bool {
    matches!(feature, "allowed_warnings" | "headers" | "warnings")
}

fn version_matches_current(range: &str) -> bool {
    let current = (3_u64, 6_u64, 0_u64);
    let Some((low, high)) = range.split_once('-') else {
        return parse_version(range.trim()) == Some(current);
    };
    let low = parse_version(low.trim());
    let high = parse_version(high.trim());
    low.map(|low| current >= low).unwrap_or(true)
        && high.map(|high| current <= high).unwrap_or(true)
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    if value.is_empty() {
        return None;
    }
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn scalar_string(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.clone()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Bool(value) => Some(value.to_string()),
        YamlValue::Null => None,
        _ => None,
    }
}

fn yaml_to_json(value: &YamlValue) -> Value {
    serde_json::to_value(value).expect("YAML value converts to JSON")
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn required_json(params: &YamlMap, key: &str) -> Result<Value, String> {
    params
        .get(yaml_key(key))
        .map(yaml_to_json)
        .ok_or_else(|| format!("required parameter [{key}] is missing"))
}

fn required_ndjson(params: &YamlMap, key: &str) -> Result<String, String> {
    let value = params
        .get(yaml_key(key))
        .ok_or_else(|| format!("required NDJSON parameter [{key}] is missing"))?;
    if let Some(value) = scalar_string(value) {
        return Ok(value);
    }
    let values = value
        .as_sequence()
        .ok_or_else(|| format!("NDJSON parameter [{key}] should be a string or sequence"))?;
    let mut output = String::new();
    for value in values {
        if let Some(line) = scalar_string(value) {
            output.push_str(&line);
        } else {
            output.push_str(
                &serde_json::to_string(&yaml_to_json(value))
                    .expect("YAML NDJSON item should serialize"),
            );
        }
        output.push('\n');
    }
    Ok(output)
}

fn required_path(params: &YamlMap, key: &str) -> Result<String, String> {
    params
        .get(yaml_key(key))
        .and_then(|value| path_list(Some(value)))
        .ok_or_else(|| format!("required path parameter [{key}] is missing"))
}

fn json_body(params: &YamlMap) -> Body {
    params
        .get(yaml_key("body"))
        .map(|value| Body::Json(yaml_to_json(value)))
        .unwrap_or(Body::Empty)
}

fn path_list(value: Option<&YamlValue>) -> Option<String> {
    match value? {
        YamlValue::Sequence(values) => {
            if values.is_empty() {
                return None;
            }
            Some(
                values
                    .iter()
                    .filter_map(scalar_string)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
        value => scalar_string(value),
    }
}

fn index_path(index: Option<&str>, suffix: &str) -> String {
    match index {
        Some(index) if !index.is_empty() => format!("/{index}/{suffix}"),
        _ => format!("/{suffix}"),
    }
}

fn query_params(params: &YamlMap, keys: &[&str]) -> Vec<(String, String)> {
    let mut query = Vec::new();
    for key in keys {
        let Some(value) = params.get(yaml_key(key)) else {
            continue;
        };
        if let Some(values) = value.as_sequence() {
            query.push((
                key.to_string(),
                values
                    .iter()
                    .filter_map(scalar_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        } else if let Some(value) = scalar_string(value) {
            query.push((key.to_string(), value));
        }
    }
    query
}

fn ignore_statuses(params: &YamlMap) -> BTreeSet<u16> {
    let Some(value) = params.get(yaml_key("ignore")) else {
        return BTreeSet::new();
    };
    match value {
        YamlValue::Sequence(values) => values
            .iter()
            .filter_map(scalar_string)
            .filter_map(|value| value.parse::<u16>().ok())
            .collect(),
        value => scalar_string(value)
            .and_then(|value| value.parse::<u16>().ok())
            .into_iter()
            .collect(),
    }
}

fn uri_with_query(path: &str, query: &[(String, String)]) -> Uri {
    if query.is_empty() {
        return path.parse().expect("test path should parse as URI");
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in query {
        serializer.append_pair(key, value);
    }
    format!("{path}?{}", serializer.finish())
        .parse()
        .expect("test path with query should parse as URI")
}

fn value_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in split_path(path) {
        current = match current {
            Value::Array(values) => values.get(segment.parse::<usize>().ok()?),
            Value::Object(object) => object.get(&segment),
            _ => return None,
        }?;
    }
    Some(current)
}

fn split_path(path: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in path.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '.' {
            segments.push(current);
            current = String::new();
        } else {
            current.push(character);
        }
    }
    segments.push(current);
    segments
}

fn json_len(value: &Value) -> Option<usize> {
    match value {
        Value::Array(values) => Some(values.len()),
        Value::Object(values) => Some(values.len()),
        Value::String(value) => Some(value.len()),
        _ => None,
    }
}

fn is_truthy(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null) | Some(Value::Bool(false)))
}
