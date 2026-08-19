//! Outbound calls to `gitquarry-tracker`'s own loopback API — issue/board/
//! milestone CRUD, replacing what used to be direct in-process
//! `gitquarry_tracker::domain::*` calls before the binary split
//! (`docs/tracker-binary-split-concept.md`). Pulled into one module here
//! since tracker's own surface (issues/board/milestones, ~20 operations) is
//! much bigger than ci's single page.

use anyhow::{Context, Result, bail};
use gitquarry_core::app::TrackerLink;
use gitquarry_wire::Uid;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use gitquarry_wire::tracker::*;
use serde::{Serialize, de::DeserializeOwned};

async fn get<T: DeserializeOwned>(tracker: &TrackerLink, path: &str) -> Result<Option<T>> {
    let url = format!("{}{path}", tracker.base_url.trim_end_matches('/'));
    let resp = tracker
        .http
        .get(&url)
        .signed(&tracker.internal_token)
        .send()
        .await
        .with_context(|| format!("couldn't reach gitquarry-tracker at {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("{}", error_body(resp).await);
    }
    resp.json()
        .await
        .map(Some)
        .with_context(|| format!("gitquarry-tracker's response from {url} wasn't valid JSON"))
}

async fn post<B: Serialize, T: DeserializeOwned>(tracker: &TrackerLink, path: &str, body: &B) -> Result<T> {
    let url = format!("{}{path}", tracker.base_url.trim_end_matches('/'));
    let resp = tracker
        .http
        .post(&url)
        .signed(&tracker.internal_token)
        .json(body)
        .send()
        .await
        .with_context(|| format!("couldn't reach gitquarry-tracker at {url}"))?;
    if !resp.status().is_success() {
        bail!("{}", error_body(resp).await);
    }
    resp.json()
        .await
        .with_context(|| format!("gitquarry-tracker's response from {url} wasn't valid JSON"))
}

/// Same as [`post`], but for a route whose success body is empty
/// (`StatusCode::OK` with no JSON) — `add_comment`/`delete_comment`.
async fn post_empty<B: Serialize>(tracker: &TrackerLink, path: &str, body: &B) -> Result<()> {
    let url = format!("{}{path}", tracker.base_url.trim_end_matches('/'));
    let resp = tracker
        .http
        .post(&url)
        .signed(&tracker.internal_token)
        .json(body)
        .send()
        .await
        .with_context(|| format!("couldn't reach gitquarry-tracker at {url}"))?;
    if !resp.status().is_success() {
        bail!("{}", error_body(resp).await);
    }
    Ok(())
}

async fn error_body(resp: reqwest::Response) -> String {
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        error: String,
    }
    let status = resp.status();
    match resp.json::<ErrorBody>().await {
        Ok(body) => body.error,
        Err(_) => format!("gitquarry-tracker returned {status}"),
    }
}

fn uid_query(uid: &Uid) -> String {
    format!("uid={uid}")
}

// ---- issues ----

pub(crate) async fn list_issues(tracker: &TrackerLink, uid: &Uid, status: Option<&str>) -> Result<Vec<IssueSummary>> {
    let mut path = format!("/_internal/issues?{}", uid_query(uid));
    if let Some(status) = status {
        path.push_str(&format!("&status={status}"));
    }
    Ok(get(tracker, &path).await?.unwrap_or_default())
}

pub(crate) async fn get_issue(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<Option<IssueSummary>> {
    get(tracker, &format!("/_internal/issues/{number}?{}", uid_query(uid))).await
}

pub(crate) async fn create_issue(
    tracker: &TrackerLink,
    uid: &Uid,
    title: &str,
    body: &str,
    actor: &str,
) -> Result<IssueSummary> {
    post(
        tracker,
        &format!("/_internal/issues?{}", uid_query(uid)),
        &CreateIssueRequest {
            title: title.to_string(),
            body: body.to_string(),
            actor: actor.to_string(),
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn edit_issue(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
    title: &str,
    body: &str,
    labels_csv: &str,
    assignees_csv: &str,
    milestone: Option<u64>,
) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/issues/{number}/edit?{}", uid_query(uid)),
        &EditIssueRequest {
            title: title.to_string(),
            body: body.to_string(),
            labels_csv: labels_csv.to_string(),
            assignees_csv: assignees_csv.to_string(),
            milestone,
        },
    )
    .await
}

pub(crate) async fn close_issue(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/issues/{number}/close?{}", uid_query(uid)),
        &(),
    )
    .await
}

pub(crate) async fn reopen_issue(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/issues/{number}/reopen?{}", uid_query(uid)),
        &(),
    )
    .await
}

pub(crate) async fn list_issue_comments(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<Vec<IssueCommentDto>> {
    Ok(get(
        tracker,
        &format!("/_internal/issues/{number}/comments?{}", uid_query(uid)),
    )
    .await?
    .unwrap_or_default())
}

pub(crate) async fn add_issue_comment(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
    actor: &str,
    body: &str,
) -> Result<()> {
    post_empty(
        tracker,
        &format!("/_internal/issues/{number}/comments?{}", uid_query(uid)),
        &AddIssueCommentRequest {
            actor: actor.to_string(),
            body: body.to_string(),
        },
    )
    .await
}

pub(crate) async fn edit_issue_comment(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
    comment_id: &str,
    actor: &str,
    body: &str,
) -> Result<bool> {
    post(
        tracker,
        &format!(
            "/_internal/issues/{number}/comments/{comment_id}/edit?{}",
            uid_query(uid)
        ),
        &EditIssueCommentRequest {
            actor: actor.to_string(),
            body: body.to_string(),
        },
    )
    .await
}

pub(crate) async fn delete_issue_comment(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
    comment_id: &str,
    actor: &str,
    is_moderator: bool,
) -> Result<()> {
    post_empty(
        tracker,
        &format!(
            "/_internal/issues/{number}/comments/{comment_id}/delete?{}",
            uid_query(uid)
        ),
        &DeleteIssueCommentRequest {
            actor: actor.to_string(),
            is_moderator,
        },
    )
    .await
}

// ---- board ----

pub(crate) async fn get_board(tracker: &TrackerLink, uid: &Uid) -> Result<BoardStateDto> {
    Ok(get(tracker, &format!("/_internal/board?{}", uid_query(uid)))
        .await?
        .unwrap_or_default())
}

pub(crate) async fn add_board_column(tracker: &TrackerLink, uid: &Uid, name: &str) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/board/columns?{}", uid_query(uid)),
        &AddBoardColumnRequest { name: name.to_string() },
    )
    .await
}

pub(crate) async fn remove_board_column(tracker: &TrackerLink, uid: &Uid, column_index: usize) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/board/columns/{column_index}/remove?{}", uid_query(uid)),
        &(),
    )
    .await
}

pub(crate) async fn add_board_card(
    tracker: &TrackerLink,
    uid: &Uid,
    column_index: usize,
    card: CardRefDto,
) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/board/columns/{column_index}/cards?{}", uid_query(uid)),
        &AddBoardCardRequest { card },
    )
    .await
}

pub(crate) async fn move_board_card(
    tracker: &TrackerLink,
    uid: &Uid,
    card_id: u64,
    to_column_index: usize,
) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/board/cards/{card_id}/move?{}", uid_query(uid)),
        &MoveBoardCardRequest { to_column_index },
    )
    .await
}

pub(crate) async fn reorder_board_card(
    tracker: &TrackerLink,
    uid: &Uid,
    card_id: u64,
    direction: ReorderDirection,
) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/board/cards/{card_id}/reorder?{}", uid_query(uid)),
        &direction,
    )
    .await
}

pub(crate) async fn remove_board_card(tracker: &TrackerLink, uid: &Uid, card_id: u64) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/board/cards/{card_id}/remove?{}", uid_query(uid)),
        &(),
    )
    .await
}

// ---- milestones ----

pub(crate) async fn list_milestones(tracker: &TrackerLink, uid: &Uid) -> Result<Vec<MilestoneSummaryDto>> {
    Ok(get(tracker, &format!("/_internal/milestones?{}", uid_query(uid)))
        .await?
        .unwrap_or_default())
}

pub(crate) async fn get_milestone(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
) -> Result<Option<MilestoneSummaryDto>> {
    get(tracker, &format!("/_internal/milestones/{number}?{}", uid_query(uid))).await
}

pub(crate) async fn create_milestone(
    tracker: &TrackerLink,
    uid: &Uid,
    title: &str,
    description: &str,
    due_date: &str,
) -> Result<MilestoneSummaryDto> {
    post(
        tracker,
        &format!("/_internal/milestones?{}", uid_query(uid)),
        &CreateMilestoneRequest {
            title: title.to_string(),
            description: description.to_string(),
            due_date: due_date.to_string(),
        },
    )
    .await
}

pub(crate) async fn edit_milestone(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
    title: &str,
    description: &str,
    due_date: &str,
) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/milestones/{number}/edit?{}", uid_query(uid)),
        &EditMilestoneRequest {
            title: title.to_string(),
            description: description.to_string(),
            due_date: due_date.to_string(),
        },
    )
    .await
}

pub(crate) async fn close_milestone(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/milestones/{number}/close?{}", uid_query(uid)),
        &(),
    )
    .await
}

pub(crate) async fn reopen_milestone(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<bool> {
    post(
        tracker,
        &format!("/_internal/milestones/{number}/reopen?{}", uid_query(uid)),
        &(),
    )
    .await
}
