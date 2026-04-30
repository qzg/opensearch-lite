pub mod alias;
pub mod field_caps;
pub mod index;
pub mod mapping;
pub mod registry;
pub mod settings;
pub mod template;

pub use crate::storage::{AliasMetadata, Database, IndexMetadata, IndexTemplate, StoredDocument};
