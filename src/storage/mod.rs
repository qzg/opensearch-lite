pub mod document_store;
pub mod mutation_log;
pub mod snapshot;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{config::Config, storage::mutation_log::Mutation};

const MUTATION_LOG_SYNC_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct Store {
    inner: Arc<RwLock<Database>>,
    commit_lock: Arc<Mutex<()>>,
    snapshot_lock: Arc<Mutex<()>>,
    snapshot_state: Arc<Mutex<SnapshotFlushState>>,
    mutation_log_syncer: Option<Arc<MutationLogSyncer>>,
    mutation_log_path: PathBuf,
    snapshot_path: PathBuf,
    snapshot_metadata_path: PathBuf,
    _data_lock: Option<Arc<File>>,
    ephemeral: bool,
    limits: StoreLimits,
    snapshot_policy: SnapshotPolicy,
}

#[derive(Debug, Clone)]
struct StoreLimits {
    max_indexes: usize,
    max_documents: usize,
    memory_limit_bytes: usize,
}

#[derive(Debug, Clone)]
struct SnapshotPolicy {
    write_threshold: usize,
    interval: Duration,
}

#[derive(Debug, Clone, Default)]
struct SnapshotFlushState {
    dirty_writes: usize,
    dirty_since: Option<SystemTime>,
    generation: u64,
    last_transaction_id: Option<String>,
}

struct MutationLogSyncer {
    state: Arc<MutationLogSyncThreadState>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

struct MutationLogSyncThreadState {
    path: PathBuf,
    dirty: Mutex<bool>,
    stop: AtomicBool,
    changed: Condvar,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Database {
    pub indexes: BTreeMap<String, IndexMetadata>,
    pub templates: BTreeMap<String, IndexTemplate>,
    #[serde(default)]
    pub registries: BTreeMap<String, BTreeMap<String, Value>>,
    pub aliases: BTreeMap<String, AliasMetadata>,
    pub seq_no: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub name: String,
    pub settings: Value,
    pub mappings: Value,
    pub aliases: BTreeSet<String>,
    pub documents: BTreeMap<String, StoredDocument>,
    pub tombstones: BTreeMap<String, u64>,
    #[serde(default)]
    pub store_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredDocument {
    pub id: String,
    pub source: Value,
    pub version: u64,
    pub seq_no: u64,
    pub primary_term: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexTemplate {
    pub name: String,
    pub index_patterns: Vec<String>,
    pub template: Value,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasMetadata {
    pub alias: String,
    pub index: String,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct StoreError {
    pub status: u16,
    pub error_type: &'static str,
    pub reason: String,
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone)]
pub enum WriteOperation {
    IndexDocument {
        index: String,
        id: String,
        source: Value,
    },
    CreateDocument {
        index: String,
        id: String,
        source: Value,
    },
    UpdateDocument {
        index: String,
        id: String,
        doc: Value,
        doc_as_upsert: bool,
        upsert: Option<Value>,
    },
    DeleteDocument {
        index: String,
        id: String,
    },
}

#[derive(Debug, Clone)]
pub enum WriteOutcome {
    Document(StoredDocument),
    Deleted { found: bool },
}

#[derive(Debug)]
struct PreparedWrite {
    mutations: Vec<Mutation>,
    outcome: PreparedOutcome,
}

#[derive(Debug)]
enum PreparedOutcome {
    Document { index: String, id: String },
    Deleted { found: bool },
}

impl StoreError {
    pub fn new(status: u16, error_type: &'static str, reason: impl Into<String>) -> Self {
        Self {
            status,
            error_type,
            reason: reason.into(),
        }
    }
}

impl PreparedOutcome {
    fn resolve(self, db: &Database) -> StoreResult<WriteOutcome> {
        match self {
            PreparedOutcome::Document { index, id } => db
                .indexes
                .get(&index)
                .and_then(|index| index.documents.get(&id))
                .cloned()
                .map(WriteOutcome::Document)
                .ok_or_else(|| internal_document_error("committed")),
            PreparedOutcome::Deleted { found } => Ok(WriteOutcome::Deleted { found }),
        }
    }
}

impl fmt::Debug for MutationLogSyncer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MutationLogSyncer")
            .field("path", &self.state.path)
            .finish_non_exhaustive()
    }
}

impl MutationLogSyncer {
    fn spawn(path: PathBuf) -> Self {
        let state = Arc::new(MutationLogSyncThreadState {
            path,
            dirty: Mutex::new(false),
            stop: AtomicBool::new(false),
            changed: Condvar::new(),
        });
        let thread_state = Arc::clone(&state);
        let handle = match thread::Builder::new()
            .name("opensearch-lite-mutation-log-sync".to_string())
            .spawn(move || run_mutation_log_syncer(thread_state))
        {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!(
                    "opensearch-lite mutation log warning: delayed sync thread did not start: {error}"
                );
                None
            }
        };
        Self {
            state,
            handle: Mutex::new(handle),
        }
    }

    fn mark_dirty(&self) {
        let Ok(mut dirty) = self.state.dirty.lock() else {
            eprintln!("opensearch-lite mutation log warning: sync state lock is poisoned");
            return;
        };
        *dirty = true;
        self.state.changed.notify_one();
    }
}

impl Drop for MutationLogSyncer {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        self.state.changed.notify_one();
        if let Ok(mut handle) = self.handle.lock() {
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
        sync_dirty_mutation_log_now(&self.state);
    }
}

fn run_mutation_log_syncer(state: Arc<MutationLogSyncThreadState>) {
    loop {
        let Ok(mut dirty) = state.dirty.lock() else {
            eprintln!("opensearch-lite mutation log warning: sync state lock is poisoned");
            return;
        };
        while !*dirty && !state.stop.load(Ordering::SeqCst) {
            let Ok(next_dirty) = state.changed.wait(dirty) else {
                eprintln!("opensearch-lite mutation log warning: sync state lock is poisoned");
                return;
            };
            dirty = next_dirty;
        }
        if state.stop.load(Ordering::SeqCst) {
            return;
        }
        let first_dirty_at = Instant::now();
        while !state.stop.load(Ordering::SeqCst) {
            let elapsed = first_dirty_at.elapsed();
            if elapsed >= MUTATION_LOG_SYNC_INTERVAL {
                break;
            }
            let remaining = MUTATION_LOG_SYNC_INTERVAL - elapsed;
            let Ok((next_dirty, _)) = state.changed.wait_timeout(dirty, remaining) else {
                eprintln!("opensearch-lite mutation log warning: sync state lock is poisoned");
                return;
            };
            dirty = next_dirty;
        }
        if state.stop.load(Ordering::SeqCst) {
            return;
        }
        if !*dirty {
            continue;
        }
        *dirty = false;
        drop(dirty);
        if let Err(error) = mutation_log::sync(&state.path) {
            eprintln!("opensearch-lite mutation log warning: delayed sync failed: {error}");
            if let Ok(mut dirty) = state.dirty.lock() {
                *dirty = true;
            }
        }
    }
}

fn sync_dirty_mutation_log_now(state: &MutationLogSyncThreadState) {
    let dirty_was_set = match state.dirty.lock() {
        Ok(mut dirty) => {
            let was_dirty = *dirty;
            *dirty = false;
            was_dirty
        }
        Err(_) => {
            eprintln!("opensearch-lite mutation log warning: sync state lock is poisoned");
            false
        }
    };
    if dirty_was_set {
        if let Err(error) = mutation_log::sync(&state.path) {
            eprintln!("opensearch-lite mutation log warning: final sync failed: {error}");
        }
    }
}

fn single_write_result(mut results: Vec<StoreResult<WriteOutcome>>) -> StoreResult<WriteOutcome> {
    results.pop().unwrap_or_else(|| {
        Err(StoreError::new(
            500,
            "document_exception",
            "write operation returned no result",
        ))
    })
}

fn internal_document_error(action: &'static str) -> StoreError {
    StoreError::new(
        500,
        "document_missing_exception",
        format!("{action} document missing"),
    )
}

fn load_durable_state(
    snapshot_path: &std::path::Path,
    snapshot_metadata_path: &std::path::Path,
    mutation_log_path: &std::path::Path,
    memory_limit_bytes: usize,
    db: &mut Database,
) -> io::Result<SnapshotFlushState> {
    let Some(metadata) = snapshot::read_metadata(snapshot_metadata_path)? else {
        mutation_log::replay_validating(mutation_log_path, db, |db| {
            validate_loaded_database_memory(db, memory_limit_bytes)
        })?;
        return Ok(SnapshotFlushState::default());
    };
    let snapshot_path = metadata
        .snapshot_file
        .as_deref()
        .map(|file| {
            snapshot_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(file)
        })
        .unwrap_or_else(|| snapshot_path.to_path_buf());
    let Some(snapshot_db) = snapshot::read_snapshot(&snapshot_path)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "snapshot metadata {} exists but snapshot {} is missing",
                snapshot_metadata_path.display(),
                snapshot_path.display()
            ),
        ));
    };
    *db = snapshot_db;
    validate_loaded_database_memory(db, memory_limit_bytes)?;
    if metadata.log_compacted {
        mutation_log::replay_validating(mutation_log_path, db, |db| {
            validate_loaded_database_memory(db, memory_limit_bytes)
        })?;
    } else {
        mutation_log::replay_after_validating(
            mutation_log_path,
            db,
            metadata.last_transaction_id.as_deref(),
            |db| validate_loaded_database_memory(db, memory_limit_bytes),
        )?;
    }
    Ok(SnapshotFlushState {
        dirty_writes: 0,
        dirty_since: None,
        generation: metadata.generation,
        last_transaction_id: metadata.last_transaction_id,
    })
}

fn snapshot_metadata(
    db: &Database,
    generation: u64,
    snapshot_file: Option<String>,
    last_transaction_id: Option<String>,
    log_compacted: bool,
) -> snapshot::SnapshotMetadata {
    let indexes = db
        .indexes
        .values()
        .map(|index| snapshot::SnapshotIndexMetadata {
            name: index.name.clone(),
            document_count: index.documents.len(),
            tombstone_count: index.tombstones.len(),
            alias_count: index.aliases.len(),
            store_size_bytes: index.store_size_bytes,
        })
        .collect();
    snapshot::SnapshotMetadata {
        version: 1,
        generation,
        snapshot_file,
        created_at_unix_millis: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
        estimated_stored_bytes: estimate_database_bytes(db),
        index_count: db.indexes.len(),
        document_count: db.document_count(),
        template_count: db.templates.len(),
        registry_object_count: db.registries.values().map(|registry| registry.len()).sum(),
        alias_count: db.aliases.len(),
        seq_no: db.seq_no,
        last_transaction_id,
        log_compacted,
        indexes,
    }
}

impl Store {
    pub fn open(config: &Config) -> io::Result<Self> {
        let mutation_log_path = config.data_dir.join("mutations.jsonl");
        let snapshot_path = config.data_dir.join("snapshot.json");
        let snapshot_metadata_path = config.data_dir.join("snapshot.meta.json");
        if !config.ephemeral {
            fs::create_dir_all(&config.data_dir)?;
            set_owner_only(&config.data_dir)?;
        }
        let data_lock = if config.ephemeral {
            None
        } else {
            Some(Arc::new(open_data_lock(&config.data_dir)?))
        };
        let mut db = Database::default();
        let mut snapshot_state = SnapshotFlushState::default();
        if !config.ephemeral {
            snapshot_state = load_durable_state(
                &snapshot_path,
                &snapshot_metadata_path,
                &mutation_log_path,
                config.memory_limit_bytes,
                &mut db,
            )?;
            validate_loaded_database_memory(&db, config.memory_limit_bytes)?;
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(db)),
            commit_lock: Arc::new(Mutex::new(())),
            snapshot_lock: Arc::new(Mutex::new(())),
            snapshot_state: Arc::new(Mutex::new(snapshot_state)),
            mutation_log_syncer: if config.ephemeral {
                None
            } else {
                Some(Arc::new(MutationLogSyncer::spawn(
                    mutation_log_path.clone(),
                )))
            },
            mutation_log_path,
            snapshot_path,
            snapshot_metadata_path,
            _data_lock: data_lock,
            ephemeral: config.ephemeral,
            limits: StoreLimits {
                max_indexes: config.max_indexes,
                max_documents: config.max_documents,
                memory_limit_bytes: config.memory_limit_bytes,
            },
            snapshot_policy: SnapshotPolicy {
                write_threshold: config.snapshot_write_threshold,
                interval: config.snapshot_interval,
            },
        })
    }

    pub fn database(&self) -> Database {
        self.inner
            .read()
            .expect("store lock is not poisoned")
            .clone()
    }

    pub fn read_database<T>(&self, f: impl FnOnce(&Database) -> T) -> StoreResult<T> {
        let db = self.inner.read().map_err(|_| {
            StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
        })?;
        Ok(f(&db))
    }

    pub fn commit(&self, mutation: Mutation) -> StoreResult<()> {
        self.commit_mutations(vec![mutation])
    }

    pub(crate) fn commit_mutations(&self, mutations: Vec<Mutation>) -> StoreResult<()> {
        if mutations.is_empty() {
            return Ok(());
        }

        let _commit = self.commit_lock.lock().map_err(|_| {
            StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
        })?;
        let snapshot_work = {
            let mut db = self.inner.write().map_err(|_| {
                StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
            })?;
            let before = db.clone();
            let mut candidate = before.clone();
            for mutation in &mutations {
                self.validate_mutation(&candidate, mutation)?;
                mutation.apply_to(&mut candidate);
                self.validate_memory(&candidate)?;
            }
            if !self.ephemeral {
                let transaction_id =
                    self.append_committed_transaction(&mut db, &before, &candidate, &mutations)?;
                Some((candidate, transaction_id, mutations.len()))
            } else {
                *db = candidate;
                None
            }
        };
        if let Some((snapshot_db, transaction_id, mutation_count)) = snapshot_work {
            self.schedule_mutation_log_sync();
            self.after_committed_transaction(&snapshot_db, Some(transaction_id), mutation_count);
        }
        Ok(())
    }

    pub fn apply_write_operations(
        &self,
        operations: Vec<WriteOperation>,
    ) -> StoreResult<Vec<StoreResult<WriteOutcome>>> {
        let _commit = self.commit_lock.lock().map_err(|_| {
            StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
        })?;
        let (results, snapshot_work) = {
            let mut db = self.inner.write().map_err(|_| {
                StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
            })?;
            let before = db.clone();
            let mut candidate = before.clone();
            let mut committed_mutations = Vec::new();
            let mut results = Vec::with_capacity(operations.len());

            for operation in operations {
                match self
                    .prepare_write_operation(&candidate, operation)
                    .and_then(|prepared| {
                        let mut operation_candidate = candidate.clone();
                        for mutation in &prepared.mutations {
                            self.validate_mutation(&operation_candidate, mutation)?;
                            mutation.apply_to(&mut operation_candidate);
                            self.validate_memory(&operation_candidate)?;
                        }
                        let outcome = prepared.outcome.resolve(&operation_candidate)?;
                        Ok((operation_candidate, prepared.mutations, outcome))
                    }) {
                    Ok((operation_candidate, mutations, outcome)) => {
                        candidate = operation_candidate;
                        committed_mutations.extend(mutations);
                        results.push(Ok(outcome));
                    }
                    Err(error) => results.push(Err(error)),
                }
            }

            let snapshot_work = if !committed_mutations.is_empty() {
                if !self.ephemeral {
                    let mutation_count = committed_mutations.len();
                    let transaction_id = self.append_committed_transaction(
                        &mut db,
                        &before,
                        &candidate,
                        &committed_mutations,
                    )?;
                    Some((candidate, transaction_id, mutation_count))
                } else {
                    *db = candidate;
                    None
                }
            } else {
                None
            };
            (results, snapshot_work)
        };
        if let Some((snapshot_db, transaction_id, mutation_count)) = snapshot_work {
            self.schedule_mutation_log_sync();
            self.after_committed_transaction(&snapshot_db, Some(transaction_id), mutation_count);
        }
        Ok(results)
    }

    pub fn apply_dynamic_write_operations_atomic<T>(
        &self,
        build_operations: impl FnOnce(&Database) -> StoreResult<(Vec<WriteOperation>, T)>,
    ) -> StoreResult<(Vec<WriteOutcome>, T)> {
        let _commit = self.commit_lock.lock().map_err(|_| {
            StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
        })?;
        let (outcomes, metadata, snapshot_work) = {
            let mut db = self.inner.write().map_err(|_| {
                StoreError::new(500, "lock_poisoned_exception", "store lock is poisoned")
            })?;
            let before = db.clone();
            let (operations, metadata) = build_operations(&before)?;
            let mut candidate = before.clone();
            let mut committed_mutations = Vec::new();
            let mut outcomes = Vec::with_capacity(operations.len());

            for operation in operations {
                let prepared = self.prepare_write_operation(&candidate, operation)?;
                for mutation in &prepared.mutations {
                    self.validate_mutation(&candidate, mutation)?;
                    mutation.apply_to(&mut candidate);
                    self.validate_memory(&candidate)?;
                }
                outcomes.push(prepared.outcome.resolve(&candidate)?);
                committed_mutations.extend(prepared.mutations);
            }

            let snapshot_work = if !committed_mutations.is_empty() {
                if !self.ephemeral {
                    let mutation_count = committed_mutations.len();
                    let transaction_id = self.append_committed_transaction(
                        &mut db,
                        &before,
                        &candidate,
                        &committed_mutations,
                    )?;
                    Some((candidate, transaction_id, mutation_count))
                } else {
                    *db = candidate;
                    None
                }
            } else {
                None
            };
            (outcomes, metadata, snapshot_work)
        };
        if let Some((snapshot_db, transaction_id, mutation_count)) = snapshot_work {
            self.schedule_mutation_log_sync();
            self.after_committed_transaction(&snapshot_db, Some(transaction_id), mutation_count);
        }
        Ok((outcomes, metadata))
    }

    fn append_committed_transaction(
        &self,
        db: &mut Database,
        before: &Database,
        candidate: &Database,
        mutations: &[Mutation],
    ) -> StoreResult<String> {
        let transaction_id = transaction_id();
        mutation_log::append_transaction_begin(&self.mutation_log_path, &transaction_id, mutations)
            .map_err(|error| {
                StoreError::new(
                    500,
                    "mutation_log_exception",
                    format!("failed to append mutation transaction: {error}"),
                )
            })?;
        *db = candidate.clone();
        if let Err(error) =
            mutation_log::append_transaction_commit(&self.mutation_log_path, &transaction_id)
        {
            *db = before.clone();
            return Err(StoreError::new(
                500,
                "mutation_log_exception",
                format!("failed to commit mutation transaction: {error}"),
            ));
        }
        Ok(transaction_id)
    }

    fn schedule_mutation_log_sync(&self) {
        if let Some(syncer) = &self.mutation_log_syncer {
            syncer.mark_dirty();
        }
    }

    fn after_committed_transaction(
        &self,
        db: &Database,
        transaction_id: Option<String>,
        mutation_count: usize,
    ) {
        if self.ephemeral {
            return;
        }
        let Ok(mut state) = self.snapshot_state.lock() else {
            eprintln!("opensearch-lite snapshot warning: snapshot state lock is poisoned");
            return;
        };
        let now = SystemTime::now();
        if state.dirty_since.is_none() {
            state.dirty_since = Some(now);
        }
        state.dirty_writes = state.dirty_writes.saturating_add(mutation_count.max(1));
        if transaction_id.is_some() {
            state.last_transaction_id = transaction_id;
        }
        let dirty_elapsed = state
            .dirty_since
            .and_then(|dirty_since| now.duration_since(dirty_since).ok())
            .unwrap_or_default();
        if state.dirty_writes < self.snapshot_policy.write_threshold
            && dirty_elapsed < self.snapshot_policy.interval
        {
            return;
        }
        let generation = state.generation.saturating_add(1);
        let last_transaction_id = state.last_transaction_id.clone();

        let Ok(_snapshot) = self.snapshot_lock.lock() else {
            eprintln!("opensearch-lite snapshot warning: snapshot lock is poisoned");
            return;
        };
        if let Err(error) = self.write_snapshot_generation(db, generation, last_transaction_id) {
            eprintln!("opensearch-lite snapshot warning: {error}");
            return;
        }
        state.generation = generation;
        state.dirty_writes = 0;
        state.dirty_since = None;
    }

    fn write_snapshot_generation(
        &self,
        db: &Database,
        generation: u64,
        last_transaction_id: Option<String>,
    ) -> io::Result<()> {
        let snapshot_file = snapshot_file_name(generation);
        let generation_snapshot_path = self
            .snapshot_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(&snapshot_file);
        snapshot::write_snapshot(&generation_snapshot_path, db)?;
        let metadata = snapshot_metadata(
            db,
            generation,
            Some(snapshot_file.clone()),
            last_transaction_id.clone(),
            false,
        );
        snapshot::write_metadata(&self.snapshot_metadata_path, &metadata)?;
        let compacted =
            mutation_log::compact_after(&self.mutation_log_path, last_transaction_id.as_deref())?;
        if compacted {
            let metadata = snapshot_metadata(
                db,
                generation,
                Some(snapshot_file),
                last_transaction_id,
                true,
            );
            snapshot::write_metadata(&self.snapshot_metadata_path, &metadata)?;
        }
        snapshot::write_snapshot(&self.snapshot_path, db)?;
        Ok(())
    }

    fn prepare_write_operation(
        &self,
        db: &Database,
        operation: WriteOperation,
    ) -> StoreResult<PreparedWrite> {
        match operation {
            WriteOperation::IndexDocument { index, id, source } => {
                let (index, mut mutations) = self.resolve_or_create_index(db, &index)?;
                mutations.push(Mutation::IndexDocument {
                    index: index.clone(),
                    id: id.clone(),
                    source,
                });
                Ok(PreparedWrite {
                    mutations,
                    outcome: PreparedOutcome::Document { index, id },
                })
            }
            WriteOperation::CreateDocument { index, id, source } => {
                let (index, mut mutations) = self.resolve_or_create_index(db, &index)?;
                mutations.push(Mutation::CreateDocument {
                    index: index.clone(),
                    id: id.clone(),
                    source,
                });
                Ok(PreparedWrite {
                    mutations,
                    outcome: PreparedOutcome::Document { index, id },
                })
            }
            WriteOperation::UpdateDocument {
                index,
                id,
                doc,
                doc_as_upsert,
                upsert,
            } => {
                let (index, mut mutations) = match db.resolve_index(&index) {
                    Some(index) => (index, Vec::new()),
                    None if doc_as_upsert || upsert.is_some() => {
                        self.resolve_or_create_index(db, &index)?
                    }
                    None => {
                        return Err(not_found(
                            "index_not_found_exception",
                            format!("no such index [{index}]"),
                        ));
                    }
                };
                let exists = db
                    .indexes
                    .get(&index)
                    .and_then(|index| index.documents.get(&id))
                    .is_some();
                if !doc_as_upsert && upsert.is_none() && !exists {
                    return Err(not_found(
                        "document_missing_exception",
                        format!("document [{id}] missing"),
                    ));
                }
                if exists {
                    mutations.push(Mutation::UpdateDocument {
                        index: index.clone(),
                        id: id.clone(),
                        doc,
                        doc_as_upsert,
                    });
                } else {
                    let source = upsert.unwrap_or(doc);
                    mutations.push(Mutation::CreateDocument {
                        index: index.clone(),
                        id: id.clone(),
                        source,
                    });
                }
                Ok(PreparedWrite {
                    mutations,
                    outcome: PreparedOutcome::Document { index, id },
                })
            }
            WriteOperation::DeleteDocument { index, id } => {
                let index = db.resolve_index(&index).ok_or_else(|| {
                    not_found(
                        "index_not_found_exception",
                        format!("no such index [{index}]"),
                    )
                })?;
                let found = db
                    .indexes
                    .get(&index)
                    .and_then(|index| index.documents.get(&id))
                    .is_some();
                let mutations = if found {
                    vec![Mutation::DeleteDocument {
                        index: index.clone(),
                        id,
                    }]
                } else {
                    Vec::new()
                };
                Ok(PreparedWrite {
                    mutations,
                    outcome: PreparedOutcome::Deleted { found },
                })
            }
        }
    }

    fn resolve_or_create_index(
        &self,
        db: &Database,
        index_or_alias: &str,
    ) -> StoreResult<(String, Vec<Mutation>)> {
        if let Some(index) = db.resolve_index(index_or_alias) {
            return Ok((index, Vec::new()));
        }
        let (settings, mappings) = template_config_for(db, index_or_alias);
        Ok((
            index_or_alias.to_string(),
            vec![Mutation::CreateIndex {
                name: index_or_alias.to_string(),
                settings,
                mappings,
            }],
        ))
    }

    pub fn create_index(&self, name: &str, body: Value) -> StoreResult<()> {
        let (settings, mappings) = extract_index_config(&body);
        self.commit(Mutation::CreateIndex {
            name: name.to_string(),
            settings,
            mappings,
        })
    }

    pub fn ensure_index_for_write(&self, name: &str) -> StoreResult<()> {
        if self.resolve_index(name).is_some() {
            return Ok(());
        }
        let db = self.database();
        let (settings, mappings) = template_config_for(&db, name);
        drop(db);
        self.commit(Mutation::CreateIndex {
            name: name.to_string(),
            settings,
            mappings,
        })
    }

    pub fn delete_index(&self, name: &str) -> StoreResult<()> {
        let index = self.resolve_index(name).ok_or_else(|| {
            not_found(
                "index_not_found_exception",
                format!("no such index [{name}]"),
            )
        })?;
        self.commit(Mutation::DeleteIndex { name: index })
    }

    pub fn put_mapping(&self, index: &str, mappings: Value) -> StoreResult<()> {
        let index = self.resolve_index(index).ok_or_else(|| {
            not_found(
                "index_not_found_exception",
                format!("no such index [{index}]"),
            )
        })?;
        self.commit(Mutation::PutMapping { index, mappings })
    }

    pub fn put_settings(&self, index: &str, settings: Value) -> StoreResult<()> {
        let index = self.resolve_index(index).ok_or_else(|| {
            not_found(
                "index_not_found_exception",
                format!("no such index [{index}]"),
            )
        })?;
        self.commit(Mutation::PutSettings { index, settings })
    }

    pub fn put_template(&self, name: &str, body: Value) -> StoreResult<()> {
        let patterns = index_patterns(&body);
        self.commit(Mutation::PutTemplate {
            name: name.to_string(),
            index_patterns: patterns,
            template: body.get("template").cloned().unwrap_or_else(|| json!({})),
            raw: body,
        })
    }

    pub fn put_registry_object(&self, namespace: &str, name: &str, raw: Value) -> StoreResult<()> {
        self.commit(Mutation::PutRegistryObject {
            namespace: namespace.to_string(),
            name: name.to_string(),
            raw,
        })
    }

    pub fn delete_registry_object(&self, namespace: &str, name: &str) -> StoreResult<()> {
        self.commit(Mutation::DeleteRegistryObject {
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }

    pub fn put_alias(&self, index: &str, alias: &str, raw: Value) -> StoreResult<()> {
        let index = self.resolve_index(index).ok_or_else(|| {
            not_found(
                "index_not_found_exception",
                format!("no such index [{index}]"),
            )
        })?;
        self.commit(Mutation::PutAlias {
            index,
            alias: alias.to_string(),
            raw,
        })
    }

    pub fn delete_alias(&self, index: &str, alias: &str) -> StoreResult<()> {
        let index = self.resolve_index(index).ok_or_else(|| {
            not_found(
                "index_not_found_exception",
                format!("no such index [{index}]"),
            )
        })?;
        self.commit(Mutation::DeleteAlias {
            index,
            alias: alias.to_string(),
        })
    }

    pub fn index_document(
        &self,
        index: &str,
        id: String,
        source: Value,
    ) -> StoreResult<StoredDocument> {
        let results = self.apply_write_operations(vec![WriteOperation::IndexDocument {
            index: index.to_string(),
            id,
            source,
        }])?;
        match single_write_result(results)? {
            WriteOutcome::Document(document) => Ok(document),
            WriteOutcome::Deleted { .. } => Err(internal_document_error("committed")),
        }
    }

    pub fn create_document(
        &self,
        index: &str,
        id: String,
        source: Value,
    ) -> StoreResult<StoredDocument> {
        let results = self.apply_write_operations(vec![WriteOperation::CreateDocument {
            index: index.to_string(),
            id,
            source,
        }])?;
        match single_write_result(results)? {
            WriteOutcome::Document(document) => Ok(document),
            WriteOutcome::Deleted { .. } => Err(internal_document_error("committed")),
        }
    }

    pub fn update_document(
        &self,
        index: &str,
        id: &str,
        doc: Value,
        doc_as_upsert: bool,
        upsert: Option<Value>,
    ) -> StoreResult<StoredDocument> {
        let results = self.apply_write_operations(vec![WriteOperation::UpdateDocument {
            index: index.to_string(),
            id: id.to_string(),
            doc,
            doc_as_upsert,
            upsert,
        }])?;
        match single_write_result(results)? {
            WriteOutcome::Document(document) => Ok(document),
            WriteOutcome::Deleted { .. } => Err(internal_document_error("updated")),
        }
    }

    pub fn delete_document(&self, index: &str, id: &str) -> StoreResult<bool> {
        let results = self.apply_write_operations(vec![WriteOperation::DeleteDocument {
            index: index.to_string(),
            id: id.to_string(),
        }])?;
        match single_write_result(results)? {
            WriteOutcome::Deleted { found } => Ok(found),
            WriteOutcome::Document(_) => Err(StoreError::new(
                500,
                "document_exception",
                "delete returned a document outcome",
            )),
        }
    }

    pub fn get_document(&self, index: &str, id: &str) -> Option<StoredDocument> {
        let db = self.inner.read().ok()?;
        let index = db.resolve_index(index)?;
        db.indexes.get(&index)?.documents.get(id).cloned()
    }

    pub fn resolve_index(&self, index_or_alias: &str) -> Option<String> {
        self.inner
            .read()
            .ok()
            .and_then(|db| db.resolve_index(index_or_alias))
    }

    pub fn generated_id() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("lite-{nanos:x}")
    }

    fn validate_mutation(&self, db: &Database, mutation: &Mutation) -> StoreResult<()> {
        match mutation {
            Mutation::CreateIndex { name, .. } => {
                if name.trim().is_empty() {
                    return Err(StoreError::new(
                        400,
                        "invalid_index_name_exception",
                        "index name must not be empty",
                    ));
                }
                if db.indexes.contains_key(name) {
                    return Err(StoreError::new(
                        400,
                        "resource_already_exists_exception",
                        format!("index [{name}] already exists"),
                    ));
                }
                if db.indexes.len() >= self.limits.max_indexes {
                    return Err(StoreError::new(
                        429,
                        "resource_limit_exception",
                        "maximum index count reached",
                    ));
                }
            }
            Mutation::DeleteIndex { name } => {
                if !db.indexes.contains_key(name) {
                    return Err(not_found(
                        "index_not_found_exception",
                        format!("no such index [{name}]"),
                    ));
                }
            }
            Mutation::PutTemplate {
                name,
                index_patterns,
                ..
            } => {
                if name.trim().is_empty() {
                    return Err(StoreError::new(
                        400,
                        "invalid_index_template_exception",
                        "index template name must not be empty",
                    ));
                }
                if index_patterns.is_empty() {
                    return Err(StoreError::new(
                        400,
                        "invalid_index_template_exception",
                        "index template requires at least one index pattern",
                    ));
                }
            }
            Mutation::DeleteTemplate { name } => {
                if !db.templates.contains_key(name) {
                    return Err(not_found(
                        "index_template_missing_exception",
                        format!("index template [{name}] missing"),
                    ));
                }
            }
            Mutation::PutRegistryObject {
                namespace, name, ..
            } => {
                validate_registry_namespace(namespace)?;
                if name.trim().is_empty() {
                    return Err(StoreError::new(
                        400,
                        "invalid_registry_object_exception",
                        "registry object name must not be empty",
                    ));
                }
            }
            Mutation::DeleteRegistryObject { namespace, name } => {
                validate_registry_namespace(namespace)?;
                if !db
                    .registries
                    .get(namespace)
                    .map(|registry| registry.contains_key(name))
                    .unwrap_or(false)
                {
                    return Err(not_found(
                        "registry_object_missing_exception",
                        format!("registry object [{namespace}/{name}] missing"),
                    ));
                }
            }
            Mutation::IndexDocument { index, id, .. }
            | Mutation::CreateDocument { index, id, .. }
            | Mutation::UpdateDocument { index, id, .. } => {
                let Some(index_meta) = db.indexes.get(index) else {
                    return Err(not_found(
                        "index_not_found_exception",
                        format!("no such index [{index}]"),
                    ));
                };
                let existing = db
                    .indexes
                    .get(index)
                    .and_then(|index| index.documents.get(id))
                    .is_some();
                if matches!(mutation, Mutation::CreateDocument { .. }) && existing {
                    return Err(StoreError::new(
                        409,
                        "version_conflict_engine_exception",
                        format!("document [{id}] already exists"),
                    ));
                }
                if matches!(
                    mutation,
                    Mutation::UpdateDocument {
                        doc_as_upsert: false,
                        ..
                    }
                ) && !existing
                {
                    return Err(not_found(
                        "document_missing_exception",
                        format!("document [{id}] missing"),
                    ));
                }
                if !existing && db.document_count() >= self.limits.max_documents {
                    return Err(StoreError::new(
                        429,
                        "resource_limit_exception",
                        "maximum document count reached",
                    ));
                }
                if matches!(mutation, Mutation::IndexDocument { .. })
                    && index_meta.name.trim().is_empty()
                {
                    return Err(StoreError::new(
                        400,
                        "invalid_index_name_exception",
                        "index name must not be empty",
                    ));
                }
            }
            Mutation::DeleteDocument { index, .. } => {
                if !db.indexes.contains_key(index) {
                    return Err(not_found(
                        "index_not_found_exception",
                        format!("no such index [{index}]"),
                    ));
                }
            }
            Mutation::PutMapping { index, .. } | Mutation::PutSettings { index, .. } => {
                if !db.indexes.contains_key(index) {
                    return Err(not_found(
                        "index_not_found_exception",
                        format!("no such index [{index}]"),
                    ));
                }
            }
            Mutation::PutAlias { index, alias, .. } => {
                if !db.indexes.contains_key(index) {
                    return Err(not_found(
                        "index_not_found_exception",
                        format!("no such index [{index}]"),
                    ));
                }
                if alias.trim().is_empty() || alias.contains('*') {
                    return Err(StoreError::new(
                        400,
                        "invalid_alias_name_exception",
                        format!("invalid alias name [{alias}]"),
                    ));
                }
                if db.indexes.contains_key(alias) {
                    return Err(StoreError::new(
                        400,
                        "invalid_alias_name_exception",
                        format!("alias [{alias}] conflicts with an existing index"),
                    ));
                }
            }
            Mutation::DeleteAlias { index, alias } => {
                let exists = db
                    .aliases
                    .get(alias)
                    .map(|metadata| metadata.index == *index)
                    .unwrap_or(false);
                if !exists {
                    return Err(StoreError::new(
                        404,
                        "aliases_not_found_exception",
                        format!("alias [{alias}] missing"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_memory(&self, db: &Database) -> StoreResult<()> {
        let bytes = estimate_database_bytes(db);
        if bytes > self.limits.memory_limit_bytes {
            return Err(StoreError::new(
                429,
                "resource_limit_exception",
                format!(
                    "estimated stored state is {bytes} bytes, exceeding configured memory limit {} bytes",
                    self.limits.memory_limit_bytes
                ),
            ));
        }
        Ok(())
    }
}

fn validate_loaded_database_memory(db: &Database, memory_limit_bytes: usize) -> io::Result<()> {
    let bytes = estimate_database_bytes(db);
    if bytes <= memory_limit_bytes {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "loaded durable state estimates stored data at {bytes} bytes, exceeding configured --memory-limit {memory_limit_bytes} bytes. Remediation: increase local or container memory, reduce local data, lower --memory-limit to fit the container, use a smaller or empty --data-dir, or move this workload to full OpenSearch locally, server-hosted OpenSearch, or cloud-hosted OpenSearch."
        ),
    ))
}

impl Database {
    pub fn resolve_index(&self, index_or_alias: &str) -> Option<String> {
        if self.indexes.contains_key(index_or_alias) {
            return Some(index_or_alias.to_string());
        }
        self.aliases
            .get(index_or_alias)
            .map(|alias| alias.index.clone())
    }

    pub fn document_count(&self) -> usize {
        self.indexes
            .values()
            .map(|index| index.documents.len())
            .sum()
    }
}

impl IndexMetadata {
    fn new(name: String, settings: Value, mappings: Value) -> Self {
        let mut index = Self {
            name,
            settings,
            mappings,
            aliases: BTreeSet::new(),
            documents: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            store_size_bytes: 0,
        };
        index.recompute_store_size();
        index
    }

    fn recompute_store_size(&mut self) {
        self.store_size_bytes = estimate_index_bytes(self);
    }
}

fn extract_index_config(body: &Value) -> (Value, Value) {
    (
        body.get("settings").cloned().unwrap_or_else(|| json!({})),
        body.get("mappings").cloned().unwrap_or_else(|| json!({})),
    )
}

fn index_patterns(body: &Value) -> Vec<String> {
    match body.get("index_patterns") {
        Some(Value::String(pattern)) => vec![pattern.clone()],
        Some(Value::Array(patterns)) => patterns
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn template_config_for(db: &Database, index_name: &str) -> (Value, Value) {
    for template in db.templates.values() {
        if template
            .index_patterns
            .iter()
            .any(|pattern| pattern_matches(pattern, index_name))
        {
            return (
                template
                    .template
                    .get("settings")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                template
                    .template
                    .get("mappings")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
        }
    }
    (json!({}), json!({}))
}

fn estimate_database_bytes(db: &Database) -> usize {
    let mut bytes = 128usize;
    for (name, index) in &db.indexes {
        bytes = bytes.saturating_add(name.len());
        bytes = bytes.saturating_add(estimate_value_bytes(&index.settings));
        bytes = bytes.saturating_add(estimate_value_bytes(&index.mappings));
        for alias in &index.aliases {
            bytes = bytes.saturating_add(alias.len());
        }
        for (id, document) in &index.documents {
            bytes = bytes.saturating_add(id.len());
            bytes = bytes.saturating_add(64);
            bytes = bytes.saturating_add(estimate_value_bytes(&document.source));
        }
        for id in index.tombstones.keys() {
            bytes = bytes.saturating_add(id.len()).saturating_add(16);
        }
    }
    for template in db.templates.values() {
        bytes = bytes.saturating_add(template.name.len());
        bytes = bytes.saturating_add(estimate_value_bytes(&template.raw));
    }
    for alias in db.aliases.values() {
        bytes = bytes.saturating_add(alias.alias.len());
        bytes = bytes.saturating_add(alias.index.len());
        bytes = bytes.saturating_add(estimate_value_bytes(&alias.raw));
    }
    for (namespace, registry) in &db.registries {
        bytes = bytes.saturating_add(namespace.len());
        for (name, raw) in registry {
            bytes = bytes.saturating_add(name.len());
            bytes = bytes.saturating_add(estimate_value_bytes(raw));
        }
    }
    bytes
}

fn estimate_index_bytes(index: &IndexMetadata) -> usize {
    let mut bytes = index.name.len();
    bytes = bytes.saturating_add(estimate_value_bytes(&index.settings));
    bytes = bytes.saturating_add(estimate_value_bytes(&index.mappings));
    for alias in &index.aliases {
        bytes = bytes.saturating_add(alias.len());
    }
    for (id, document) in &index.documents {
        bytes = bytes.saturating_add(estimate_document_bytes(id, &document.source));
    }
    for id in index.tombstones.keys() {
        bytes = bytes.saturating_add(estimate_tombstone_bytes(id));
    }
    bytes
}

fn estimate_document_bytes(id: &str, source: &Value) -> usize {
    id.len()
        .saturating_add(64)
        .saturating_add(estimate_value_bytes(source))
}

fn estimate_tombstone_bytes(id: &str) -> usize {
    id.len().saturating_add(16)
}

fn estimate_value_bytes(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX / 2)
}

fn transaction_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("tx-{nanos:x}")
}

fn snapshot_file_name(generation: u64) -> String {
    format!("snapshot.{generation:020}.json")
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.split_once('*') {
        Some((prefix, suffix)) => value.starts_with(prefix) && value.ends_with(suffix),
        None => pattern == value,
    }
}

fn not_found(error_type: &'static str, reason: String) -> StoreError {
    StoreError::new(404, error_type, reason)
}

fn validate_registry_namespace(namespace: &str) -> StoreResult<()> {
    if matches!(
        namespace,
        "component_template" | "legacy_template" | "ingest_pipeline" | "search_pipeline" | "script"
    ) {
        Ok(())
    } else {
        Err(StoreError::new(
            400,
            "invalid_registry_namespace_exception",
            format!("unsupported registry namespace [{namespace}]"),
        ))
    }
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) -> io::Result<()> {
    Ok(())
}

impl Mutation {
    pub fn apply_to(&self, db: &mut Database) {
        match self {
            Mutation::CreateIndex {
                name,
                settings,
                mappings,
            } => {
                db.indexes.entry(name.clone()).or_insert_with(|| {
                    IndexMetadata::new(name.clone(), settings.clone(), mappings.clone())
                });
            }
            Mutation::DeleteIndex { name } => {
                db.indexes.remove(name);
                db.aliases.retain(|_, alias| alias.index != *name);
            }
            Mutation::PutTemplate {
                name,
                index_patterns,
                template,
                raw,
            } => {
                db.templates.insert(
                    name.clone(),
                    IndexTemplate {
                        name: name.clone(),
                        index_patterns: index_patterns.clone(),
                        template: template.clone(),
                        raw: raw.clone(),
                    },
                );
            }
            Mutation::DeleteTemplate { name } => {
                db.templates.remove(name);
            }
            Mutation::PutRegistryObject {
                namespace,
                name,
                raw,
            } => {
                db.registries
                    .entry(namespace.clone())
                    .or_default()
                    .insert(name.clone(), raw.clone());
            }
            Mutation::DeleteRegistryObject { namespace, name } => {
                if let Some(registry) = db.registries.get_mut(namespace) {
                    registry.remove(name);
                    if registry.is_empty() {
                        db.registries.remove(namespace);
                    }
                }
            }
            Mutation::PutMapping { index, mappings } => {
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    merge_object(&mut index_meta.mappings, mappings);
                    index_meta.recompute_store_size();
                }
            }
            Mutation::PutSettings { index, settings } => {
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    merge_object(&mut index_meta.settings, settings);
                    index_meta.recompute_store_size();
                }
            }
            Mutation::PutAlias { index, alias, raw } => {
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    index_meta.aliases.insert(alias.clone());
                    index_meta.recompute_store_size();
                }
                db.aliases.insert(
                    alias.clone(),
                    AliasMetadata {
                        alias: alias.clone(),
                        index: index.clone(),
                        raw: raw.clone(),
                    },
                );
            }
            Mutation::DeleteAlias { index, alias } => {
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    index_meta.aliases.remove(alias);
                    index_meta.recompute_store_size();
                }
                db.aliases.remove(alias);
            }
            Mutation::IndexDocument { index, id, source } => {
                db.seq_no += 1;
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    if let Some(existing) = index_meta.documents.get(id) {
                        index_meta.store_size_bytes = index_meta
                            .store_size_bytes
                            .saturating_sub(estimate_document_bytes(id, &existing.source));
                    }
                    if index_meta.tombstones.remove(id).is_some() {
                        index_meta.store_size_bytes = index_meta
                            .store_size_bytes
                            .saturating_sub(estimate_tombstone_bytes(id));
                    }
                    let version = index_meta
                        .documents
                        .get(id)
                        .map(|doc| doc.version + 1)
                        .unwrap_or(1);
                    let new_bytes = estimate_document_bytes(id, source);
                    index_meta.documents.insert(
                        id.clone(),
                        StoredDocument {
                            id: id.clone(),
                            source: source.clone(),
                            version,
                            seq_no: db.seq_no,
                            primary_term: 1,
                        },
                    );
                    index_meta.store_size_bytes =
                        index_meta.store_size_bytes.saturating_add(new_bytes);
                }
            }
            Mutation::CreateDocument { index, id, source } => {
                db.seq_no += 1;
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    if index_meta.documents.contains_key(id) {
                        return;
                    }
                    if index_meta.tombstones.remove(id).is_some() {
                        index_meta.store_size_bytes = index_meta
                            .store_size_bytes
                            .saturating_sub(estimate_tombstone_bytes(id));
                    }
                    index_meta.documents.insert(
                        id.clone(),
                        StoredDocument {
                            id: id.clone(),
                            source: source.clone(),
                            version: 1,
                            seq_no: db.seq_no,
                            primary_term: 1,
                        },
                    );
                    index_meta.store_size_bytes = index_meta
                        .store_size_bytes
                        .saturating_add(estimate_document_bytes(id, source));
                }
            }
            Mutation::UpdateDocument {
                index,
                id,
                doc,
                doc_as_upsert,
            } => {
                db.seq_no += 1;
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    let mut source = match index_meta.documents.get(id) {
                        Some(document) => document.source.clone(),
                        None if *doc_as_upsert => json!({}),
                        None => return,
                    };
                    merge_object(&mut source, doc);
                    if let Some(existing) = index_meta.documents.get(id) {
                        index_meta.store_size_bytes = index_meta
                            .store_size_bytes
                            .saturating_sub(estimate_document_bytes(id, &existing.source));
                    }
                    let version = index_meta
                        .documents
                        .get(id)
                        .map(|doc| doc.version + 1)
                        .unwrap_or(1);
                    let new_bytes = estimate_document_bytes(id, &source);
                    index_meta.documents.insert(
                        id.clone(),
                        StoredDocument {
                            id: id.clone(),
                            source,
                            version,
                            seq_no: db.seq_no,
                            primary_term: 1,
                        },
                    );
                    if index_meta.tombstones.remove(id).is_some() {
                        index_meta.store_size_bytes = index_meta
                            .store_size_bytes
                            .saturating_sub(estimate_tombstone_bytes(id));
                    }
                    index_meta.store_size_bytes =
                        index_meta.store_size_bytes.saturating_add(new_bytes);
                }
            }
            Mutation::DeleteDocument { index, id } => {
                db.seq_no += 1;
                if let Some(index_meta) = db.indexes.get_mut(index) {
                    if let Some(document) = index_meta.documents.remove(id) {
                        index_meta.store_size_bytes = index_meta
                            .store_size_bytes
                            .saturating_sub(estimate_document_bytes(id, &document.source))
                            .saturating_add(estimate_tombstone_bytes(id));
                        index_meta.tombstones.insert(id.clone(), db.seq_no);
                    }
                }
            }
        }
    }
}

fn open_data_lock(data_dir: &std::path::Path) -> io::Result<File> {
    let lock_path = data_dir.join(".opensearch-lite.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    file.try_lock_exclusive().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to lock {}: {error}", lock_path.display()),
        )
    })?;
    Ok(file)
}

fn merge_object(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                target.insert(key.clone(), value.clone());
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}
