use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
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
    file.sync_all()?;
    Ok(())
}

pub fn replay(path: &Path, db: &mut Database) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut raw = Vec::new();
    File::open(path)?.read_to_end(&mut raw)?;
    let ends_with_newline = raw.last() == Some(&b'\n');
    let reader = BufReader::new(raw.as_slice());
    let mut lines = reader.lines().peekable();
    let mut pending_transactions: BTreeMap<String, Vec<Mutation>> = BTreeMap::new();
    while let Some(line) = lines.next() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match replay_record(&line, &mut pending_transactions, db) {
            Ok(()) => {}
            Err(error) if lines.peek().is_none() && !ends_with_newline => {
                eprintln!("opensearch-lite ignored torn final mutation record: {error}");
            }
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
    }
    Ok(())
}

fn replay_record(
    line: &str,
    pending_transactions: &mut BTreeMap<String, Vec<Mutation>>,
    db: &mut Database,
) -> serde_json::Result<()> {
    let value: Value = serde_json::from_str(line)?;
    match value.get("transaction").and_then(Value::as_str) {
        Some("begin") => {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mutations = serde_json::from_value::<Vec<Mutation>>(
                value.get("mutations").cloned().unwrap_or_else(|| json!([])),
            )?;
            pending_transactions.insert(id, mutations);
            Ok(())
        }
        Some("commit") => {
            let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
            if let Some(mutations) = pending_transactions.remove(id) {
                for mutation in mutations {
                    mutation.apply_to(db);
                }
            }
            Ok(())
        }
        Some(_) => Ok(()),
        None => {
            let record = serde_json::from_value::<MutationRecord>(value)?;
            record.mutation.apply_to(db);
            Ok(())
        }
    }
}
