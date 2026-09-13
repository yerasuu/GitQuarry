//! The backup section's own copy. Spot-checks rather than a line-by-line
//! transcript: what matters is that both locales are really filled in (a
//! `msg!` arm left as a copy of the other reads fine and is wrong) and that
//! the two warnings an operator has to actually read say the thing they
//! exist to say.

use gitquarry_i18n::Locale;
use gitquarry_i18n::web::settings::*;

#[test]
fn both_locales_are_filled_in_and_differ() {
    let pairs = [
        (settings_backup_prose(Locale::Es), settings_backup_prose(Locale::En)),
        (
            settings_backup_dest_hint(Locale::Es),
            settings_backup_dest_hint(Locale::En),
        ),
        (
            settings_backup_class_derived_hint(Locale::Es),
            settings_backup_class_derived_hint(Locale::En),
        ),
        (
            settings_backup_delete_grace_hint(Locale::Es),
            settings_backup_delete_grace_hint(Locale::En),
        ),
        (
            settings_backup_secrets_warning(Locale::Es),
            settings_backup_secrets_warning(Locale::En),
        ),
    ];
    for (es, en) in pairs {
        assert!(!es.is_empty() && !en.is_empty());
        assert_ne!(es, en, "both locales carry the same text: {es}");
    }
}

#[test]
fn the_destination_hint_names_every_form_it_accepts() {
    for locale in [Locale::Es, Locale::En] {
        let placeholder = settings_backup_dest_placeholder(locale);
        assert!(placeholder.contains("bucket:"));
        assert!(placeholder.contains("path:"));
        assert!(placeholder.contains("self"));
    }
}

#[test]
fn the_grace_hint_says_empty_means_never_deleting() {
    // The default is "never propagate a deletion", and that's the whole
    // reason an accidental purge is recoverable — the copy has to say so.
    assert!(settings_backup_delete_grace_placeholder(Locale::En).contains("never"));
    assert!(settings_backup_delete_grace_placeholder(Locale::Es).contains("nunca"));
}

#[test]
fn the_secrets_warning_names_what_the_destination_holds() {
    let en = settings_backup_secrets_warning(Locale::En);
    for expected in ["ssh host key", "signing key", "password hashes", "TOTP"] {
        assert!(en.contains(expected), "the warning should mention {expected}: {en}");
    }
}
