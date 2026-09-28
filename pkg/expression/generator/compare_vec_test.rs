// Copyright 2026 AsterSQL.

use crate::compare_vec::{COMPARES_MAP, HEADER, TYPES_MAP, generate_dot_go, generate_test_dot_go};

#[test]
fn generated_compare_source_preserves_go_coalesce_semantics() {
    let source = String::from_utf8(generate_dot_go(COMPARES_MAP, TYPES_MAP).unwrap()).unwrap();

    assert!(HEADER.contains("Unless required by applicable law or agreed to in writing"));
    assert!(source.contains(
        "func (b *builtinCoalesceRealSig) fallbackEvalReal(ctx EvalContext, input *chunk.Chunk, result *chunk.Column) error"
    ));
    assert!(source.contains("x[i] = res"));
    assert!(source.contains("i64s[i] = args[i]"));
    assert!(source.contains("fsp := b.tp.GetDecimal()"));
    assert!(source.contains("i64s[i].SetFsp(fsp)"));
    assert!(source.contains("bufs := make([]*chunk.Column, argLen)"));
    assert!(source.contains("result.AppendString(bufs[j].GetString(i))"));
    assert!(source.contains("if afterWarns > beforeWarns {"));
}

#[test]
fn generated_compare_tests_preserve_go_data_generators() {
    let source = String::from_utf8(generate_test_dot_go(COMPARES_MAP, TYPES_MAP).unwrap()).unwrap();

    assert!(source.contains("geners: []dataGenerator{"));
    assert_eq!(
        source
            .matches("gener{*newDefaultGener(0.2, types.ETReal)}")
            .count(),
        3
    );
    assert!(!source.contains(
        "ast.NullEQ: {\n\t\t{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETInt"
    ));
}
