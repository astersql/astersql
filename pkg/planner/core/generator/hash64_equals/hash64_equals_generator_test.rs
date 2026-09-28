// Copyright 2026 AsterSQL.

use super::GenHash64Equals4LogicalOps;

#[test]
fn generated_output_matches_go_generator_contract() {
    let generated = GenHash64Equals4LogicalOps().expect("Rust generator should succeed");
    let committed = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../operator/logicalop/hash64_equals_generated.go"
    ))
    .expect("committed Go generator output should be readable");

    // The existing Rust API intentionally calls its receiver `p`; Go reflection emits
    // the semantically equivalent name `op`. Normalize that single lexical difference.
    let generated = String::from_utf8(generated).expect("generated Go should be UTF-8");
    let normalized = generated
        .replace("(p *", "(op *")
        .replace("p.", "op.")
        .replace("if p == nil", "if op == nil");
    let committed = String::from_utf8(committed).expect("committed Go should be UTF-8");
    for (index, (actual, expected)) in normalized.lines().zip(committed.lines()).enumerate() {
        assert_eq!(
            actual,
            expected,
            "Rust generator differs from Go at line {}",
            index + 1
        );
    }
    assert_eq!(
        normalized.lines().count(),
        committed.lines().count(),
        "Rust generator and Go output must have the same line count"
    );
    assert_eq!(
        normalized.len(),
        committed.len(),
        "Rust generator and Go output must have the same byte length"
    );
}
