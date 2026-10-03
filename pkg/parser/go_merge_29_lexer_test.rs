// Copyright 2026 AsterSQL.

#[test]
fn go_merge_29_sql_parentheses_depth() {
    let mut scanner = NewScanner("(".repeat(10_001));
    let mut value = yySymType::default();
    for _ in 0..10_000 {
        assert_eq!(scanner.Lex(&mut value), '(' as i32);
    }
    assert_eq!(scanner.Lex(&mut value), token::invalid);
    assert!(scanner.Errors().1.iter().any(|error| {
        error
            .to_string()
            .contains("parentheses nesting depth exceeds maximum 10000")
    }));
    scanner.reset("(".to_owned());
    assert_eq!(scanner.Lex(&mut value), '(' as i32);
    assert!(scanner.Errors().1.is_empty());
    scanner.reset(")(".to_owned());
    assert_eq!(scanner.Lex(&mut value), ')' as i32);
    assert_eq!(scanner.Lex(&mut value), '(' as i32);
}

#[test]
fn go_merge_29_hint_parentheses_depth() {
    let mut scanner = hintScanner::default();
    scanner.scanner.reset("(".repeat(10_001));
    let mut value = yyhintSymType::default();
    for _ in 0..10_000 {
        assert_eq!(scanner.Lex(&mut value), '(' as i32);
    }
    assert_eq!(scanner.Lex(&mut value), hintInvalid);
    assert!(scanner.scanner.Errors().1.iter().any(|error| {
        error
            .to_string()
            .contains("parentheses nesting depth exceeds maximum 10000")
    }));
}

#[test]
fn go_merge_29_new_token_and_builtin_mappings() {
    for (word, expected) in [
        ("ALERT", token::alert),
        ("ASYNC", token::r#async),
        ("AUTO", token::auto),
        ("COMPLETE", token::complete),
        ("DELTA", token::delta),
        ("FAST", token::fast),
        ("IMMEDIATE", token::immediate),
        ("MATERIALIZED", token::materialized),
        ("OPERATE", token::operate),
        ("PLACE", token::place),
        ("STORAGE_CLASS", token::storageClass),
        ("TRANSITIONS", token::transitions),
    ] {
        let mut scanner = NewScanner(word.to_owned());
        assert_eq!(scanner.Lex(&mut yySymType::default()), expected, "{word}");
    }
    for (word, expected) in [
        ("MAX_COUNT(", token::builtinMaxCount),
        ("MIN_COUNT(", token::builtinMinCount),
    ] {
        let mut scanner = NewScanner(word.to_owned());
        assert_eq!(scanner.Lex(&mut yySymType::default()), expected, "{word}");
    }
}

#[test]
fn parentheses_depth_error_stays_fatal_across_warning_conversion() {
    let mut scanner = NewScanner("(".repeat(10_001));
    let mut value = yySymType::default();
    for _ in 0..10_000 {
        assert_eq!(scanner.Lex(&mut value), '(' as i32);
    }
    assert_eq!(scanner.Lex(&mut value), token::invalid);
    let depth_error = scanner.Errors().1[0].to_string();
    scanner.lastErrorAsWarn();
    assert_eq!(scanner.Errors().1.len(), 1);
    assert!(scanner.Errors().0.is_empty());

    scanner.AppendError(errors::New("ordinary hint error"));
    scanner.lastErrorAsWarn();
    assert_eq!(scanner.Errors().0.len(), 1);
    assert_eq!(scanner.Errors().0[0].to_string(), "ordinary hint error");
    assert_eq!(scanner.Errors().1[0].to_string(), depth_error);
    scanner.lastErrorAsWarn();
    assert_eq!(scanner.Errors().1.len(), 1);
    assert_eq!(scanner.Errors().0.len(), 1);

    scanner.reset("select 1".to_owned());
    scanner.AppendError(errors::New("ordinary error after reset"));
    scanner.lastErrorAsWarn();
    assert!(scanner.Errors().1.is_empty());
    assert_eq!(scanner.Errors().0.len(), 1);
    assert_eq!(scanner.Errors().0[0].to_string(), "ordinary error after reset");
}
