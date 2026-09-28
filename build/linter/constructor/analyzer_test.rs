// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

const RUST_SOURCE: &str = include_str!("analyzer.rs");
const RUST_CONSTRUCT_FIXTURE: &str = include_str!("testdata/src/t/construct.rs");
const RUST_FLAG_FIXTURE: &str = include_str!(
    "testdata/src/github.com/pingcap/tidb/pkg/util/linter/constructor/constructorflag.rs"
);

#[test]
fn analyzer_wiring_uses_rust_api_shape_and_go_skip_contract() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"constructor\"",
        "doc: \"Check developers don't create structs manually without using constructors\"",
        "requires: &[]",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn required_type_information_is_unwrapped_before_underlying_access() {
    assert!(RUST_SOURCE.contains("expect(\"composite literal type information is required\")"));
    assert!(RUST_SOURCE.contains("expect(\"call expression type information is required\")"));
    assert!(RUST_SOURCE.contains("let Some(t) = pass.TypesInfo.TypeOf(n.Type) else"));
    assert!(!RUST_SOURCE.contains("TypeOf(n).Underlying()"));
    assert!(!RUST_SOURCE.contains("t.unwrap().Underlying()"));
}

#[test]
fn named_marker_matching_preserves_go_object_and_package_requirements() {
    assert!(RUST_SOURCE.contains("expect(\"named type must have an object\")"));
    assert!(RUST_SOURCE.contains("expect(\"constructor marker type must have a package\")"));
    assert!(
        RUST_SOURCE.contains("obj.Name() == \"Constructor\" && pkg.Path() == ConstructorUtilPath")
    );
}

#[test]
fn analyzer_preserves_all_go_detection_branches() {
    for required in [
        "ptr.Elem().Underlying().as_struct()",
        "ignoreFields.contains_key(field.Name())",
        "strings::Split(ctorTag.Value(), \",\")",
        "ctors.extend(getConstructorList(fieldStruct, None))",
        "for i in (0..stack.len()).rev()",
        "if !push",
        "ast::Expr::KeyValueExpr",
        "if fun.Name != \"new\" || n.Args.is_empty()",
        "if t.as_pointer().is_some()",
        "ast::Node::CompositeLit",
        "ast::Node::CallExpr",
        "ast::Node::ValueSpec",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn rust_fixtures_cover_the_same_go_diagnostic_matrix_and_marker() {
    assert_eq!(
        RUST_CONSTRUCT_FIXTURE
            .matches("want `struct can only be constructed in constructors")
            .count(),
        11
    );
    for required in [
        "NewStructWithSpecificConstructor",
        "AnotherConstructor",
        "AnonymousStructFixture",
        "compositeImplicitInitiate1",
        "compositeImplicitInitiate2",
    ] {
        assert!(
            RUST_CONSTRUCT_FIXTURE.contains(required),
            "missing fixture: {required}"
        );
    }
    assert!(RUST_FLAG_FIXTURE.contains("pub struct Constructor;"));
    assert!(RUST_SOURCE.contains("github.com/pingcap/tidb/pkg/util/linter/constructor"));
}
