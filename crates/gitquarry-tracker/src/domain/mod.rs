pub mod board;
pub mod issue;
pub mod milestone;

use anyhow::{Result, bail};
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

/// One line of free text (a title, a name): trimmed, non-empty, at most
/// `max` bytes and free of control characters, which would otherwise break
/// the single-line places it is rendered in (a heading, a `<option>`, a log
/// line).
pub(crate) fn validate_line(what: &str, raw: &str, max: usize) -> Result<String> {
    let line = raw.trim();
    if line.is_empty() || line.len() > max {
        bail!(DomainError::Invalid(format!(
            "empty or too-long {what} (max {max} bytes)"
        )));
    }
    if line.contains(char::is_control) {
        bail!(DomainError::Invalid(format!("invalid {what} (control characters)")));
    }
    Ok(line.to_string())
}

/// Multi-line free text: at most `max` bytes, and no control characters
/// other than the line breaks and tabs a textarea submits.
pub(crate) fn validate_text(what: &str, text: &str, max: usize) -> Result<()> {
    if text.len() > max {
        bail!(DomainError::Invalid(format!("{what} too long (max {max} bytes)")));
    }
    if text.contains(|c: char| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        bail!(DomainError::Invalid(format!("invalid {what} (control characters)")));
    }
    Ok(())
}
