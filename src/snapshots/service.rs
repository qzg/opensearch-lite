use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    config::Config,
    storage::{Database, StoreError, StoreResult},
};

#[derive(Clone)]
pub struct SnapshotService {
    root: Arc<PathBuf>,
    catalog_lock: Arc<Mutex<()>>,
    ephemeral: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepositoryData {
    version: u32,
    generation: u64,
    repository: RepositoryDefinition,
    snapshots: BTreeMap<String, SnapshotManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepositoryDefinition {
    name: String,
    #[serde(rename = "type")]
    repository_type: String,
    settings: Value,
    created_at_unix_millis: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotManifest {
    name: String,
    uuid: String,
    state: String,
    indices: Vec<String>,
    database_blob: String,
    started_at_unix_millis: u128,
    completed_at_unix_millis: u128,
    index_count: usize,
    document_count: usize,
    total_shards: u64,
    successful_shards: u64,
    failed_shards: u64,
    failures: Vec<Value>,
}

impl SnapshotService {
    pub fn from_config(config: &Config) -> Self {
        Self {
            root: Arc::new(config.data_dir.join("repositories")),
            catalog_lock: Arc::new(Mutex::new(())),
            ephemeral: config.ephemeral,
        }
    }

    pub fn put_repository(&self, name: &str, body: Value) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let name = validate_name(name, "repository")?;
        let repository_type = repository_type(&body)?;
        let settings = body.get("settings").cloned().unwrap_or_else(|| json!({}));
        if !settings.is_object() {
            return Err(invalid_repository("repository settings must be an object"));
        }
        validate_repository_settings(&settings)?;

        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let mut data = self
            .read_latest_optional(&name)?
            .unwrap_or_else(|| RepositoryData {
                version: 1,
                generation: 0,
                repository: RepositoryDefinition {
                    name: name.clone(),
                    repository_type: repository_type.clone(),
                    settings: settings.clone(),
                    created_at_unix_millis: now_millis(),
                },
                snapshots: BTreeMap::new(),
            });
        data.repository.repository_type = repository_type;
        data.repository.settings = settings;
        self.write_next_generation(&name, &mut data)?;
        Ok(json!({ "acknowledged": true }))
    }

    pub fn get_repositories(&self, names: Option<&str>) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let names = match names {
            Some(raw) => expand_names(raw, self.repository_names()?, "repository")?,
            None => self.repository_names()?,
        };
        let mut output = serde_json::Map::new();
        for name in names {
            let data = self.read_latest(&name)?;
            output.insert(
                name,
                json!({
                    "type": data.repository.repository_type,
                    "settings": data.repository.settings
                }),
            );
        }
        Ok(Value::Object(output))
    }

    pub fn delete_repository(&self, names: &str) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let names = exact_names(names, self.repository_names()?, "repository")?;
        for name in names {
            let dir = self.repository_dir(&name);
            if !dir.exists() {
                return Err(repository_missing(&name));
            }
            fs::remove_dir_all(&dir).map_err(io_error)?;
            sync_directory(self.root.as_ref()).map_err(io_error)?;
        }
        Ok(json!({ "acknowledged": true }))
    }

    pub fn verify_repository(&self, name: &str) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let name = validate_name(name, "repository")?;
        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        self.read_latest(&name)?;
        Ok(json!({
            "nodes": {
                "opensearch-lite-local": {
                    "name": "opensearch-lite-local"
                }
            }
        }))
    }

    pub fn cleanup_repository(&self, name: &str) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let name = validate_name(name, "repository")?;
        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let data = self.read_latest(&name)?;
        let referenced = data
            .snapshots
            .values()
            .map(|snapshot| snapshot.database_blob.clone())
            .collect::<BTreeSet<_>>();
        let blobs_dir = self.repository_dir(&name).join("blobs");
        let mut deleted_blobs = 0u64;
        let mut deleted_bytes = 0u64;
        if blobs_dir.exists() {
            for entry in fs::read_dir(&blobs_dir).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                if !entry.file_type().map_err(io_error)?.is_file() {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().to_string();
                if referenced.contains(&file_name) {
                    continue;
                }
                deleted_bytes += entry.metadata().map_err(io_error)?.len();
                fs::remove_file(entry.path()).map_err(io_error)?;
                deleted_blobs += 1;
            }
            sync_directory(&blobs_dir).map_err(io_error)?;
        }
        Ok(json!({
            "results": {
                "deleted_bytes": deleted_bytes,
                "deleted_blobs": deleted_blobs
            }
        }))
    }

    pub fn create_snapshot(
        &self,
        repository: &str,
        snapshot: &str,
        db: &Database,
        body: Value,
    ) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let repository = validate_name(repository, "repository")?;
        let snapshot = validate_name(snapshot, "snapshot")?;
        let indices = selected_indices(db, &body)?;
        let snapshot_db = database_subset(db, &indices);
        let bytes = serde_json::to_vec_pretty(&snapshot_db).map_err(|error| {
            StoreError::new(
                500,
                "snapshot_exception",
                format!("failed to serialize snapshot: {error}"),
            )
        })?;
        let blob_name = format!("database-{:016x}.json", fnv1a64(&bytes));
        let now = now_millis();

        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let mut data = self.read_latest(&repository)?;
        if data.snapshots.contains_key(&snapshot) {
            return Err(StoreError::new(
                409,
                "snapshot_already_exists_exception",
                format!("snapshot [{snapshot}] already exists in repository [{repository}]"),
            ));
        }
        let blobs_dir = self.repository_dir(&repository).join("blobs");
        fs::create_dir_all(&blobs_dir).map_err(io_error)?;
        let blob_path = blobs_dir.join(&blob_name);
        if !blob_path.exists() {
            write_file_atomically(&blob_path, &bytes).map_err(io_error)?;
        }
        let manifest = SnapshotManifest {
            name: snapshot.clone(),
            uuid: format!(
                "opensearch-lite-{now}-{:016x}",
                fnv1a64(snapshot.as_bytes())
            ),
            state: "SUCCESS".to_string(),
            indices,
            database_blob: blob_name,
            started_at_unix_millis: now,
            completed_at_unix_millis: now,
            index_count: snapshot_db.indexes.len(),
            document_count: snapshot_db.document_count(),
            total_shards: snapshot_db.indexes.values().map(index_total_shards).sum(),
            successful_shards: snapshot_db.indexes.values().map(index_total_shards).sum(),
            failed_shards: 0,
            failures: Vec::new(),
        };
        data.snapshots.insert(snapshot.clone(), manifest.clone());
        self.write_next_generation(&repository, &mut data)?;
        Ok(json!({
            "accepted": true,
            "snapshot": manifest.response_body(&repository)
        }))
    }

    pub fn get_snapshots(&self, repository: &str, names: &str) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let repository = validate_name(repository, "repository")?;
        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let data = self.read_latest(&repository)?;
        let names = expand_names(names, data.snapshots.keys().cloned().collect(), "snapshot")?;
        let snapshots = names
            .into_iter()
            .map(|name| {
                data.snapshots
                    .get(&name)
                    .map(|snapshot| snapshot.response_body(&repository))
                    .ok_or_else(|| snapshot_missing(&repository, &name))
            })
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(json!({ "snapshots": snapshots }))
    }

    pub fn delete_snapshot(&self, repository: &str, names: &str) -> StoreResult<Value> {
        self.ensure_persistent()?;
        let repository = validate_name(repository, "repository")?;
        let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
        let mut data = self.read_latest(&repository)?;
        let names = exact_names(names, data.snapshots.keys().cloned().collect(), "snapshot")?;
        for name in names {
            if data.snapshots.remove(&name).is_none() {
                return Err(snapshot_missing(&repository, &name));
            }
        }
        self.write_next_generation(&repository, &mut data)?;
        Ok(json!({ "acknowledged": true }))
    }

    fn repository_names(&self) -> StoreResult<Vec<String>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(self.root.as_ref()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.file_type().map_err(io_error)?.is_dir() {
                names.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    fn read_latest(&self, repository: &str) -> StoreResult<RepositoryData> {
        self.read_latest_optional(repository)?
            .ok_or_else(|| repository_missing(repository))
    }

    fn read_latest_optional(&self, repository: &str) -> StoreResult<Option<RepositoryData>> {
        let dir = self.repository_dir(repository);
        let latest_path = dir.join("index.latest");
        if !latest_path.exists() {
            return Ok(None);
        }
        let mut raw = String::new();
        OpenOptions::new()
            .read(true)
            .open(&latest_path)
            .and_then(|mut file| file.read_to_string(&mut raw))
            .map_err(io_error)?;
        let generation = raw.trim().parse::<u64>().map_err(|error| {
            StoreError::new(
                500,
                "repository_exception",
                format!(
                    "repository [{repository}] has corrupt index.latest [{}]: {error}",
                    latest_path.display()
                ),
            )
        })?;
        let path = dir.join(generation_file_name(generation));
        let mut contents = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&path)
            .and_then(|mut file| file.read_to_end(&mut contents))
            .map_err(io_error)?;
        serde_json::from_slice(&contents)
            .map(Some)
            .map_err(|error| {
                StoreError::new(
                    500,
                    "repository_exception",
                    format!(
                        "repository [{repository}] has corrupt generation [{}]: {error}",
                        path.display()
                    ),
                )
            })
    }

    fn write_next_generation(
        &self,
        repository: &str,
        data: &mut RepositoryData,
    ) -> StoreResult<()> {
        data.generation += 1;
        let dir = self.repository_dir(repository);
        fs::create_dir_all(&dir).map_err(io_error)?;
        let bytes = serde_json::to_vec_pretty(data).map_err(|error| {
            StoreError::new(
                500,
                "repository_exception",
                format!("failed to serialize repository [{repository}]: {error}"),
            )
        })?;
        write_file_atomically(&dir.join(generation_file_name(data.generation)), &bytes)
            .map_err(io_error)?;
        write_file_atomically(
            &dir.join("index.latest"),
            format!("{}\n", data.generation).as_bytes(),
        )
        .map_err(io_error)?;
        Ok(())
    }

    fn repository_dir(&self, repository: &str) -> PathBuf {
        self.root.join(repository)
    }

    fn ensure_persistent(&self) -> StoreResult<()> {
        if self.ephemeral {
            return Err(StoreError::new(
                501,
                "opensearch_lite_unsupported_api_exception",
                "snapshot repositories are unavailable in --ephemeral mode",
            ));
        }
        Ok(())
    }
}

impl SnapshotManifest {
    fn response_body(&self, repository: &str) -> Value {
        json!({
            "snapshot": self.name,
            "repository": repository,
            "uuid": self.uuid,
            "state": self.state,
            "include_global_state": false,
            "indices": self.indices,
            "start_time_in_millis": self.started_at_unix_millis,
            "end_time_in_millis": self.completed_at_unix_millis,
            "duration_in_millis": self.completed_at_unix_millis.saturating_sub(self.started_at_unix_millis),
            "failures": self.failures,
            "shards": {
                "total": self.total_shards,
                "failed": self.failed_shards,
                "successful": self.successful_shards
            }
        })
    }
}

fn validate_name(raw: &str, kind: &'static str) -> StoreResult<String> {
    let name = raw.trim();
    if name.is_empty()
        || matches!(name, "." | "..")
        || matches!(name, "_all" | "all")
        || (kind == "snapshot" && name.starts_with('_'))
        || name.contains('/')
        || name.contains('\\')
        || name.contains(',')
        || name.contains('*')
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(match kind {
            "repository" => invalid_repository(format!("invalid repository name [{raw}]")),
            _ => StoreError::new(
                400,
                "invalid_snapshot_name_exception",
                format!("invalid snapshot name [{raw}]"),
            ),
        });
    }
    Ok(name.to_string())
}

fn repository_type(body: &Value) -> StoreResult<String> {
    let Some(raw) = body.get("type") else {
        return Ok("fs".to_string());
    };
    let Some(repository_type) = raw.as_str() else {
        return Err(invalid_repository("repository type must be a string"));
    };
    let repository_type = repository_type.trim();
    if repository_type.is_empty() {
        return Err(invalid_repository("repository type must not be empty"));
    }
    if !matches!(repository_type, "fs" | "opensearch_lite") {
        return Err(invalid_repository(format!(
            "repository type [{repository_type}] is not supported by OpenSearch Lite"
        )));
    }
    Ok(repository_type.to_string())
}

fn validate_repository_settings(settings: &Value) -> StoreResult<()> {
    let Some(location) = settings.get("location") else {
        return Ok(());
    };
    let Some(location) = location.as_str() else {
        return Err(invalid_repository(
            "repository settings.location must be a string",
        ));
    };
    if location.trim().is_empty()
        || Path::new(location).is_absolute()
        || location.contains('\\')
        || location
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(invalid_repository(format!(
            "repository settings.location [{location}] must be a safe relative path"
        )));
    }
    Ok(())
}

fn expand_names(raw: &str, available: Vec<String>, kind: &'static str) -> StoreResult<Vec<String>> {
    let requested = raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if requested.is_empty()
        || requested
            .iter()
            .any(|name| matches!(*name, "_all" | "*" | "all"))
    {
        return Ok(available);
    }
    let mut names = Vec::new();
    for name in requested {
        let name = validate_name(name, kind)?;
        if !available.contains(&name) {
            return Err(match kind {
                "repository" => repository_missing(&name),
                _ => snapshot_missing("", &name),
            });
        }
        names.push(name);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn exact_names(raw: &str, available: Vec<String>, kind: &'static str) -> StoreResult<Vec<String>> {
    let requested = raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return Err(match kind {
            "repository" => invalid_repository(format!("invalid repository name [{raw}]")),
            _ => StoreError::new(
                400,
                "invalid_snapshot_name_exception",
                format!("invalid snapshot name [{raw}]"),
            ),
        });
    }
    let mut names = Vec::new();
    for name in requested {
        let name = validate_name(name, kind)?;
        if !available.contains(&name) {
            return Err(match kind {
                "repository" => repository_missing(&name),
                _ => snapshot_missing("", &name),
            });
        }
        names.push(name);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn selected_indices(db: &Database, body: &Value) -> StoreResult<Vec<String>> {
    let raw = match body.get("indices") {
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        Some(_) => {
            return Err(StoreError::new(
                400,
                "parse_exception",
                "snapshot indices must be a string or array of strings",
            ))
        }
        None => vec!["_all".to_string()],
    };
    let mut names = Vec::new();
    for requested in raw {
        if matches!(requested.as_str(), "_all" | "*" | "all") {
            names.extend(db.indexes.keys().cloned());
        } else if requested.contains('*') {
            names.extend(
                db.indexes
                    .keys()
                    .filter(|name| wildcard_matches(&requested, name))
                    .cloned(),
            );
        } else if db.indexes.contains_key(&requested) {
            names.push(requested);
        } else {
            return Err(StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{requested}]"),
            ));
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn database_subset(db: &Database, indices: &[String]) -> Database {
    let keep = indices.iter().cloned().collect::<BTreeSet<_>>();
    let mut snapshot = db.clone();
    snapshot.indexes.retain(|name, _| keep.contains(name));
    snapshot
        .aliases
        .retain(|_, alias| snapshot.indexes.contains_key(&alias.index));
    snapshot
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.split_once('*') {
        Some((prefix, suffix)) => value.starts_with(prefix) && value.ends_with(suffix),
        None => pattern == value,
    }
}

fn index_total_shards(index: &crate::storage::IndexMetadata) -> u64 {
    index
        .settings
        .pointer("/index/number_of_shards")
        .and_then(Value::as_u64)
        .unwrap_or(1)
}

fn generation_file_name(generation: u64) -> String {
    format!("index-{generation:06}.json")
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn invalid_repository(reason: impl Into<String>) -> StoreError {
    StoreError::new(400, "repository_exception", reason.into())
}

fn repository_missing(repository: &str) -> StoreError {
    StoreError::new(
        404,
        "repository_missing_exception",
        format!("repository [{repository}] missing"),
    )
}

fn snapshot_missing(repository: &str, snapshot: &str) -> StoreError {
    let scope = if repository.is_empty() {
        format!("snapshot [{snapshot}] missing")
    } else {
        format!("snapshot [{snapshot}] missing in repository [{repository}]")
    };
    StoreError::new(404, "snapshot_missing_exception", scope)
}

fn lock_error() -> StoreError {
    StoreError::new(
        500,
        "repository_exception",
        "repository catalog lock is poisoned",
    )
}

fn io_error(error: io::Error) -> StoreError {
    StoreError::new(
        500,
        "repository_exception",
        format!("repository filesystem operation failed: {error}"),
    )
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temp_path_for(path);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp_path, path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) => path.with_extension(format!("{extension}.tmp")),
        None => path.with_extension("tmp"),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    OpenOptions::new().read(true).open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
