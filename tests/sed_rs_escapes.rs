//! Pins down sed_rs's handling of `\xHH`/`\oNNN` escapes, which diverges
//! from GNU sed (reported upstream). GNU sed 4.9 interprets these as
//! character escapes in both the pattern and replacement of `s///`;
//! sed_rs 1.1.0 copies them through as literal text.
//!
//! `instrument::wrap_script` works around this by embedding the raw
//! placeholder byte in the generated script instead of a `\x02` escape.
//! If these tests start failing, sed_rs has fixed the divergence upstream
//! and the workaround (plus this file) can be dropped.
//!
//! To see the GNU-expected behaviour side by side:
//!   printf 'a\nb\n' | sed 'N; s/\n/\x02/g' | od -c

#[test]
fn hex_escape_in_replacement_is_literal_text() {
    // GNU: printf 'a\nb\n' | sed 'N; s/\n/\x02/g'  →  "a\x02b\n" (STX byte)
    let out = sed_rs::eval("N; s/\\n/\\x02/g", "a\nb\n").unwrap();
    assert_eq!(out, "a\\x02b\n"); // literal backslash-x-0-2, not the byte
}

#[test]
fn octal_escape_in_replacement_is_literal_text() {
    // GNU: printf 'a\nb\n' | sed 'N; s/\n/\o002/g'  →  "a\x02b\n" (STX byte)
    let out = sed_rs::eval("N; s/\\n/\\o002/g", "a\nb\n").unwrap();
    assert_eq!(out, "a\\o002b\n");
}

#[test]
fn raw_byte_in_script_works_as_workaround() {
    // Embedding the actual control byte in the script string is handled
    // correctly — this is what wrap_script relies on.
    let out = sed_rs::eval("N; s/\\n/\u{2}/g", "a\nb\n").unwrap();
    assert_eq!(out, "a\u{2}b\n");
}
