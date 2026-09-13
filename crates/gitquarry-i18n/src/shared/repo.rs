//! Repo-domain messages: existence/branch/tag/permission/query-param checks —
//! reused across `domain::repo`, `webui::repo::*`, `webui::repo_settings`, `webui::fork`.

use super::super::{Locale, msg};

msg!(
    /// A repo name that doesn't resolve to any existing repo — confirmed
    /// duplicated verbatim across `auth::domain::user::{parse_repos_field,set_repo_perm}`, `webui::fork`,
    /// `webui::repo::releases`, `auth::web::favorites`.
    repo_not_found(repo: &str) {
        Locale::Es => "repo '{repo}' no existe",
        Locale::En => "repo '{repo}' doesn't exist",
    }
);

msg!(
    /// Placeholder for a uid-keyed repo reference (`User::repos`) whose
    /// `RepoConfig` no longer resolves — single call site
    /// (`auth::web::users::render_user`).
    repo_deleted() {
        Locale::Es => "(repo eliminado)",
        Locale::En => "(deleted repo)",
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
    /// A destination repo name already claimed by a live repo — confirmed
    /// duplicated verbatim across `webui::fork`, `webui::repo_settings` (move).
    repo_already_exists_at(name: &str) {
        Locale::Es => "ya existe un repo en '{name}'",
        Locale::En => "there's already a repo at '{name}'",
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
    /// `webui::fork::handler_fork_post`: the target namespace isn't the
    /// user's own username, nor a group they own/admin.
    invalid_namespace(namespace: &str) {
        Locale::Es => "namespace invalido: '{namespace}' no es tuyo ni un grupo que administres",
        Locale::En => "invalid namespace: '{namespace}' isn't yours nor a group you manage",
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
    /// `webui::repo::require_number`: the `?number=` query param the `merge`
    /// view needs wasn't present, or wasn't a valid MR number.
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
