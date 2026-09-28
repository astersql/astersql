// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// partialjson 迁移补充单元测试。
//
// 对照 Go `topLevelJSONTokenIter` / `ExtractTopLevelMembers`：校验顶层键值 token 顺序、
// 非法输入错误文案、跳过未请求嵌套、以及取齐全部请求键后提前结束与缺失键 EOF。

use super::{ExtractTopLevelMembers, Token, newTopLevelJSONTokenIter};

/// 构造数字 token，便于与期望序列比对。
fn number(value: &str) -> Token {
    Token::Number(value.to_owned())
}

/// 迭代器按 Go 顺序产出顶层名与值 token；嵌套对象以 Delim 形式展开。
#[test]
fn migration_iter_matches_go_token_order() {
    let mut iter = newTopLevelJSONTokenIter(br#"{"a":1,"long":{"skip":[true,null]},"b":"val"}"#);

    assert_eq!(iter.readName().unwrap(), "a");
    assert_eq!(iter.readOrDiscardValue(false).unwrap(), vec![number("1")]);
    assert_eq!(iter.readName().unwrap(), "long");
    assert_eq!(
        iter.readOrDiscardValue(false).unwrap(),
        vec![
            Token::Delim('{'),
            Token::String("skip".to_owned()),
            Token::Delim('['),
            Token::Bool(true),
            Token::Null,
            Token::Delim(']'),
            Token::Delim('}'),
        ]
    );
    assert_eq!(iter.readName().unwrap(), "b");
    assert_eq!(
        iter.readOrDiscardValue(false).unwrap(),
        vec![Token::String("val".to_owned())]
    );
    assert!(iter.next(false).unwrap_err().is_eof());
}

/// 残缺/非对象/非法值等错误路径的文案片段与 Go 一致。
#[test]
fn migration_iter_matches_go_error_paths() {
    for (content, expected) in [
        ("{", "unexpected EOF"),
        ("[]", "expected '{' for topLevelJSONTokenIter"),
        ("{a}", "expected value"),
        ("{]", "mismatched closing delimiter"),
    ] {
        let mut iter = newTopLevelJSONTokenIter(content.as_bytes());
        // 一直 next 直到出错，再断言错误消息包含期望片段。
        let err = loop {
            match iter.next(false) {
                Ok(_) => continue,
                Err(err) => break err,
            }
        };
        assert!(
            err.to_string().contains(expected),
            "content={content}, err={err}"
        );
    }
}

/// 只抽取请求键；未请求的深层嵌套不进入结果 map。
#[test]
fn migration_extract_skips_unrequested_nested_values() {
    let names = vec!["a".to_owned(), "b".to_owned()];
    let values = ExtractTopLevelMembers(
        br#"{"skip":{"large":[0,1,{"x":2}]},"a":1,"b":["v"]}"#,
        &names,
    )
    .unwrap();

    assert_eq!(values["a"], vec![number("1")]);
    assert_eq!(
        values["b"],
        vec![
            Token::Delim('['),
            Token::String("v".to_owned()),
            Token::Delim(']')
        ]
    );
}

/// 请求键全部找到后可提前结束，即使后续字节非法；空 names 对非 JSON 也返回空 map。
#[test]
fn migration_extract_stops_after_all_requested_keys() {
    let values =
        ExtractTopLevelMembers(br#"{"wanted":1,"trailing":]"#, &["wanted".to_owned()]).unwrap();
    assert_eq!(values["wanted"], vec![number("1")]);

    let empty = ExtractTopLevelMembers(b"not JSON", &[]).unwrap();
    assert!(empty.is_empty());
}

/// 请求的顶层成员不存在时以 EOF 错误返回。
#[test]
fn migration_extract_reports_missing_member() {
    let err = ExtractTopLevelMembers(br#"{"a":1}"#, &["missing".to_owned()]).unwrap_err();
    assert!(err.is_eof(), "err={err}");
}
