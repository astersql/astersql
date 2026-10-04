// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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
use crate::*;
fn parse(sql: &str) -> ast::ExprNode {
    let statement = parser_dependency::New()
        .ParseOneStmt(&format!("select {sql}"), "", "")
        .unwrap();
    statement
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .unwrap()
        .Fields
        .Fields[0]
        .Expr
        .clone()
        .unwrap()
}
#[test]
fn embedding_ast_helpers_detect_nested_calls_and_preserve_raw_constants() {
    let direct = parse("embed_text('mock/json', text, '{\"plus\":0.5}')");
    let nested = parse("vec_dims(embed_text('mock/json', text))");
    assert!(ContainsEmbedTextFunc(Some(&direct)));
    assert!(ContainsEmbedTextFunc(Some(&nested)));
    assert!(!ContainsEmbedTextFunc(None));
    assert!(!ContainsEmbedTextFunc(Some(&parse("vec_dims(vec)"))));
    assert!(IsEmbedTextFuncCall(&direct));
    assert!(!IsEmbedTextFuncCall(&nested));
    let info = ExtractEmbedTextInfo(&direct).unwrap();
    assert_eq!(info.ModelNameWithProvider, "mock/json");
    assert_eq!(info.OptsInJSON, "{\"plus\":0.5}");
    assert!(EmbedTextInfo::Equal(Some(&info), Some(&info.clone())));
    assert!(!EmbedTextInfo::Equal(Some(&info), None));
    assert!(EmbedTextInfo::Equal(None, None));
    assert_ne!(
        info,
        ExtractEmbedTextInfo(&parse("embed_text('mock/json', text, '{ \"plus\":0.5}')")).unwrap()
    );
    for sql in [
        "embed_text('mock/json', text)",
        "embed_text('mock/json', text, '')",
    ] {
        assert!(
            ExtractEmbedTextInfo(&parse(sql))
                .unwrap()
                .OptsInJSON
                .is_empty()
        );
    }
}
#[test]
fn embedding_ast_metadata_rejects_invalid_generated_arguments() {
    for (sql, message) in [
        ("vec_dims(vec)", "only generated columns"),
        ("embed_text('mock/json')", "invalid EMBED_TEXT() usage"),
        (
            "embed_text('mock/json', text, '{}', 'extra')",
            "invalid EMBED_TEXT() usage",
        ),
        (
            "embed_text(model, text)",
            "model name using string constant",
        ),
        ("embed_text(1, text)", "model name using string constant"),
        (
            "embed_text('mock/json', text, opts)",
            "JSON options using string constant",
        ),
        (
            "embed_text('mock/json', text, 1)",
            "JSON options using string constant",
        ),
        (
            "embed_text('mock/json', text, '{invalid}')",
            "expects options in JSON format",
        ),
        (
            "embed_text('mock/json', text, '[]')",
            "expects options in JSON format",
        ),
        (
            "embed_text('mock/json', text, 'null')",
            "expects options in JSON format",
        ),
    ] {
        assert!(
            ExtractEmbedTextInfo(&parse(sql))
                .unwrap_err()
                .to_string()
                .contains(message),
            "{sql}"
        );
    }
}
