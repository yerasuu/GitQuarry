use gitquarry_i18n::*;

#[test]
fn locale_parse_recognizes_es_and_en_only() {
    assert_eq!(Locale::parse("es"), Some(Locale::Es));
    assert_eq!(Locale::parse("en"), Some(Locale::En));
    assert_eq!(Locale::parse("fr"), None);
    assert_eq!(Locale::parse(""), None);
}

#[test]
fn default_locale_is_english() {
    assert_eq!(Locale::DEFAULT, Locale::En);
}
