//! Turns a bucket listing into publish epochs, key by key, before the sweep
//! decides anything. This is the edge where the repo uid, the mirror id and
//! the epoch enter as text: each is validated, and a key is only taken for
//! an epoch's object when rebuilding it through [`epoch_prefix`] gives the
//! same key back, so a key that merely parses (`007`, `+7`, an id in upper
//! case) is reported instead of deleted.

use crate::publish::mapping::epoch_prefix;
use crate::validate::validate_mirror_id;
use gitquarry_core::storage::REPOS_PREFIX;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::validate::validate_repo_uid;
use std::collections::BTreeMap;

/// What an epoch holds, grouped in the order the sweep deletes it.
#[derive(Debug, Default)]
pub(crate) struct EpochKeys {
    pub(crate) idx: Vec<String>,
    pub(crate) packs: Vec<String>,
    pub(crate) inverse: Vec<String>,
    pub(crate) excluded: Vec<String>,
    pub(crate) map: Vec<String>,
    pub(crate) bytes: u64,
    pub(crate) pack_bytes: u64,
}

impl EpochKeys {
    pub(crate) fn objects(&self) -> usize {
        self.batches().iter().map(|(_, keys)| keys.len()).sum()
    }

    /// Deletion order. A pack's `.idx` goes before its `.pack` (the reverse
    /// of the upload), and `map/` goes last: while any of it is left, the
    /// next sweep still finds the epoch and finishes the job.
    pub(crate) fn batches(&self) -> [(&'static str, &[String]); 5] {
        [
            ("pack index", &self.idx),
            ("pack", &self.packs),
            ("inverse", &self.inverse),
            ("excluded", &self.excluded),
            ("map", &self.map),
        ]
    }
}

pub(crate) type MirrorKey = (Uid, String);

#[derive(Debug, Default)]
pub(crate) struct Listing {
    pub(crate) mirrors: BTreeMap<MirrorKey, BTreeMap<u32, EpochKeys>>,
    /// Keys under some `repos/<x>/publish/` that are no epoch object.
    pub(crate) unrecognized: Vec<String>,
}

pub(crate) fn classify(entries: Vec<(String, u64)>) -> Listing {
    let mut listing = Listing::default();
    for (key, size) in entries {
        let Some((uid, rest)) = locate(&key) else {
            continue;
        };
        let Some((uid, mirror_id, epoch, part, tail)) = parse(uid, rest) else {
            listing.unrecognized.push(key);
            continue;
        };
        if key != format!("{}/{tail}", epoch_prefix(&uid, mirror_id, epoch)) {
            listing.unrecognized.push(key);
            continue;
        }
        let keys = listing
            .mirrors
            .entry((uid, mirror_id.to_string()))
            .or_default()
            .entry(epoch)
            .or_default();
        keys.bytes += size;
        match part {
            Part::Idx => keys.idx.push(key),
            Part::Pack => {
                keys.pack_bytes += size;
                keys.packs.push(key);
            }
            Part::Inverse => keys.inverse.push(key),
            Part::Excluded => keys.excluded.push(key),
            Part::Map => keys.map.push(key),
        }
    }
    listing
}

enum Part {
    Idx,
    Pack,
    Inverse,
    Excluded,
    Map,
}

/// The uid segment and what follows `publish/`, for a key under some
/// `repos/<x>/publish/`; `None` for every other key of the bucket.
fn locate(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix(REPOS_PREFIX)?.strip_prefix('/')?;
    let (uid, rest) = rest.split_once('/')?;
    Some((uid, rest.strip_prefix("publish/")?))
}

fn parse<'a>(uid: &str, rest: &'a str) -> Option<(Uid, &'a str, u32, Part, &'a str)> {
    let uid = validate_repo_uid(uid).ok()?;
    let (mirror_id, rest) = rest.split_once('/')?;
    validate_mirror_id(mirror_id).ok()?;
    let (epoch, tail) = rest.split_once('/')?;
    let epoch = epoch.parse().ok()?;
    Some((uid, mirror_id, epoch, part(tail)?, tail))
}

fn part(tail: &str) -> Option<Part> {
    if let Some(name) = tail.strip_prefix("objects/pack/pack-") {
        let (hash, part) = match name.strip_suffix(".pack") {
            Some(hash) => (hash, Part::Pack),
            None => (name.strip_suffix(".idx")?, Part::Idx),
        };
        return (matches!(hash.len(), 40 | 64) && lower_hex(hash)).then_some(part);
    }
    let (dir, shard) = tail.split_once('/')?;
    if shard.len() != 2 || !lower_hex(shard) {
        return None;
    }
    match dir {
        "inverse" => Some(Part::Inverse),
        "excluded" => Some(Part::Excluded),
        "map" => Some(Part::Map),
        _ => None,
    }
}

fn lower_hex(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
