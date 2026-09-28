// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("util.rs");

mod compiled_util {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    pub mod token {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct Pos(pub i32);

        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub struct Position {
            pub Filename: String,
            pub Line: i32,
        }

        #[derive(Clone, Debug)]
        pub struct File {
            name: String,
            base: i32,
            size: usize,
            lines: Vec<usize>,
        }

        impl File {
            pub fn Base(&self) -> i32 {
                self.base
            }

            pub fn SetLinesForContent(&mut self, content: &[u8]) {
                self.lines = vec![0];
                self.lines.extend(
                    content
                        .iter()
                        .enumerate()
                        .filter_map(|(offset, byte)| (*byte == b'\n').then_some(offset + 1)),
                );
            }
        }

        #[derive(Clone, Debug)]
        pub struct FileSet {
            files: Vec<Box<File>>,
            next_base: i32,
        }

        impl Default for FileSet {
            fn default() -> Self {
                Self {
                    files: Vec::new(),
                    next_base: 1,
                }
            }
        }

        impl FileSet {
            pub fn AddFile(&mut self, name: &str, base: i32, size: usize) -> *mut File {
                let base = if base < 0 { self.next_base } else { base };
                self.next_base = base + size as i32 + 1;
                let mut file = Box::new(File {
                    name: name.to_string(),
                    base,
                    size,
                    lines: vec![0],
                });
                let pointer = file.as_mut() as *mut File;
                self.files.push(file);
                pointer
            }

            pub fn PositionFor(&self, pos: Pos, _adjusted: bool) -> Position {
                for file in &self.files {
                    let offset = pos.0 - file.base;
                    if offset >= 0 && offset as usize <= file.size {
                        let line = file
                            .lines
                            .iter()
                            .take_while(|line_start| **line_start <= offset as usize)
                            .count() as i32;
                        return Position {
                            Filename: file.name.clone(),
                            Line: line.max(1),
                        };
                    }
                }
                Position::default()
            }
        }
    }

    pub mod ast {
        use super::token;

        #[derive(Clone)]
        pub struct Comment {
            pub Text: String,
        }

        #[derive(Clone)]
        pub struct CommentGroup {
            pub List: Vec<*mut Comment>,
        }

        #[derive(Clone)]
        pub struct Node {
            position: token::Pos,
        }

        impl Node {
            pub fn new(position: token::Pos) -> Self {
                Self { position }
            }

            pub fn Pos(&self) -> token::Pos {
                self.position
            }
        }

        pub struct File {
            position: token::Pos,
            pub Comments: Vec<*mut Comment>,
            pub CommentMap: Vec<(Node, Vec<CommentGroup>)>,
        }

        impl File {
            pub fn new(position: token::Pos) -> Self {
                Self {
                    position,
                    Comments: Vec::new(),
                    CommentMap: Vec::new(),
                }
            }

            pub fn Pos(&self) -> token::Pos {
                self.position
            }
        }

        #[derive(Clone)]
        pub struct BasicLit {
            pub Value: String,
        }

        #[derive(Clone)]
        pub struct Ident {
            pub Name: String,
        }

        pub struct ImportSpec {
            pub Path: BasicLit,
            pub Name: Option<Ident>,
        }

        pub fn NewCommentMap(
            _fset: &token::FileSet,
            file: *mut File,
            _comments: Vec<*mut Comment>,
        ) -> Vec<(Node, Vec<CommentGroup>)> {
            unsafe { (*file).CommentMap.clone() }
        }
    }

    pub mod loader {
        use super::ast;

        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub struct TypeInfo(pub usize);

        pub struct PackageInfo {
            pub Pkg: String,
            pub Importable: bool,
            pub TransitivelyErrorFree: bool,
            pub Files: Vec<*mut ast::File>,
            pub Errors: Vec<String>,
            pub Info: TypeInfo,
        }
    }

    pub mod analysis {
        use super::{ast, loader, token};
        use std::any::Any;
        use std::collections::HashMap;
        use std::rc::Rc;

        pub type Error = String;
        pub type RunResult = Result<Option<Box<dyn Any>>, Error>;

        pub enum Run {
            Function(fn(&mut Pass) -> RunResult),
            Closure(Box<dyn FnMut(&mut Pass) -> RunResult>),
        }

        impl Run {
            pub fn call(&mut self, pass: &mut Pass) -> RunResult {
                match self {
                    Self::Function(function) => function(pass),
                    Self::Closure(function) => function(pass),
                }
            }
        }

        pub struct Analyzer {
            pub Name: &'static str,
            pub Doc: &'static str,
            pub Requires: Vec<&'static Analyzer>,
            pub Run: Option<Run>,
            pub RunDespiteErrors: bool,
            pub ResultType: Option<fn() -> std::any::TypeId>,
        }

        // The production analysis API stores synchronized callbacks. This focused model is
        // single-threaded but needs the same static analyzer shape.
        unsafe impl Sync for Analyzer {}

        #[derive(Clone, Copy)]
        pub struct Diagnostic {
            pub Pos: token::Pos,
        }

        #[derive(Clone)]
        pub struct Pass {
            pub Files: Vec<*mut ast::File>,
            pub Fset: token::FileSet,
            pub ResultOf: HashMap<*const Analyzer, Rc<dyn Any>>,
            pub Report: Rc<dyn Fn(Diagnostic)>,
            pub Pkg: String,
            pub TypesInfo: loader::TypeInfo,
        }
    }

    mod reflect {
        pub fn TypeOf<T: 'static>() -> std::any::TypeId {
            std::any::TypeId::of::<T>()
        }
    }

    mod report {
        use super::{token, token::Position};

        pub fn DisplayPosition(fset: &token::FileSet, pos: token::Pos) -> Position {
            fset.PositionFor(pos, false)
        }
    }

    mod exclude {
        include!("exclude.rs");
    }

    mod implementation {
        use super::exclude::shouldRun;
        use super::{analysis, ast, loader, reflect, report, token};

        include!("util.rs");
    }

    fn add_ast_file(
        fset: &mut token::FileSet,
        name: &str,
        content: &[u8],
    ) -> (*mut ast::File, token::Pos) {
        let token_file = fset.AddFile(name, -1, content.len());
        unsafe {
            (*token_file).SetLinesForContent(content);
            let position = token::Pos((*token_file).Base());
            (Box::into_raw(Box::new(ast::File::new(position))), position)
        }
    }

    fn pass(files: Vec<*mut ast::File>, fset: token::FileSet) -> analysis::Pass {
        analysis::Pass {
            Files: files,
            Fset: fset,
            ResultOf: HashMap::new(),
            Report: Rc::new(|_| {}),
            Pkg: "example".to_string(),
            TypesInfo: loader::TypeInfo(7),
        }
    }

    #[test]
    fn parse_directive_and_comment_map_match_go() {
        assert_eq!(
            implementation::parseDirective("//lint:ignore gofmt revive".to_string()),
            (
                implementation::skipType::skipLinter,
                vec!["gofmt".to_string(), "revive".to_string()]
            )
        );
        assert_eq!(
            implementation::parseDirective("//lint:file-ignore revive".to_string()),
            (
                implementation::skipType::skipFile,
                vec!["revive".to_string()]
            )
        );
        assert_eq!(
            implementation::parseDirective("//lint:unknown x".to_string()),
            (implementation::skipType::skipNone, Vec::new())
        );
        assert_eq!(
            implementation::parseDirective("//nolint:gofmt, revive".to_string()),
            (
                implementation::skipType::skipLinter,
                vec!["gofmt, revive".to_string()]
            )
        );

        let mut fset = token::FileSet::default();
        let (file, position) = add_ast_file(&mut fset, "directives.go", b"package p\n");
        let lint = Box::into_raw(Box::new(ast::Comment {
            Text: "//nolint:gofmt".to_string(),
        }));
        let ordinary = Box::into_raw(Box::new(ast::Comment {
            Text: "// ordinary".to_string(),
        }));
        unsafe {
            (*file).Comments = vec![lint, ordinary];
            (*file).CommentMap = vec![(
                ast::Node::new(position),
                vec![ast::CommentGroup {
                    List: vec![lint, ordinary],
                }],
            )];
        }
        let dirs = implementation::ParseDirectives(vec![file], &fset);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].Command, implementation::skipType::skipLinter);
        assert_eq!(dirs[0].Linters, vec!["gofmt"]);
        assert_eq!(dirs[0].Directive, lint);

        let mut pass = pass(vec![file], fset);
        let result = implementation::doDirectives(&mut pass)
            .unwrap()
            .expect("directive analyzer returns a result");
        assert_eq!(
            result
                .downcast_ref::<Vec<implementation::Directive>>()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(implementation::Directives.Name, "directives");
        assert!(implementation::Directives.RunDespiteErrors);
        assert_eq!(
            (implementation::Directives.ResultType.unwrap())(),
            std::any::TypeId::of::<Vec<implementation::Directive>>()
        );
    }

    #[test]
    fn skip_analyzer_filters_files_and_same_line_diagnostics() {
        let mut fset = token::FileSet::default();
        let (ignored_file, ignored_pos) = add_ast_file(&mut fset, "ignored.go", b"a\nb\n");
        let (kept_file, kept_pos) = add_ast_file(&mut fset, "kept.go", b"a\nb\n");
        let same_line = token::Pos(kept_pos.0 + 2);

        let seen_file_count = Rc::new(RefCell::new(0));
        let seen_file_count_from_run = Rc::clone(&seen_file_count);
        let mut analyzer = analysis::Analyzer {
            Name: "gofmt",
            Doc: "test",
            Requires: Vec::new(),
            Run: Some(analysis::Run::Closure(Box::new(move |pass| {
                *seen_file_count_from_run.borrow_mut() = pass.Files.len();
                (pass.Report)(analysis::Diagnostic { Pos: ignored_pos });
                (pass.Report)(analysis::Diagnostic { Pos: same_line });
                (pass.Report)(analysis::Diagnostic { Pos: kept_pos });
                Ok(None)
            }))),
            RunDespiteErrors: false,
            ResultType: None,
        };
        implementation::SkipAnalyzer(&mut analyzer);
        assert_eq!(analyzer.Requires[0].Name, "directives");

        let reported = Rc::new(RefCell::new(Vec::new()));
        let reported_by_pass = Rc::clone(&reported);
        let mut pass = pass(vec![ignored_file, kept_file], fset);
        pass.Report =
            Rc::new(move |diagnostic| reported_by_pass.borrow_mut().push(diagnostic.Pos.0));
        pass.ResultOf.insert(
            std::ptr::from_ref(&implementation::Directives),
            Rc::new(vec![
                implementation::Directive {
                    Command: implementation::skipType::skipFile,
                    Linters: vec!["gofmt".to_string()],
                    Directive: std::ptr::null_mut(),
                    Node: ast::Node::new(ignored_pos),
                },
                implementation::Directive {
                    Command: implementation::skipType::skipLinter,
                    Linters: vec!["gofmt, revive".to_string()],
                    Directive: std::ptr::null_mut(),
                    Node: ast::Node::new(same_line),
                },
            ]),
        );
        let same_name = analysis::Analyzer {
            Name: "directives",
            Doc: "different analyzer with the same name",
            Requires: Vec::new(),
            Run: None,
            RunDespiteErrors: false,
            ResultType: None,
        };
        pass.ResultOf.insert(
            std::ptr::from_ref(&same_name),
            Rc::new("wrong result must not collide by name"),
        );
        analyzer.Run.as_mut().unwrap().call(&mut pass).unwrap();
        assert_eq!(*seen_file_count.borrow(), 1);
        assert_eq!(&*reported.borrow(), &[kept_pos.0]);
    }

    #[test]
    fn missing_directives_panics_like_go_type_assertion() {
        let mut analyzer = analysis::Analyzer {
            Name: "gofmt",
            Doc: "test",
            Requires: Vec::new(),
            Run: Some(analysis::Run::Closure(Box::new(|_| Ok(None)))),
            RunDespiteErrors: false,
            ResultType: None,
        };
        implementation::SkipAnalyzer(&mut analyzer);
        let mut pass = pass(Vec::new(), token::FileSet::default());
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                analyzer.Run.as_mut().unwrap().call(&mut pass).unwrap();
            }))
            .is_err()
        );
    }

    #[test]
    fn config_wrapper_filters_files_before_calling_the_original_analyzer() {
        let mut fset = token::FileSet::default();
        let (ordinary, _) = add_ast_file(&mut fset, "some.go", b"package p\n");
        let (generated, _) = add_ast_file(&mut fset, "uca_generated.go", b"package p\n");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_by_run = Rc::clone(&seen);
        let mut analyzer = analysis::Analyzer {
            Name: "gofmt",
            Doc: "test",
            Requires: Vec::new(),
            Run: Some(analysis::Run::Closure(Box::new(move |pass| {
                *seen_by_run.borrow_mut() = pass
                    .Files
                    .iter()
                    .map(|file| {
                        pass.Fset
                            .PositionFor(unsafe { (**file).Pos() }, false)
                            .Filename
                    })
                    .collect();
                Ok(None)
            }))),
            RunDespiteErrors: false,
            ResultType: None,
        };
        implementation::SkipAnalyzerByConfig(&mut analyzer);
        let mut pass = pass(vec![ordinary, generated], fset);
        analyzer.Run.as_mut().unwrap().call(&mut pass).unwrap();
        assert_eq!(&*seen.borrow(), &["some.go"]);
    }

    #[test]
    fn pure_helpers_keep_go_byte_and_import_contracts() {
        assert_eq!(implementation::FormatCode("value"), "`value`");
        assert_eq!(implementation::FormatCode("`value`"), "`value`");

        assert_eq!(implementation::FindOffset("界x\ny".as_bytes(), 1, 2), 3);
        assert_eq!(implementation::FindOffset(&[0xff, b'x', b'\n'], 1, 2), 1);
        assert_eq!(implementation::FindOffset(b"a\nb", 2, 1), 2);
        assert_eq!(implementation::FindOffset(b"a", 1, 2), -1);

        let aliased = Box::into_raw(Box::new(ast::ImportSpec {
            Path: ast::BasicLit {
                Value: "\"example/path\"".to_string(),
            },
            Name: Some(ast::Ident {
                Name: "alias".to_string(),
            }),
        }));
        assert_eq!(
            implementation::GetPackageName(vec![aliased], "example/path", "default"),
            "alias"
        );
        assert_eq!(
            implementation::GetPackageName(vec![aliased], "missing", "default"),
            ""
        );
    }

    #[test]
    fn read_file_and_loader_adapter_preserve_side_effects() {
        let path = std::env::temp_dir().join(format!(
            "astersql-util-test-{}-{}.go",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        std::fs::write(&path, b"a\nb").unwrap();
        let mut fset = token::FileSet::default();
        let (content, token_file) =
            implementation::ReadFile(&mut fset, path.to_str().unwrap()).unwrap();
        assert_eq!(content, b"a\nb");
        let second_line = unsafe { token::Pos((*token_file).Base() + 2) };
        assert_eq!(fset.PositionFor(second_line, false).Line, 2);
        std::fs::remove_file(&path).unwrap();

        let pass = pass(Vec::new(), fset);
        let info = implementation::MakeFakeLoaderPackageInfo(&pass);
        assert_eq!(info.Pkg, "example");
        assert!(info.Importable);
        assert!(info.TransitivelyErrorFree);
        assert_eq!(info.Info, loader::TypeInfo(7));
        assert!(info.Errors.is_empty());

        let missing = std::env::temp_dir().join(format!(
            "astersql-util-missing-{}-{}.go",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        let error = match implementation::ReadFile(
            &mut token::FileSet::default(),
            missing.to_str().unwrap(),
        ) {
            Ok(_) => panic!("missing file must fail"),
            Err(error) => error,
        };
        assert_eq!(error.Op, "open");
        assert_eq!(error.Path, missing.to_string_lossy());
        assert_eq!(error.Err.kind(), std::io::ErrorKind::NotFound);
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn analyzer_wrappers_preserve_go_failure_and_pointer_contracts() {
    for required in [
        ".expect(\"Directives result is missing\")",
        ".expect(\"Directives result has the wrong type\")",
        "pub fn MakeFakeLoaderPackageInfo(pass: &analysis::Pass) -> Box<loader::PackageInfo>",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("unwrap_or_default()"));
    assert!(RUST_SOURCE.contains(".get(&(std::ptr::from_ref(&Directives)))"));
    assert!(!RUST_SOURCE.contains(".get(Directives.Name)"));
}

#[test]
fn byte_offsets_can_represent_go_strings_with_invalid_utf8() {
    assert!(RUST_SOURCE.contains("pub fn FindOffset(fileText: &[u8]"));
    assert!(RUST_SOURCE.contains("fn goRuneLen"));
    assert!(!RUST_SOURCE.contains("pub fn FindOffset(fileText: &str"));
}

#[test]
fn completed_port_preserves_license_without_placeholders() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2022 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("Rust 草稿"));
}

#[test]
fn read_file_retains_operation_path_and_io_error() {
    for required in [
        "pub struct ReadFileError",
        "pub Op: &'static str",
        "pub Path: String",
        "pub Err: std::io::Error",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("map_err(|err| err.to_string())"));
}
