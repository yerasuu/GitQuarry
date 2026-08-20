use gitquarry_webui::webui::export::*;

#[test]
fn guesses_common_extensions() {
    assert_eq!(guess_content_type("logo.png"), "image/png");
    assert_eq!(guess_content_type("README.md"), "text/plain; charset=utf-8");
    assert_eq!(guess_content_type("binary.bin"), "application/octet-stream");
}

#[test]
fn detects_images() {
    assert!(is_image("a.svg"));
    assert!(!is_image("a.rs"));
}

#[test]
fn forces_download_for_svg_to_block_inline_script_execution() {
    let content_type = guess_content_type("logo.svg");
    let disposition = raw_disposition("assets/logo.svg", content_type).expect("svg must force a download");
    assert_eq!(disposition, "attachment; filename=\"logo.svg\"");
}

#[test]
fn does_not_force_download_for_non_svg() {
    assert_eq!(raw_disposition("README.md", guess_content_type("README.md")), None);
    assert_eq!(raw_disposition("logo.png", guess_content_type("logo.png")), None);
}

#[test]
fn strips_quotes_from_disposition_filename() {
    let disposition = raw_disposition("weird\"name.svg", "image/svg+xml").expect("svg must force a download");
    assert_eq!(disposition, "attachment; filename=\"weird'name.svg\"");
}

/// PDF keeps `application/pdf` (an `<embed>` needs it) but must not render
/// inline from the raw URL: a PDF can carry JavaScript, and the viewer that
/// runs it sits largely outside what this app's CSP can reach.
#[test]
fn raw_pdf_is_handed_over_as_a_download() {
    let content_type = guess_content_type("docs/spec.pdf");
    assert_eq!(content_type, "application/pdf");
    let disposition = raw_disposition("docs/spec.pdf", content_type).expect("pdf must force a download");
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(disposition.contains("spec.pdf"), "{disposition}");
}

#[test]
fn attachment_keeps_the_filename_inside_its_quoted_string() {
    assert_eq!(attachment("notes.txt"), "attachment; filename=\"notes.txt\"");
    // A repo or asset name carrying a quote used to close the string early
    // and let the rest read as header parameters.
    assert_eq!(attachment("a\"b.zip"), "attachment; filename=\"a'b.zip\"");
    assert_eq!(attachment("back\\slash"), "attachment; filename=\"back'slash\"");
    // Nothing a header value can't carry survives.
    assert_eq!(attachment("bad\r\nname"), "attachment; filename=\"badname\"");
    // And an empty result still names something.
    assert_eq!(attachment("   "), "attachment; filename=\"file\"");
}
