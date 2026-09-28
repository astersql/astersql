// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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
const EXPRESSION_RUST: &str =
    include_str!("testdata/src/github.com/pingcap/tidb/pkg/expression/expression.rs");
const EXPRESSION_GO: &str =
    include_str!("testdata/src/github.com/pingcap/tidb/pkg/expression/expression.go");
const CROSS_PACKAGE_RUST: &str = include_str!("testdata/src/t/test_file.rs");
const CROSS_PACKAGE_GO: &str = include_str!("testdata/src/t/test_file.go");

mod compiled_printexpression {
    mod ast {
        #[derive(Default)]
        pub struct Ident {
            pub Name: String,
        }

        pub struct SelectorExpr {
            pub Sel: Ident,
            pub X: Box<Expr>,
        }

        pub enum Expr {
            SelectorExpr(SelectorExpr),
            Ident(Ident),
            Other,
        }

        impl Expr {
            pub fn Pos(&self) -> usize {
                0
            }
        }

        pub struct CallExpr {
            pub Fun: Expr,
            pub Args: Vec<Expr>,
        }

        impl Default for CallExpr {
            fn default() -> Self {
                Self {
                    Fun: Expr::Other,
                    Args: Vec::new(),
                }
            }
        }

        pub enum Node {
            CallExpr(CallExpr),
            Other,
        }
    }

    mod analysis {
        use super::{ast, types};
        use std::any::Any;

        pub type Error = String;

        pub struct Analyzer {
            pub name: &'static str,
            pub doc: &'static str,
            pub requires: &'static [&'static Analyzer],
            pub run: fn(&mut Pass) -> Result<Option<Box<dyn Any>>, Error>,
        }

        pub struct ResultMap;

        impl ResultMap {
            pub fn get(&self, _analyzer: &Analyzer) -> Option<&'static Box<dyn Any>> {
                None
            }
        }

        pub struct Pass {
            pub ResultOf: ResultMap,
            pub TypesInfo: types::Info,
            pub reports: Vec<(usize, String)>,
        }

        impl Pass {
            pub fn Reportf(&mut self, position: usize, message: &str) {
                self.reports.push((position, message.to_string()));
            }
        }
    }

    mod inspector {
        use super::ast;

        pub struct Inspector;

        impl Inspector {
            pub fn Preorder<F>(&self, _filter: Vec<ast::Node>, _visit: F)
            where
                F: FnMut(ast::Node),
            {
            }
        }
    }

    mod types {
        use super::{ast, implementation};
        use std::any::Any;

        pub struct Func(pub &'static str);

        impl Func {
            pub fn Name(&self) -> &str {
                self.0
            }
        }

        pub enum Kind<'a> {
            Pointer(&'a Pointer),
            Slice(&'a Slice),
            Other,
        }

        pub trait Type: Any {
            fn as_any(&self) -> &dyn Any;
            fn kind(&self) -> Kind<'_>;
            fn as_method_lookup(&self) -> Option<&dyn implementation::methodLookup>;
        }

        pub struct Plain;

        impl Type for Plain {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn kind(&self) -> Kind<'_> {
                Kind::Other
            }

            fn as_method_lookup(&self) -> Option<&dyn implementation::methodLookup> {
                None
            }
        }

        pub struct Pointer(pub Box<dyn Type>);

        impl Pointer {
            pub fn Elem(&self) -> &dyn Type {
                self.0.as_ref()
            }
        }

        impl Type for Pointer {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn kind(&self) -> Kind<'_> {
                Kind::Pointer(self)
            }

            fn as_method_lookup(&self) -> Option<&dyn implementation::methodLookup> {
                None
            }
        }

        pub struct Slice(pub Box<dyn Type>);

        impl Slice {
            pub fn Elem(&self) -> &dyn Type {
                self.0.as_ref()
            }
        }

        impl Type for Slice {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn kind(&self) -> Kind<'_> {
                Kind::Slice(self)
            }

            fn as_method_lookup(&self) -> Option<&dyn implementation::methodLookup> {
                None
            }
        }

        pub struct Interface {
            pub methods: Vec<Func>,
        }

        impl Type for Interface {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn kind(&self) -> Kind<'_> {
                Kind::Other
            }

            fn as_method_lookup(&self) -> Option<&dyn implementation::methodLookup> {
                Some(self)
            }
        }

        pub struct Named {
            pub methods: Vec<Func>,
            pub underlying: Box<dyn Type>,
        }

        impl Type for Named {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn kind(&self) -> Kind<'_> {
                Kind::Other
            }

            fn as_method_lookup(&self) -> Option<&dyn implementation::methodLookup> {
                Some(self)
            }
        }

        pub struct TypeAndValue {
            pub Type: Option<Box<dyn Type>>,
        }

        pub struct TypeMap {
            pub value: Option<TypeAndValue>,
        }

        impl TypeMap {
            pub fn get(&self, _expr: &ast::Expr) -> Option<&TypeAndValue> {
                self.value.as_ref()
            }
        }

        pub struct Info {
            pub Types: TypeMap,
        }
    }

    mod inspect {
        use super::analysis;

        fn run(
            _pass: &mut analysis::Pass,
        ) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
            Ok(None)
        }

        pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
            name: "inspect",
            doc: "inspect",
            requires: &[],
            run,
        };
    }

    mod util {
        use super::analysis;

        pub fn SkipAnalyzerByConfig(_analyzer: &analysis::Analyzer) {}
        pub fn SkipAnalyzer(_analyzer: &analysis::Analyzer) {}
    }

    mod implementation {
        use super::{analysis, ast, inspect, inspector, types, util};

        include!("analyzer.rs");

        impl methodLookup for types::Interface {
            fn NumMethods(&self) -> usize {
                self.methods.len()
            }

            fn Method(&self, i: usize) -> &types::Func {
                &self.methods[i]
            }

            fn Underlying(&self) -> &dyn types::Type {
                self
            }
        }

        impl methodLookup for types::Named {
            fn NumMethods(&self) -> usize {
                self.methods.len()
            }

            fn Method(&self, i: usize) -> &types::Func {
                &self.methods[i]
            }

            fn Underlying(&self) -> &dyn types::Type {
                self.underlying.as_ref()
            }
        }
    }

    fn selector(receiver: ast::Expr, method: &str) -> ast::Expr {
        ast::Expr::SelectorExpr(ast::SelectorExpr {
            Sel: ast::Ident {
                Name: method.to_string(),
            },
            X: Box::new(receiver),
        })
    }

    fn ident(name: &str) -> ast::Expr {
        ast::Expr::Ident(ast::Ident {
            Name: name.to_string(),
        })
    }

    fn info(typ: Option<Box<dyn types::Type>>) -> types::Info {
        types::Info {
            Types: types::TypeMap {
                value: Some(types::TypeAndValue { Type: typ }),
            },
        }
    }

    #[test]
    fn format_target_detection_matches_go_switch() {
        for method in ["Printf", "Sprintf", "Println"] {
            assert!(implementation::funcIsFormat(&selector(
                ident("fmt"),
                method
            )));
            assert!(!implementation::funcIsFormat(&selector(
                ident("other"),
                method
            )));
        }
        for method in [
            "GenWithStack",
            "GenWithStackByArgs",
            "FastGen",
            "FastGenByArgs",
        ] {
            assert!(implementation::funcIsFormat(&selector(
                ast::Expr::Other,
                method
            )));
        }
        assert!(!implementation::funcIsFormat(&selector(
            ident("fmt"),
            "Print"
        )));
        assert!(!implementation::funcIsFormat(&ast::Expr::Other));
    }

    #[test]
    fn type_rule_handles_missing_pointer_slice_and_underlying_interface() {
        let expr = ast::Expr::Other;
        assert!(!implementation::argIsNotAllowed(&info(None), &expr));

        let constant = types::Named {
            methods: vec![types::Func("StringWithCtx")],
            underlying: Box::new(types::Plain),
        };
        let nested: Box<dyn types::Type> =
            Box::new(types::Pointer(Box::new(types::Slice(Box::new(constant)))));
        assert!(implementation::argIsNotAllowed(&info(Some(nested)), &expr));

        let column = types::Named {
            methods: vec![types::Func("StringWithCtx"), types::Func("String")],
            underlying: Box::new(types::Plain),
        };
        assert!(!implementation::argIsNotAllowed(
            &info(Some(Box::new(column))),
            &expr,
        ));

        let alias = types::Named {
            methods: Vec::new(),
            underlying: Box::new(types::Interface {
                methods: vec![types::Func("StringWithCtx")],
            }),
        };
        assert!(implementation::argIsNotAllowed(
            &info(Some(Box::new(alias))),
            &expr,
        ));
    }
}

#[test]
fn analyzer_dependency_and_result_lookup_preserve_go_identity() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"printexpression\"",
        "doc: \"Avoid printing expression directly.\"",
        "requires: &[&inspect::Analyzer]",
        ".get(&inspect::Analyzer)",
        ".downcast_ref::<inspector::Inspector>()",
        "ast::Node::CallExpr",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn format_entry_points_and_receiver_guard_match_go() {
    for required in [
        "\"Printf\" | \"Sprintf\" | \"Println\"",
        "i.Name == \"fmt\"",
        "\"GenWithStack\" | \"GenWithStackByArgs\" | \"FastGen\" | \"FastGenByArgs\"",
        "for arg in &expr.Args",
        "argIsNotAllowed(&pass.TypesInfo, arg)",
        "avoid printing expression directly. Please use `Expression.StringWithCtx()` to get a string",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn method_walk_is_safe_and_preserves_underlying_interface_fallback() {
    for required in [
        "fn Method(&self, i: usize) -> &types::Func",
        "pub trait methodLookup: types::Type",
        "elementType(typ.as_ref()).as_method_lookup()",
        "let name = typ.Method(i).Name();",
        "if implStringWithCtx && !implString",
        "typ.as_any().is::<types::Interface>()",
        "typ.Underlying().as_any().downcast_ref::<types::Interface>()",
        "return typIsNotAllowed(iface)",
        "types::Kind::Pointer(pointer) => elementType(pointer.Elem())",
        "types::Kind::Slice(slice) => elementType(slice.Elem())",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("*const types::Func"));
    assert!(!RUST_SOURCE.contains("unsafe"));
}

#[test]
fn fixtures_keep_the_go_diagnostic_matrix() {
    assert_eq!(EXPRESSION_GO.matches("// want `").count(), 3);
    assert_eq!(EXPRESSION_RUST.matches("// want `").count(), 3);
    assert_eq!(CROSS_PACKAGE_GO.matches("// want `").count(), 3);
    assert_eq!(CROSS_PACKAGE_RUST.matches("// want `").count(), 3);

    for required in [
        "pub trait Expression",
        "fn StringWithCtx(&self)",
        "impl Expression for Constant",
        "impl Column",
        "pub fn String(&self)",
        "fmt::Printf(\"%v\", conList)",
    ] {
        assert!(EXPRESSION_RUST.contains(required), "missing: {required}");
    }
    for required in [
        "pub struct EmbeddedConstant",
        "pub Constant: expression::Constant",
        "pub trait ExpressionWithString: expression::Expression",
        "fmt::Println(exprWithString)",
        "fmt::Println(col)",
        "fmt::Println(embedded)",
    ] {
        assert!(CROSS_PACKAGE_RUST.contains(required), "missing: {required}");
    }
}

#[test]
fn completed_port_preserves_licenses_without_placeholder_claims() {
    for source in [RUST_SOURCE, EXPRESSION_RUST, CROSS_PACKAGE_RUST] {
        assert!(source.starts_with("// Copyright 2026 AsterSQL."));
        assert!(source.contains("Copyright 2024 PingCAP, Inc."));
        assert!(!source.contains("当前不保证可编译"));
        assert!(!source.contains("测试草稿"));
    }
}
