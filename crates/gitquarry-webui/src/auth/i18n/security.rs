//! `auth::web::security`'s page copy.

use gitquarry_i18n::{Locale, msg};

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

// Placeholder for the re-auth field in front of a credential change
// (a new password, a new SSH key) -- see
// `crate::auth::web::security::require_reauth_for_credential_change`.
msg!(reauth_placeholder() {
    Locale::Es => "contraseña actual o código 2FA",
    Locale::En => "current password or 2FA code",
});
