//! `POST /_remote_pulls/comment` — federation layer 4's live PR-proxy
//! comment form, proxied from core (which already validated
//! session+CSRF; see `web::session`'s own doc comment). The read side of
//! this tab (list/detail) stays entirely core-side, calling a peer
//! directly with no federation involvement — only the *write* needs
//! federation, since [`crate::access::PeerCredential`] storage lives here
//! now, not in core.
//!
//! JSON, not a form: core's own `RemoteCommentForm` (the browser-facing
//! one) has a `csrf` field this one deliberately doesn't — core checks
//! that itself before ever proxying, and re-encodes as JSON rather than
//! forwarding the original `x-www-form-urlencoded` body verbatim (see
//! `webui::federation_proxy::proxy_remote_comment`'s own doc comment).

use crate::entities::PeerAccessLevel;
use crate::state::State_;
use crate::web::session::forwarded_user_uid;
use crate::{access, domain, instance_client};
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

#[derive(Serialize)]
struct ApiError<'a> {
    error: &'a str,
}

fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, &ApiError { error: message })
}

#[derive(Deserialize)]
pub(crate) struct RemoteCommentForm {
    peer: String,
    repo: String,
    number: u64,
    #[serde(default)]
    body: String,
}

/// `Full`-tier only (section 6: PR-proxy is the highest capability a
/// linked peer can grant) and requires the viewer to already have a
/// [`crate::access::PeerCredential`] linked for this peer — same "no
/// credential, no write" rule the single-binary predecessor enforced.
pub(crate) async fn handler_comment_post(
    State(state): State_,
    headers: HeaderMap,
    Json(form): Json<RemoteCommentForm>,
) -> Response {
    let Some(user_uid) = forwarded_user_uid(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "missing X-Gitquarry-User-Uid");
    };
    let peer = match access::get_peer(state.backend.as_ref(), &state.cipher, &form.peer).await {
        Ok(Some(peer)) => peer,
        _ => return json_error(StatusCode::NOT_FOUND, "no such linked peer"),
    };
    if peer.banned {
        return json_error(StatusCode::FORBIDDEN, "peer is banned");
    }
    if peer.access_level != PeerAccessLevel::Full {
        return json_error(StatusCode::FORBIDDEN, "peer isn't allowed to receive PR-proxy writes");
    }
    let credential =
        match domain::peer_credential::get(state.backend.as_ref(), &state.cipher, &user_uid, &form.peer).await {
            Ok(Some(credential)) => credential,
            _ => return json_error(StatusCode::FORBIDDEN, "no linked credential for this peer"),
        };
    match instance_client::post_comment(
        &peer.web_base_url,
        &form.repo,
        form.number,
        form.body.trim(),
        &credential.secret,
    )
    .await
    {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}
