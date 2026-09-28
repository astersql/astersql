// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `misc.rs` 词法辅助（字符类、规则树、关键字映射、标识符分词）的单元测试。
//
// 核对 MySQL 字节级字符分类、标识符合法性、运算符最长匹配（ruleTable）
// 以及 IGNORE_SPACE / 窗口函数开关对关键字识别的影响。

use super::token::*;

/// 字母、数字、标识符与用户变量字符分类应与 MySQL 规则一致。
#[test]
fn character_classes_match_mysql_byte_rules() {
    assert!(isLetter(b'a'));
    assert!(isLetter(b'Z'));
    assert!(!isLetter(b'0'));
    assert!(isDigit(b'0'));
    assert!(isDigit(b'9'));
    assert!(!isDigit(b'/'));
    assert!(isIdentChar(b'_'));
    assert!(isIdentChar(b'$'));
    assert!(isIdentExtend(0x80));
    assert!(!isIdentExtend(0x7f));
    assert!(isUserVarChar(b'.'));
    assert!(!isIdentChar(b'.'));
}

/// `isInCorrectIdentifierName` 保留 Go 侧“反向命名”语义（非法为 true）。
#[test]
fn incorrect_identifier_name_keeps_go_reverse_semantics() {
    assert!(isInCorrectIdentifierName(""));
    assert!(isInCorrectIdentifierName("trailing "));
    assert!(!isInCorrectIdentifierName("valid_name"));
    assert!(!isInCorrectIdentifierName("embedded space"));
}

/// 规则树根节点应挂载单字符与最长运算符（如 `<=>`）及特殊前缀函数。
#[test]
fn rule_table_contains_single_and_longest_operator_tokens() {
    let root = &*ruleTable;
    assert_eq!(root.token, token::invalid);
    assert_eq!(
        root.childs[b'?' as usize].as_ref().unwrap().token,
        paramMarker
    );
    assert_eq!(root.childs[b'=' as usize].as_ref().unwrap().token, eq);

    // `<=` 再接 `>` 得到空值安全相等 `<=>`（nulleq）。
    let less_node = root.childs[b'<' as usize].as_ref().unwrap();
    assert_eq!(less_node.token, b'<' as i32);
    assert_eq!(less_node.childs[b'=' as usize].as_ref().unwrap().token, le);
    assert_eq!(
        less_node.childs[b'=' as usize].as_ref().unwrap().childs[b'>' as usize]
            .as_ref()
            .unwrap()
            .token,
        nulleq,
    );
    assert!(
        root.childs[b'X' as usize]
            .as_ref()
            .unwrap()
            .function
            .is_some()
    );
}

/// 普通关键字、内建函数、窗口函数与 Hint token 映射应齐全且大小写敏感。
#[test]
fn keyword_maps_cover_regular_builtin_window_and_hint_tokens() {
    assert!(isInTokenMap("SELECT"));
    assert!(!isInTokenMap("select"));
    assert!(!isInTokenMap("NOT_A_TIDB_KEYWORD"));
    assert_eq!(lookup_token(btFuncTokenMap, "COUNT"), builtinCount);
    assert_eq!(lookup_token(windowFuncTokenMap, "ROW_NUMBER"), rowNumber);
    assert_eq!(lookup_token(hintTokenMap, "HASH_JOIN"), hintHashJoin);
}

/// 限定名、IGNORE_SPACE 与窗口函数开关决定关键字或标识符。
#[test]
fn identifier_tokenization_respects_qualification_and_ignore_space() {
    /// 取扫描器返回的第一个 token。
    fn first_token(input_sql: &str, ignore_space: bool, enable_window: bool) -> i32 {
        let mut scanner = NewScanner(input_sql.to_owned());
        if ignore_space {
            scanner.SetSQLMode(mysql::ModeIgnoreSpace);
        }
        scanner.EnableWindowFunc(enable_window);
        let mut symbol = yySymType::default();
        scanner.Lex(&mut symbol)
    }

    assert_eq!(first_token("select.", false, true), token::identifier);
    assert_eq!(first_token("db.select", false, true), token::identifier);
    assert_eq!(first_token("count(", false, false), builtinCount);
    assert_eq!(first_token("count   (", false, false), token::identifier);
    assert_eq!(first_token("count   (", true, false), builtinCount);
    assert_eq!(first_token("row_number", false, false), token::identifier);
    assert_eq!(first_token("row_number", false, true), rowNumber);
}
