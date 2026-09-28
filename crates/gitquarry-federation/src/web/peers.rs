//! `/_peers`: admin/moderator management of linked instances (federation
//! layer 1) — proxied from core, which already validated
//! session+CSRF+the admin-or-moderator gate before ever reaching here (see
//! `web::session`'s own doc comment). Same add/delete-only shape as the
//! single-binary predecessor: no edit flow, re-pointing an existing peer at
//! a different URL isn't something this design calls for.

use crate::entities::{Peer, PeerAccessLevel};
use crate::state::State_;
use crate::web::session::{escape_html, forwarded_csrf, forwarded_user};
use crate::{access, domain};
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

fn parse_access_level(s: &str) -> Option<PeerAccessLevel> {
    match s {
        "browse_only" => Some(PeerAccessLevel::BrowseOnly),
        "browse_and_fork" => Some(PeerAccessLevel::BrowseAndFork),
        "full" => Some(PeerAccessLevel::Full),
        _ => None,
    }
}

fn access_level_value(level: PeerAccessLevel) -> &'static str {
    match level {
        PeerAccessLevel::BrowseOnly => "browse_only",
        PeerAccessLevel::BrowseAndFork => "browse_and_fork",
        PeerAccessLevel::Full => "full",
    }
}

fn access_level_label(level: PeerAccessLevel) -> &'static str {
    match level {
        PeerAccessLevel::BrowseOnly => "browse-only",
        PeerAccessLevel::BrowseAndFork => "browse + fork",
        PeerAccessLevel::Full => "full (browse + fork + PR-proxy)",
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        "missing X-Gitquarry-User — this page must be reached through gitquarry-core's proxy",
    )
        .into_response()
}

pub(crate) async fn handler_peers_get(State(state): State_, headers: HeaderMap) -> Response {
    if forwarded_user(&headers).is_none() {
        return unauthorized();
    }
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    Html(render_page(&peers, &forwarded_csrf(&headers), None)).into_response()
}

#[derive(Deserialize)]
pub(crate) struct PeersForm {
    action: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    web_base_url: String,
    #[serde(default)]
    git_base_url: String,
    #[serde(default)]
    access_level: String,
    #[serde(default)]
    reason: String,
}

pub(crate) async fn handler_peers_post(
    State(state): State_,
    headers: HeaderMap,
    Form(form): Form<PeersForm>,
) -> Response {
    let Some(actor) = forwarded_user(&headers) else {
        return unauthorized();
    };
    let message = match form.action.as_str() {
        "add_peer" => {
            let name = form.name.trim();
            match domain::peer::add(
                state.backend.as_ref(),
                &state.cipher,
                name,
                form.web_base_url.trim(),
                form.git_base_url.trim(),
                PeerAccessLevel::Full,
                false,
            )
            .await
            {
                Ok(Some(_)) => None,
                Ok(None) => Some(format!("a peer named '{name}' already exists")),
                Err(e) => Some(e.to_string()),
            }
        }
        "delete_peer" => {
            domain::peer::delete(state.backend.as_ref(), &state.cipher, form.name.trim()).await;
            None
        }
        "set_access_level" => match parse_access_level(form.access_level.trim()) {
            None => Some(format!("invalid access level: {}", form.access_level)),
            Some(level) => {
                match domain::peer::set_access_level(state.backend.as_ref(), &state.cipher, form.name.trim(), level)
                    .await
                {
                    Ok(()) => None,
                    Err(e) => Some(e.to_string()),
                }
            }
        },
        "ban_peer" => {
            match domain::peer::ban(
                state.backend.as_ref(),
                &state.cipher,
                form.name.trim(),
                &form.reason,
                &actor,
            )
            .await
            {
                Ok(true) => None,
                Ok(false) => Some(format!("no such linked peer: {}", form.name.trim())),
                Err(e) => Some(e.to_string()),
            }
        }
        "unban_peer" => match domain::peer::unban(state.backend.as_ref(), &state.cipher, form.name.trim()).await {
            Ok(true) => None,
            Ok(false) => Some(format!("'{}' isn't currently banned", form.name.trim())),
            Err(e) => Some(e.to_string()),
        },
        "request_sister" => request_sister(&state, form.name.trim()).await,
        other => Some(format!("unknown action: {other}")),
    };
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    Html(render_page(&peers, &forwarded_csrf(&headers), message)).into_response()
}

/// Section 7's initiator side: marks `sister_requested` locally, then
/// immediately attempts the handshake round-trip. `Ok(true)` back from the
/// other side means it already had `sister_requested` set for us too —
/// both sides confirm in that single round trip. Otherwise this side is
/// left in the "requested, waiting on the other admin" state, same as
/// [`crate::sister::request`]'s own doc comment describes.
async fn request_sister(state: &crate::state::FederationState, name: &str) -> Option<String> {
    if let Err(e) = crate::sister::request(state.backend.as_ref(), &state.cipher, name).await {
        return Some(e.to_string());
    }
    let Some(peer) = access::get_peer(state.backend.as_ref(), &state.cipher, name)
        .await
        .ok()
        .flatten()
    else {
        return Some(format!("no such linked peer: {name}"));
    };
    let my_name = match &state.self_announce {
        Some(info) => info.name.clone(),
        None => {
            return Some(format!(
                "sister requested locally — this instance has no own_web_base_url/own_git_base_url configured, so it can't identify itself to '{name}' yet; the handshake will complete once the sister side requests back and syncs"
            ));
        }
    };
    let secret = access::generate_secret();
    match crate::instance_client::sister_handshake(&peer.web_base_url, &my_name, &secret).await {
        Ok(true) => {
            if let Err(e) =
                crate::sister::confirm_after_handshake(state.backend.as_ref(), &state.cipher, name, &secret).await
            {
                return Some(format!(
                    "handshake confirmed by '{name}' but failed to save locally: {e}"
                ));
            }
            None
        }
        Ok(false) => Some(format!(
            "sister requested — waiting for '{name}' to request sister status back before this confirms"
        )),
        Err(e) => Some(format!(
            "sister requested locally, but the handshake call to '{name}' failed: {e:#}"
        )),
    }
}

fn access_level_select(csrf: &str, name: &str, current: PeerAccessLevel) -> String {
    let mut options = String::new();
    for level in [
        PeerAccessLevel::BrowseOnly,
        PeerAccessLevel::BrowseAndFork,
        PeerAccessLevel::Full,
    ] {
        let selected = if level == current { " selected" } else { "" };
        options.push_str(&format!(
            "<option value=\"{}\"{selected}>{}</option>",
            access_level_value(level),
            access_level_label(level)
        ));
    }
    format!(
        "<form method=\"post\" action=\"/_peers\" style=\"display:inline\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"action\" value=\"set_access_level\">\
         <input type=\"hidden\" name=\"name\" value=\"{name}\">\
         <select name=\"access_level\" onchange=\"this.form.submit()\">{options}</select>\
         </form>"
    )
}

fn ban_control(csrf: &str, name: &str, peer: &Peer) -> String {
    if peer.banned {
        format!(
            "<span class=\"muted\">banned: {}</span><br>\
             <form method=\"post\" action=\"/_peers\" style=\"display:inline\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"action\" value=\"unban_peer\">\
             <input type=\"hidden\" name=\"name\" value=\"{name}\">\
             <button type=\"submit\">unban</button></form>",
            escape_html(peer.ban_reason.as_deref().unwrap_or(""))
        )
    } else {
        format!(
            "<form method=\"post\" action=\"/_peers\" style=\"display:inline\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"action\" value=\"ban_peer\">\
             <input type=\"hidden\" name=\"name\" value=\"{name}\">\
             <input name=\"reason\" placeholder=\"reason (required)\" size=\"20\">\
             <button type=\"submit\">ban</button></form>"
        )
    }
}

/// Section 7's mutual-consent status for one row — only offered at all for
/// a manually-linked peer (`!auto_linked`), matching [`crate::sister::request`]'s
/// own rejection of an auto-linked one.
fn sister_control(csrf: &str, name: &str, peer: &Peer) -> String {
    if peer.auto_linked {
        return "<span class=\"muted\">n/a (auto-linked)</span>".to_string();
    }
    if peer.sister_confirmed {
        return "<span class=\"muted\">sisters</span>".to_string();
    }
    if peer.sister_requested {
        return "<span class=\"muted\">requested, awaiting other side</span>".to_string();
    }
    format!(
        "<form method=\"post\" action=\"/_peers\" style=\"display:inline\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"action\" value=\"request_sister\">\
         <input type=\"hidden\" name=\"name\" value=\"{name}\">\
         <button type=\"submit\">request sister status</button></form>"
    )
}

fn render_page(peers: &[Peer], csrf: &str, message: Option<String>) -> String {
    let csrf = escape_html(csrf);
    let mut rows = String::new();
    if peers.is_empty() {
        rows.push_str("<tr><td colspan=\"8\"><em>no linked instances yet.</em></td></tr>");
    }
    for peer in peers {
        let name = escape_html(&peer.name);
        rows.push_str(&format!(
            "<tr><td>{name}</td><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>\
             <form method=\"post\" action=\"/_peers\" style=\"display:inline\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"action\" value=\"delete_peer\">\
             <input type=\"hidden\" name=\"name\" value=\"{name}\">\
             <button type=\"submit\">delete</button></form></td></tr>",
            escape_html(&peer.web_base_url),
            escape_html(&peer.git_base_url),
            escape_html(&peer.linked_at),
            access_level_select(&csrf, &name, peer.access_level),
            ban_control(&csrf, &name, peer),
            sister_control(&csrf, &name, peer),
        ));
    }
    let banner = message
        .map(|m| format!("<p>{}</p>", escape_html(&m)))
        .unwrap_or_default();
    format!(
        "<p class=\"muted\">stored under <code>.conf/peers/&lt;name&gt;.json</code>.</p>\
         {banner}\
         <table class=\"list\"><tr><th>name</th><th>web url</th><th>git url</th><th>linked</th><th>access level</th><th>ban</th><th>sister</th><th></th></tr>{rows}</table>\
         <h4>add a peer</h4>\
         <form method=\"post\" action=\"/_peers\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"action\" value=\"add_peer\">\
         <p>name <input name=\"name\" size=\"30\"></p>\
         <p>web base url <input name=\"web_base_url\" size=\"50\" placeholder=\"https://otherhost:8080\"></p>\
         <p>git base url <input name=\"git_base_url\" size=\"50\" placeholder=\"ssh://otherhost:2222\"></p>\
         <button type=\"submit\">link</button></form>"
    )
}
