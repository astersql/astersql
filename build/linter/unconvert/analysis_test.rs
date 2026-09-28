// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analysis.rs");

mod compiled_unconvert {
    mod token {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Token(pub u8);

        pub type Pos = usize;
        pub const NoPos: Pos = 0;
        pub const SHL: Token = Token(1);
        pub const SHR: Token = Token(2);
        pub const EQL: Token = Token(3);
        pub const NEQ: Token = Token(4);
        pub const LSS: Token = Token(5);
        pub const GTR: Token = Token(6);
        pub const LEQ: Token = Token(7);
        pub const GEQ: Token = Token(8);
        pub const ADD: Token = Token(9);
        pub const SUB: Token = Token(10);
        pub const MUL: Token = Token(11);
        pub const QUO: Token = Token(12);
        pub const REM: Token = Token(13);
        pub const AND: Token = Token(14);
        pub const OR: Token = Token(15);
        pub const XOR: Token = Token(16);
        pub const AND_NOT: Token = Token(17);
        pub const LAND: Token = Token(18);
        pub const LOR: Token = Token(19);
        pub const NOT: Token = Token(20);
        pub const OTHER: Token = Token(21);
    }

    mod ast {
        use super::token;

        #[derive(Clone, Debug)]
        pub struct Ident {
            pub id: usize,
            pub Name: String,
        }

        #[derive(Clone, Debug)]
        pub struct BinaryExpr {
            pub Op: token::Token,
            pub X: Box<Expr>,
            pub Y: Box<Expr>,
        }

        #[derive(Clone, Debug)]
        pub struct UnaryExpr {
            pub Op: token::Token,
            pub X: Box<Expr>,
        }

        #[derive(Clone, Debug)]
        pub struct ParenExpr {
            pub X: Box<Expr>,
        }

        #[derive(Clone, Debug)]
        pub struct SelectorExpr {
            pub Sel: Box<Expr>,
        }

        #[derive(Clone, Debug)]
        pub struct BasicLit {
            pub id: usize,
        }

        #[derive(Clone, Debug)]
        pub struct CallExpr {
            pub Fun: Expr,
            pub Args: Vec<Expr>,
            pub Ellipsis: token::Pos,
            pub position: usize,
        }

        impl CallExpr {
            pub fn Pos(&self) -> usize {
                self.position
            }
        }

        impl Default for CallExpr {
            fn default() -> Self {
                Self {
                    Fun: Expr::Other { id: 0 },
                    Args: Vec::new(),
                    Ellipsis: token::NoPos,
                    position: 0,
                }
            }
        }

        #[derive(Clone, Debug)]
        pub enum Expr {
            BinaryExpr(BinaryExpr),
            UnaryExpr(UnaryExpr),
            BasicLit(BasicLit),
            ParenExpr(ParenExpr),
            SelectorExpr(SelectorExpr),
            Ident(Ident),
            CallExpr(Box<CallExpr>),
            Other { id: usize },
        }

        impl Expr {
            pub fn key(&self) -> usize {
                match self {
                    Self::BasicLit(literal) => literal.id,
                    Self::Other { id } => *id,
                    Self::Ident(ident) => ident.id,
                    Self::BinaryExpr(expr) => expr.X.key(),
                    Self::UnaryExpr(expr) => expr.X.key(),
                    Self::ParenExpr(expr) => expr.X.key(),
                    Self::SelectorExpr(expr) => expr.Sel.key(),
                    Self::CallExpr(expr) => expr.position,
                }
            }

            pub fn as_ident(&self) -> Option<&Ident> {
                match self {
                    Self::Ident(ident) => Some(ident),
                    _ => None,
                }
            }

            pub fn as_paren_expr(&self) -> Option<&ParenExpr> {
                match self {
                    Self::ParenExpr(expr) => Some(expr),
                    _ => None,
                }
            }
        }

        #[derive(Clone, Debug)]
        pub enum Node {
            CallExpr(CallExpr),
            Other,
        }
    }

    mod types {
        use super::ast;

        pub const IsUntyped: u32 = 1;

        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Basic {
            info: u32,
        }

        impl Basic {
            pub fn new(info: u32) -> Self {
                Self { info }
            }

            pub fn Info(&self) -> u32 {
                self.info
            }
        }

        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum Type {
            Basic(Basic),
            Named(&'static str),
        }

        impl Type {
            pub fn as_basic(&self) -> Option<&Basic> {
                match self {
                    Self::Basic(basic) => Some(basic),
                    _ => None,
                }
            }
        }

        #[derive(Clone, Debug)]
        pub struct TypeAndValue {
            pub Type: Type,
            pub is_type: bool,
        }

        impl TypeAndValue {
            pub fn IsType(&self) -> bool {
                self.is_type
            }
        }

        #[derive(Clone, Debug)]
        pub struct Builtin {
            name: String,
        }

        impl Builtin {
            pub fn Name(&self) -> &str {
                &self.name
            }
        }

        #[derive(Clone, Debug)]
        pub struct Package;

        #[derive(Clone, Debug)]
        pub struct Object {
            package: Option<Package>,
            name: String,
            typ: Type,
            builtin: Option<Builtin>,
        }

        impl Object {
            pub fn nil() -> Self {
                Self {
                    package: None,
                    name: "nil".to_string(),
                    typ: Type::Basic(Basic::new(IsUntyped)),
                    builtin: None,
                }
            }

            pub fn constant(name: &str, untyped: bool) -> Self {
                Self {
                    package: Some(Package),
                    name: name.to_string(),
                    typ: Type::Basic(Basic::new(if untyped { IsUntyped } else { 0 })),
                    builtin: None,
                }
            }

            pub fn builtin(name: &str) -> Self {
                Self {
                    package: None,
                    name: name.to_string(),
                    typ: Type::Named("builtin"),
                    builtin: Some(Builtin {
                        name: name.to_string(),
                    }),
                }
            }

            pub fn Pkg(&self) -> Option<&Package> {
                self.package.as_ref()
            }

            pub fn Name(&self) -> &str {
                &self.name
            }

            pub fn Type(&self) -> &Type {
                &self.typ
            }

            pub fn as_builtin(&self) -> Option<&Builtin> {
                self.builtin.as_ref()
            }
        }

        #[derive(Default)]
        pub struct TypeMap(pub Vec<(usize, TypeAndValue)>);

        impl TypeMap {
            pub fn get(&self, expr: &ast::Expr) -> Option<&TypeAndValue> {
                self.0
                    .iter()
                    .find(|(id, _)| *id == expr.key())
                    .map(|(_, value)| value)
            }
        }

        #[derive(Default)]
        pub struct UseMap(pub Vec<(usize, Object)>);

        impl UseMap {
            pub fn get(&self, ident: &ast::Ident) -> Option<&Object> {
                self.0
                    .iter()
                    .find(|(id, _)| *id == ident.id)
                    .map(|(_, object)| object)
            }
        }

        #[derive(Default)]
        pub struct Info {
            pub Types: TypeMap,
            pub Uses: UseMap,
        }

        pub fn Identical(left: &Type, right: &Type) -> bool {
            left == right
        }
    }

    mod analysis {
        use super::types;
        use std::any::Any;

        pub type Error = String;

        pub struct Analyzer {
            pub name: &'static str,
            pub doc: &'static str,
            pub requires: &'static [&'static Analyzer],
            pub run: fn(&mut Pass) -> Result<Option<Box<dyn Any>>, Error>,
        }

        pub struct ResultMap {
            pub value: &'static dyn Any,
        }

        impl ResultMap {
            pub fn get(&self, _analyzer: &Analyzer) -> Option<&'static dyn Any> {
                Some(self.value)
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

        pub struct Inspector {
            pub nodes: Vec<ast::Node>,
        }

        impl Inspector {
            pub fn Preorder<F>(&self, _filter: Vec<ast::Node>, mut visit: F)
            where
                F: FnMut(ast::Node),
            {
                for node in self.nodes.clone() {
                    visit(node);
                }
            }
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
        use super::{analysis, ast, inspect, inspector, token, types, util};

        #[rustfmt::skip]
        include!("analysis.rs");
    }

    fn ident(id: usize, name: &str) -> ast::Expr {
        ast::Expr::Ident(ast::Ident {
            id,
            Name: name.to_string(),
        })
    }

    fn basic(id: usize) -> ast::Expr {
        ast::Expr::BasicLit(ast::BasicLit { id })
    }

    fn binary(op: token::Token, left: ast::Expr, right: ast::Expr) -> ast::Expr {
        ast::Expr::BinaryExpr(ast::BinaryExpr {
            Op: op,
            X: Box::new(left),
            Y: Box::new(right),
        })
    }

    fn call(fun: ast::Expr, args: Vec<ast::Expr>, position: usize) -> ast::Expr {
        ast::Expr::CallExpr(Box::new(ast::CallExpr {
            Fun: fun,
            Args: args,
            Ellipsis: token::NoPos,
            position,
        }))
    }

    fn typed(typ: types::Type, is_type: bool) -> types::TypeAndValue {
        types::TypeAndValue { Type: typ, is_type }
    }

    #[test]
    fn untyped_value_recursion_matches_every_go_expression_branch() {
        let info = types::Info {
            Uses: types::UseMap(vec![
                (20, types::Object::nil()),
                (21, types::Object::constant("constant", true)),
                (22, types::Object::constant("variable", false)),
                (30, types::Object::builtin("real")),
                (31, types::Object::builtin("imag")),
                (32, types::Object::builtin("complex")),
                (33, types::Object::builtin("len")),
            ]),
            ..Default::default()
        };

        assert!(implementation::isUntypedValue(&basic(1), &info));
        assert!(implementation::isUntypedValue(
            &binary(token::SHL, basic(2), ident(22, "variable")),
            &info,
        ));
        assert!(implementation::isUntypedValue(
            &binary(token::EQL, ident(22, "variable"), ident(22, "variable")),
            &info,
        ));
        assert!(implementation::isUntypedValue(
            &binary(token::ADD, basic(3), basic(4)),
            &info,
        ));
        assert!(!implementation::isUntypedValue(
            &binary(token::LOR, basic(5), ident(22, "variable")),
            &info,
        ));

        let unary = ast::Expr::UnaryExpr(ast::UnaryExpr {
            Op: token::NOT,
            X: Box::new(basic(6)),
        });
        assert!(implementation::isUntypedValue(&unary, &info));
        let unsupported_unary = ast::Expr::UnaryExpr(ast::UnaryExpr {
            Op: token::OTHER,
            X: Box::new(basic(7)),
        });
        assert!(!implementation::isUntypedValue(&unsupported_unary, &info));

        let paren = ast::Expr::ParenExpr(ast::ParenExpr {
            X: Box::new(basic(8)),
        });
        assert!(implementation::isUntypedValue(&paren, &info));
        let selector = ast::Expr::SelectorExpr(ast::SelectorExpr {
            Sel: Box::new(basic(9)),
        });
        assert!(implementation::isUntypedValue(&selector, &info));
        assert!(implementation::isUntypedValue(&ident(20, "nil"), &info));
        assert!(implementation::isUntypedValue(
            &ident(21, "constant"),
            &info,
        ));
        assert!(!implementation::isUntypedValue(
            &ident(22, "variable"),
            &info,
        ));
        assert!(!implementation::isUntypedValue(
            &ident(23, "missing"),
            &info
        ));

        assert!(implementation::isUntypedValue(
            &call(ident(30, "real"), vec![basic(10)], 40),
            &info,
        ));
        assert!(implementation::isUntypedValue(
            &call(ident(31, "imag"), vec![basic(11)], 41),
            &info,
        ));
        assert!(implementation::isUntypedValue(
            &call(ident(32, "complex"), vec![basic(12), basic(13)], 42),
            &info,
        ));
        assert!(!implementation::isUntypedValue(
            &call(
                ident(32, "complex"),
                vec![basic(14), ident(22, "variable")],
                43,
            ),
            &info,
        ));
        assert!(!implementation::isUntypedValue(
            &call(ident(33, "len"), vec![basic(15)], 44),
            &info,
        ));
    }

    #[test]
    fn builtin_lookup_strips_parentheses_and_rejects_non_builtins() {
        let info = types::Info {
            Uses: types::UseMap(vec![
                (1, types::Object::builtin("real")),
                (2, types::Object::constant("real", false)),
            ]),
            ..Default::default()
        };
        let nested = ast::Expr::ParenExpr(ast::ParenExpr {
            X: Box::new(ast::Expr::ParenExpr(ast::ParenExpr {
                X: Box::new(ident(1, "real")),
            })),
        });

        assert_eq!(
            implementation::asBuiltin(&nested, &info).unwrap().Name(),
            "real"
        );
        assert!(implementation::asBuiltin(&ident(2, "real"), &info).is_none());
        assert!(implementation::asBuiltin(&ident(3, "missing"), &info).is_none());
        assert!(implementation::asBuiltin(&basic(4), &info).is_none());
    }

    #[test]
    fn run_preserves_go_filters_error_paths_and_diagnostics() {
        let int_type = types::Type::Named("int");
        let string_type = types::Type::Named("string");
        let calls = vec![
            ast::Node::Other,
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(1, "int"),
                Args: Vec::new(),
                Ellipsis: token::NoPos,
                position: 1,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(2, "int"),
                Args: vec![ident(102, "x")],
                Ellipsis: 99,
                position: 2,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(3, "int"),
                Args: vec![ident(103, "x")],
                Ellipsis: token::NoPos,
                position: 3,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(4, "ordinaryFunction"),
                Args: vec![ident(104, "x")],
                Ellipsis: token::NoPos,
                position: 4,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(5, "int"),
                Args: vec![ident(105, "x")],
                Ellipsis: token::NoPos,
                position: 5,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(6, "int"),
                Args: vec![ident(106, "stringValue")],
                Ellipsis: token::NoPos,
                position: 6,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(7, "int"),
                Args: vec![basic(107)],
                Ellipsis: token::NoPos,
                position: 7,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(8, "_cgoCheckPointer"),
                Args: vec![ident(108, "x")],
                Ellipsis: token::NoPos,
                position: 8,
            }),
            ast::Node::CallExpr(ast::CallExpr {
                Fun: ident(9, "int"),
                Args: vec![ident(109, "x")],
                Ellipsis: token::NoPos,
                position: 9,
            }),
        ];
        let inspector: &'static inspector::Inspector =
            Box::leak(Box::new(inspector::Inspector { nodes: calls }));
        let mut pass = analysis::Pass {
            ResultOf: analysis::ResultMap { value: inspector },
            TypesInfo: types::Info {
                Types: types::TypeMap(vec![
                    (2, typed(int_type.clone(), true)),
                    (102, typed(int_type.clone(), false)),
                    (103, typed(int_type.clone(), false)),
                    (4, typed(int_type.clone(), false)),
                    (104, typed(int_type.clone(), false)),
                    (5, typed(int_type.clone(), true)),
                    (6, typed(int_type.clone(), true)),
                    (106, typed(string_type, false)),
                    (7, typed(int_type.clone(), true)),
                    (107, typed(int_type.clone(), false)),
                    (8, typed(int_type.clone(), true)),
                    (108, typed(int_type.clone(), false)),
                    (9, typed(int_type.clone(), true)),
                    (109, typed(int_type, false)),
                ]),
                Uses: types::UseMap::default(),
            },
            reports: Vec::new(),
        };

        assert!(implementation::run(&mut pass).unwrap().is_none());
        assert_eq!(
            pass.reports,
            vec![
                (3, "missing type".to_string()),
                (5, "missing type".to_string()),
                (9, "unnecessary conversion".to_string()),
            ]
        );
    }
}

#[test]
fn analyzer_has_a_compilable_rust_api_shape() {
    for required in [
        "// Copyright 2026 AsterSQL.",
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: Name",
        "doc: \"Remove unnecessary type conversions\"",
        "requires: &[&inspect::Analyzer]",
        "run,",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }

    assert!(!RUST_SOURCE.contains("Lazy<analysis::Analyzer>"));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
}

#[test]
fn conversion_filters_and_untyped_rules_match_go_source() {
    for required in [
        "call.Args.len() != 1 || call.Ellipsis != token::NoPos",
        "pass.Reportf(call.Pos(), \"missing type\")",
        "if !ft.IsType()",
        "if !types::Identical(&ft.Type, &at.Type)",
        "if isUntypedValue(&call.Args[0], &pass.TypesInfo)",
        "ident.Name == \"_cgoCheckPointer\"",
        "pass.Reportf(call.Pos(), \"unnecessary conversion\")",
        "token::SHL | token::SHR",
        "token::EQL | token::NEQ | token::LSS | token::GTR | token::LEQ | token::GEQ",
        "isUntypedValue(&n.X, info) && isUntypedValue(&n.Y, info)",
        "obj.Pkg().is_none() && obj.Name() == \"nil\"",
        "b.Info() & types::IsUntyped != 0",
        "\"real\" | \"imag\"",
        "\"complex\"",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
