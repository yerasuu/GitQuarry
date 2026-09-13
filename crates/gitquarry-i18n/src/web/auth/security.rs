use super::super::super::{Locale, msg};

msg!(logged_in_as() { Locale::Es => "Sesión iniciada como ", Locale::En => "Logged in as " });
msg!(admin_badge() { Locale::Es => "admin", Locale::En => "admin" });
msg!(managed_elsewhere_prefix() {
    Locale::Es => ". El nombre de usuario, el correo y el indicador ",
    Locale::En => ". Username, email, and the ",
});
msg!(managed_elsewhere_suffix() {
    Locale::Es => " los gestiona un admin en ",
    Locale::En => " flag are managed by an admin in ",
});
msg!(own_keys_and_password_suffix() {
    Locale::Es => " — aquí solo están tus propias claves SSH y tu contraseña.",
    Locale::En => " — here it's only your own SSH keys and your password.",
});

msg!(ssh_keys_heading() { Locale::Es => "claves SSH", Locale::En => "SSH keys" });
msg!(pubkey_column_header() { Locale::Es => "clave pública", Locale::En => "pubkey" });
msg!(no_keys_yet_hint() { Locale::Es => "(sin claves todavía)", Locale::En => "(no keys yet)" });
msg!(delete_button() { Locale::Es => "eliminar", Locale::En => "delete" });
msg!(remove_ssh_key_confirm(fingerprint: &str) {
    Locale::Es => "¿eliminar la clave SSH '{fingerprint}'? dejara de poder usarse para push/clone por ssh.",
    Locale::En => "remove SSH key '{fingerprint}'? it stops working for ssh push/clone.",
});
msg!(remove_gpg_key_confirm(fingerprint: &str) {
    Locale::Es => "¿eliminar la clave GPG '{fingerprint}'? los commits futuros ya no se verificaran con ella.",
    Locale::En => "remove GPG key '{fingerprint}'? future commits won't verify against it.",
});
msg!(add_pubkey_placeholder() {
    Locale::Es => "ssh-ed25519 AAAA... comentario",
    Locale::En => "ssh-ed25519 AAAA... comment",
});
msg!(add_key_button() { Locale::Es => "añadir clave", Locale::En => "add key" });

msg!(secondary_emails_heading() { Locale::Es => "emails adicionales", Locale::En => "additional emails" });
msg!(secondary_emails_purpose() {
    Locale::Es => "Además de tu email de cuenta, se usan para reconocer tus commits (avatar, firma verificada) cuando el commit usa una dirección distinta.",
    Locale::En => "Besides your account email, used to recognize your commits (avatar, verified signature) when a commit carries a different address.",
});
msg!(secondary_email_column_header() { Locale::Es => "email", Locale::En => "email" });
msg!(no_secondary_emails_yet_hint() { Locale::Es => "(sin emails adicionales)", Locale::En => "(no additional emails)" });
msg!(add_secondary_email_placeholder() { Locale::Es => "otro@ejemplo.com", Locale::En => "another@example.com" });
msg!(add_secondary_email_button() { Locale::Es => "añadir email", Locale::En => "add email" });

msg!(gpg_keys_heading() { Locale::Es => "claves GPG", Locale::En => "GPG keys" });
msg!(gpg_keys_purpose() {
    Locale::Es => "Usadas para verificar firmas en los assets de tus releases.",
    Locale::En => "Used to verify signatures on your releases' assets.",
});
msg!(gpg_key_column_header() { Locale::Es => "huella", Locale::En => "fingerprint" });
msg!(no_gpg_keys_yet_hint() { Locale::Es => "(sin claves todavía)", Locale::En => "(no keys yet)" });
msg!(add_gpg_key_placeholder() {
    Locale::Es => "-----BEGIN PGP PUBLIC KEY BLOCK-----...",
    Locale::En => "-----BEGIN PGP PUBLIC KEY BLOCK-----...",
});
msg!(add_gpg_key_button() { Locale::Es => "añadir clave", Locale::En => "add key" });

msg!(password_heading() { Locale::Es => "contraseña", Locale::En => "password" });
msg!(password_purpose_prefix() {
    Locale::Es => "Para iniciar sesión en la interfaz web (",
    Locale::En => "To log in to the web UI (",
});
msg!(password_purpose_suffix() {
    Locale::Es => ") sin depender de OIDC.",
    Locale::En => ") without depending on OIDC.",
});
msg!(password_no_changes_placeholder() { Locale::Es => "(sin cambios)", Locale::En => "(no changes)" });
msg!(password_not_set_placeholder() { Locale::Es => "(sin contraseña)", Locale::En => "(no password)" });
msg!(change_password_button() { Locale::Es => "cambiar contraseña", Locale::En => "change password" });
msg!(set_password_button() { Locale::Es => "establecer contraseña", Locale::En => "set password" });
msg!(change_password_confirm() {
    Locale::Es => "¿cambiar la contraseña? cerrara la posibilidad de entrar con la anterior.",
    Locale::En => "change password? the old one stops working immediately.",
});

msg!(two_factor_heading() {
    Locale::Es => "verificación en dos pasos (TOTP)",
    Locale::En => "two-factor verification (TOTP)",
});
msg!(totp_active_hint() {
    Locale::Es => "2FA está activo. Para desactivarlo, confirma tu contraseña o un código actual.",
    Locale::En => "2FA is active. To disable it, confirm your password or a current code.",
});
msg!(totp_disable_placeholder() {
    Locale::Es => "contraseña o código de 6 dígitos",
    Locale::En => "password or 6-digit code",
});
msg!(disable_totp_button() { Locale::Es => "desactivar 2FA", Locale::En => "disable 2FA" });
msg!(disable_totp_confirm() {
    Locale::Es => "¿desactivar 2FA? la cuenta quedara protegida solo por la contraseña.",
    Locale::En => "disable 2FA? the account is then protected by password alone.",
});
msg!(totp_scan_hint() {
    Locale::Es => "Escanea o copia este secreto en tu app de autenticación y confirma con un código.",
    Locale::En => "Scan or copy this secret into your authenticator app, then confirm with a code.",
});
msg!(totp_open_app_link() {
    Locale::Es => "abrir en la app de autenticación",
    Locale::En => "open in the authenticator app",
});
msg!(totp_confirm_placeholder() { Locale::Es => "código de 6 dígitos", Locale::En => "6-digit code" });
msg!(confirm_totp_button() { Locale::Es => "confirmar y activar", Locale::En => "confirm and activate" });
msg!(totp_inactive_hint() {
    Locale::Es => "No está activo. Añade una capa extra de seguridad al inicio de sesión con contraseña.",
    Locale::En => "Not active. Adds an extra layer of security to password login.",
});
msg!(enable_totp_button() { Locale::Es => "activar 2FA", Locale::En => "activate 2FA" });

msg!(recovery_codes_heading() { Locale::Es => "códigos de recuperación", Locale::En => "recovery codes" });
msg!(recovery_codes_reveal_warning() {
    Locale::Es => "Guárdalos ahora: no se pueden volver a ver. Cada uno funciona una sola vez, en vez de un código TOTP, si pierdes el dispositivo.",
    Locale::En => "Save these now: they can't be shown again. Each one works exactly once, in place of a TOTP code, if you lose your device.",
});
msg!(recovery_codes_remaining_hint(count: usize) {
    Locale::Es => "Te quedan {count} código(s) sin usar. Regenerar invalida los que no hayas usado.",
    Locale::En => "{count} unused code(s) left. Regenerating invalidates any you haven't used.",
});
msg!(regenerate_recovery_codes_placeholder() {
    Locale::Es => "contraseña o código de 6 dígitos",
    Locale::En => "password or 6-digit code",
});
msg!(regenerate_recovery_codes_button() { Locale::Es => "regenerar códigos", Locale::En => "regenerate codes" });
msg!(regenerate_recovery_codes_confirm() {
    Locale::Es => "¿regenerar los codigos de recuperacion? los actuales dejaran de servir.",
    Locale::En => "regenerate recovery codes? the current ones stop working.",
});

// Placeholder for the re-auth field in front of a credential change
// (a new password, a new SSH key) -- see
// `crate::auth::web::security::require_reauth_for_credential_change`.
msg!(reauth_placeholder() {
    Locale::Es => "contraseña actual o código 2FA",
    Locale::En => "current password or 2FA code",
});

// Field labels. These pages moved from placeholder-only inputs to real
// `<label>`s: a placeholder disappears on the first keystroke, is not
// announced as a label, and gives no click target.
msg!(add_pubkey_label() { Locale::Es => "clave pública SSH", Locale::En => "SSH public key" });
msg!(add_pubkey_hint() {
    Locale::Es => "La línea completa de tu .pub, incluido el tipo y el comentario final.",
    Locale::En => "The full line from your .pub file, key type and trailing comment included.",
});
msg!(add_gpg_key_label() { Locale::Es => "clave pública GPG", Locale::En => "GPG public key" });
msg!(reauth_label() { Locale::Es => "confirma tu identidad", Locale::En => "confirm it is you" });
msg!(reauth_hint() {
    Locale::Es => "Tu contraseña actual, o un código 2FA si lo tienes activo.",
    Locale::En => "Your current password, or a 2FA code if you have it enabled.",
});
msg!(new_password_label() { Locale::Es => "contraseña nueva", Locale::En => "new password" });
msg!(confirm_password_label() { Locale::Es => "repite la contraseña nueva", Locale::En => "repeat the new password" });
msg!(password_mismatch() {
    Locale::Es => "las dos contraseñas no coinciden",
    Locale::En => "the two passwords don't match",
});
msg!(clear_password_hint() {
    Locale::Es => "Déjala en blanco para quedarte sin contraseña y entrar solo por SSH u OIDC.",
    Locale::En => "Leave blank to end up with no password and sign in only over SSH or OIDC.",
});

// Primary-email block on `/_account/profile`. Self-service only where OIDC
// can't resolve a login by the address — see
// `gitquarry_core::auth::domain::user::set_own_email`.
msg!(primary_email_heading() { Locale::Es => "correo principal", Locale::En => "primary email" });
msg!(primary_email_purpose() {
    Locale::Es => "La dirección con la que se te identifica: enlaza tus commits con esta cuenta y, si hay OIDC, resuelve tu login.",
    Locale::En => "The address you're identified by: it links your commits to this account and, with OIDC, resolves your login.",
});
msg!(primary_email_label() { Locale::Es => "correo principal", Locale::En => "primary email" });
msg!(primary_email_hint() {
    Locale::Es => "Déjalo en blanco para quedarte sin correo principal.",
    Locale::En => "Leave blank to end up with no primary email.",
});
msg!(primary_email_admin_managed() {
    Locale::Es => "Con OIDC configurado esta dirección decide a qué cuenta entra un login, así que solo un admin puede cambiarla desde /_users.",
    Locale::En => "With OIDC configured this address decides which account a login lands on, so only an admin can change it, from /_users.",
});
msg!(no_primary_email() { Locale::Es => "(sin correo principal)", Locale::En => "(no primary email)" });
msg!(save_email_button() { Locale::Es => "guardar correo", Locale::En => "save email" });
msg!(save_email_confirm() {
    Locale::Es => "¿cambiar el correo principal? se usara para notificaciones y recuperacion de cuenta.",
    Locale::En => "change primary email? it's used for notifications and account recovery.",
});
msg!(remove_secondary_email_confirm(email: &str) {
    Locale::Es => "¿quitar el correo secundario '{email}'?",
    Locale::En => "remove secondary email '{email}'?",
});
