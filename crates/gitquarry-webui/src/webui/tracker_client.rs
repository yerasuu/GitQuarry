//! Outbound calls to `gitquarry-tracker`'s own loopback API — issue/board/
//! milestone CRUD, replacing what used to be direct in-process
//! `gitquarry_tracker::domain::*` calls before the binary split
//! (`docs/tracker-binary-split-concept.md`). Pulled into one module here
//! since tracker's own surface (issues/board/milestones, ~20 operations) is
//! much bigger than ci's single page.

use anyhow::{Context, Result, anyhow, bail};
use gitquarry_core::app::TrackerLink;
use gitquarry_wire::Uid;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use gitquarry_wire::tracker::*;
use reqwest::Url;
use serde::{Serialize, de::DeserializeOwned};

/// `/_internal/<segments>?uid=<uid>` under `tracker.base_url`. Every segment
/// and query value is percent-encoded on its own, so caller data (a comment
/// id, a status filter) can never add a path segment, a query pair or a
/// fragment and point the call at another repo's `uid`.
fn endpoint(tracker: &TrackerLink, uid: &Uid, segments: &[&str]) -> Result<Url> {
    let mut url = Url::parse(&tracker.base_url)
        .with_context(|| format!("gitquarry-tracker base URL {} isn't a valid URL", tracker.base_url))?;
    url.path_segments_mut()
        .map_err(|()| anyhow!("gitquarry-tracker base URL {} can't carry a path", tracker.base_url))?
        .pop_if_empty()
        .push("_internal")
        .extend(segments);
    url.query_pairs_mut().clear().append_pair("uid", uid.as_str());
    Ok(url)
}

async fn get<T: DeserializeOwned>(tracker: &TrackerLink, url: Url) -> Result<Option<T>> {
    let resp = tracker
        .http
        .get(url.clone())
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

async fn post<B: Serialize, T: DeserializeOwned>(tracker: &TrackerLink, url: Url, body: &B) -> Result<T> {
    let resp = tracker
        .http
        .post(url.clone())
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
async fn post_empty<B: Serialize>(tracker: &TrackerLink, url: Url, body: &B) -> Result<()> {
    let resp = tracker
        .http
        .post(url.clone())
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
        // A body that isn't tracker's JSON error shape (a proxy's page, a
        // dropped connection) still has a status worth reporting.
        Err(_) => format!("gitquarry-tracker returned {status}"),
    }
}

// ---- issues ----

pub(crate) async fn list_issues(tracker: &TrackerLink, uid: &Uid, status: Option<&str>) -> Result<Vec<IssueSummary>> {
    let mut url = endpoint(tracker, uid, &["issues"])?;
    if let Some(status) = status {
        url.query_pairs_mut().append_pair("status", status);
    }
    Ok(get(tracker, url).await?.unwrap_or_default())
}

pub(crate) async fn get_issue(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<Option<IssueSummary>> {
    get(tracker, endpoint(tracker, uid, &["issues", &number.to_string()])?).await
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
        endpoint(tracker, uid, &["issues"])?,
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
        endpoint(tracker, uid, &["issues", &number.to_string(), "edit"])?,
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
        endpoint(tracker, uid, &["issues", &number.to_string(), "close"])?,
        &(),
    )
    .await
}

pub(crate) async fn reopen_issue(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<bool> {
    post(
        tracker,
        endpoint(tracker, uid, &["issues", &number.to_string(), "reopen"])?,
        &(),
    )
    .await
}

pub(crate) async fn list_issue_comments(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<Vec<IssueCommentDto>> {
    Ok(get(
        tracker,
        endpoint(tracker, uid, &["issues", &number.to_string(), "comments"])?,
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
        endpoint(tracker, uid, &["issues", &number.to_string(), "comments"])?,
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
        endpoint(
            tracker,
            uid,
            &["issues", &number.to_string(), "comments", comment_id, "edit"],
        )?,
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
        endpoint(
            tracker,
            uid,
            &["issues", &number.to_string(), "comments", comment_id, "delete"],
        )?,
        &DeleteIssueCommentRequest {
            actor: actor.to_string(),
            is_moderator,
        },
    )
    .await
}

// ---- board ----

pub(crate) async fn get_board(tracker: &TrackerLink, uid: &Uid) -> Result<BoardStateDto> {
    Ok(get(tracker, endpoint(tracker, uid, &["board"])?)
        .await?
        .unwrap_or_default())
}

pub(crate) async fn add_board_column(tracker: &TrackerLink, uid: &Uid, name: &str) -> Result<bool> {
    post(
        tracker,
        endpoint(tracker, uid, &["board", "columns"])?,
        &AddBoardColumnRequest { name: name.to_string() },
    )
    .await
}

pub(crate) async fn remove_board_column(tracker: &TrackerLink, uid: &Uid, column_index: usize) -> Result<bool> {
    post(
        tracker,
        endpoint(tracker, uid, &["board", "columns", &column_index.to_string(), "remove"])?,
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
        endpoint(tracker, uid, &["board", "columns", &column_index.to_string(), "cards"])?,
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
        endpoint(tracker, uid, &["board", "cards", &card_id.to_string(), "move"])?,
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
        endpoint(tracker, uid, &["board", "cards", &card_id.to_string(), "reorder"])?,
        &direction,
    )
    .await
}

pub(crate) async fn remove_board_card(tracker: &TrackerLink, uid: &Uid, card_id: u64) -> Result<bool> {
    post(
        tracker,
        endpoint(tracker, uid, &["board", "cards", &card_id.to_string(), "remove"])?,
        &(),
    )
    .await
}

// ---- milestones ----

pub(crate) async fn list_milestones(tracker: &TrackerLink, uid: &Uid) -> Result<Vec<MilestoneSummaryDto>> {
    Ok(get(tracker, endpoint(tracker, uid, &["milestones"])?)
        .await?
        .unwrap_or_default())
}

pub(crate) async fn get_milestone(
    tracker: &TrackerLink,
    uid: &Uid,
    number: u64,
) -> Result<Option<MilestoneSummaryDto>> {
    get(tracker, endpoint(tracker, uid, &["milestones", &number.to_string()])?).await
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
        endpoint(tracker, uid, &["milestones"])?,
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
        endpoint(tracker, uid, &["milestones", &number.to_string(), "edit"])?,
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
        endpoint(tracker, uid, &["milestones", &number.to_string(), "close"])?,
        &(),
    )
    .await
}

pub(crate) async fn reopen_milestone(tracker: &TrackerLink, uid: &Uid, number: u64) -> Result<bool> {
    post(
        tracker,
        endpoint(tracker, uid, &["milestones", &number.to_string(), "reopen"])?,
        &(),
    )
    .await
}
