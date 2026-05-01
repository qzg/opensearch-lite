use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::storage::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mutation {
    CreateIndex {
        name: String,
        settings: Value,
        mappings: Value,
    },
    DeleteIndex {
        name: String,
    },
    PutTemplate {
        name: String,
        index_patterns: Vec<String>,
        template: Value,
        raw: Value,
    },
    DeleteTemplate {
        name: String,
    },
    PutRegistryObject {
        namespace: String,
        name: String,
        raw: Value,
    },
    DeleteRegistryObject {
        namespace: String,
        name: String,
    },
    PutMapping {
        index: String,
        mappings: Value,
    },
    PutSettings {
        index: String,
        settings: Value,
    },
    PutAlias {
        index: String,
        alias: String,
        raw: Value,
    },
    DeleteAlias {
        index: String,
        alias: String,
    },
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
    },
    DeleteDocument {
        index: String,
        id: String,
    },
    RenameDocument {
        index: String,
        old_id: String,
        new_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MutationRecord {
    version: u32,
    #[serde(flatten)]
    mutation: Mutation,
}

pub fn append(path: &Path, mutation: &Mutation) -> io::Result<()> {
    append_mutation_record(path, mutation)
}

pub fn append_transaction_begin(
    path: &Path,
    transaction_id: &str,
    mutations: &[Mutation],
) -> io::Result<()> {
    append_json_record(
        path,
        &json!({
            "version": 1,
            "transaction": "begin",
            "id": transaction_id,
            "mutations": mutations
        }),
    )
}

pub fn append_transaction_commit(path: &Path, transaction_id: &str) -> io::Result<()> {
    append_json_record(
        path,
        &json!({
            "version": 1,
            "transaction": "commit",
            "id": transaction_id
        }),
    )
}

pub fn sync(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    OpenOptions::new().read(true).open(path)?.sync_all()
}

fn append_mutation_record(path: &Path, mutation: &Mutation) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let record = MutationRecord {
        version: 1,
        mutation: mutation.clone(),
    };
    append_json_record(path, &record)
}

fn append_json_record(path: &Path, record: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, record).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn replay(path: &Path, db: &mut Database) -> io::Result<()> {
    replay_validating(path, db, |_| Ok(()))
}

pub fn replay_validating(
    path: &Path,
    db: &mut Database,
    validate: impl FnMut(&Database) -> io::Result<()>,
) -> io::Result<()> {
    replay_from(path, db, ReplayStart::Beginning, validate)
}

pub fn replay_after(
    path: &Path,
    db: &mut Database,
    high_water_transaction_id: Option<&str>,
) -> io::Result<()> {
    replay_after_validating(path, db, high_water_transaction_id, |_| Ok(()))
}

pub fn replay_after_validating(
    path: &Path,
    db: &mut Database,
    high_water_transaction_id: Option<&str>,
    validate: impl FnMut(&Database) -> io::Result<()>,
) -> io::Result<()> {
    match high_water_transaction_id {
        Some(transaction_id) => replay_from(path, db, ReplayStart::After(transaction_id), validate),
        None => replay_validating(path, db, validate),
    }
}

pub fn compact_after(path: &Path, high_water_transaction_id: Option<&str>) -> io::Result<bool> {
    let Some(transaction_id) = high_water_transaction_id else {
        return Ok(false);
    };
    if !path.exists() {
        return Ok(false);
    }
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut offset = 0u64;
    let mut cut_offset = None;
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        if is_commit_record_for(trim_line_ending(&line), transaction_id) {
            cut_offset = Some(offset + bytes as u64);
            break;
        }
        offset += bytes as u64;
    }
    let Some(cut_offset) = cut_offset else {
        return Ok(false);
    };

    let mut source = File::open(path)?;
    source.seek(SeekFrom::Start(cut_offset))?;
    write_compacted_stream_atomically(path, transaction_id, &mut source)?;
    Ok(true)
}

fn replay_from(
    path: &Path,
    db: &mut Database,
    start: ReplayStart<'_>,
    mut validate: impl FnMut(&Database) -> io::Result<()>,
) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut pending_transactions: BTreeMap<String, Vec<Mutation>> = BTreeMap::new();
    let mut waiting_for_high_water = matches!(start, ReplayStart::After(_));
    let mut high_water_found = false;
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let ended_with_newline = line.ends_with('\n');
        let trimmed = trim_line_ending(&line);
        if let ReplayStart::After(transaction_id) = start {
            if waiting_for_high_water {
                if is_commit_record_for(trimmed, transaction_id)
                    || is_compaction_marker_for(trimmed, transaction_id)
                {
                    waiting_for_high_water = false;
                    high_water_found = true;
                }
                continue;
            }
        }
        match replay_record(trimmed, &mut pending_transactions, db, &mut validate) {
            Ok(()) => {}
            Err(error) if !ended_with_newline && reader.fill_buf()?.is_empty() => {
                eprintln!("opensearch-lite ignored torn final mutation record: {error}");
            }
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
    }
    if let ReplayStart::After(transaction_id) = start {
        if !high_water_found {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "snapshot high-water transaction [{transaction_id}] was not present in mutation log"
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ReplayStart<'a> {
    Beginning,
    After(&'a str),
}

fn replay_record(
    line: &str,
    pending_transactions: &mut BTreeMap<String, Vec<Mutation>>,
    db: &mut Database,
    validate: &mut impl FnMut(&Database) -> io::Result<()>,
) -> io::Result<()> {
    let value: Value = serde_json::from_str(line).map_err(invalid_json)?;
    match value.get("transaction").and_then(Value::as_str) {
        Some("begin") => {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mutations = serde_json::from_value::<Vec<Mutation>>(
                value.get("mutations").cloned().unwrap_or_else(|| json!([])),
            )
            .map_err(invalid_json)?;
            pending_transactions.insert(id, mutations);
            Ok(())
        }
        Some("commit") => {
            let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
            if let Some(mutations) = pending_transactions.remove(id) {
                for mutation in mutations {
                    mutation.apply_to(db);
                    validate(db)?;
                }
            }
            Ok(())
        }
        Some(_) => Ok(()),
        None => {
            let record = serde_json::from_value::<MutationRecord>(value).map_err(invalid_json)?;
            record.mutation.apply_to(db);
            validate(db)?;
            Ok(())
        }
    }
}

fn is_commit_record_for(line: &str, transaction_id: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            if value.get("transaction").and_then(Value::as_str) == Some("commit")
                && value.get("id").and_then(Value::as_str) == Some(transaction_id)
            {
                Some(())
            } else {
                None
            }
        })
        .is_some()
}

fn is_compaction_marker_for(line: &str, transaction_id: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            if value.get("transaction").and_then(Value::as_str) == Some("compacted_after")
                && value.get("id").and_then(Value::as_str) == Some(transaction_id)
            {
                Some(())
            } else {
                None
            }
        })
        .is_some()
}

fn trim_line_ending(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn write_compacted_stream_atomically(
    path: &Path,
    transaction_id: &str,
    source: &mut impl Read,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temp_path_for(path);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)?;
    serde_json::to_writer(
        &mut file,
        &json!({
            "version": 1,
            "transaction": "compacted_after",
            "id": transaction_id
        }),
    )
    .map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    io::copy(source, &mut file)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp_path, path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn temp_path_for(path: &Path) -> PathBuf {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) => path.with_extension(format!("{extension}.compact.tmp")),
        None => path.with_extension("compact.tmp"),
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
