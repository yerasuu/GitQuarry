//! Human-run operator maintenance commands (`gitquarry <command> ...`),
//! as opposed to the unified server (`gitquarry_server::serve`) or the
//! internal `pre-receive-hook` entry point git itself invokes.

pub mod backfill_repo_index;
pub mod backup;
pub mod fsck;
pub mod repack;
pub mod restore;
