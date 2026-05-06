use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

use crate::storage::Database;

const MAX_SCROLL_CONTEXTS: usize = 32;
const SCROLL_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_PIT_CONTEXTS: usize = 16;
const MAX_PIT_RETAINED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    inner: Arc<Mutex<RuntimeInner>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct RuntimeInner {
    scrolls: BTreeMap<String, ScrollCursor>,
    pits: BTreeMap<String, PitContext>,
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
    terminal_empty_pending: bool,
}

#[derive(Debug, Clone)]
pub struct ScrollPage {
    pub scroll_id: String,
    pub hits: Vec<Value>,
    pub total: Value,
    pub max_score: Value,
}

#[derive(Debug, Clone)]
struct PitContext {
    database: Arc<Database>,
    created_at_unix_millis: u64,
    expires_at: Instant,
    keep_alive: Duration,
    bytes: usize,
}

#[derive(Debug, Clone)]
pub struct PitCreateResult {
    pub pit_id: String,
    pub creation_time: u64,
    pub total_shards: u64,
}

#[derive(Debug, Clone)]
pub struct PitInfo {
    pub pit_id: String,
    pub creation_time: u64,
    pub keep_alive_millis: u64,
}

#[derive(Debug, Clone)]
pub struct PitDeleteResult {
    pub pit_id: String,
    pub successful: bool,
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
            terminal_empty_pending: remaining_hits.is_empty(),
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
        inner.scrolls.insert(scroll_id.clone(), cursor);
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
        let mut remove_cursor = false;
        let page = {
            let cursor = inner.scrolls.get_mut(scroll_id)?;
            cursor.last_accessed = Instant::now();
            if cursor.terminal_empty_pending {
                remove_cursor = true;
                ScrollPage {
                    scroll_id: scroll_id.to_string(),
                    hits: Vec::new(),
                    total: cursor.total.clone(),
                    max_score: cursor.max_score.clone(),
                }
            } else {
                let (hits, position) = page_hits(&cursor.hits, cursor.position, cursor.batch_size);
                cursor.position = position;
                if position >= cursor.hits.len() {
                    cursor.terminal_empty_pending = true;
                    cursor.hits.clear();
                    cursor.position = 0;
                    cursor.bytes = estimate_scroll_bytes(&[], &cursor.total, &cursor.max_score);
                }
                ScrollPage {
                    scroll_id: scroll_id.to_string(),
                    hits,
                    total: cursor.total.clone(),
                    max_score: cursor.max_score.clone(),
                }
            }
        };
        if remove_cursor {
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

    pub fn create_pit(
        &self,
        database: Database,
        keep_alive: Duration,
        total_shards: u64,
        max_bytes: usize,
    ) -> Result<PitCreateResult, RuntimeError> {
        let bytes = estimate_database_bytes(&database);
        if bytes > max_bytes || bytes > MAX_PIT_RETAINED_BYTES {
            return Err(RuntimeError::new(
                429,
                "resource_limit_exception",
                format!("PIT context would retain {bytes} bytes, exceeding the local PIT budget"),
            ));
        }

        let pit_id = self.next_runtime_id("pit");
        let creation_time = now_millis();
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_pits(&mut inner);
        if inner.pits.len() >= MAX_PIT_CONTEXTS
            || total_pit_bytes(&inner).saturating_add(bytes) > max_bytes
            || total_pit_bytes(&inner).saturating_add(bytes) > MAX_PIT_RETAINED_BYTES
        {
            return Err(RuntimeError::new(
                429,
                "resource_limit_exception",
                "maximum local PIT contexts reached",
            ));
        }

        inner.pits.insert(
            pit_id.clone(),
            PitContext {
                database: Arc::new(database),
                created_at_unix_millis: creation_time,
                expires_at: Instant::now() + keep_alive,
                keep_alive,
                bytes,
            },
        );

        Ok(PitCreateResult {
            pit_id,
            creation_time,
            total_shards,
        })
    }

    pub fn list_pits(&self) -> Vec<PitInfo> {
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_pits(&mut inner);
        inner
            .pits
            .iter()
            .map(|(pit_id, pit)| PitInfo {
                pit_id: pit_id.clone(),
                creation_time: pit.created_at_unix_millis,
                keep_alive_millis: duration_millis(pit.keep_alive),
            })
            .collect()
    }

    pub fn delete_pits(&self, pit_ids: &[String]) -> Vec<PitDeleteResult> {
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_pits(&mut inner);
        pit_ids
            .iter()
            .map(|pit_id| {
                inner.pits.remove(pit_id);
                PitDeleteResult {
                    pit_id: pit_id.clone(),
                    successful: true,
                }
            })
            .collect()
    }

    pub fn delete_all_pits(&self) -> Vec<PitDeleteResult> {
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_pits(&mut inner);
        let pit_ids = inner.pits.keys().cloned().collect::<Vec<_>>();
        pit_ids
            .into_iter()
            .map(|pit_id| {
                inner.pits.remove(&pit_id);
                PitDeleteResult {
                    pit_id,
                    successful: true,
                }
            })
            .collect()
    }

    pub fn pit_database(
        &self,
        pit_id: &str,
        keep_alive: Option<Duration>,
    ) -> Option<Arc<Database>> {
        let mut inner = self.inner.lock().expect("runtime lock is not poisoned");
        purge_expired_pits(&mut inner);
        inner.pits.get_mut(pit_id).map(|pit| {
            if let Some(keep_alive) = keep_alive {
                pit.keep_alive = keep_alive;
                pit.expires_at = Instant::now() + keep_alive;
            }
            pit.database.clone()
        })
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
        format!("mainstack-search-{prefix}:{id}")
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
                "node": "mainstack-search",
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

fn purge_expired_pits(inner: &mut RuntimeInner) {
    let now = Instant::now();
    inner.pits.retain(|_, pit| pit.expires_at > now);
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

fn total_pit_bytes(inner: &RuntimeInner) -> usize {
    inner.pits.values().map(|pit| pit.bytes).sum::<usize>()
}

fn estimate_scroll_bytes(hits: &[Value], total: &Value, max_score: &Value) -> usize {
    hits.iter()
        .map(estimate_value_bytes)
        .sum::<usize>()
        .saturating_add(estimate_value_bytes(total))
        .saturating_add(estimate_value_bytes(max_score))
}

fn estimate_database_bytes(database: &Database) -> usize {
    serde_json::to_vec(database)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX / 4)
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

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(duration_millis)
        .unwrap_or_default()
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, payload_size: usize) -> Value {
        json!({
            "_id": id,
            "_source": {
                "payload": "x".repeat(payload_size)
            }
        })
    }

    #[test]
    fn terminal_empty_scroll_state_drops_retained_hits_before_next_request() {
        let runtime = RuntimeState::default();
        let first = runtime
            .create_scroll(
                vec![hit("1", 2048), hit("2", 2048)],
                json!({ "value": 2, "relation": "eq" }),
                Value::Null,
                1,
                100_000,
            )
            .expect("scroll context is within budget");
        let scroll_id = first.scroll_id;

        let final_hit = runtime
            .next_scroll(&scroll_id)
            .expect("final non-empty page is retained");
        assert_eq!(final_hit.hits.len(), 1);

        {
            let inner = runtime.inner.lock().expect("runtime lock is not poisoned");
            let cursor = inner
                .scrolls
                .get(&scroll_id)
                .expect("terminal page is pending");
            assert!(cursor.terminal_empty_pending);
            assert!(cursor.hits.is_empty());
            assert_eq!(cursor.position, 0);
            assert_eq!(
                cursor.bytes,
                estimate_scroll_bytes(&[], &cursor.total, &cursor.max_score)
            );
        }

        let terminal_empty = runtime
            .next_scroll(&scroll_id)
            .expect("terminal empty page is returned once");
        assert!(terminal_empty.hits.is_empty());
        assert!(runtime.next_scroll(&scroll_id).is_none());
    }
}
