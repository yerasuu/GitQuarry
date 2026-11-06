//! This process's own loopback HTTP server — `listen` in
//! `gitquarry-tracker.toml`. Every route takes `?uid=` (tracker only ever
//! addresses a repo by its uid, never a display name) plus, for a write,
//! an `actor` field carrying the already-decided identity `webui` resolved
//! before ever reaching here — tracker itself has zero session/CSRF concept
//! of its own (`docs/tracker-binary-split-concept.md`'s auth-model decision).
//! Every response here is English-only, machine-to-machine, same posture
//! `gitquarry_core::internal_api`'s own `API_LOCALE` doc comment gives.

use crate::domain::{board, issue, milestone};
use crate::state::{TrackerState, TrackerTarget};
use crate::storage::board::{Board, BoardCard, BoardColumn, CardRef};
use crate::storage::issues::{IssueComment, IssueMeta, IssueStatus};
use crate::storage::milestones::{MilestoneMeta, MilestoneStatus};
use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use gitquarry_wire::tracker::*;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

const API_LOCALE: gitquarry_i18n::Locale = gitquarry_i18n::Locale::En;

async fn handler_health() -> &'static str {
    "ok"
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    #[derive(Serialize)]
    struct ErrorBody {
        error: String,
    }
    (status, Json(ErrorBody { error: message.into() })).into_response()
}

fn target_from(params: &HashMap<String, String>) -> Result<TrackerTarget, Box<Response>> {
    match params.get("uid") {
        Some(uid) => Ok(TrackerTarget {
            uid: gitquarry_wire::Uid::from(uid.clone()),
        }),
        None => Err(Box::new(json_error(StatusCode::BAD_REQUEST, "missing ?uid="))),
    }
}

fn issue_summary(issue: IssueMeta) -> IssueSummary {
    IssueSummary {
        number: issue.number,
        title: issue.title,
        body: issue.body,
        status: match issue.status {
            IssueStatus::Open => gitquarry_wire::tracker::IssueStatus::Open,
            IssueStatus::Closed => gitquarry_wire::tracker::IssueStatus::Closed,
        },
        author: issue.author,
        assignees: issue.assignees,
        labels: issue.labels,
        milestone: issue.milestone,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
    }
}

fn comment_dto(id: String, comment: IssueComment) -> IssueCommentDto {
    IssueCommentDto {
        id,
        author: comment.author,
        body: comment.body,
        created_at: comment.created_at,
    }
}

fn milestone_summary(m: MilestoneMeta) -> MilestoneSummaryDto {
    MilestoneSummaryDto {
        number: m.number,
        title: m.title,
        description: m.description,
        due_date: m.due_date,
        status: match m.status {
            MilestoneStatus::Open => gitquarry_wire::tracker::MilestoneStatus::Open,
            MilestoneStatus::Closed => gitquarry_wire::tracker::MilestoneStatus::Closed,
        },
        open_issues: m.open_issues,
        closed_issues: m.closed_issues,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn card_ref_from(dto: CardRefDto) -> CardRef {
    match dto {
        CardRefDto::Issue(n) => CardRef::Issue(n),
        CardRefDto::MergeRequest(n) => CardRef::MergeRequest(n),
        CardRefDto::Note(t) => CardRef::Note(t),
    }
}

fn card_ref_dto(card: CardRef) -> CardRefDto {
    match card {
        CardRef::Issue(n) => CardRefDto::Issue(n),
        CardRef::MergeRequest(n) => CardRefDto::MergeRequest(n),
        CardRef::Note(t) => CardRefDto::Note(t),
    }
}

fn board_dto(board: Board) -> BoardStateDto {
    BoardStateDto {
        columns: board
            .columns
            .into_iter()
            .map(|c: BoardColumn| BoardColumnDto {
                name: c.name,
                cards: c
                    .cards
                    .into_iter()
                    .map(|card: BoardCard| BoardCardDto {
                        id: card.id,
                        card: card_ref_dto(card.card),
                    })
                    .collect(),
            })
            .collect(),
        next_card_id: board.next_card_id,
    }
}

// ---- issues ----

async fn handler_issue_list(
    State(state): State<Arc<TrackerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    let status_filter = params.get("status").and_then(|s| match s.as_str() {
        "open" => Some(IssueStatus::Open),
        "closed" => Some(IssueStatus::Closed),
        _ => None,
    });
    match issue::list(&state, &target, status_filter).await {
        Ok(issues) => Json(issues.into_iter().map(issue_summary).collect::<Vec<_>>()).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_get(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::get(&state, &target, number).await {
        Ok(Some(issue)) => Json(issue_summary(issue)).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "no such issue"),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_create(
    State(state): State<Arc<TrackerState>>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<CreateIssueRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::create(&state, &target, &req.title, &req.body, &req.actor).await {
        Ok(issue) => Json(issue_summary(issue)).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_edit(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<EditIssueRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    let edit = issue::IssueEdit {
        title: &req.title,
        body: &req.body,
        labels_csv: &req.labels_csv,
        assignees_csv: &req.assignees_csv,
        milestone: req.milestone,
    };
    match issue::edit(&state, &target, number, edit, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_close(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::close(&state, &target, number, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_reopen(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::reopen(&state, &target, number, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_comments_list(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::list_comments(&state, &target, number).await {
        Ok(comments) => Json(
            comments
                .into_iter()
                .map(|(id, c)| comment_dto(id, c))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_comment_add(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<AddIssueCommentRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::add_comment(&state, &target, number, &req.actor, &req.body, API_LOCALE).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_comment_edit(
    State(state): State<Arc<TrackerState>>,
    Path((number, comment_id)): Path<(u64, String)>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<EditIssueCommentRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::edit_comment(&state, &target, number, &comment_id, &req.actor, &req.body, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_issue_comment_delete(
    State(state): State<Arc<TrackerState>>,
    Path((number, comment_id)): Path<(u64, String)>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<DeleteIssueCommentRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match issue::delete_comment(
        &state,
        &target,
        number,
        &comment_id,
        &req.actor,
        req.is_moderator,
        API_LOCALE,
    )
    .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

// ---- board ----

async fn handler_board_get(
    State(state): State<Arc<TrackerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::get(&state, &target).await {
        Ok((board, _etag)) => Json(board_dto(board)).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_add_column(
    State(state): State<Arc<TrackerState>>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<AddBoardColumnRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::add_column(&state, &target, &req.name).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_rename_column(
    State(state): State<Arc<TrackerState>>,
    Path(column_index): Path<usize>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<RenameBoardColumnRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::rename_column(&state, &target, column_index, &req.name, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_remove_column(
    State(state): State<Arc<TrackerState>>,
    Path(column_index): Path<usize>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::remove_column(&state, &target, column_index, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_add_card(
    State(state): State<Arc<TrackerState>>,
    Path(column_index): Path<usize>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<AddBoardCardRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::add_card(&state, &target, column_index, card_ref_from(req.card), API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_move_card(
    State(state): State<Arc<TrackerState>>,
    Path(card_id): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<MoveBoardCardRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::move_card_to_column(&state, &target, card_id, req.to_column_index, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_reorder_card(
    State(state): State<Arc<TrackerState>>,
    Path(card_id): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
    Json(direction): Json<ReorderDirection>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    let result = match direction {
        ReorderDirection::Up => board::move_card_up(&state, &target, card_id, API_LOCALE).await,
        ReorderDirection::Down => board::move_card_down(&state, &target, card_id, API_LOCALE).await,
    };
    match result {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_board_remove_card(
    State(state): State<Arc<TrackerState>>,
    Path(card_id): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match board::remove_card(&state, &target, card_id, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

// ---- milestones ----

async fn handler_milestone_list(
    State(state): State<Arc<TrackerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match milestone::list(&state, &target).await {
        Ok(milestones) => Json(milestones.into_iter().map(milestone_summary).collect::<Vec<_>>()).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_milestone_get(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match milestone::get(&state, &target, number).await {
        Ok(Some(m)) => Json(milestone_summary(m)).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "no such milestone"),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_milestone_create(
    State(state): State<Arc<TrackerState>>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<CreateMilestoneRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match milestone::create(&state, &target, &req.title, &req.description, &req.due_date).await {
        Ok(m) => Json(milestone_summary(m)).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_milestone_edit(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
    Json(req): Json<EditMilestoneRequest>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match milestone::edit(
        &state,
        &target,
        number,
        &req.title,
        &req.description,
        &req.due_date,
        API_LOCALE,
    )
    .await
    {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_milestone_close(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match milestone::close(&state, &target, number, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn handler_milestone_reopen(
    State(state): State<Arc<TrackerState>>,
    Path(number): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target = match target_from(&params) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match milestone::reopen(&state, &target, number, API_LOCALE).await {
        Ok(cas) => Json(cas == s3db::Cas::Ok).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

// ---- core -> tracker push ----

/// `POST /_internal/tracker/merge-request-merged` — core's own MR-merge code
/// path pushes here (`AppContext.tracker`, if configured) right after a
/// merge lands, carrying the merged MR's `title`/`description` directly so
/// this never needs a round trip back to ask for them.
async fn handler_merge_request_merged(
    State(state): State<Arc<TrackerState>>,
    Json(notification): Json<MergeRequestMergedNotification>,
) -> Response {
    let target = TrackerTarget {
        uid: notification.repo_uid,
    };
    match issue::close_referenced_by_merge_request(
        &state,
        &target,
        &notification.title,
        &notification.description,
        API_LOCALE,
    )
    .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

pub fn router(state: Arc<TrackerState>) -> Router {
    Router::new()
        .route("/_internal/health", get(handler_health))
        .route("/_internal/issues", get(handler_issue_list).post(handler_issue_create))
        .route("/_internal/issues/{number}", get(handler_issue_get))
        .route("/_internal/issues/{number}/edit", post(handler_issue_edit))
        .route("/_internal/issues/{number}/close", post(handler_issue_close))
        .route("/_internal/issues/{number}/reopen", post(handler_issue_reopen))
        .route(
            "/_internal/issues/{number}/comments",
            get(handler_issue_comments_list).post(handler_issue_comment_add),
        )
        .route(
            "/_internal/issues/{number}/comments/{comment_id}/edit",
            post(handler_issue_comment_edit),
        )
        .route(
            "/_internal/issues/{number}/comments/{comment_id}/delete",
            post(handler_issue_comment_delete),
        )
        .route("/_internal/board", get(handler_board_get))
        .route("/_internal/board/columns", post(handler_board_add_column))
        .route(
            "/_internal/board/columns/{column_index}/rename",
            post(handler_board_rename_column),
        )
        .route(
            "/_internal/board/columns/{column_index}/remove",
            post(handler_board_remove_column),
        )
        .route(
            "/_internal/board/columns/{column_index}/cards",
            post(handler_board_add_card),
        )
        .route("/_internal/board/cards/{card_id}/move", post(handler_board_move_card))
        .route(
            "/_internal/board/cards/{card_id}/reorder",
            post(handler_board_reorder_card),
        )
        .route(
            "/_internal/board/cards/{card_id}/remove",
            post(handler_board_remove_card),
        )
        .route(
            "/_internal/milestones",
            get(handler_milestone_list).post(handler_milestone_create),
        )
        .route("/_internal/milestones/{number}", get(handler_milestone_get))
        .route("/_internal/milestones/{number}/edit", post(handler_milestone_edit))
        .route("/_internal/milestones/{number}/close", post(handler_milestone_close))
        .route("/_internal/milestones/{number}/reopen", post(handler_milestone_reopen))
        .route(
            "/_internal/tracker/merge-request-merged",
            post(handler_merge_request_merged),
        )
        .with_state(state)
}

pub async fn run_server(bind: &str, state: Arc<TrackerState>, internal_token: String) -> Result<()> {
    let app = router(state).layer(axum::middleware::from_fn(move |req, next| {
        let token = internal_token.clone();
        async move { gitquarry_wire::ipc::require_internal_token(&token, req, next).await }
    }));
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding gitquarry-tracker's own listener to {bind}"))?;
    axum::serve(listener, app)
        .await
        .context("gitquarry-tracker's web server failed")
}
