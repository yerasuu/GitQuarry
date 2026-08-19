//! Messages reused across 2+ call sites — collapsing a currently-duplicated
//! Spanish literal onto one function here, instead of translating it
//! separately at each site, is the whole point of this module.

use super::{Locale, msg};

msg!(
    /// A `Cas::Conflict` on a bucket-side write: someone else's edit landed
    /// first. Confirmed duplicated verbatim across 9 files before this
    /// (`domain::repo::rename` plus 8 web `Flash::Err` sites) — this is the
    /// first one converted; the rest follow in later commits.
    cas_conflict() {
        Locale::Es => "conflicto guardando (otra edicion concurrente) — recarga y reintenta.",
        Locale::En => "conflict saving (another edit happened concurrently) — reload and retry.",
    }
);

msg!(
    /// A confirm-dialog's dismiss action — the footer "Cancel" button and
    /// the header `×` both abandon the dialog the same way, so they share
    /// this one string rather than minting two near-duplicates.
    cancel_button() {
        Locale::Es => "cancelar",
        Locale::En => "cancel",
    }
);

msg!(
    /// A flash message's `×` — acknowledging/removing a one-line status
    /// notice, not the same action `cancel_button` names (abandoning a
    /// dialog before it does anything) even though both render as `×`.
    dismiss_button() {
        Locale::Es => "cerrar",
        Locale::En => "dismiss",
    }
);

msg!(
    /// `webui::page_chrome::copy_button`'s resting label.
    copy_button() {
        Locale::Es => "copiar",
        Locale::En => "copy",
    }
);

msg!(
    /// `webui::page_chrome::copy_button`'s label for the brief window right
    /// after a successful `navigator.clipboard.writeText` (see `app.js`'s
    /// `[data-copy]` handler) — swapped back to `copy_button` after ~1.2s.
    copied_label() {
        Locale::Es => "copiado",
        Locale::En => "copied",
    }
);

msg!(
    /// A username that doesn't resolve to any existing `User` — confirmed
    /// duplicated verbatim across `auth::domain::group`/`auth::domain::user`/
    /// `domain::repo`/`auth::web::favorites`.
    user_not_found(username: &str) {
        Locale::Es => "usuario '{username}' no existe",
        Locale::En => "user '{username}' doesn't exist",
    }
);

msg!(
    /// A repo name that doesn't resolve to any existing repo — confirmed
    /// duplicated verbatim across `auth::domain::group::parse_team_repos`,
    /// `auth::domain::user::{parse_repos_field,set_repo_perm}`, `webui::fork`,
    /// `webui::repo::releases`, `auth::web::favorites`.
    repo_not_found(repo: &str) {
        Locale::Es => "repo '{repo}' no existe",
        Locale::En => "repo '{repo}' doesn't exist",
    }
);

msg!(
    /// The `other` arm of a form action `match` — a submitted `action`
    /// field that doesn't name any action the handler knows. Confirmed
    /// duplicated verbatim across `auth::web::preferences`/`auth::web::groups`/
    /// `auth::web::favorites`/`auth::web::security`/`auth::web::users`.
    unknown_action(other: &str) {
        Locale::Es => "accion desconocida: {other}",
        Locale::En => "unknown action: {other}",
    }
);

msg!(
    /// Placeholder for a uid-keyed reference (`Group::owners`/
    /// `Team::members`) whose `User` no longer resolves — confirmed
    /// duplicated verbatim across `auth::web::groups` (owner list, team member
    /// list, both via `render_group`/`render_team`).
    user_deleted() {
        Locale::Es => "(usuario eliminado)",
        Locale::En => "(deleted user)",
    }
);

msg!(
    /// Placeholder for a uid-keyed repo reference (`Team::repos`/
    /// `User::repos`) whose `RepoConfig` no longer resolves — confirmed
    /// duplicated verbatim across `auth::web::groups::render_team`,
    /// `auth::web::users::render_user`.
    repo_deleted() {
        Locale::Es => "(repo eliminado)",
        Locale::En => "(deleted repo)",
    }
);

msg!(
    /// A pubkey fingerprint already claimed by a different user — confirmed
    /// duplicated verbatim across `auth::domain::user::{create,add_pubkey}`.
    key_already_associated() {
        Locale::Es => "esa clave ya esta asociada a otro usuario",
        Locale::En => "that key is already associated with another user",
    }
);

msg!(
    /// `auth::domain::user::add_secondary_email`: the email is already
    /// claimed by a different user, either as their primary `email` or one
    /// of their own `secondary_emails`.
    email_already_associated() {
        Locale::Es => "ese email ya esta asociado a otro usuario",
        Locale::En => "that email is already associated with another user",
    }
);

msg!(
    /// A `perm` form field that isn't one of the five
    /// [`crate::auth::access::Permission`] tier names — confirmed
    /// duplicated verbatim across `auth::domain::group::add_team`,
    /// `auth::domain::user::parse_repos_field`.
    invalid_permission(other: &str) {
        Locale::Es => "permiso invalido '{other}' (usar guest, reporter, developer, maintainer u owner)",
        Locale::En => "invalid permission '{other}' (use guest, reporter, developer, maintainer, or owner)",
    }
);

msg!(
    /// `branch` isn't one of the repo's actual branches — confirmed
    /// duplicated verbatim across `domain::repo::{set_default_branch,
    /// set_protected_branches}`.
    not_a_branch(branch: &str) {
        Locale::Es => "'{branch}' no es una rama de este repo",
        Locale::En => "'{branch}' isn't a branch of this repo",
    }
);

msg!(
    /// `tag` isn't one of the repo's actual tags — confirmed duplicated
    /// verbatim across `domain::repo::set_protected_tags`,
    /// `domain::release::create`.
    not_a_tag(tag: &str) {
        Locale::Es => "'{tag}' no es un tag de este repo",
        Locale::En => "'{tag}' isn't a tag of this repo",
    }
);

msg!(
    /// `domain::release::generate_notes`: the commit range since the
    /// previous tag (or, on a first release, up to `tag` itself) is empty —
    /// most likely `tag` was cut with no new commits since whatever tag
    /// came right before it.
    no_commits_since_previous_tag() {
        Locale::Es => "(sin commits nuevos desde el tag anterior)",
        Locale::En => "(no new commits since the previous tag)",
    }
);

msg!(
    /// `tag` already names a ref in this repo — `domain::repo::create_tag`.
    tag_already_exists(tag: &str) {
        Locale::Es => "ya existe una referencia '{tag}' en este repo",
        Locale::En => "a ref named '{tag}' already exists in this repo",
    }
);

msg!(
    /// `domain::repo::delete_tag`: rejects deleting a protected tag, same
    /// invariant `mirror::hook::check_protected` enforces at push time.
    protected_tag_cannot_be_deleted(tag: &str) {
        Locale::Es => "'{tag}' es una etiqueta protegida y no puede eliminarse",
        Locale::En => "'{tag}' is a protected tag and can't be deleted",
    }
);

msg!(
    /// `domain::repo::delete_branch`: unconditional, same "no owner bypass"
    /// shape as [`protected_tag_cannot_be_deleted`] — the nuanced
    /// owners-can-still-force-push exception `mirror::hook::can_push_protected`
    /// applies at the git-push layer doesn't carry over to this button.
    protected_branch_cannot_be_deleted(branch: &str) {
        Locale::Es => "'{branch}' es una rama protegida y no puede eliminarse",
        Locale::En => "'{branch}' is a protected branch and can't be deleted",
    }
);

msg!(
    /// `domain::repo::delete_branch`: the repo's `Manifest::head` can't be
    /// removed out from under it — same "pick a new one first" requirement
    /// GitHub/Forgejo both enforce on their own default branch.
    default_branch_cannot_be_deleted(branch: &str) {
        Locale::Es => "'{branch}' es la rama por defecto y no puede eliminarse",
        Locale::En => "'{branch}' is the default branch and can't be deleted",
    }
);

msg!(
    /// A downloadable `git archive` of a rev, gzipped-tar form — the summary
    /// tab's own `rev` and a release's tag both link here (`repo_link(...,
    /// "snapshot", rev).fmt("tar.gz")`), same message either way.
    snapshot_tar_gz() {
        Locale::Es => "instantanea .tar.gz",
        Locale::En => "snapshot .tar.gz",
    }
);

msg!(
    /// Same as [`snapshot_tar_gz`], zip form.
    snapshot_zip() {
        Locale::Es => "instantanea .zip",
        Locale::En => "snapshot .zip",
    }
);

msg!(
    /// A team name already taken within the same group — single call site
    /// (`auth::domain::group::add_team`), converted alongside its sibling
    /// [`invalid_permission`] check in the same function rather than left
    /// half-converted.
    team_already_exists(name: &str) {
        Locale::Es => "ya existe un equipo '{name}' en este grupo",
        Locale::En => "a team named '{name}' already exists in this group",
    }
);

msg!(
    /// A TOTP code that doesn't verify against the stored secret — confirmed
    /// duplicated verbatim across `auth::domain::user::confirm_totp`,
    /// `auth::web::login`'s post-password TOTP step.
    incorrect_code() {
        Locale::Es => "codigo incorrecto",
        Locale::En => "incorrect code",
    }
);

msg!(
    /// `enable_totp` called while 2FA is already on — single call site,
    /// converted alongside its sibling [`incorrect_code`] check in the
    /// same file rather than left half-converted.
    totp_already_enabled() {
        Locale::Es => "2FA ya esta activo — desactivalo antes de generar un secreto nuevo",
        Locale::En => "2FA is already on — turn it off before generating a new secret",
    }
);

msg!(
    /// `confirm_totp` called with no pending (unconfirmed) secret to check
    /// `code` against — single call site, same file as the two above.
    no_pending_totp_enrollment() {
        Locale::Es => "no hay una inscripcion 2FA pendiente",
        Locale::En => "no pending 2FA enrollment",
    }
);

msg!(
    /// A team name that doesn't exist on a group — confirmed duplicated
    /// verbatim across `auth::domain::group::find_team_mut`'s two callers
    /// (`add_team_member`, `remove_team_member`).
    team_not_found(team: &str) {
        Locale::Es => "equipo '{team}' no existe",
        Locale::En => "team '{team}' doesn't exist",
    }
);

msg!(
    /// A bulk `"repo:perm"` field entry missing its `:` separator — single
    /// call site (`auth::domain::user::parse_repos_field`), converted alongside
    /// its sibling [`invalid_permission`] check in the same function.
    invalid_entry(entry: &str) {
        Locale::Es => "entrada invalida (esperaba 'repo:read' o 'repo:write'): {entry}",
        Locale::En => "invalid entry (expected 'repo:read' or 'repo:write'): {entry}",
    }
);

msg!(
    /// The session's own `User` record vanished between the request's own
    /// login check and a later re-fetch (rare — a concurrent delete) —
    /// confirmed duplicated verbatim across `auth::web::favorites` (x2),
    /// `auth::web::preferences`, `auth::web::security`.
    account_no_longer_exists() {
        Locale::Es => "tu usuario ya no existe",
        Locale::En => "your account no longer exists",
    }
);

msg!(
    /// A destination repo name already claimed by a live repo — confirmed
    /// duplicated verbatim across `webui::fork`, `webui::repo_settings` (move).
    repo_already_exists_at(name: &str) {
        Locale::Es => "ya existe un repo en '{name}'",
        Locale::En => "there's already a repo at '{name}'",
    }
);

msg!(
    /// A username/group name collision at creation time — confirmed
    /// duplicated verbatim across `auth::web::groups`, `auth::web::users`.
    name_already_taken(name: &str) {
        Locale::Es => "'{name}' ya esta en uso (usuario o grupo)",
        Locale::En => "'{name}' is already taken (by a user or a group)",
    }
);

msg!(
    /// A repo-name collision on the *destination* of a move/fork, when the
    /// destination is literally the same repo it started from — confirmed
    /// duplicated verbatim across `webui::fork`, `webui::repo_settings` (move).
    destination_is_same_repo() {
        Locale::Es => "el destino es el mismo repo",
        Locale::En => "the destination is the same repo",
    }
);

msg!(
    /// A username rate-limited by `LoginAttempts` — confirmed duplicated
    /// verbatim across `auth::web::login`'s password-form and TOTP-step handlers.
    too_many_failed_attempts() {
        Locale::Es => "demasiados intentos fallidos para este usuario — esperá unos minutos y volvé a intentar",
        Locale::En => "too many failed attempts for this account — wait a few minutes and try again",
    }
);

msg!(
    /// Wrong username or password on the local login form — single call
    /// site, converted alongside its sibling
    /// [`too_many_failed_attempts`] check in the same handler.
    wrong_username_or_password() {
        Locale::Es => "usuario o password incorrectos",
        Locale::En => "wrong username or password",
    }
);

msg!(
    /// `fork`'s post-copy existence check failed — single call site,
    /// converted since its only caller (`webui::fork`) already resolves a
    /// `Locale`.
    copy_seems_to_have_failed() {
        Locale::Es => "la copia parece haber fallado, reintenta",
        Locale::En => "the copy seems to have failed, try again",
    }
);

msg!(
    /// `domain::repo::create_from_template`: the named source repo exists
    /// but never opted into `RepoConfig::is_template` — use `webui::fork`
    /// instead if the intent is a real fork, not a template.
    repo_not_a_template() {
        Locale::Es => "ese repo no esta marcado como plantilla",
        Locale::En => "that repo isn't marked as a template",
    }
);

msg!(
    /// No `ReleaseMeta` exists for `tag` yet — confirmed duplicated
    /// verbatim across `domain::release::{edit,add_asset,remove_asset}`.
    release_not_found(tag: &str) {
        Locale::Es => "no existe un release para el tag '{tag}'",
        Locale::En => "no release exists for tag '{tag}'",
    }
);

msg!(
    /// `add_asset` called with a filename already attached to the release —
    /// single call site, converted alongside its sibling
    /// [`release_not_found`] check in the same function.
    asset_already_exists(filename: &str) {
        Locale::Es => "ya hay un asset llamado '{filename}' en este release — borralo antes de resubir",
        Locale::En => "there's already an asset named '{filename}' on this release — delete it before re-uploading",
    }
);

msg!(
    /// `remove_asset` called with a filename that isn't attached to the
    /// release — single call site, same file as [`asset_already_exists`].
    asset_not_found(filename: &str) {
        Locale::Es => "no hay un asset llamado '{filename}' en este release",
        Locale::En => "there's no asset named '{filename}' on this release",
    }
);

msg!(
    /// Same fact as [`repo_not_found`] but the "en el bucket" phrasing
    /// three admin-page 404s (`webui::repo_settings`, `webui::repo::mod`'s
    /// summary/settings-tab dispatch) use verbatim — kept as its own key
    /// rather than merged into `repo_not_found`: the extra words are a
    /// deliberate wording choice for those specific pages, not incidental
    /// drift to collapse away.
    repo_not_found_in_bucket(repo: &str) {
        Locale::Es => "repo '{repo}' no existe en el bucket",
        Locale::En => "repo '{repo}' doesn't exist in the bucket",
    }
);

msg!(
    /// A raw-export route (`webui::raw`) hitting a repo with nothing
    /// pushed yet: there's no HEAD to default to, so it 404s instead of
    /// the HTML views' `empty_repo_body` fallback — a raw response has
    /// nothing sensible to render for "empty".
    repo_has_no_commits(repo: &str) {
        Locale::Es => "repo '{repo}' no tiene commits todavía",
        Locale::En => "repo '{repo}' has no commits yet",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`'s `add_group` permission gate.
    only_admin_creates_groups() {
        Locale::Es => "solo un admin puede crear grupos",
        Locale::En => "only an admin can create groups",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`'s `delete_group` permission gate.
    only_admin_deletes_groups() {
        Locale::Es => "solo un admin puede borrar grupos",
        Locale::En => "only an admin can delete groups",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`'s `set_storage_quota`
    /// permission gate — same reasoning `auth::web::users`'s whole route
    /// being admin-only already has: an owner shouldn't self-raise their
    /// own allowance.
    only_admin_sets_storage_quota() {
        Locale::Es => "solo un admin puede fijar el limite de almacenamiento",
        Locale::En => "only an admin can set the storage quota",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`: `me` isn't an owner of the named
    /// group (nor a global admin) for an action that requires managing it.
    you_dont_manage_group(group: &str) {
        Locale::Es => "no administras el grupo '{group}'",
        Locale::En => "you don't manage the group '{group}'",
    }
);

msg!(
    /// A group name that doesn't resolve to any existing `Group` —
    /// distinct from `user_not_found`/`repo_not_found`, only one call
    /// site (`auth::web::groups`) so no duplicate to collapse here.
    group_not_found(group: &str) {
        Locale::Es => "grupo '{group}' no existe",
        Locale::En => "group '{group}' doesn't exist",
    }
);

msg!(
    /// `webui::fork::handler_fork_post`: the target namespace isn't the
    /// user's own username, nor a group they own/admin.
    invalid_namespace(namespace: &str) {
        Locale::Es => "namespace invalido: '{namespace}' no es tuyo ni un grupo que administres",
        Locale::En => "invalid namespace: '{namespace}' isn't yours nor a group you manage",
    }
);

msg!(
    /// `auth::web::favorites::handler_favorites_page`: the target user hasn't
    /// opted into `favorites_public`, so their starred list is private.
    favorites_not_public(username: &str) {
        Locale::Es => "los favoritos de '{username}' no son publicos",
        Locale::En => "'{username}''s favorites aren't public",
    }
);

msg!(
    /// `auth::web::security`'s `disable_totp` re-auth gate: neither the account
    /// password nor a live TOTP code matched. Distinct from
    /// `wrong_username_or_password` (login form) — different wording,
    /// different call site, not a verbatim duplicate.
    wrong_password_or_code() {
        Locale::Es => "password o codigo incorrecto",
        Locale::En => "wrong password or code",
    }
);

msg!(
    /// `settings::web::require_admin`: a logged-in non-admin hit an
    /// admin-only page (`/_settings`, `/_users`, a repo's settings tab).
    account_is_not_admin() {
        Locale::Es => "tu usuario no es admin",
        Locale::En => "your account isn't an admin",
    }
);

msg!(
    /// `auth::domain::user::set_own_email` refusing while OIDC is
    /// configured — the address is a login identity there, so only an admin
    /// assigns it.
    primary_email_is_admin_managed() {
        Locale::Es => "con OIDC configurado, el correo principal solo lo puede cambiar un admin",
        Locale::En => "with OIDC configured, only an admin can change the primary email",
    }
);

msg!(
    /// `settings::web::handler_settings_post`'s OIDC "all 4 fields or none"
    /// validation.
    oidc_fields_go_together() {
        Locale::Es => "OIDC: los 4 campos (issuer/client_id/client_secret/redirect_base_url) van juntos o ninguno",
        Locale::En => "OIDC: the 4 fields (issuer/client_id/client_secret/redirect_base_url) go together or none at all",
    }
);

msg!(
    /// `auth::web::users::apply_users_action`'s `add_repo_perm`: the `repo` field
    /// was submitted empty.
    missing_repo() {
        Locale::Es => "falta el repo",
        Locale::En => "missing repo",
    }
);

msg!(
    /// `auth::web::preferences::handler_preferences_post`'s `set_text_size_pref`:
    /// `TextSize::parse` rejected the submitted value.
    invalid_size(value: &str) {
        Locale::Es => "tamaño invalido: {value}",
        Locale::En => "invalid size: {value}",
    }
);

msg!(
    /// `auth::web::preferences::handler_preferences_post`'s `set_theme_pref`: not
    /// one of the 3 accepted values (`""`/`"dark"`/`"light"`).
    invalid_theme(value: &str) {
        Locale::Es => "tema invalido: {value}",
        Locale::En => "invalid theme: {value}",
    }
);

msg!(
    /// `auth::web::preferences::handler_preferences_post`'s `set_density_pref`:
    /// not one of the 3 accepted values (`""`/`"dense"`/`"airy"`).
    invalid_density(value: &str) {
        Locale::Es => "densidad invalida: {value}",
        Locale::En => "invalid density: {value}",
    }
);

msg!(
    /// `webui::require_csrf`: the submitted `csrf` field didn't match this
    /// session's cookie (missing, expired, or a stale form). No `ctx` in
    /// scope here, so this only gets the cookie-override/built-in-default
    /// tiers of locale resolution, not the account-preference middle one.
    invalid_csrf() {
        Locale::Es => "token CSRF invalido o vencido — recarga la pagina y reintenta",
        Locale::En => "invalid or expired CSRF token — reload the page and try again",
    }
);

msg!(
    /// `webui::require_own_profile`: the `{username}` URL segment of a
    /// self-service page doesn't match the logged-in session's own account.
    only_your_own_account() {
        Locale::Es => "solo podes acceder a tu propia cuenta",
        Locale::En => "you can only access your own account",
    }
);

msg!(
    /// `webui::export::plain`: the raw blob view's own cap, bigger than the
    /// syntax-highlighted HTML view's `MAX_BLOB_BYTES` (see the constant's
    /// own doc comment) but still bounded.
    file_too_large(size: u64, limit: u64) {
        Locale::Es => "archivo de {size} bytes, demasiado grande para servir crudo (limite {limit})",
        Locale::En => "file is {size} bytes, too large to serve raw (limit {limit})",
    }
);

msg!(
    /// `webui::export::snapshot`'s `fmt` arm — anything other than
    /// `tar.gz`/`zip`.
    invalid_snapshot_format(other: &str) {
        Locale::Es => "formato de snapshot invalido: {other} (usar tar.gz o zip)",
        Locale::En => "invalid snapshot format: {other} (use tar.gz or zip)",
    }
);

msg!(
    /// `webui::export::release_asset`: no asset by that filename on that
    /// release. Distinct from `asset_not_found` — different wording (names
    /// the tag too) and this is the only call site for this exact phrasing.
    no_asset_on_release(filename: &str, tag: &str) {
        Locale::Es => "no hay un asset '{filename}' en el release '{tag}'",
        Locale::En => "there's no asset '{filename}' on release '{tag}'",
    }
);

msg!(
    /// `webui::export::release_asset`: `tempfile::tempdir()` itself failed —
    /// distinct from `git::lfs`'s/`commands::repack`'s own tempdir-context
    /// messages, which stay hardcoded Spanish (no web `Locale` in scope
    /// there).
    creating_temp_dir_for_asset() {
        Locale::Es => "creando directorio temporal para descarga de asset",
        Locale::En => "creating a scratch directory for the asset download",
    }
);

msg!(
    /// `webui::repo_settings::handler_repo_conf_post`'s `delete_repo`
    /// confirmation: the typed name doesn't match the repo's own.
    type_name_to_confirm_deletion(name: &str) {
        Locale::Es => "escribi '{name}' exacto para confirmar el borrado",
        Locale::En => "type '{name}' exactly to confirm deletion",
    }
);

msg!(
    /// `webui::repo_settings::move_repo`'s confirmation: same shape as
    /// `type_name_to_confirm_deletion` but for a move, not a
    /// delete — kept as its own key rather than a shared "action" param so
    /// each locale's own `format!` stays independent, per this module's
    /// own doc comment.
    type_name_to_confirm_move(name: &str) {
        Locale::Es => "escribi '{name}' exacto para confirmar el movimiento",
        Locale::En => "type '{name}' exactly to confirm the move",
    }
);

msg!(
    /// `webui::repo_settings::move_repo`: `new_namespace` doesn't resolve to
    /// any existing user or group.
    namespace_not_found(namespace: &str) {
        Locale::Es => "namespace invalido: '{namespace}' no es un usuario ni un grupo existente",
        Locale::En => "invalid namespace: '{namespace}' isn't an existing user or group",
    }
);

msg!(
    /// `webui::repo::releases::require_write_target`: `me`'s effective
    /// permission on the repo is below `Write` — shared by every release
    /// mutation (create/edit/delete/asset upload/asset delete), all of
    /// which funnel through this one check.
    write_permission_required() {
        Locale::Es => "necesitas permiso de escritura sobre este repo",
        Locale::En => "you need write permission on this repo",
    }
);

msg!(
    /// `webui::repo_settings::require_repo_authority`: the actor has some
    /// permission on this repo but not enough for the action attempted
    /// (below `Maintainer` for settings, below `Owner` for delete/transfer/
    /// membership) — distinct from `account_is_not_admin`, since this actor
    /// isn't necessarily lacking global admin, just this repo's tier.
    insufficient_repo_permission() {
        Locale::Es => "tu permiso sobre este repo no alcanza para esta accion",
        Locale::En => "your permission on this repo isn't enough for this action",
    }
);

msg!(
    /// `webui::repo::releases::handler_create_post`'s `Cas::Conflict` — a
    /// release for that tag already exists (raced with another create).
    /// Distinct from `cas_conflict` (a concurrent-edit conflict on an
    /// existing object): this is "already exists", not "changed under you".
    release_already_exists_for_tag(tag: &str) {
        Locale::Es => "ya existe un release para el tag '{tag}'",
        Locale::En => "a release for tag '{tag}' already exists",
    }
);

msg!(
    /// `webui::repo::releases::handler_upload_asset_post`: the multipart body
    /// itself failed to parse (malformed request, not a field-level error).
    invalid_multipart(error: impl std::fmt::Display) {
        Locale::Es => "multipart invalido: {error}",
        Locale::En => "invalid multipart body: {error}",
    }
);

msg!(
    /// `webui::repo::releases::handler_upload_asset_post`: the scratch
    /// tempfile for the uploaded asset couldn't be created.
    creating_temp_file(error: impl std::fmt::Display) {
        Locale::Es => "creando temporal: {error}",
        Locale::En => "creating scratch file: {error}",
    }
);

msg!(
    /// `webui::repo::releases::handler_upload_asset_post`: reading the next
    /// chunk of the uploaded `file` field failed mid-stream.
    reading_file(error: impl std::fmt::Display) {
        Locale::Es => "leyendo archivo: {error}",
        Locale::En => "reading file: {error}",
    }
);

msg!(
    /// `webui::repo::releases::handler_upload_asset_post`: the uploaded asset
    /// exceeded `MAX_ASSET_UPLOAD_BYTES` mid-stream.
    asset_too_large(limit: u64) {
        Locale::Es => "archivo demasiado grande (max {limit} bytes)",
        Locale::En => "file too large (max {limit} bytes)",
    }
);

msg!(
    /// `webui::repo::releases::handler_upload_asset_post`: the form had no
    /// `file` multipart field at all.
    missing_file_to_upload() {
        Locale::Es => "falta el archivo a subir",
        Locale::En => "missing file to upload",
    }
);

msg!(
    /// `auth::web::login`: a session/pending-totp/oidc-flow cookie's `HeaderValue`
    /// failed to parse — confirmed duplicated verbatim across 5 sites
    /// (`handler_login_local` x2, `handler_login_totp_post` x2,
    /// `handler_login_oidc`, `handler_callback` x2 — 7 total call sites).
    building_set_cookie() {
        Locale::Es => "armando Set-Cookie",
        Locale::En => "building Set-Cookie",
    }
);

msg!(
    /// `auth::web::login::{handler_login_oidc,handler_callback}`: `ctx.oidc` is
    /// `None` but the route was hit anyway (stale link, or OIDC got
    /// disabled after the page rendered). Distinct from `auth::oidc`'s own
    /// identical-text startup error (that one has no `Locale` — discovery
    /// runs once at boot, not from a request).
    oidc_not_configured() {
        Locale::Es => "OIDC no esta configurado en este server",
        Locale::En => "OIDC isn't configured on this server",
    }
);

msg!(
    /// `auth::web::login::handler_login_totp_post`: no `pending-totp` cookie at
    /// all (expired, or navigated to `/_login_totp` directly).
    no_totp_login_in_progress() {
        Locale::Es => "no hay un login en curso (cookie vencida) — volve a /_login",
        Locale::En => "no login in progress (expired cookie) — go back to /_login",
    }
);

msg!(
    /// `auth::web::login::handler_login_totp_post`: the `pending-totp` cookie's
    /// base64 didn't decode.
    invalid_totp_cookie() {
        Locale::Es => "cookie invalida",
        Locale::En => "invalid cookie",
    }
);

msg!(
    /// `auth::web::login::handler_login_totp_post`: the decrypted `pending-totp`
    /// cookie didn't deserialize.
    corrupt_totp_cookie() {
        Locale::Es => "cookie corrupta",
        Locale::En => "corrupt cookie",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP itself reported an error
    /// (denied consent, misconfiguration, etc.) via `?error=`/
    /// `?error_description=`.
    oidc_provider_rejected_login(err: &str, desc: &str) {
        Locale::Es => "el provider OIDC rechazo el login: {err} {desc}",
        Locale::En => "the OIDC provider rejected the login: {err} {desc}",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP redirected back with no
    /// `?code=` param.
    missing_code_param() {
        Locale::Es => "falta ?code=",
        Locale::En => "missing ?code=",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP redirected back with no
    /// `?state=` param.
    missing_state_param() {
        Locale::Es => "falta ?state=",
        Locale::En => "missing ?state=",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: no `oidc-flow` cookie at all
    /// (expired, or the callback was hit without starting at
    /// `/_login_oidc` first).
    no_oidc_login_in_progress() {
        Locale::Es => "no hay un login OIDC en curso (cookie de flujo ausente o vencida)",
        Locale::En => "no OIDC login in progress (missing or expired flow cookie)",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the `oidc-flow` cookie's base64
    /// didn't decode.
    invalid_oidc_flow_cookie() {
        Locale::Es => "cookie de flujo OIDC invalida",
        Locale::En => "invalid OIDC flow cookie",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the decrypted `oidc-flow` cookie
    /// didn't deserialize.
    corrupt_oidc_flow_cookie() {
        Locale::Es => "cookie de flujo OIDC corrupta",
        Locale::En => "corrupt OIDC flow cookie",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the flow cookie's own `csrf` value
    /// doesn't match the IdP's returned `state`.
    oidc_state_mismatch() {
        Locale::Es => "state no coincide (posible CSRF, o el login tardo demasiado)",
        Locale::En => "state mismatch (possible CSRF, or the login took too long)",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: building the HTTP client for the
    /// code/token exchange failed. Same text as `auth::oidc::discover`'s
    /// startup-time error — kept separate since that one has no `Locale`
    /// (runs once at boot, not from a request).
    failed_to_build_oidc_client() {
        Locale::Es => "no pude armar el cliente HTTP para OIDC",
        Locale::En => "couldn't build the HTTP client for OIDC",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: exchanging the authorization code
    /// for a token failed against the IdP.
    code_token_exchange_failed() {
        Locale::Es => "intercambio de code por token fallo",
        Locale::En => "exchanging the code for a token failed",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the token response had no
    /// `id_token` at all.
    oidc_missing_id_token() {
        Locale::Es => "el provider OIDC no devolvio id_token",
        Locale::En => "the OIDC provider didn't return an id_token",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the `id_token`'s signature/claims
    /// didn't verify.
    failed_to_verify_id_token() {
        Locale::Es => "no pude verificar el id_token",
        Locale::En => "couldn't verify the id_token",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP explicitly asserted
    /// `email_verified: false`.
    oidc_email_not_verified() {
        Locale::Es => "el provider OIDC marco este email como no verificado",
        Locale::En => "the OIDC provider marked this email as unverified",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the verified `id_token` had no
    /// `email` claim (the provider needs the `email` scope requested).
    id_token_missing_email_claim() {
        Locale::Es => "el id_token no tiene claim 'email' (agregar scope 'email' del lado del provider)",
        Locale::En => "the id_token has no 'email' claim (add the 'email' scope on the provider side)",
    }
);

msg!(
    /// `webui::repo::require_sha`: the `?sha=` query param a `commit`/
    /// `patch`/`rawdiff` view needs wasn't present at all.
    missing_sha_param() {
        Locale::Es => "falta ?sha=",
        Locale::En => "missing ?sha=",
    }
);

msg!(
    /// `webui::repo::require_tag`: the `?tag=` query param a `release`/
    /// `release_asset` view needs wasn't present at all.
    missing_tag_param() {
        Locale::Es => "falta ?tag=",
        Locale::En => "missing ?tag=",
    }
);

msg!(
    /// `webui::repo::require_number`: the `?number=` query param the `pull`
    /// view needs wasn't present, or wasn't a valid PR number.
    missing_number_param() {
        Locale::Es => "falta o es invalido ?number=",
        Locale::En => "missing or invalid ?number=",
    }
);

msg!(
    /// `webui::repo::require_filename`: the `?filename=` query param a
    /// `release_asset` view needs wasn't present at all.
    missing_filename_param() {
        Locale::Es => "falta ?filename=",
        Locale::En => "missing ?filename=",
    }
);

msg!(
    /// `webui::repo::handler_repo`'s `compare` view: the `?base=` query
    /// param wasn't present at all.
    missing_base_param() {
        Locale::Es => "falta ?base=",
        Locale::En => "missing ?base=",
    }
);

msg!(
    /// `webui::repo::handler_repo`'s `compare` view: the `?head=` query
    /// param wasn't present at all.
    missing_head_param() {
        Locale::Es => "falta ?head=",
        Locale::En => "missing ?head=",
    }
);

msg!(
    /// `webui::repo::handler_repo`'s `blame` view: `?path=` was empty — blame
    /// is inherently per-file, unlike `log`'s optional path filter.
    blame_requires_path() {
        Locale::Es => "blame necesita ?path=",
        Locale::En => "blame needs ?path=",
    }
);

msg!(
    /// `webui::repo::handler_repo`'s final `match view` arm — a `?view=`
    /// value that doesn't name any view this handler knows.
    unknown_view(other: &str) {
        Locale::Es => "view desconocida: {other}",
        Locale::En => "unknown view: {other}",
    }
);

msg!(
    /// No `PullRequestMeta` exists for `number` in this repo — confirmed
    /// duplicated across `domain::pull_request::{get, transition, add_comment}`.
    pull_request_not_found(number: u64) {
        Locale::Es => "no existe el pull request #{number}",
        Locale::En => "no pull request #{number} exists",
    }
);

msg!(
    /// `domain::pull_request::transition`: the PR's current status doesn't
    /// allow the requested move (e.g. reopening a merged PR, closing an
    /// already-closed one).
    pull_request_wrong_status() {
        Locale::Es => "este pull request no esta en un estado valido para esa accion",
        Locale::En => "this pull request isn't in a valid status for that action",
    }
);

msg!(
    /// `domain::pull_request::create`: a cross-repo PR's `source_repo` isn't
    /// actually a fork of the target repo (`RepoConfig::forked_from` doesn't
    /// point back at it) — cross-repo PRs are only allowed fork -> upstream,
    /// never between two unrelated repos.
    source_repo_not_a_fork() {
        Locale::Es => "el repo origen no es un fork de este repo",
        Locale::En => "the source repo isn't a fork of this repo",
    }
);

msg!(
    /// `domain::pull_request::merge`'s `MergeTreeOutcome::Conflicted` case:
    /// `paths` is the conflicting files, already comma-joined by the caller.
    pull_request_merge_conflict(paths: &str) {
        Locale::Es => "no se puede fusionar automaticamente, conflictos en: {paths}",
        Locale::En => "can't auto-merge, conflicts in: {paths}",
    }
);

msg!(
    /// `domain::pull_request::merge`: the PR is still a draft — flip it
    /// with `domain::pull_request::mark_ready` before merging.
    pull_request_is_draft() {
        Locale::Es => "este pull request todavia es un borrador",
        Locale::En => "this pull request is still a draft",
    }
);

msg!(
    /// `domain::pull_request::merge`: at least one reviewer's *latest*
    /// verdict ([`crate::domain::pull_request::review_summary`]) is
    /// `ChangesRequested` — merging is blocked until they either submit a
    /// new verdict or the requested changes land.
    pull_request_changes_requested_pending() {
        Locale::Es => "hay cambios pedidos sin resolver en este pull request",
        Locale::En => "this pull request has unresolved requested changes",
    }
);

msg!(
    /// `domain::pull_request::merge`, via `missing_required_reviewers`:
    /// `names` is a comma-separated list of usernames who still haven't
    /// approved but are required to, either by `RepoConfig::required_reviewers`
    /// or by `CODEOWNERS` for a path this PR touches.
    pull_request_missing_required_reviewers(names: &str) {
        Locale::Es => "falta la aprobacion de: {names}",
        Locale::En => "still missing approval from: {names}",
    }
);

msg!(
    /// `domain::pull_request::edit_metadata`: a submitted reviewer
    /// username doesn't resolve to a real user — same check as
    /// `unknown_assignee`, worded for the reviewers field instead.
    unknown_reviewer(username: &str) {
        Locale::Es => "no existe el usuario \"{username}\"",
        Locale::En => "no such user \"{username}\"",
    }
);

msg!(
    /// `domain::pull_request::add_line_comment`: the submitted anchor has no
    /// path, or a line number of 0 (line numbers are 1-based) — a hand-rolled
    /// POST, since every form that offers a line comment fills both in.
    invalid_comment_anchor() {
        Locale::Es => "el comentario no apunta a una linea valida del diff",
        Locale::En => "the comment doesn't point at a valid diff line",
    }
);

msg!(
    /// `domain::pull_request::add_review`: a PR's own author submitting an
    /// approval or a change request on it. Reviewing your own work isn't a
    /// signal, so only the neutral `Commented` verdict is allowed there.
    cannot_review_own_pull_request() {
        Locale::Es => "no podes aprobar ni pedir cambios en tu propio pull request",
        Locale::En => "you can't approve or request changes on your own pull request",
    }
);

msg!(
    /// No `IssueMeta` exists for `number` in this repo — same shape as
    /// `pull_request_not_found`, confirmed duplicated across
    /// `domain::issue::{get, transition, add_comment, edit}`.
    issue_not_found(number: u64) {
        Locale::Es => "no existe el issue #{number}",
        Locale::En => "no issue #{number} exists",
    }
);

msg!(
    /// `domain::issue::transition`: the issue's current status doesn't
    /// allow the requested move (e.g. reopening an already-open issue).
    issue_wrong_status() {
        Locale::Es => "este issue no esta en un estado valido para esa accion",
        Locale::En => "this issue isn't in a valid status for that action",
    }
);

msg!(
    /// `domain::issue::edit`: a submitted label name isn't in the repo's
    /// own `RepoConfig.labels` vocabulary.
    unknown_label(name: &str) {
        Locale::Es => "no existe la etiqueta \"{name}\" en este repo",
        Locale::En => "no such label \"{name}\" on this repo",
    }
);

msg!(
    /// `domain::issue::edit`: a submitted assignee username doesn't
    /// resolve to a real user.
    unknown_assignee(username: &str) {
        Locale::Es => "no existe el usuario \"{username}\"",
        Locale::En => "no such user \"{username}\"",
    }
);

msg!(
    /// `domain::board::{rename_column, remove_column, add_card, move_card}`:
    /// the column index doesn't name an existing column — most likely a
    /// stale form submitted after someone else already removed it.
    board_column_not_found() {
        Locale::Es => "esa columna ya no existe",
        Locale::En => "that column no longer exists",
    }
);

msg!(
    /// `domain::board::remove_column`: refuses to drop a column that still
    /// has cards on it — a `Note` card's text has no copy anywhere else, so
    /// silently discarding it on column removal would be real data loss,
    /// not just an organizational change. Move or remove every card first.
    board_column_not_empty() {
        Locale::Es => "mueve o borra las tarjetas de esta columna antes de borrarla",
        Locale::En => "move or remove this column's cards before deleting it",
    }
);

msg!(
    /// `domain::board::{move_card, remove_card}`: the card id doesn't name
    /// an existing card — most likely a stale form submitted after someone
    /// else already moved/removed it.
    board_card_not_found() {
        Locale::Es => "esa tarjeta ya no existe",
        Locale::En => "that card no longer exists",
    }
);

msg!(
    /// `domain::board::add_card`: an `Issue`/`PullRequest` card must point
    /// at a real, existing issue/PR in this same repo — never a dangling
    /// number a card would otherwise render as a broken link forever.
    board_card_target_not_found() {
        Locale::Es => "ese issue o pull request no existe en este repo",
        Locale::En => "that issue or pull request doesn't exist in this repo",
    }
);

msg!(
    /// Editing a comment is strictly author-only, no override at all.
    /// Deleting one is author-only too *unless* the actor is `Developer`+ on
    /// the repo (`domain::issue::delete_comment`'s own `is_moderator`
    /// param) — moderating someone else's abusive comment away is different
    /// from silently rewriting it. Shared between
    /// `domain::issue::{edit_comment, delete_comment}` and
    /// `domain::pull_request`'s own equivalents.
    not_comment_author() {
        Locale::Es => "solo quien escribio el comentario puede editarlo o borrarlo",
        Locale::En => "only the comment's own author can edit or delete it",
    }
);

msg!(
    /// What a 5xx tells the client, in place of the underlying error. The
    /// blanket `From<E> for AppError` turns any failure into a 500, so the
    /// real chain is whatever bubbled up — git's stderr, an S3 key, a
    /// mirror path on disk — none of which a client should be handed. The
    /// full chain still goes to the server log; see
    /// `gitquarry_webui::webui::error::AppError::client_message`.
    internal_error() {
        Locale::Es => "error interno del servidor",
        Locale::En => "internal server error",
    }
);

msg!(
    /// A PR was submitted with no title and its source branch's tip commit
    /// has no subject either, so there is nothing to name it with — see
    /// `gitquarry_core::domain::pull_request::resolve_title_and_description`.
    pull_request_needs_a_title() {
        Locale::Es => "el pull request necesita un titulo, y el ultimo commit de la rama no tiene asunto",
        Locale::En => "the pull request needs a title, and the branch's last commit has no subject",
    }
);
