//! Release/asset messages — `gitquarry_domain::release` and
//! `webui::repo::releases`'s upload/download paths.

use super::super::{Locale, msg};

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
    /// `webui::repo::releases::handler_upload_asset_post`: a text field
    /// (`tag`/`csrf`/`signature`) exceeded its in-memory cap.
    text_field_too_large(field: &str, limit_bytes: usize) {
        Locale::Es => "el campo '{field}' supera {limit_bytes} bytes",
        Locale::En => "field '{field}' exceeds {limit_bytes} bytes",
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
