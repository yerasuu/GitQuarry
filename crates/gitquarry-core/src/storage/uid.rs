//! Re-export of [`gitquarry_wire::Uid`] — see that crate for the type
//! itself. Kept as a re-export (not a `git mv`) so existing call sites
//! (`storage::uid::Uid`) don't need to change now that the type lives in
//! the shared `gitquarry-wire` crate.
pub use gitquarry_wire::Uid;
