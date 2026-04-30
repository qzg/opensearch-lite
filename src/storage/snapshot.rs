use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::storage::Database;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotMetadata {
    pub version: u32,
    pub generation: u64,
    #[serde(default)]
    pub snapshot_file: Option<String>,
    pub created_at_unix_millis: u128,
    pub estimated_stored_bytes: usize,
    pub index_count: usize,
    pub document_count: usize,
    pub template_count: usize,
    #[serde(default)]
    pub registry_object_count: usize,
    pub alias_count: usize,
    pub seq_no: u64,
    #[serde(default)]
    pub last_transaction_id: Option<String>,
    pub log_compacted: bool,
    pub indexes: Vec<SnapshotIndexMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotIndexMetadata {
    pub name: String,
    pub document_count: usize,
    pub tombstone_count: usize,
    pub alias_count: usize,
    pub store_size_bytes: usize,
}

pub fn write_snapshot(path: &Path, db: &Database) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(db).map_err(io::Error::other)?;
    write_file_atomically(path, &bytes)
}

pub fn read_snapshot(path: &Path) -> io::Result<Option<Database>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut contents = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(path)?
        .read_to_end(&mut contents)?;
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn write_metadata(path: &Path, metadata: &SnapshotMetadata) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(metadata).map_err(io::Error::other)?;
    write_file_atomically(path, &bytes)
}

pub fn read_metadata(path: &Path) -> io::Result<Option<SnapshotMetadata>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut contents = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(path)?
        .read_to_end(&mut contents)?;
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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
