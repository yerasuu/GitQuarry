//! `auth::web::avatar`'s page copy.

use gitquarry_i18n::{Locale, msg};

msg!(heading() { Locale::Es => "avatar", Locale::En => "avatar" });
msg!(description() {
    Locale::Es => "Imagen de perfil opcional (PNG/JPEG/WebP, max 2 MB). Sin una, se usa un identicon generado a partir de tu usuario.",
    Locale::En => "Optional profile image (PNG/JPEG/WebP, max 2 MB). Without one, a generated identicon based on your username is used instead.",
});
msg!(upload_button() { Locale::Es => "subir", Locale::En => "upload" });
msg!(remove_button() { Locale::Es => "quitar avatar", Locale::En => "remove avatar" });
msg!(remove_avatar_confirm() {
    Locale::Es => "¿quitar el avatar?",
    Locale::En => "remove avatar?",
});
