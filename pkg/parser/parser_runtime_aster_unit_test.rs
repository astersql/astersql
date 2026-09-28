// Copyright 2026 AsterSQL.

#[derive(Default)]
struct RuntimeTestLexer;

impl yyLexer for RuntimeTestLexer {
    fn Lex(&mut self, _lval: &mut yySymType) -> isize {
        yyEOFCode
    }

    fn Errorf(&self, format: &str, _args: &[&dyn Any]) -> ParserError {
        ParserError(format.to_owned())
    }

    fn AppendError(&mut self, _err: ParserError) {}

    fn AppendWarn(&mut self, _err: ParserError) {}

    fn Errors(&self) -> (Vec<ParserError>, Vec<ParserError>) {
        (Vec::new(), Vec::new())
    }
}

#[test]
fn parser_runtime_rhs_uses_one_based_left_to_right_borrow_and_take() {
    let mut values = vec![
        yySymType {
            ident: "left".to_owned(),
            ..Default::default()
        },
        yySymType {
            ident: "right".to_owned(),
            ..Default::default()
        },
    ];
    let mut rhs = Rhs::new(&mut values);

    assert_eq!(rhs.len(), 2);
    assert_eq!(
        rhs.borrow(1).map(|value| value.ident.as_str()),
        Some("left")
    );
    assert_eq!(
        rhs.borrow(2).map(|value| value.ident.as_str()),
        Some("right")
    );
    rhs.borrow_mut(2)
        .expect("$2 is present")
        .ident
        .push_str("-mutated");
    assert_eq!(
        rhs.borrow(2).map(|value| value.ident.as_str()),
        Some("right-mutated")
    );
    assert!(rhs.borrow(0).is_none());
    assert!(rhs.borrow(3).is_none());

    let taken = rhs.take(1).expect("$1 is present");
    assert_eq!(taken.ident, "left");
    assert_eq!(rhs.borrow(1).map(|value| value.ident.as_str()), Some(""));
    assert!(rhs.take(0).is_none());
    assert!(rhs.take(3).is_none());
}

#[test]
fn parser_runtime_rhs_default_value_moves_first_symbol() {
    let mut values = vec![
        yySymType {
            ident: "first".to_owned(),
            ..Default::default()
        },
        yySymType {
            ident: "second".to_owned(),
            ..Default::default()
        },
    ];

    let output = Rhs::new(&mut values).take_default();

    assert_eq!(output.ident, "first");
    assert!(values[0].ident.is_empty());
    assert_eq!(values[1].ident, "second");
}

#[test]
fn parser_runtime_dispatches_stable_rule_id_without_legacy_adapter() {
    let rule_id = RULE_IDS_BY_REDUCTION
        .iter()
        .copied()
        .find(|rule_id| rule_id.as_str() == "emptystmt--bede6c684b818500")
        .expect("EmptyStmt has a generated stable RuleId");
    let mut values = Vec::new();
    let mut output = yySymType::default();
    let mut parser_state = New();
    let mut lexer = RuntimeTestLexer;

    let handled = parser_actions::apply(
        rule_id,
        Rhs::new(&mut values),
        parser_actions::Context::new(&mut output, &mut parser_state, &mut lexer),
    )
    .expect("named action succeeds");

    assert!(handled);
    assert!(output.statement.is_none());
}
