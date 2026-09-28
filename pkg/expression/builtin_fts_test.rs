// Copyright 2026 AsterSQL.

use crate::builtin_fts::{
    FtsAgainst, FtsError, FtsSignature, FulltextSearchModifier, MatchArgument, build_match_word,
    build_mysql_match_against, set_mysql_match_against_modifier,
};

#[test]
fn fts_builders_enforce_go_argument_counts_before_other_validation() {
    assert_eq!(
        build_match_word(false, FtsAgainst::String("word".into()), &[]),
        Err(FtsError::IncorrectParameterCount)
    );
    assert_eq!(
        build_match_word(
            true,
            FtsAgainst::String("word".into()),
            &[MatchArgument::StringColumn, MatchArgument::StringColumn],
        ),
        Err(FtsError::IncorrectParameterCount)
    );
    assert_eq!(
        build_mysql_match_against(FtsAgainst::String("word".into()), &[]),
        Err(FtsError::IncorrectParameterCount)
    );
}

#[test]
fn fts_builders_preserve_go_validation_evaluation_and_modifier_semantics() {
    assert_eq!(
        build_match_word(
            false,
            FtsAgainst::String("word".into()),
            &[MatchArgument::StringColumn],
        ),
        Err(FtsError::StarterOnly)
    );
    for against in [
        FtsAgainst::Null,
        FtsAgainst::NonStringConstant,
        FtsAgainst::NonConstant,
    ] {
        assert_eq!(
            build_match_word(true, against, &[MatchArgument::StringColumn]),
            Err(FtsError::NonConstantAgainst)
        );
    }
    assert_eq!(
        build_match_word(
            true,
            FtsAgainst::String("word".into()),
            &[MatchArgument::NonColumn],
        ),
        Err(FtsError::NonColumn)
    );

    let match_word = build_match_word(
        true,
        FtsAgainst::String("word".into()),
        &[MatchArgument::NonStringColumn],
    )
    .unwrap();
    assert_eq!(match_word.against(), "word");
    assert_eq!(match_word.column_count(), 1);
    assert!(match_word.fts_function_used());
    assert_eq!(match_word.eval_real(), Err(FtsError::MatchWordOutsideIndex));

    assert_eq!(
        build_mysql_match_against(FtsAgainst::NonConstant, &[MatchArgument::StringColumn]),
        Err(FtsError::NonConstantAgainst)
    );
    assert_eq!(
        build_mysql_match_against(
            FtsAgainst::NonStringConstant,
            &[MatchArgument::StringColumn],
        ),
        Err(FtsError::NonStringAgainst)
    );
    assert_eq!(
        build_mysql_match_against(FtsAgainst::Null, &[MatchArgument::NonColumn]),
        Err(FtsError::NonColumn)
    );
    assert_eq!(
        build_mysql_match_against(
            FtsAgainst::String("word".into()),
            &[MatchArgument::NonStringColumn],
        ),
        Err(FtsError::NonStringColumn)
    );

    let null_mysql =
        build_mysql_match_against(FtsAgainst::Null, &[MatchArgument::StringColumn]).unwrap();
    assert_eq!(null_mysql.eval_real(), Ok(None));
    let mysql = build_mysql_match_against(
        FtsAgainst::String("word".into()),
        &[MatchArgument::StringColumn, MatchArgument::StringColumn],
    )
    .unwrap();
    assert_eq!(mysql.column_count(), 2);
    assert_eq!(mysql.eval_real(), Err(FtsError::MatchAgainstOutsideIndex));

    let mut signature = FtsSignature::Mysql(mysql);
    set_mysql_match_against_modifier(&mut signature, FulltextSearchModifier::Boolean).unwrap();
    assert_eq!(
        signature.mysql_modifier(),
        Some(FulltextSearchModifier::Boolean)
    );
    let mut wrong_signature = FtsSignature::MatchWord(match_word);
    assert_eq!(
        set_mysql_match_against_modifier(
            &mut wrong_signature,
            FulltextSearchModifier::QueryExpansion,
        ),
        Err(FtsError::UnexpectedSignature)
    );
}
