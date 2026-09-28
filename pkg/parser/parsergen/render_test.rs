// Copyright 2026 AsterSQL.

use crate::Grammar;
use crate::render_rust;

#[test]
fn token_names_with_grammar_punctuation_render_as_distinct_rust_identifiers() {
    let grammar = Grammar::parse(
        r#"
%start statement;
%token DASH-NAME 256;
%token DASH.NAME 257;
%%
statement : DASH-NAME
          | DASH.NAME;
"#,
    )
    .expect("punctuated token names are valid grammar identifiers");

    let rendered = render_rust(&grammar).expect("grammar should render");

    assert!(rendered.contains("pub const DASH_u2D_NAME: u32 = 256;"));
    assert!(rendered.contains("pub const DASH_u2E_NAME: u32 = 257;"));
}
