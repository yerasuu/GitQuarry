//! Linked-instance CRUD ([`peers`], federation layer 1), fine-grained
//! personal access tokens this instance mints for others ([`tokens`],
//! federation layer 4), and the reverse — a local user's own credential
//! toward a linked peer ([`peer_tokens`], federation layer 4). Every `pub`
//! item each file declares is re-exported here so call sites use
//! `access::xxx` uniformly.

pub mod peer_tokens;
mod peers;
#[cfg(any(test, feature = "test-util"))]
pub mod tokens;
#[cfg(not(any(test, feature = "test-util")))]
mod tokens;

pub use peer_tokens::*;
pub use peers::*;
pub use tokens::*;
