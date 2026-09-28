// Copyright 2026 AsterSQL.

use crate::{Grammar, GrammarErrorKind};

#[test]
fn grammar_rejects_the_reserved_eof_token_number() {
    let source = r#"
%start input;
%token EOF_COLLISION 0;
%%
input : EOF_COLLISION;
"#;

    let error = Grammar::parse(source).expect_err("token number zero must remain reserved for EOF");
    assert_eq!(error.kind, GrammarErrorKind::ReservedTokenNumber);
    assert_eq!(error.span.line, 3);
    assert!(error.message.contains("reserved for end-of-input"));
}
