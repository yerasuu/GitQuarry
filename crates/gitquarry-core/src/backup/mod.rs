//! Whole-instance backup.
//!
//! Two mechanisms, split by what the data actually is, because the
//! encryption envelope makes them cost wildly different amounts.
//! Everything in a bucket is sealed with the object's own key as AAD, and
//! the data key itself is *wrapped* in `.conf/encryption.json` rather than
//! derived per bucket (`s3db::bootstrap`). So:
//!
//! - **Replica** ([`replica`]): destination key == source key, in a bucket
//!   carrying a verbatim copy of `.conf/encryption.json`. AAD matches, so
//!   every object is a raw ciphertext passthrough — no decrypt, no plaintext
//!   on disk, no CPU per byte — and the destination is itself a bootable
//!   instance (`gitquarry --bucket replica`), which is what disaster
//!   recovery actually needs.
//! - **Snapshot** ([`snapshot`]): destination key differs
//!   (`.conf/backups/<id>/…`), so every object must be decrypted and
//!   re-sealed. Correct for the small mutable config set, where versioned
//!   history is the point and the volume is negligible; wasteful for packs
//!   and LFS objects, which are immutable and get their history for free
//!   under the replica.
//!
//! [`inventory`] is what decides which data goes through which, and is the
//! only place that knows the bucket's full layout.

pub mod dest;
pub mod inventory;
mod pager;
pub mod prune;
pub mod replica;
pub mod report;
pub mod schedule;
pub mod snapshot;
