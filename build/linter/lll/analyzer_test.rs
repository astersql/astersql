// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

mod compiled_lll {
    mod token {
        #[derive(Clone, Copy)]
        pub struct Pos(pub i32);

        pub struct File {
            pub base: i32,
        }

        impl File {
            pub fn Base(&self) -> i32 {
                self.base
            }
        }
    }

    mod analysis {
        use super::token;
        use std::any::Any;

        pub type Error = String;

        pub struct Analyzer {
            pub name: &'static str,
            pub doc: &'static str,
            pub requires: &'static [&'static Analyzer],
            pub run: fn(&mut Pass) -> Result<Option<Box<dyn Any>>, Error>,
        }

        pub struct AstFile {
            pub position: i32,
        }

        impl AstFile {
            pub fn Pos(&self) -> i32 {
                self.position
            }
        }

        pub struct Position {
            pub Filename: String,
        }

        pub struct FileSet;

        impl FileSet {
            pub fn PositionFor(&self, _position: i32, _adjusted: bool) -> Position {
                Position {
                    Filename: String::new(),
                }
            }
        }

        pub struct Pass {
            pub Files: Vec<AstFile>,
            pub Fset: FileSet,
        }

        impl Pass {
            pub fn Reportf(&mut self, position: token::Pos, _message: &str) {
                let _ = position.0;
            }
        }
    }

    mod util {
        use super::{analysis, token};

        pub fn ReadFile(
            _fset: &mut analysis::FileSet,
            _filename: &str,
        ) -> Result<(Vec<u8>, *mut token::File), String> {
            Err("unused in scanner tests".to_string())
        }

        pub fn SkipAnalyzerByConfig(_analyzer: &analysis::Analyzer) {}
        pub fn SkipAnalyzer(_analyzer: &analysis::Analyzer) {}
    }

    mod implementation {
        use super::{analysis, token, util};

        include!("analyzer.rs");
    }

    #[test]
    fn scanner_counts_runes_tabs_and_invalid_utf8_like_go() {
        use std::io::Cursor;

        let mut unicode = Cursor::new("界".repeat(121).into_bytes());
        let issues = implementation::scanLLLIssues(&mut unicode, "unicode.go", 120, " ")
            .expect("unicode line should scan");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].Line, 1);
        assert_eq!(issues[0].Text, "line is 121 characters");

        let mut tab = Cursor::new(b"\t".to_vec());
        let issues = implementation::scanLLLIssues(&mut tab, "tab.go", 3, "    ")
            .expect("tab line should scan");
        assert_eq!(issues[0].Text, "line is 4 characters");

        let mut invalid = Cursor::new(vec![0xff; 121]);
        let issues = implementation::scanLLLIssues(&mut invalid, "invalid.go", 120, " ")
            .expect("Go scanner accepts arbitrary bytes");
        assert_eq!(issues[0].Text, "line is 121 characters");

        let mut final_cr = Cursor::new(b"abcd\r".to_vec());
        let issues = implementation::scanLLLIssues(&mut final_cr, "cr.go", 4, " ")
            .expect("ScannerLines drops a final carriage return at EOF");
        assert!(issues.is_empty());
    }

    #[test]
    fn scanner_skips_directives_and_import_blocks() {
        use std::io::Cursor;

        let long = "x".repeat(140);
        let input = format!("//go:{long}\nimport (\n{long}\n)\nimport {long}\n");
        let mut reader = Cursor::new(input.into_bytes());
        let issues = implementation::scanLLLIssues(&mut reader, "ignored.go", 120, " ")
            .expect("ignored lines should scan");
        assert!(issues.is_empty());
    }

    #[test]
    fn scanner_matches_go_max_token_size_branch_and_stops() {
        use std::io::Cursor;

        let mut input = vec![b'a'; implementation::MAX_SCAN_TOKEN_SIZE];
        input.extend_from_slice(b"\nthis subsequent line is intentionally ignored");

        let mut tolerated = Cursor::new(input.clone());
        let issues = implementation::scanLLLIssues(&mut tolerated, "generated.go", 120, " ")
            .expect("small configured limit tolerates Scanner ErrTooLong");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].Line, 0);
        assert_eq!(issues[0].Text, "line is more than 65536 characters");

        let mut rejected = Cursor::new(input);
        let error = implementation::scanLLLIssues(
            &mut rejected,
            "generated.go",
            implementation::MAX_SCAN_TOKEN_SIZE as i32,
            " ",
        )
        .expect_err("large configured limit must expose Scanner ErrTooLong");
        assert_eq!(
            error,
            "can't scan file generated.go: bufio.Scanner: token too long"
        );
    }

    #[test]
    fn byte_line_offsets_match_go_for_column_one() {
        assert_eq!(implementation::findLineOffset(b"abc\ndef", 1), 0);
        assert_eq!(implementation::findLineOffset(b"abc\ndef", 2), 4);
        assert_eq!(implementation::findLineOffset(b"abc\n", 2), -1);
        assert_eq!(implementation::findLineOffset(&[0xff, b'\n', b'x'], 2), 2);
    }
}

#[test]
fn analyzer_and_run_wiring_use_rust_api_shape() {
    for required in [
        "// Copyright 2026 AsterSQL.",
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: lllName",
        "doc: \"Reports long lines\"",
        "requires: &[]",
        "run: |pass: &mut analysis::Pass|",
        "Option<Box<dyn std::any::Any>>",
        "analysis::Error",
        "LineLength: 120",
        "TabWidth: 1",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn files_errors_and_diagnostics_preserve_go_contract() {
    for required in [
        "Vec::with_capacity(pass.Files.len())",
        "!pos.Filename.ends_with(\"failpoint_binding__.go\")",
        "util::ReadFile(&mut pass.Fset, &i.Filename)",
        "can't get file {} contents: {}",
        "let file_base = unsafe { (*tf).Base() }",
        "findLineOffset(&fileContent, i.Line)",
        "pass.Reportf(pos, \"too long\")",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("String::from_utf8_lossy"));
}

#[test]
fn scanner_detects_oversized_tokens_before_unbounded_allocation() {
    assert!(RUST_SOURCE.contains("scanner.fill_buf()"));
    assert!(RUST_SOURCE.contains("scanner.consume("));
    assert!(!RUST_SOURCE.contains(".read_until("));
}
