use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde_json::{json, Value};

const MAX_SCROLL_CONTEXTS: usize = 32;
const SCROLL_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    inner: Arc<Mutex<RuntimeInner>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct RuntimeInner {
    scrolls: BTreeMap<String, ScrollCursor>,
    tasks: BTreeMap<String, TaskRecord>,
}

#[derive(Debug, Clone)]
struct ScrollCursor {
    hits: Vec<Value>,
    position: usize,
    batch_size: usize,
    total: Value,
    max_score: Value,
    last_accessed: Instant,
    bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ScrollPage {
    pub scroll_id: String,
    pub hits: Vec<Value>,
    pub total: Value,
    pub max_score: Value,
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: String,
    pub action: String,
    pub response: Value,
}

#[derive(Debug, Clone)]
pub struct RuntimeError {
    pub status: u16,
    pub error_type: &'static str,
    pub reason: String,
}

impl RuntimeState {
    pub fn create_scroll(
        &self,
        hits: Vec<Value>,
        total: Value,
        max_score: Value,
        batch_size: usize,
        max_bytes: usize,
    ) -> Result<ScrollPage, RuntimeError> {
        let scroll_id = self.next_runtime_id("scroll");
        let batch_size = batch_size.max(1);
        let (page_hits, position) = page_hits(&hits, 0, batch_size);
        let remaining_hits = if position < hits.len() {
            hits[position..].to_vec()
        } else {
            Vec::new()
        };
        if remaining_hits.is_empty() {
            return Ok(ScrollPage {
                scroll_id,
                hits: page_hits,
                total,
                max_score,
            });
        }
        let bytes = estimate_scroll_bytes(&remaining_hits, &total, &max_score);
        if bytes > max_bytes {
            return Err(RuntimeError::new(
                429,
                "resource_limit_exception",
                format!(
                    "scroll context would retain {bytes} bytes, exceeding the local scroll budget"
                ),
            ));
        }
        let cursor = ScrollCursor {
            hits: remaining_hits,
            position: 0,
            batch_size,
            total: total.clone(),
            max_score: max_score.clone(),
            last_accessed: Instant::now(),
            bytes,
        };
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_scrolls(&mut inner);
        evict_scrolls_for_budget(&mut inner, bytes, max_bytes);
        if inner.scrolls.len() >= MAX_SCROLL_CONTEXTS
            || total_scroll_bytes(&inner).saturating_add(bytes) > max_bytes
        {
            return Err(RuntimeError::new(
                429,
                "resource_limit_exception",
                "maximum local scroll contexts reached",
            ));
        }
        if !cursor.hits.is_empty() {
            inner.scrolls.insert(scroll_id.clone(), cursor);
        }
        Ok(ScrollPage {
            scroll_id,
            hits: page_hits,
            total,
            max_score,
        })
    }

    pub fn next_scroll(&self, scroll_id: &str) -> Option<ScrollPage> {
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_scrolls(&mut inner);
        let (page, exhausted) = {
            let cursor = inner.scrolls.get_mut(scroll_id)?;
            let (hits, position) = page_hits(&cursor.hits, cursor.position, cursor.batch_size);
            cursor.position = position;
            cursor.last_accessed = Instant::now();
            let exhausted = position >= cursor.hits.len();
            (
                ScrollPage {
                    scroll_id: scroll_id.to_string(),
                    hits,
                    total: cursor.total.clone(),
                    max_score: cursor.max_score.clone(),
                },
                exhausted,
            )
        };
        if exhausted {
            inner.scrolls.remove(scroll_id);
        }
        Some(page)
    }

    pub fn clear_scrolls(&self, scroll_ids: &[String]) -> usize {
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_scrolls(&mut inner);
        scroll_ids
            .iter()
            .filter(|scroll_id| inner.scrolls.remove(*scroll_id).is_some())
            .count()
    }

    pub fn record_completed_task(&self, action: &str, response: Value) -> String {
        let id = self.next_runtime_id("task");
        let record = TaskRecord {
            id: id.clone(),
            action: action.to_string(),
            response,
        };
        self.inner
            .lock()
            .expect("runtime lock is not poisoned")
            .tasks
            .insert(id.clone(), record);
        id
    }

    pub fn task(&self, task_id: &str) -> Option<TaskRecord> {
        self.inner
            .lock()
            .expect("runtime lock is not poisoned")
            .tasks
            .get(task_id)
            .cloned()
    }

    fn next_runtime_id(&self, prefix: &str) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        format!("opensearch-lite-{prefix}:{id}")
    }
}

impl RuntimeError {
    fn new(status: u16, error_type: &'static str, reason: impl Into<String>) -> Self {
        Self {
            status,
            error_type,
            reason: reason.into(),
        }
    }
}

impl TaskRecord {
    pub fn response_body(&self) -> Value {
        let numeric_id = self
            .id
            .rsplit(':')
            .next()
            .and_then(|id| id.parse::<u64>().ok())
            .unwrap_or(0);
        json!({
            "completed": true,
            "task": {
                "node": "opensearch-lite",
                "id": numeric_id,
                "type": "transport",
                "action": self.action,
                "status": {
                    "total": self.response.get("total").and_then(Value::as_u64).unwrap_or(0),
                    "updated": self.response.get("updated").and_then(Value::as_u64).unwrap_or(0),
                    "created": self.response.get("created").and_then(Value::as_u64).unwrap_or(0),
                    "deleted": self.response.get("deleted").and_then(Value::as_u64).unwrap_or(0)
                }
            },
            "response": self.response
        })
    }
}

fn purge_expired_scrolls(inner: &mut RuntimeInner) {
    let now = Instant::now();
    inner
        .scrolls
        .retain(|_, cursor| now.duration_since(cursor.last_accessed) < SCROLL_TTL);
}

fn evict_scrolls_for_budget(inner: &mut RuntimeInner, incoming_bytes: usize, max_bytes: usize) {
    while (inner.scrolls.len() >= MAX_SCROLL_CONTEXTS
        || total_scroll_bytes(inner).saturating_add(incoming_bytes) > max_bytes)
        && !inner.scrolls.is_empty()
    {
        let Some(oldest) = inner
            .scrolls
            .iter()
            .min_by_key(|(_, cursor)| cursor.last_accessed)
            .map(|(scroll_id, _)| scroll_id.clone())
        else {
            return;
        };
        inner.scrolls.remove(&oldest);
    }
}

fn total_scroll_bytes(inner: &RuntimeInner) -> usize {
    inner
        .scrolls
        .values()
        .map(|cursor| cursor.bytes)
        .sum::<usize>()
}

fn estimate_scroll_bytes(hits: &[Value], total: &Value, max_score: &Value) -> usize {
    hits.iter()
        .map(estimate_value_bytes)
        .sum::<usize>()
        .saturating_add(estimate_value_bytes(total))
        .saturating_add(estimate_value_bytes(max_score))
}

fn estimate_value_bytes(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX / 4)
}

fn page_hits(hits: &[Value], position: usize, batch_size: usize) -> (Vec<Value>, usize) {
    let end = position.saturating_add(batch_size).min(hits.len());
    (hits[position.min(hits.len())..end].to_vec(), end)
}
