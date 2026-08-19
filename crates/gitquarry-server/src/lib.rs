//! Thin lib target over the same modules `main.rs` runs, so `tests/*.rs`
//! integration tests can link against this crate — a plain binary target
//! has no rlib to link against.

pub mod serve;
pub mod setup;
