use std::{collections::BTreeMap, fs, io, path::Path};

use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
use serde::Deserialize;

use crate::security::context::{Principal, Role};

#[derive(Debug, Clone)]
pub struct UserStore {
    users: BTreeMap<String, UserRecord>,
}

#[derive(Debug, Clone)]
struct UserRecord {
    password_hash: String,
    roles: Vec<Role>,
}

#[derive(Debug, Deserialize)]
struct UsersFile {
    users: Vec<UserEntry>,
}

#[derive(Debug, Deserialize)]
struct UserEntry {
    username: String,
    password_hash: String,
    roles: Vec<Role>,
}

impl UserStore {
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let raw = fs::read_to_string(path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to read users file {}: {error}", path.display()),
            )
        })?;
        Self::from_json(&raw).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    pub fn from_json(raw: &str) -> Result<Self, String> {
        let parsed: UsersFile = serde_json::from_str(raw)
            .map_err(|error| format!("failed to parse users file JSON: {error}"))?;
        if parsed.users.is_empty() {
            return Err("users file must contain at least one user".to_string());
        }

        let mut users = BTreeMap::new();
        for entry in parsed.users {
            if entry.username.trim().is_empty() {
                return Err("users file contains a user with an empty username".to_string());
            }
            if entry.roles.is_empty() {
                return Err(format!(
                    "user [{}] must have at least one role",
                    entry.username
                ));
            }
            PasswordHash::new(&entry.password_hash).map_err(|error| {
                format!(
                    "user [{}] has an invalid PHC password hash: {error}",
                    entry.username
                )
            })?;
            if users
                .insert(
                    entry.username.clone(),
                    UserRecord {
                        password_hash: entry.password_hash,
                        roles: entry.roles,
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate user [{}] in users file", entry.username));
            }
        }

        Ok(Self { users })
    }

    pub fn verify(&self, username: &str, password: &str) -> Option<Principal> {
        let record = self.users.get(username)?;
        let parsed_hash = PasswordHash::new(&record.password_hash).ok()?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .ok()?;
        Some(Principal {
            username: username.to_string(),
            roles: record.roles.clone(),
        })
    }
}
