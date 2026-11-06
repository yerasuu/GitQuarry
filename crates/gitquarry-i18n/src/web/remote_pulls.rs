//! `webui::remote_pulls`'s page copy — federation layer 4's live PR-proxy
//! tab. Status labels/column headers/comment copy are shared with the
//! local merge-request view via [`super::repo::merges`] directly; only the
//! proxy-specific "no access without a credential" copy lives here.

use crate::{Locale, msg};

msg!(no_pulls_or_no_access_hint() {
    Locale::Es => "(sin pull requests, o no tenes acceso para verlos)",
    Locale::En => "(no pull requests, or you don't have access to see them)",
});

msg!(link_credential_prompt_prefix() {
    Locale::Es => "Vincula una credencial hacia esta instancia en ",
    Locale::En => "Link a credential toward this peer at ",
});
msg!(link_credential_prompt_link() { Locale::Es => "tus tokens", Locale::En => "your tokens page" });
msg!(link_credential_prompt_suffix() { Locale::Es => " para comentar aqui.", Locale::En => " to comment here." });

msg!(log_in_to_comment_hint() {
    Locale::Es => "Inicia sesion y vincula una credencial hacia esta instancia para comentar aqui.",
    Locale::En => "Log in, then link a credential toward this peer, to comment here.",
});
