use serde::{Deserialize, Serialize};

/// `/_api/instance`'s reply — unifies the two near-duplicate ad-hoc structs
/// the single-binary federation module used to define on each side of the
/// same round-trip (one `Serialize`-only for the response, one
/// `Deserialize`-only for the client verifying a peer).
///
/// `total_bans_issued` is a ban-abuse accountability counter (see the
/// federation moderation layer): always recomputed live from the emitting
/// instance's own `Peer` collection (`count where banned == true`), never a
/// stored counter that could drift out of sync with the actual ban list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub total_bans_issued: u64,
}
