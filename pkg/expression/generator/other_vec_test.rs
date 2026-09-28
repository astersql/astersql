// Copyright 2026 AsterSQL.

use crate::other_vec::{IN_SIGS_TMPL, generate_dot_go, generate_test_dot_go};
use std::io::Write;
use std::process::{Command, Stdio};

fn assert_gofmt_accepts(source: &[u8]) {
    let mut child = Command::new("gofmt")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("gofmt is required by the Go generator");
    child.stdin.take().unwrap().write_all(source).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "generated Go is invalid: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn production_template_preserves_go_error_and_type_branches() {
    let bytes = generate_dot_go().unwrap();
    assert_gofmt_accepts(&bytes);
    let source = String::from_utf8(bytes).unwrap();

    assert_eq!(
        source.matches("func (b *builtinIn").count(),
        IN_SIGS_TMPL.len() * 2
    );
    assert!(source.contains("isUnsigned := mysql.HasUnsignedFlag(args[j].GetType(ctx).GetFlag())"));
    assert!(source.contains("key, err := arg0.ToHashKey()"));
    assert!(source.contains("if err != nil {\n\t\t\t\treturn err\n\t\t\t}"));
    assert!(!source.contains("if err != nil { return false }"));

    let json = source
        .split("func (b *builtinInJSONSig) vecEvalInt")
        .nth(1)
        .expect("JSON specialization")
        .split("func (b *builtinInJSONSig) vectorized")
        .next()
        .unwrap();
    assert!(!json.contains("if len(b.hashSet) != 0"));
    assert!(!json.contains("if b.hasNull"));

    for comparison in [
        "compareSignedAndUnsignedInts",
        "arg0.Compare(&arg1)",
        "arg0.Compare(arg1)",
        "cmp.Compare(arg0, arg1)",
        "types.CompareBinaryJSON(arg0, arg1)",
        "types.CompareString(arg0, arg1, b.collation)",
    ] {
        assert!(source.contains(comparison), "missing {comparison}");
    }
}

#[test]
fn generated_tests_preserve_go_generators_constants_and_entry_points() {
    let bytes = generate_test_dot_go().expect("test generator should render");
    assert_gofmt_accepts(&bytes);
    let source = String::from_utf8(bytes).expect("test output should be UTF-8");

    assert_eq!(
        source.matches("retEvalType:").count(),
        IN_SIGS_TMPL.len() * 2
    );
    assert_eq!(
        source.matches("geners: []dataGenerator{").count(),
        IN_SIGS_TMPL.len()
    );
    assert_eq!(
        source.matches("constants: []*Constant{").count(),
        IN_SIGS_TMPL.len()
    );
    for literal in [
        "types.NewDatum(1)",
        "types.NewStringDatum(\"aaaa\")",
        "dateTimeFromString(\"2019-01-01\")",
        "types.CreateBinaryJSON(\"aaaa\")",
        "time.Duration(1000)",
        "types.NewFloat64Datum(0.1)",
        "types.NewDecFromInt(10)",
    ] {
        assert!(source.contains(literal), "missing {literal}");
    }
    assert!(!source.contains("generatedInConstantsFor"));
    assert!(source.contains("if err := d.FromFloat64(f); err != nil"));
    assert!(source.contains("if err := j.UnmarshalJSON([]byte(jsonStr)); err != nil"));
    assert!(source.contains("func TestVectorizedBuiltinOtherEvalOneVecGenerated"));
    assert!(source.contains("func BenchmarkVectorizedBuiltinOtherFuncGenerated"));
}
