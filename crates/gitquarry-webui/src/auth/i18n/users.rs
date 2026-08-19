//! `auth::web::users`'s page copy.

use gitquarry_i18n::{Locale, msg};

// Intro paragraph, split at every inline `code { ... }` boundary — maud
// needs each plain-text run between the `code` elements as its own node, so
// each run gets its own message instead of one giant format string.
msg!(intro_stored_prefix() {
    Locale::Es => "Guardado uno por usuario en ",
    Locale::En => "Stored one per user in ",
});
msg!(intro_bucket_and_keys() {
    Locale::Es => " dentro del bucket. Cada usuario puede tener varias claves SSH (perder una no hace perder los permisos: solo se elimina esa clave), permisos por repo (",
    Locale::En => " inside the bucket. Each user can have several SSH keys (losing one doesn't lose the permissions: only that key gets removed), per-repo permissions (",
});
msg!(intro_perms_default() {
    Locale::Es => " como valor por defecto), y opcionalmente una contraseña para entrar a la interfaz web (",
    Locale::En => " as a default), and optionally a password to log in to the web UI (",
});
msg!(intro_password_login() {
    Locale::Es => ") sin depender de OIDC. ",
    Locale::En => ") without depending on OIDC. ",
});
msg!(intro_admin_grants() {
    Locale::Es => " otorga acceso total: ",
    Locale::En => " grants full access: ",
});
msg!(intro_settings_page() {
    Locale::Es => ", esta pagina, y cualquier repo ",
    Locale::En => ", this page, and any ",
});
msg!(intro_private_repo() {
    Locale::Es => " sin importar los permisos. Sobre ",
    Locale::En => " repo regardless of permissions. Over ",
});
msg!(intro_ssh_no_password() {
    Locale::Es => " no hay contraseña: la identidad ahi es la clave (",
    Locale::En => " there's still no password: identity there is the key (",
});
msg!(intro_clone_resolved() {
    Locale::Es => "), resuelta al usuario propietario.",
    Locale::En => "), resolved to the owning user.",
});

msg!(users_empty_hint() {
    Locale::Es => "(vacio, aun no se creo ningun usuario)",
    Locale::En => "(empty, no user created yet)",
});
msg!(users_search_placeholder() { Locale::Es => "buscar usuarios", Locale::En => "search users" });
msg!(users_search_button() { Locale::Es => "buscar", Locale::En => "search" });
msg!(add_user_heading() { Locale::Es => "agregar usuario", Locale::En => "add user" });

msg!(username_label() { Locale::Es => "usuario", Locale::En => "username" });
msg!(email_label() { Locale::Es => "correo", Locale::En => "email" });
msg!(password_label() { Locale::Es => "contraseña", Locale::En => "password" });
msg!(admin_label() { Locale::Es => "admin", Locale::En => "admin" });
msg!(pubkey_label() { Locale::Es => "clave publica", Locale::En => "pubkey" });
msg!(repos_label() { Locale::Es => "repos", Locale::En => "repos" });
msg!(repo_label() { Locale::Es => "repo", Locale::En => "repo" });
msg!(permission_label() { Locale::Es => "permiso", Locale::En => "permission" });

msg!(email_placeholder() {
    Locale::Es => "(opcional, para login con OIDC)",
    Locale::En => "(optional, for OIDC login)",
});
msg!(password_placeholder() {
    Locale::Es => "(opcional, para login con contraseña)",
    Locale::En => "(optional, for password login)",
});
msg!(pubkey_optional_placeholder() {
    Locale::Es => "(opcional) ssh-ed25519 AAAA... comentario",
    Locale::En => "(optional) ssh-ed25519 AAAA... comment",
});
msg!(pubkey_key_placeholder() {
    Locale::Es => "ssh-ed25519 AAAA... comentario",
    Locale::En => "ssh-ed25519 AAAA... comment",
});
msg!(repo_field_placeholder() {
    Locale::Es => "proyectoA (o * como valor por defecto)",
    Locale::En => "projectA (or * as default)",
});

msg!(admin_full_access_checkbox() {
    Locale::Es => " acceso total (ajustes/usuarios/repos privados)",
    Locale::En => " full access (settings/users/private repos)",
});

msg!(create_button() { Locale::Es => "crear", Locale::En => "create" });
msg!(delete_button() { Locale::Es => "eliminar", Locale::En => "delete" });
msg!(add_key_button() { Locale::Es => "agregar clave", Locale::En => "add key" });
msg!(add_update_repo_button() { Locale::Es => "agregar/actualizar repo", Locale::En => "add/update repo" });
msg!(change_password_button() { Locale::Es => "cambiar contraseña", Locale::En => "change password" });
msg!(set_password_button() { Locale::Es => "establecer contraseña", Locale::En => "set password" });
msg!(storage_quota_placeholder() { Locale::Es => "50M, 5G... (vacio = sin limite)", Locale::En => "50M, 5G... (empty = unlimited)" });
msg!(storage_quota_current_prefix() { Locale::Es => "limite actual: ", Locale::En => "current limit: " });
msg!(set_storage_quota_button() { Locale::Es => "guardar limite", Locale::En => "save limit" });
msg!(remove_admin_button() { Locale::Es => "quitar admin", Locale::En => "remove admin" });
msg!(make_admin_button() { Locale::Es => "hacer admin", Locale::En => "make admin" });
msg!(disable_totp_button() {
    Locale::Es => "deshabilitar 2FA (perdi el dispositivo)",
    Locale::En => "disable 2FA (lost the device)",
});
msg!(delete_user_button() { Locale::Es => "eliminar usuario", Locale::En => "delete user" });
msg!(remove_button() { Locale::Es => "quitar", Locale::En => "remove" });

msg!(password_no_changes_placeholder() { Locale::Es => "(sin cambios)", Locale::En => "(no changes)" });
msg!(password_none_placeholder() { Locale::Es => "(sin contraseña)", Locale::En => "(no password)" });

msg!(no_keys_hint() { Locale::Es => "(sin claves todavia)", Locale::En => "(no keys yet)" });
msg!(no_repos_hint_before_star() {
    Locale::Es => "(sin repos todavia — ",
    Locale::En => "(no repos yet — ",
});
msg!(no_repos_hint_after_star() {
    Locale::Es => " es el valor por defecto para quien no este listado)",
    Locale::En => " is the default for anyone not listed)",
});

msg!(permission_guest_label() { Locale::Es => "invitado", Locale::En => "guest" });
msg!(permission_reporter_label() { Locale::Es => "lector", Locale::En => "reporter" });
msg!(permission_developer_label() { Locale::Es => "desarrollador", Locale::En => "developer" });
msg!(permission_maintainer_label() { Locale::Es => "mantenedor", Locale::En => "maintainer" });
msg!(permission_owner_label() { Locale::Es => "propietario", Locale::En => "owner" });

// Admin-only primary-email field -- see
// `gitquarry_core::auth::domain::user::set_email` on why this is admin-only
// and not part of `/_account/security`.
msg!(set_email_button() { Locale::Es => "guardar correo", Locale::En => "save email" });
msg!(set_email_placeholder() {
    Locale::Es => "correo (vacio = ninguno)",
    Locale::En => "email (empty = none)",
});
