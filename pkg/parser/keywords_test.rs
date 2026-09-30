// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 关键字表测试：首项、总长度、保留字数量与分段内排序。

// 描述 parser 关键字表的测试断言。
#[path = "keywords.rs"]
mod parser;

/// 简化版 testify require.Equal，用于与 Go 断言形状对齐。
#[allow(non_snake_case)]
mod require {
    use std::fmt::Debug;
    /// 期望值与实际值相等，否则 panic。
    pub fn Equal<T: Debug + PartialEq<U>, U: Debug>(expected: T, actual: U) {
        assert_eq!(expected, actual);
    }
}

/// 简化版 testing.T.Errorf，用于排序失败时提示重新生成 keywords。
#[allow(non_snake_case)]
mod testing {
    /// 打印提示后 panic，对应 Go 的 t.Errorf 终止语义。
    pub fn Errorf(message: &str, left: &str, right: &str) -> ! {
        panic!("{}: {} / {}", message.trim(), left, right)
    }
}

// test_keywords 对应 Go 的 TestKeywords：检查关键字表首项，并确认 TiDB 自有关键字 ADMIN 已包含。
/// 校验 Keywords[0] 为保留字 ADD，且包含 TiDB 关键字 ADMIN。
#[test]
fn test_keywords() {
    // Go 原断言：parser.Keywords[0].Word == "ADD"，且 Reserved 为 true。
    require::Equal("ADD", parser::Keywords[0].Word);
    require::Equal(true, parser::Keywords[0].Reserved);

    let mut found = false;
    for kw in parser::Keywords.iter() {
        if kw.Word == "ADMIN" {
            found = true;
        }
    }
    // Go 使用 require.Equal 附带提示信息；这里保留相同检查语义。
    assert!(found, "TiDBKeyword ADMIN is part of the list");
    assert!(
        parser::Keywords
            .iter()
            .any(|keyword| keyword.Word == "AUTO" && !keyword.Reserved),
        "AUTO is part of the unreserved keyword list"
    );
}

// test_keywords_length 对应 Go 的 TestKeywordsLength：固定总关键字数和保留关键字数量。
/// 校验总关键字数 695、保留字数 233（与 Go 生成器快照一致）。
#[test]
fn test_keywords_length() {
    require::Equal(695, parser::Keywords.len());

    let mut reserved_nr = 0;
    for kw in parser::Keywords.iter() {
        if kw.Reserved {
            reserved_nr += 1;
        }
    }
    require::Equal(233, reserved_nr);
}

/// Go 生成列表不包含 MariaDB 专用的 MONITOR 关键字。
#[test]
fn test_keywords_excludes_mariadb_only_words() {
    assert!(parser::Keywords.iter().all(|kw| kw.Word != "MONITOR"));
}

// test_keywords_sorting 对应 Go 的 TestKeywordsSorting：同一 Section 内必须按 Word 升序排列。
/// 同一 Section 内 Word 必须升序；否则提示更新 Rust 文法并重新生成。
#[test]
fn test_keywords_sorting() {
    for (i, kw) in parser::Keywords.iter().enumerate() {
        if i > 1
            && parser::Keywords[i - 1].Word > kw.Word
            && parser::Keywords[i - 1].Section == kw.Section
        {
            // 排序错误时提示更新 Rust 文法并重新生成关键字表。
            testing::Errorf(
                "{} should come after {}, please update main.astergram and re-generate keywords.rs\n",
                parser::Keywords[i - 1].Word,
                kw.Word,
            );
        }
    }
}
