// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("checker.rs");

#[cfg(unix)]
mod compiled_checker {
    mod analysis {
        use std::any::Any;

        pub type Error = std::io::Error;

        pub struct Analyzer {
            pub name: &'static str,
            pub doc: &'static str,
            pub requires: &'static [&'static Analyzer],
            pub run: fn(&mut Pass) -> Result<Option<Box<dyn Any>>, Error>,
        }

        pub struct File {
            pub position: usize,
        }

        impl File {
            pub fn Pos(&self) -> usize {
                self.position
            }
        }

        pub struct Position {
            pub Filename: String,
        }

        pub struct FileSet;

        impl FileSet {
            pub fn PositionFor(&self, _position: usize, _adjusted: bool) -> Position {
                Position {
                    Filename: String::new(),
                }
            }
        }

        pub struct Pass {
            pub Files: Vec<File>,
            pub Fset: FileSet,
        }

        impl Pass {
            pub fn Reportf(&mut self, _position: usize, _message: &str) {}
        }
    }

    mod util {
        use super::analysis;

        pub fn SkipAnalyzerByConfig(_analyzer: &analysis::Analyzer) {}
    }

    mod implementation {
        use super::{analysis, util};

        include!("checker.rs");
    }

    #[test]
    fn go_filemode_rendering_matches_regular_special_and_device_modes() {
        let cases = [
            (0o100755, "-rwxr-xr-x"),
            (0o104755, "urwxr-xr-x"),
            (0o102755, "grwxr-xr-x"),
            (0o101755, "trwxr-xr-x"),
            (0o040755, "drwxr-xr-x"),
            (0o120777, "Lrwxrwxrwx"),
            (0o060660, "Drw-rw----"),
            (0o020660, "Dcrw-rw----"),
            (0o010644, "prw-r--r--"),
            (0o140777, "Srwxrwxrwx"),
        ];
        for (mode, expected) in cases {
            assert_eq!(implementation::formatGoFileMode(mode), expected);
        }
    }
}

#[test]
fn analyzer_literal_and_run_signature_use_rust_api_shape() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: Name",
        "doc: \"Go files should not have execution permission\"",
        "requires: &[]",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("once_cell::sync::Lazy"));
}

#[test]
fn metadata_errors_and_execute_bit_filter_match_go() {
    for required in [
        "pass.Fset.PositionFor(file_position, false).Filename",
        "if !fn_name.is_empty()",
        "std::fs::metadata(&fn_name)?",
        "if mode & 0o111 != 0",
        "pass.Reportf(",
        "file_position",
        "[{}] source code file should not have execute permission {}",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn reported_mode_uses_go_filemode_symbolic_rendering() {
    for required in [
        "pub fn formatGoFileMode(mode: u32) -> String",
        "const FILE_TYPE_MASK: u32 = 0o170000",
        "0o040000 => prefix.push('d')",
        "0o120000 => prefix.push('L')",
        "0o060000 => prefix.push('D')",
        "0o010000 => prefix.push('p')",
        "0o140000 => prefix.push('S')",
        "0o020000 => prefix.push('D')",
        "if mode & 0o4000 != 0",
        "if mode & 0o2000 != 0",
        "if mode & 0o1000 != 0",
        "prefix.push('c')",
        "const PERMISSIONS: [(u32, char); 9]",
        "let rendered_mode = formatGoFileMode(mode)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("permission {:o}"));
}
