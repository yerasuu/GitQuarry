//! DTOs crossing the `gitquarry-webui` ↔ `gitquarry-tracker` loopback
//! boundary (`docs/tracker-binary-split-concept.md`) — issue/board/milestone
//! read shapes plus their write requests, each write request carrying the
//! already-decided actor explicitly since `gitquarry-tracker` itself has no
//! session/CSRF concept of its own (`webui` is the only caller, and it
//! already did the permission check before reaching here).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IssueSummary {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub status: IssueStatus,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub milestone: Option<u64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IssueCommentDto {
    /// The thread storage key — what a follow-up edit/delete addresses this
    /// comment by, same "the thread key is the comment's only id" shape
    /// `gitquarry-tracker`'s own `storage::thread` module already has.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardRefDto {
    Issue(u64),
    MergeRequest(u64),
    Note(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardCardDto {
    pub id: u64,
    pub card: CardRefDto,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardColumnDto {
    pub name: String,
    pub cards: Vec<BoardCardDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardStateDto {
    pub columns: Vec<BoardColumnDto>,
    #[serde(default)]
    pub next_card_id: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MilestoneStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MilestoneSummaryDto {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub due_date: String,
    #[serde(default)]
    pub status: MilestoneStatus,
    #[serde(default)]
    pub open_issues: u32,
    #[serde(default)]
    pub closed_issues: u32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIssueRequest {
    pub title: String,
    pub body: String,
    pub actor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditIssueRequest {
    pub title: String,
    pub body: String,
    pub labels_csv: String,
    pub assignees_csv: String,
    pub milestone: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddIssueCommentRequest {
    pub actor: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditIssueCommentRequest {
    pub actor: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteIssueCommentRequest {
    pub actor: String,
    /// Same "caller already resolved this" posture every field here has —
    /// `webui`'s own `require_manage_permission` check, not re-derived.
    pub is_moderator: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddBoardColumnRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameBoardColumnRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddBoardCardRequest {
    pub card: CardRefDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveBoardCardRequest {
    pub to_column_index: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ReorderDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMilestoneRequest {
    pub title: String,
    pub description: String,
    pub due_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMilestoneRequest {
    pub title: String,
    pub description: String,
    pub due_date: String,
}

/// `POST /_internal/tracker/event` (tracker → core): tracker's own domain
/// events, forwarded so core's single `EventBus` stays the one source of
/// truth for "one event source, many consumers" (webhooks, future
/// automation) — see `docs/webhooks-concept.md`'s own Phase 1 for why that
/// invariant matters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerEvent {
    pub repo_uid: crate::Uid,
    pub kind: TrackerEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackerEventKind {
    IssueOpened { number: u64 },
    IssueClosed { number: u64 },
}

/// `POST /_internal/tracker/merge-request-merged` (core → tracker): pushed
/// right after a merge, carrying enough of the MR's own text for tracker's
/// issue-auto-close parse — core's push includes title/description directly
/// so tracker never needs a round trip back to ask for them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRequestMergedNotification {
    pub repo_uid: crate::Uid,
    pub mr_number: u64,
    pub title: String,
    pub description: String,
}
