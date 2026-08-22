//! `webui::repo::releases`'s page copy.

use super::super::{Locale, msg};

msg!(release_column() { Locale::Es => "lanzamiento", Locale::En => "release" });
msg!(assets_column() { Locale::Es => "archivos", Locale::En => "assets" });
msg!(created_column() { Locale::Es => "creado", Locale::En => "created" });
msg!(no_releases_yet_hint() { Locale::Es => "(sin lanzamientos aun)", Locale::En => "(no releases yet)" });
msg!(create_release_heading() { Locale::Es => "crear lanzamiento", Locale::En => "create release" });
msg!(tag_label() { Locale::Es => "etiqueta", Locale::En => "tag" });
msg!(tag_placeholder() { Locale::Es => "v1.0", Locale::En => "v1.0" });
msg!(title_label() { Locale::Es => "titulo", Locale::En => "title" });
msg!(notes_label() { Locale::Es => "notas", Locale::En => "notes" });
msg!(prerelease_checkbox_label() { Locale::Es => "pre-lanzamiento", Locale::En => "this is a pre-release" });
msg!(prerelease_badge() { Locale::Es => "pre-lanzamiento", Locale::En => "pre-release" });
msg!(draft_checkbox_label() { Locale::Es => "guardar como borrador", Locale::En => "save as draft" });
msg!(draft_badge() { Locale::Es => "borrador", Locale::En => "draft" });
msg!(create_release_button() { Locale::Es => "crear lanzamiento", Locale::En => "create release" });
msg!(generate_notes_button() {
    Locale::Es => "generar notas desde los commits",
    Locale::En => "generate notes from commits",
});

msg!(dangling_tag_notice_prefix() { Locale::Es => "la etiqueta ", Locale::En => "tag " });
msg!(dangling_tag_notice_suffix() {
    Locale::Es => " ya no existe en este repo — el lanzamiento se mantiene, no se elimina automaticamente.",
    Locale::En => " no longer exists on this repo — the release stays, it isn't auto-deleted.",
});
msg!(created_by_prefix() { Locale::Es => "creado ", Locale::En => "created " });
msg!(created_by_middle() { Locale::Es => " por ", Locale::En => " by " });

msg!(source_code_tar_gz() { Locale::Es => "código fuente (tar.gz)", Locale::En => "Source code (tar.gz)" });
msg!(source_code_zip() { Locale::Es => "código fuente (zip)", Locale::En => "Source code (zip)" });

msg!(assets_heading() { Locale::Es => "archivos", Locale::En => "assets" });
msg!(file_column() { Locale::Es => "archivo", Locale::En => "file" });
msg!(size_column() { Locale::Es => "tamaño", Locale::En => "size" });
msg!(sha256_column() { Locale::Es => "sha256", Locale::En => "sha256" });
msg!(no_assets_hint() { Locale::Es => "(sin archivos)", Locale::En => "(no assets)" });
msg!(delete_button() { Locale::Es => "eliminar", Locale::En => "delete" });

msg!(upload_asset_heading() { Locale::Es => "subir archivo", Locale::En => "upload asset" });
msg!(upload_button() { Locale::Es => "subir", Locale::En => "upload" });
msg!(signature_placeholder() {
    Locale::Es => "firma detached OpenPGP (opcional) — -----BEGIN PGP SIGNATURE-----...",
    Locale::En => "detached OpenPGP signature (optional) — -----BEGIN PGP SIGNATURE-----...",
});
msg!(signature_verified_badge() { Locale::Es => "firma verificada", Locale::En => "signature verified" });
msg!(signature_unverified_badge() { Locale::Es => "firma no verificada", Locale::En => "signature not verified" });
msg!(edit_heading() { Locale::Es => "editar", Locale::En => "edit" });
msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(delete_release_heading() { Locale::Es => "eliminar lanzamiento", Locale::En => "delete release" });
msg!(delete_release_button() {
    Locale::Es => "eliminar lanzamiento (y sus archivos)",
    Locale::En => "delete release (and its assets)",
});
msg!(delete_release_confirm(tag: &str) {
    Locale::Es => "¿eliminar el lanzamiento '{tag}' y todos sus archivos? no se puede deshacer.",
    Locale::En => "delete release '{tag}' and all its assets? this cannot be undone.",
});
msg!(delete_asset_confirm(filename: &str) {
    Locale::Es => "¿eliminar el archivo '{filename}'? no se puede deshacer.",
    Locale::En => "delete asset '{filename}'? this cannot be undone.",
});
