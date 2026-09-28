// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::matcherParser;

#[test]
fn empty_delimited_patterns_match_go_parse_failures() {
    let parser = matcherParser {
        fileName: "<cmdline>".to_owned(),
        lineNum: 1,
    };

    for (pattern, expected) in [
        ("//", "syntax error: incomplete regexp"),
        (r#""""#, "syntax error: incomplete quoted identifier"),
        ("``", "syntax error: incomplete quoted identifier"),
    ] {
        let error = parser
            .parsePattern(pattern, false)
            .err()
            .expect("Go rejects empty delimited patterns");
        assert!(
            error.to_string().contains(expected),
            "pattern {pattern:?}: {error}"
        );
    }
}

#[test]
fn wildcard_class_errors_match_go_parser_and_compiler_boundaries() {
    let parser = matcherParser {
        fileName: "<cmdline>".into(),
        lineNum: 1,
    };
    for (pattern, expected) in [
        (
            "[!]",
            "invalid pattern: error parsing regexp: missing closing ]: `[^]$`",
        ),
        ("[]]", "syntax error: failed to parse character class"),
        ("[]", "syntax error: failed to parse character class"),
    ] {
        assert_eq!(
            parser.parsePattern(pattern, false).unwrap_err().to_string(),
            format!("at <cmdline>:1: {expected}")
        );
    }
}
