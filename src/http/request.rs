use std::collections::BTreeMap;

use bytes::Bytes;
use http::{HeaderMap, Method, Uri};
use url::form_urlencoded;

use crate::security::SecurityContext;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: BTreeMap<String, String>,
    pub body: Bytes,
    pub security: SecurityContext,
}

impl Request {
    pub fn from_parts(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Self {
        Self::from_parts_with_security(
            method,
            uri,
            headers,
            body,
            SecurityContext::disabled_loopback(),
        )
    }

    pub fn from_parts_with_security(
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
        security: SecurityContext,
    ) -> Self {
        let query = uri.query().map(parse_query).unwrap_or_default();
        let headers = headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect();

        Self {
            method,
            path: uri.path().to_string(),
            query,
            headers,
            body,
            security,
        }
    }

    pub fn body_json(&self) -> Result<serde_json::Value, String> {
        if self.body.is_empty() {
            return Ok(serde_json::Value::Object(Default::default()));
        }
        serde_json::from_slice(&self.body).map_err(|error| format!("malformed JSON body: {error}"))
    }

    pub fn query_value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http::{HeaderMap, Method, Uri};

    use super::Request;

    #[test]
    fn query_parser_preserves_duplicates_and_utf8() {
        let request = Request::from_parts(
            Method::GET,
            "/_search?index=one&index=two&q=%C3%A9"
                .parse::<Uri>()
                .unwrap(),
            HeaderMap::new(),
            Bytes::new(),
        );

        assert_eq!(
            request.query,
            vec![
                ("index".to_string(), "one".to_string()),
                ("index".to_string(), "two".to_string()),
                ("q".to_string(), "é".to_string()),
            ]
        );
    }
}
