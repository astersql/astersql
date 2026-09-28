use super::*;

#[test]
fn lookup_uses_unicode_lowercase_like_go() {
    let result = Lookup(" \u{212a}OI8-R\n").expect("Kelvin sign lowercases to ASCII k");
    assert_eq!(result.name, "koi8-r");
}
