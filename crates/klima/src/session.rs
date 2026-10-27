//! The stored MELCloud session. The password is never stored.

use jiff::Timestamp;
use melcloud::{ContextKey, Session};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::files::{read_json, remove, write_json};
use crate::paths::Paths;

#[derive(Serialize, Deserialize)]
struct Stored {
    email: String,
    context_key: String,
    expires_at: Option<Timestamp>,
    use_fahrenheit: bool,
    logged_in_at: Timestamp,
}

/// A logged-in account.
#[derive(Debug, Clone)]
pub struct Account {
    pub email: String,
    pub session: Session,
}

impl Account {
    pub fn expired(&self, now: Timestamp) -> bool {
        self.session.expires_at().is_some_and(|at| at <= now)
    }
}

pub fn save(paths: &Paths, email: &str, session: &Session, now: Timestamp) -> Result<(), AppError> {
    let stored = Stored {
        email: email.to_owned(),
        context_key: session.key().expose().to_owned(),
        expires_at: session.expires_at(),
        use_fahrenheit: session.use_fahrenheit(),
        logged_in_at: now,
    };
    write_json(&paths.session(), &stored)
}

pub fn load(paths: &Paths) -> Result<Option<Account>, AppError> {
    let stored: Option<Stored> = read_json(&paths.session())?;
    Ok(stored.map(|s| Account {
        email: s.email,
        session: Session::new(
            ContextKey::new(s.context_key),
            s.expires_at,
            s.use_fahrenheit,
        ),
    }))
}

/// Forgets the session; `false` when there was none.
pub fn delete(paths: &Paths) -> Result<bool, AppError> {
    remove(&paths.session())
}
