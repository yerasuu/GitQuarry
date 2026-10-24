//! The gates of `docs/publish-mirror-concept.md` §5, each a function from a
//! finished rewrite to its findings. The preview runs them dry over the
//! whole rewritten history; a snapshot run ([`super::run`]) runs them over
//! the one tree it is about to push. Either way a gate with any finding
//! blocks, and none of them reads or writes anything on the public side.

use super::filter::{ExcludedBy, Filter};
use super::objects::{EMPTY_BLOB, ObjectKind, ObjectReader, Oid, TreeEntry, to_hex};
use super::rewrite::{IdentRole, Rewriter, entry_kind, join};
use super::secrets::{Scan, scan_blob, scan_text};
use super::{Publisher, SecretAllow, UnmappedPolicy};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gate {
    Filter,
    BlobSet,
    License,
    Secrets,
    Identity,
    Host,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Finding {
    /// `.gitquarry/publish.toml` exists in the source ref but couldn't be
    /// used. Blocks rather than being ignored: ignoring it would drop the
    /// exclusions its author meant to add.
    RepoFilterInvalid {
        error: String,
    },
    /// A path the filter excludes is in a tree about to be published.
    ExcludedPathPublished {
        path: String,
        rule: ExcludedBy,
    },
    /// The same content is published at one path and excluded at another,
    /// so the path filter didn't keep it private. A history run leaves a
    /// path empty when that copy lies in history an earlier run of the
    /// epoch handled: `published_path` for content already public,
    /// `excluded_path` for content seen excluded before.
    BlobAlsoExcluded {
        sha: String,
        published_path: String,
        excluded_path: String,
    },
    LicenseMissing,
    /// The mirror declares an SPDX id and nothing in the published tree
    /// states it.
    SpdxNotDeclared {
        expected: String,
    },
    SpdxMismatch {
        expected: String,
        found: String,
    },
    /// `path` is empty for a commit message, `sha` then names the private
    /// commit; otherwise `sha` is the blob's.
    Secret {
        path: String,
        sha: String,
        rule: String,
        line: usize,
    },
    /// A text file over the scan limit: not known to be clean.
    SecretScanSkipped {
        path: String,
        sha: String,
    },
    UnmappedIdentity {
        email: String,
        role: IdentRole,
        commits: usize,
        example_commit: String,
    },
    HostNotAllowed {
        error: String,
    },
    /// A snapshot commit needs an author, and the mirror names none.
    PublisherMissing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateReport {
    pub gate: Gate,
    pub findings: Vec<Finding>,
}

impl GateReport {
    pub fn passed(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Every file, directory and gitlink in the published trees, by path,
/// walked once per (path, tree). Built from the rewrite's *output*, so the
/// audits below see what would be pushed, not what the walk meant to keep.
pub(crate) struct PublishedSet {
    /// Blob id → first published path holding it.
    pub blobs: HashMap<Oid, String>,
    pub filter_findings: Vec<Finding>,
}

pub(crate) fn published_set(rewriter: &Rewriter, filter: &Filter) -> PublishedSet {
    published_set_from(
        rewriter.published.iter().map(|commit| commit.tree),
        &rewriter.public_trees,
        filter,
    )
}

/// [`published_set`] over the trees under `roots` only: a snapshot publishes
/// one tree, not the rewritten history.
pub(crate) fn published_set_from(
    roots: impl IntoIterator<Item = Oid>,
    public_trees: &HashMap<Oid, Vec<TreeEntry>>,
    filter: &Filter,
) -> PublishedSet {
    let mut set = PublishedSet {
        blobs: HashMap::new(),
        filter_findings: Vec::new(),
    };
    let mut seen = HashSet::new();
    let mut stack: Vec<(String, Oid)> = roots.into_iter().map(|tree| (String::new(), tree)).collect();
    while let Some((dir, tree)) = stack.pop() {
        if !seen.insert((dir.clone(), tree)) {
            continue;
        }
        let Some(entries) = public_trees.get(&tree) else {
            continue;
        };
        for entry in entries {
            let path = join(&dir, &entry.name_lossy());
            let kind = entry_kind(entry);
            if let Some(rule) = filter.excludes_published_path(&path, kind) {
                set.filter_findings.push(Finding::ExcludedPathPublished {
                    path: path.clone(),
                    rule,
                });
            }
            if entry.is_tree() {
                stack.push((path, entry.oid));
            } else if !entry.is_gitlink() {
                set.blobs.entry(entry.oid).or_insert(path);
            }
        }
    }
    set
}

/// Filter audit: no published path is one the filter excludes.
pub(crate) fn filter_gate(set: &PublishedSet, repo_filter_error: Option<String>) -> GateReport {
    let mut findings: Vec<Finding> = repo_filter_error
        .map(|error| Finding::RepoFilterInvalid { error })
        .into_iter()
        .collect();
    findings.extend(set.filter_findings.iter().cloned());
    GateReport {
        gate: Gate::Filter,
        findings,
    }
}

/// Blob-set audit: no blob is both published and found at an excluded
/// path. This is the one that catches a secret that once lived at a public
/// path and was later moved under an excluded one (note #46): the path
/// filter alone says nothing about the old copy.
pub(crate) fn blob_set_gate(set: &PublishedSet, rewriter: &Rewriter) -> GateReport {
    blob_set_gate_from(set.blobs.iter().filter_map(|(oid, published_path)| {
        rewriter
            .excluded_blobs
            .get(oid)
            .map(|excluded_path| (*oid, published_path.clone(), excluded_path.clone()))
    }))
}

/// [`blob_set_gate`] over pairs found elsewhere: `(blob, published path,
/// excluded path)`, either path empty when the copy is in a part of the
/// history an earlier run handled. The empty blob is never a finding.
pub(crate) fn blob_set_gate_from(pairs: impl IntoIterator<Item = (Oid, String, String)>) -> GateReport {
    let mut leaked: Vec<(String, Oid, String)> = pairs
        .into_iter()
        .filter(|(oid, _, _)| *oid != EMPTY_BLOB)
        .map(|(oid, published, excluded)| (published, oid, excluded))
        .collect();
    leaked.sort();
    leaked.dedup();
    let findings = leaked
        .into_iter()
        .map(|(published_path, oid, excluded_path)| Finding::BlobAlsoExcluded {
            sha: to_hex(&oid),
            published_path,
            excluded_path,
        })
        .collect();
    GateReport {
        gate: Gate::BlobSet,
        findings,
    }
}

/// Names a licence file at the tree root, the places forges look.
fn is_license_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING"]
        .iter()
        .any(|stem| upper == *stem || upper.starts_with(&format!("{stem}.")) || upper.starts_with(&format!("{stem}-")))
}

/// Licence gate on the public head tree. An SPDX id counts as declared only
/// when the tree says so explicitly — an `SPDX-License-Identifier:` line in
/// a root licence file, or a REUSE-style `LICENSES/<id>.txt` — never by
/// guessing from the licence text.
pub(crate) fn license_gate(
    reader: &mut ObjectReader,
    rewriter: &Rewriter,
    head_tree: Option<Oid>,
    spdx: Option<&str>,
) -> Result<GateReport> {
    let mut findings = Vec::new();
    let root = head_tree.and_then(|t| rewriter.public_trees.get(&t));
    let licenses: Vec<Oid> = root
        .into_iter()
        .flatten()
        .filter(|e| !e.is_tree() && !e.is_gitlink() && is_license_name(&e.name_lossy()))
        .map(|e| e.oid)
        .collect();
    if licenses.is_empty() {
        findings.push(Finding::LicenseMissing);
    }
    if let Some(expected) = spdx {
        let reuse_file = format!("{expected}.txt");
        let has_reuse = root
            .into_iter()
            .flatten()
            .find(|e| e.is_tree() && e.name == b"LICENSES")
            .and_then(|dir| rewriter.public_trees.get(&dir.oid))
            .is_some_and(|files| files.iter().any(|f| f.name == reuse_file.as_bytes()));
        let mut declared = Vec::new();
        for oid in &licenses {
            let body = reader.read_kind(oid, ObjectKind::Blob)?;
            declared.extend(
                String::from_utf8_lossy(&body)
                    .lines()
                    .filter_map(|line| line.split_once("SPDX-License-Identifier:"))
                    .map(|(_, id)| id.trim().to_string()),
            );
        }
        if let Some(found) = declared.iter().find(|id| id.as_str() != expected) {
            findings.push(Finding::SpdxMismatch {
                expected: expected.to_string(),
                found: found.clone(),
            });
        } else if declared.is_empty() && !has_reuse {
            findings.push(Finding::SpdxNotDeclared {
                expected: expected.to_string(),
            });
        }
    }
    Ok(GateReport {
        gate: Gate::License,
        findings,
    })
}

/// Secret scan over every published blob (every one of them entered the
/// public history through some published diff) and every published commit
/// message, each given with the private commit id its allowlist entry is
/// keyed by. Binary blobs aren't scanned; text past the size limit is a
/// finding of its own.
pub(crate) fn secret_gate<'m>(
    reader: &mut ObjectReader,
    set: &PublishedSet,
    messages: impl IntoIterator<Item = (Oid, &'m [u8])>,
    allowlist: &[SecretAllow],
) -> Result<GateReport> {
    let allowed = |path: &str, sha: &str| allowlist.iter().any(|allow| allow.path == path && allow.sha == sha);
    let mut findings = Vec::new();
    let mut blobs: Vec<(&Oid, &String)> = set.blobs.iter().collect();
    blobs.sort_by(|a, b| a.1.cmp(b.1));
    for (oid, path) in blobs {
        let sha = to_hex(oid);
        if allowed(path, &sha) {
            continue;
        }
        match scan_blob(&reader.read_kind(oid, ObjectKind::Blob)?) {
            Scan::Binary => {}
            Scan::TooLarge => findings.push(Finding::SecretScanSkipped {
                path: path.clone(),
                sha,
            }),
            Scan::Hits(hits) => findings.extend(hits.into_iter().map(|hit| Finding::Secret {
                path: path.clone(),
                sha: sha.clone(),
                rule: hit.rule.to_string(),
                line: hit.line,
            })),
        }
    }
    for (private, message) in messages {
        let sha = to_hex(&private);
        if allowed("", &sha) {
            continue;
        }
        findings.extend(
            scan_text(&String::from_utf8_lossy(message))
                .into_iter()
                .map(|hit| Finding::Secret {
                    path: String::new(),
                    sha: sha.clone(),
                    rule: hit.rule.to_string(),
                    line: hit.line,
                }),
        );
    }
    Ok(GateReport {
        gate: Gate::Secrets,
        findings,
    })
}

/// Identity gate: with `unmapped = block`, every author and committer of a
/// published commit resolves in the identity map. Collapsed commits never
/// appear publicly and aren't checked.
pub(crate) fn identity_gate(rewriter: &Rewriter, policy: UnmappedPolicy) -> GateReport {
    identity_gate_over(
        rewriter
            .published
            .iter()
            .map(|commit| (commit.private, commit.unmapped.as_slice())),
        policy,
    )
}

/// [`identity_gate`] over any published objects: each private id with the
/// identities the map didn't cover on it.
pub(crate) fn identity_gate_over<'a>(
    objects: impl IntoIterator<Item = (Oid, &'a [(IdentRole, String)])>,
    policy: UnmappedPolicy,
) -> GateReport {
    let mut findings = Vec::new();
    if policy == UnmappedPolicy::Block {
        // In order of first appearance; a handful of identities at most.
        for (private, unmapped) in objects {
            for (role, email) in unmapped {
                let existing = findings.iter_mut().find_map(|f| match f {
                    Finding::UnmappedIdentity {
                        email: e,
                        role: r,
                        commits,
                        ..
                    } if e == email && r == role => Some(commits),
                    _ => None,
                });
                match existing {
                    Some(commits) => *commits += 1,
                    None => findings.push(Finding::UnmappedIdentity {
                        email: email.clone(),
                        role: *role,
                        commits: 1,
                        example_commit: to_hex(&private),
                    }),
                }
            }
        }
    }
    GateReport {
        gate: Gate::Identity,
        findings,
    }
}

/// Identity gate of a snapshot run: its one commit is authored by the
/// mirror's publisher, never by a private identity, so the only thing left
/// to check is that there is one.
pub(crate) fn publisher_gate(publisher: Option<&Publisher>) -> GateReport {
    GateReport {
        gate: Gate::Identity,
        findings: publisher.map_or_else(|| vec![Finding::PublisherMissing], |_| Vec::new()),
    }
}

/// Host gate: the instance allowlist, the same rule as the save edge,
/// applied again so that narrowing the allowlist reaches stored mirrors.
pub(crate) fn host_gate(remote_url: &str, allowed_hosts: &[String]) -> GateReport {
    let findings = crate::validate::check_host_allowed(remote_url, allowed_hosts)
        .err()
        .map(|e| Finding::HostNotAllowed { error: e.to_string() })
        .into_iter()
        .collect();
    GateReport {
        gate: Gate::Host,
        findings,
    }
}
