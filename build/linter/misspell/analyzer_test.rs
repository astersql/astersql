// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

mod compiled_misspell {
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

    mod misspell {
        pub static DictMain: &[&str] = &[];

        pub struct Replacer {
            pub Replacements: Vec<&'static str>,
        }

        pub struct Diff {
            pub Line: i32,
            pub Column: i32,
            pub Original: String,
            pub Corrected: String,
        }

        impl Replacer {
            pub fn RemoveRule(&mut self, _ignore: &[String]) {}
            pub fn Compile(&mut self) {}

            pub fn Replace(&self, input: &str) -> (String, Vec<Diff>) {
                (input.to_string(), Vec::new())
            }
        }
    }

    mod util {
        use super::{analysis, token};

        pub fn ReadFile(
            _fset: &mut analysis::FileSet,
            _filename: &str,
        ) -> Result<(Vec<u8>, *mut token::File), String> {
            Err("unused in byte helper tests".to_string())
        }

        pub fn SkipAnalyzerByConfig(_analyzer: &analysis::Analyzer) {}
        pub fn SkipAnalyzer(_analyzer: &analysis::Analyzer) {}
    }

    mod implementation {
        use super::{analysis, misspell, token, util};

        include!("analyzer.rs");
    }

    #[test]
    fn misspell_input_is_valid_utf8_with_byte_offsets_preserved() {
        let original = b"teh\xffword\xe7\x95\x8c\xfeend";
        let sanitized = implementation::sanitizeForMisspell(original);
        assert_eq!(sanitized.len(), original.len());
        assert_eq!(sanitized.as_bytes(), b"teh\0word\xe7\x95\x8c\0end");
    }

    #[test]
    fn go_find_offset_counts_valid_runes_and_each_invalid_byte() {
        assert_eq!(implementation::findOffset(b"abc\ndef", 1, 1), 0);
        assert_eq!(implementation::findOffset("界teh".as_bytes(), 1, 2), 3);
        assert_eq!(
            implementation::findOffset(&[0xff, b't', b'e', b'h'], 1, 2),
            1
        );
        assert_eq!(implementation::findOffset(b"abc\ndef", 2, 1), 4);
        assert_eq!(implementation::findOffset(b"abc", 1, 0), -1);
    }
}

#[test]
fn analyzer_configuration_and_run_shape_match_go() {
    for required in [
        "// Copyright 2026 AsterSQL.",
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: Name",
        "doc: \"Checks the spelling error in code\"",
        "requires: &[]",
        "run",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "Replacements: misspell::DictMain.to_vec()",
        "Locale: String::new()",
        "IgnoreWords: Vec::new()",
        "r.RemoveRule(&settings.IgnoreWords)",
        "r.Compile()",
        "Vec::with_capacity(pass.Files.len())",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn file_errors_plain_text_replacement_and_diagnostics_match_go() {
    for required in [
        "util::ReadFile(&mut pass.Fset, fileName)",
        "can't get file {} contents: {}",
        "r.Replace(&sanitizeForMisspell(&fileContent))",
        "let file_base = unsafe { (*tf).Base() }",
        "findOffset(&fileContent, diff.Line, diff.Column)",
        "[{}] `{}` is a misspelling of `{}`",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("String::from_utf8_lossy"));
}
