pub mod board;
pub mod issue;
pub mod milestone;

use std::fmt;

/// A refusal written for the person who made the request, carried inside
/// the `anyhow::Error` a domain function returns. `web` answers these with
/// their message and a 4xx; any error that isn't one of these (storage,
/// decryption, a failed call to core) is internal, and its text never
/// leaves the process.
#[derive(Debug)]
pub enum DomainError {
    /// The request itself is wrong: a bad field, an entity in the wrong
    /// state for the operation, a reference to something that doesn't exist.
    Invalid(String),
    /// The entity the request addresses doesn't exist.
    NotFound(String),
    /// The entity exists but the actor may not do this to it.
    Forbidden(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::Invalid(m) | DomainError::NotFound(m) | DomainError::Forbidden(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DomainError {}
