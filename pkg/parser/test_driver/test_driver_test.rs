// Copyright 2026 AsterSQL.

use crate::ValueExpr;

/// Go `strconv.Quote` uses the short `\a` and `\v` escapes for these controls.
#[test]
fn value_expr_format_uses_go_string_escapes() {
    let expression = ValueExpr::new(Box::new(String::from("\u{7}\u{b}")), "", "");
    let mut output = Vec::new();

    expression.Format(&mut output);

    assert_eq!(String::from_utf8(output).unwrap(), r#""\a\v""#);
}
