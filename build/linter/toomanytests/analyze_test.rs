// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyze.rs");

mod compiled_toomanytests {
    mod token {
        use std::collections::HashMap;

        #[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
        pub struct Pos(pub i32);

        pub struct File {
            pub name: String,
        }

        impl File {
            pub fn Name(&self) -> &str {
                &self.name
            }
        }

        pub struct Position {
            pub Filename: String,
        }

        pub struct FileSet {
            pub files: HashMap<Pos, File>,
        }

        impl FileSet {
            pub fn File(&self, pos: Pos) -> Option<&File> {
                self.files.get(&pos)
            }

            pub fn Position(&self, pos: Pos) -> Position {
                Position {
                    Filename: self
                        .files
                        .get(&pos)
                        .map(|file| file.name.clone())
                        .unwrap_or_default(),
                }
            }
        }
    }

    mod ast {
        use super::token;

        pub struct Ident {
            pub Name: String,
        }

        pub struct FuncDecl {
            pub Name: Ident,
            pub Recv: Option<()>,
        }

        pub enum Decl {
            FuncDecl(FuncDecl),
            Other,
        }

        impl Decl {
            pub fn as_func_decl(&self) -> Option<&FuncDecl> {
                match self {
                    Self::FuncDecl(decl) => Some(decl),
                    Self::Other => None,
                }
            }
        }

        pub struct File {
            pub position: token::Pos,
            pub Decls: Vec<Decl>,
        }

        impl File {
            pub fn Pos(&self) -> token::Pos {
                self.position
            }
        }
    }

    mod analysis {
        use super::{ast, token};
        use std::any::Any;

        pub type Error = String;

        pub struct Analyzer {
            pub name: &'static str,
            pub doc: &'static str,
            pub requires: &'static [&'static Analyzer],
            pub run: fn(&mut Pass) -> Result<Option<Box<dyn Any>>, Error>,
        }

        pub struct Pass {
            pub Files: Vec<ast::File>,
            pub Fset: token::FileSet,
            pub reports: Vec<(token::Pos, String)>,
        }

        impl Pass {
            pub fn Reportf(&mut self, pos: token::Pos, message: &str) {
                self.reports.push((pos, message.to_string()));
            }
        }
    }

    mod strings {
        pub fn HasPrefix(value: &str, prefix: &str) -> bool {
            value.starts_with(prefix)
        }

        pub fn HasSuffix(value: &str, suffix: &str) -> bool {
            value.ends_with(suffix)
        }
    }

    mod filepath {
        pub fn Dir(path: &str) -> String {
            match path.rfind('/') {
                Some(0) => "/".to_string(),
                Some(index) => path[..index].to_string(),
                None => ".".to_string(),
            }
        }
    }

    mod util {
        use super::analysis;

        pub fn SkipAnalyzerByConfig(_analyzer: &analysis::Analyzer) {}
        pub fn SkipAnalyzer(_analyzer: &analysis::Analyzer) {}
    }

    mod implementation {
        use super::{analysis, filepath, strings, token, util};

        include!("analyze.rs");
    }

    fn function(name: &str, method: bool) -> ast::Decl {
        ast::Decl::FuncDecl(ast::FuncDecl {
            Name: ast::Ident {
                Name: name.to_string(),
            },
            Recv: method.then_some(()),
        })
    }

    #[test]
    fn helper_boundaries_match_go() {
        let test_file = token::File {
            name: "pkg/x/a_test.go".to_string(),
        };
        let ordinary_file = token::File {
            name: "pkg/x/a.go".to_string(),
        };
        assert!(implementation::isTestFile(&test_file));
        assert!(!implementation::isTestFile(&ordinary_file));
        assert_eq!(implementation::checkRule("pkg/planner/core"), 210);
        assert_eq!(implementation::checkRule("pkg/util/topsql/reporter"), 90);
        assert_eq!(implementation::checkRule("pkg/other"), 50);
    }

    #[test]
    fn run_counts_only_go_test_declarations_and_reports_strictly_above_limit() {
        use std::collections::HashMap;

        let mut declarations = (0..50)
            .map(|index| function(&format!("Test{index}"), false))
            .collect::<Vec<_>>();
        declarations.extend([
            function("TestMain", false),
            function("TestMethod", true),
            function("BenchmarkOther", false),
            ast::Decl::Other,
        ]);
        let mut pass = analysis::Pass {
            Files: vec![ast::File {
                position: token::Pos(7),
                Decls: declarations,
            }],
            Fset: token::FileSet {
                files: HashMap::from([(
                    token::Pos(7),
                    token::File {
                        name: "pkg/other/file_test.go".to_string(),
                    },
                )]),
            },
            reports: Vec::new(),
        };
        assert!(implementation::run(&mut pass).unwrap().is_none());
        assert!(pass.reports.is_empty(), "the limit itself is allowed");

        pass.Files[0].Decls.push(function("TestOverflow", false));
        assert!(implementation::run(&mut pass).unwrap().is_none());
        assert_eq!(
            pass.reports,
            vec![(
                token::Pos(7),
                "pkg/other: Too many test cases in one package: 51".to_string(),
            )]
        );
    }
}

#[test]
fn analyzer_uses_rust_api_shape_and_separate_run_entry() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"toomanytests\"",
        "doc: \"too many tests in the package\"",
        "requires: &[]",
        "run,",
        "pub fn run(pass: &mut analysis::Pass)",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("Run: Some("));
    assert!(!RUST_SOURCE.contains("Lazy<analysis::Analyzer>"));
}

#[test]
fn scan_matches_go_top_level_test_function_contract() {
    for required in [
        ".File(f.Pos())",
        ".expect(\"AST file position must belong to the pass FileSet\")",
        "if !isTestFile(astFile)",
        "for n in &f.Decls",
        "if let Some(funcDecl) = n.as_func_decl()",
        "strings::HasPrefix(&funcDecl.Name.Name, \"Test\")",
        "funcDecl.Recv.is_none()",
        "funcDecl.Name.Name != \"TestMain\"",
        "cnt += 1",
        "pos = f.Pos()",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn package_threshold_diagnostic_and_skip_wiring_match_go() {
    for required in [
        "\"pkg/planner/core\" => 210",
        "\"pkg/util/topsql/reporter\" =>",
        "_ => 50",
        "let pkgName = filepath::Dir(&pass.Fset.Position(pos).Filename)",
        "if cnt > checkRule(&pkgName)",
        "&format!(\"{}: Too many test cases in one package: {}\", pkgName, cnt)",
        "util::SkipAnalyzerByConfig(&Analyzer);",
        "util::SkipAnalyzer(&Analyzer);",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn completed_port_preserves_license_without_placeholders() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2023 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("这个草稿"));
}
