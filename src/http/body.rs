use bytes::Bytes;

use crate::responses::{open_search_error, Response};

pub fn ensure_body_limit(body: &Bytes, limit: usize) -> Result<(), Response> {
    if body.len() > limit {
        return Err(open_search_error(
            413,
            "content_too_long_exception",
            format!(
                "request body is {} bytes, which exceeds the configured limit of {} bytes",
                body.len(),
                limit
            ),
            Some("Reduce the request size or raise --max-body-size for this local run."),
        ));
    }
    Ok(())
}
