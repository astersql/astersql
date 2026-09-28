// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

mod compiled_prealloc {
    mod ast {
        pub struct File {
            pub position: i32,
        }
    }

    mod analysis {
        use super::ast;
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
            pub reports: Vec<(i32, String)>,
        }

        impl Pass {
            pub fn Reportf(&mut self, position: i32, message: &str) {
                self.reports.push((position, message.to_string()));
            }
        }
    }

    mod prealloc {
        use super::ast;
        use std::sync::{Mutex, atomic::AtomicUsize};

        pub static CALLS: Mutex<Vec<(usize, bool, bool, bool)>> = Mutex::new(Vec::new());
        pub static PANIC_ON_CALL: AtomicUsize = AtomicUsize::new(0);

        pub struct Hint {
            pub Pos: i32,
            pub DeclaredSliceName: String,
        }

        pub fn Check(
            files: &[ast::File],
            simple: bool,
            range_loops: bool,
            for_loops: bool,
        ) -> Vec<Hint> {
            assert_eq!(files.len(), 1);
            let call_number = {
                let mut calls = CALLS.lock().expect("calls lock");
                calls.push((files.as_ptr() as usize, simple, range_loops, for_loops));
                calls.len()
            };
            if PANIC_ON_CALL.load(std::sync::atomic::Ordering::SeqCst) == call_number {
                panic!("configured prealloc panic");
            }
            vec![Hint {
                Pos: files[0].position,
                DeclaredSliceName: "items".to_string(),
            }]
        }
    }

    mod util {
        use super::analysis;

        pub fn FormatCode(code: &str) -> String {
            format!("`{code}`")
        }

        pub fn SkipAnalyzerByConfig(_analyzer: &analysis::Analyzer) {}
        pub fn SkipAnalyzer(_analyzer: &analysis::Analyzer) {}
    }

    mod implementation {
        use super::{analysis, ast, prealloc, util};

        include!("analyzer.rs");
    }

    #[test]
    fn run_checks_each_original_ast_file_with_go_defaults_and_reports_hints() {
        prealloc::CALLS.lock().expect("calls lock").clear();
        let mut pass = analysis::Pass {
            Files: vec![ast::File { position: 11 }, ast::File { position: 22 }],
            reports: Vec::new(),
        };
        let expected_addresses: Vec<usize> = pass
            .Files
            .iter()
            .map(|file| file as *const ast::File as usize)
            .collect();

        let result = implementation::run(&mut pass).expect("prealloc run should succeed");
        assert!(result.is_none());
        let calls = prealloc::CALLS.lock().expect("calls lock");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (expected_addresses[0], true, true, false));
        assert_eq!(calls[1], (expected_addresses[1], true, true, false));
        drop(calls);
        assert_eq!(
            pass.reports,
            vec![
                (11, "[prealloc] Consider preallocating `items`".to_string()),
                (22, "[prealloc] Consider preallocating `items`".to_string()),
            ]
        );

        prealloc::CALLS.lock().expect("calls lock").clear();
        prealloc::PANIC_ON_CALL.store(2, std::sync::atomic::Ordering::SeqCst);
        let mut pass = analysis::Pass {
            Files: vec![ast::File { position: 11 }, ast::File { position: 22 }],
            reports: Vec::new(),
        };
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = implementation::run(&mut pass);
        }));
        prealloc::PANIC_ON_CALL.store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(panic.is_err());
        assert_eq!(
            pass.reports,
            vec![(11, "[prealloc] Consider preallocating `items`".to_string())]
        );
    }
}

#[test]
fn analyzer_settings_and_skip_wiring_match_go() {
    for required in [
        "// Copyright 2026 AsterSQL.",
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: Name",
        "doc: \"Finds slice declarations that could potentially be preallocated\"",
        "requires: &[]",
        "run",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "Simple: true",
        "RangeLoops: true",
        "ForLoops: false",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn check_borrows_original_ast_and_diagnostic_matches_go() {
    for required in [
        "let hints = {",
        "prealloc::Check(",
        "std::slice::from_ref(f)",
        "s.Simple",
        "s.RangeLoops",
        "s.ForLoops",
        "hint.Pos",
        "[{}] Consider preallocating {}",
        "util::FormatCode(&hint.DeclaredSliceName)",
        "for index in 0..pass.Files.len()",
        "let f = &pass.Files[index]",
        "pass.Reportf(",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("f as *const ast::File"));
}

#[test]
fn completed_port_preserves_license_without_placeholder_claims() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2022 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("当前不会真正调用"));
}
