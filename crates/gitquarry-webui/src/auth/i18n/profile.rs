//! `auth::web::profile`'s page copy.

use gitquarry_i18n::{Locale, msg};

msg!(no_repos_visible() {
    Locale::Es => "(sin repos visibles)",
    Locale::En => "(no repos visible)",
});
msg!(account_settings_link() { Locale::Es => "ajustes de cuenta", Locale::En => "account settings" });
