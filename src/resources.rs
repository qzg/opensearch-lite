use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{config::Config, storage::snapshot};

const DEFAULT_RESERVED_OVERHEAD_BYTES: usize = 128 * 1024 * 1024;
const CGROUP_UNBOUNDED_LIMIT: usize = 1usize << 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDiagnostics {
    pub configured_memory_limit_bytes: usize,
    pub detected_container_limit_bytes: Option<usize>,
    pub reserved_overhead_bytes: usize,
    pub effective_safe_memory_bytes: Option<usize>,
    pub snapshot_estimated_stored_bytes: Option<usize>,
    pub snapshot_document_count: Option<usize>,
    pub snapshot_index_count: Option<usize>,
}

impl ResourceDiagnostics {
    pub fn summary(&self) -> String {
        format!(
            "configured data memory budget: {}; detected container memory limit: {}; reserved runtime overhead: {}; effective safe data budget: {}; snapshot estimate: {}; snapshot indexes: {}; snapshot documents: {}",
            bytes_label(self.configured_memory_limit_bytes),
            optional_bytes_label(self.detected_container_limit_bytes),
            bytes_label(self.reserved_overhead_bytes),
            optional_bytes_label(self.effective_safe_memory_bytes),
            optional_bytes_label(self.snapshot_estimated_stored_bytes),
            optional_count_label(self.snapshot_index_count),
            optional_count_label(self.snapshot_document_count)
        )
    }
}

pub fn validate(config: &Config) -> io::Result<ResourceDiagnostics> {
    validate_with_container_limit(config, detect_container_memory_limit())
}

pub fn validate_with_container_limit(
    config: &Config,
    detected_container_limit_bytes: Option<usize>,
) -> io::Result<ResourceDiagnostics> {
    let snapshot_metadata = if config.ephemeral {
        None
    } else {
        snapshot::read_metadata(&snapshot_metadata_path(config))?
    };
    let effective_safe_memory_bytes = detected_container_limit_bytes.map(|limit| {
        limit
            .saturating_sub(DEFAULT_RESERVED_OVERHEAD_BYTES)
            .max(limit / 2)
    });
    let diagnostics = ResourceDiagnostics {
        configured_memory_limit_bytes: config.memory_limit_bytes,
        detected_container_limit_bytes,
        reserved_overhead_bytes: DEFAULT_RESERVED_OVERHEAD_BYTES,
        effective_safe_memory_bytes,
        snapshot_estimated_stored_bytes: snapshot_metadata
            .as_ref()
            .map(|metadata| metadata.estimated_stored_bytes),
        snapshot_document_count: snapshot_metadata
            .as_ref()
            .map(|metadata| metadata.document_count),
        snapshot_index_count: snapshot_metadata
            .as_ref()
            .map(|metadata| metadata.index_count),
    };
    validate_diagnostics(&diagnostics)?;
    Ok(diagnostics)
}

pub fn detect_container_memory_limit() -> Option<usize> {
    detect_container_memory_limit_at(Path::new("/sys/fs/cgroup"))
}

pub fn detect_container_memory_limit_at(cgroup_root: &Path) -> Option<usize> {
    let candidates = [
        cgroup_root.join("memory.max"),
        cgroup_root.join("memory").join("memory.limit_in_bytes"),
        cgroup_root.join("memory.limit_in_bytes"),
    ];
    candidates
        .iter()
        .filter_map(|path| read_cgroup_limit(path).ok().flatten())
        .min()
}

fn validate_diagnostics(diagnostics: &ResourceDiagnostics) -> io::Result<()> {
    if let Some(effective_safe_memory_bytes) = diagnostics.effective_safe_memory_bytes {
        if diagnostics.configured_memory_limit_bytes > effective_safe_memory_bytes {
            return Err(memory_error(format!(
                "configured --memory-limit {} exceeds the safe data budget {} for the detected container memory limit {}",
                bytes_label(diagnostics.configured_memory_limit_bytes),
                bytes_label(effective_safe_memory_bytes),
                optional_bytes_label(diagnostics.detected_container_limit_bytes)
            )));
        }
    }
    if let Some(snapshot_estimated_stored_bytes) = diagnostics.snapshot_estimated_stored_bytes {
        if snapshot_estimated_stored_bytes > diagnostics.configured_memory_limit_bytes {
            return Err(memory_error(format!(
                "snapshot metadata estimates stored data at {}, exceeding configured --memory-limit {}",
                bytes_label(snapshot_estimated_stored_bytes),
                bytes_label(diagnostics.configured_memory_limit_bytes)
            )));
        }
        if let Some(effective_safe_memory_bytes) = diagnostics.effective_safe_memory_bytes {
            if snapshot_estimated_stored_bytes > effective_safe_memory_bytes {
                return Err(memory_error(format!(
                    "snapshot metadata estimates stored data at {}, exceeding the safe data budget {} for this container",
                    bytes_label(snapshot_estimated_stored_bytes),
                    bytes_label(effective_safe_memory_bytes)
                )));
            }
        }
    }
    Ok(())
}

fn snapshot_metadata_path(config: &Config) -> PathBuf {
    config.data_dir.join("snapshot.meta.json")
}

fn read_cgroup_limit(path: &Path) -> io::Result<Option<usize>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let trimmed = contents.trim();
    if trimmed.is_empty() || trimmed == "max" {
        return Ok(None);
    }
    let Ok(limit) = trimmed.parse::<usize>() else {
        return Ok(None);
    };
    if limit == 0 || limit >= CGROUP_UNBOUNDED_LIMIT {
        Ok(None)
    } else {
        Ok(Some(limit))
    }
}

fn memory_error(reason: String) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{reason}. Remediation: increase local or container memory, reduce local data, lower --memory-limit to fit the container, use a smaller or empty --data-dir, or move this workload to full OpenSearch locally, server-hosted OpenSearch, or cloud-hosted OpenSearch."),
    )
}

fn optional_bytes_label(value: Option<usize>) -> String {
    value
        .map(bytes_label)
        .unwrap_or_else(|| "unknown".to_string())
}

fn optional_count_label(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn bytes_label(bytes: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB {
        format!("{:.1}MiB ({bytes} bytes)", bytes as f64 / MIB as f64)
    } else {
        format!("{bytes} bytes")
    }
}
