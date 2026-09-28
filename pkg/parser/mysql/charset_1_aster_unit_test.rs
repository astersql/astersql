// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// MySQL 字符集 / 排序规则（collation）与 SQL Mode 常量相关的单元测试。
//
// 校验名称↔ID 映射、`IsRangeGraph` 图形字符判定，以及 SQL Mode 展开与
// TiDBX 版本号转换与 Go 侧行为一致。

use crate::charset::*;
use crate::r#const::*;

/// 字符集默认 collation ID 与双向 Collations 表查找应与 Go 一致。
#[test]
fn charset_1_maps_charsets_and_collations_like_go() {
    assert_eq!(CharsetNameToID("utf8mb4"), UTF8MB4DefaultCollationID);
    assert_eq!(CharsetNameToID("gb18030"), GB18030DefaultCollationID);
    assert_eq!(CharsetNameToID("unknown"), 0);

    for (id, name) in Collations {
        assert_eq!(GetCollationNameByID(*id), Some(*name));
        assert_eq!(GetCollationIDByName(name), Some(*id));
    }
    assert_eq!(GetCollationNameByID(17), None);
    assert_eq!(GetCollationIDByName("not_a_collation"), None);
}

/// `IsRangeGraph` 应接受 Unicode 图形类字符并拒绝空白/控制类。
#[test]
fn charset_1_range_graph_matches_go_unicode_tables() {
    // Letters, combining marks, numbers, punctuation and symbols are present
    // in the unicode.RangeTable list used by charset.go.
    // 字母、组合音标、数字、标点与符号应在图形范围内。
    for ch in ['A', '中', '́', '١', '—', '€'] {
        assert!(IsRangeGraph(ch), "{ch:?} should be accepted");
    }
    for ch in [' ', '\n', '\u{0000}', '\u{2028}'] {
        assert!(!IsRangeGraph(ch), "{ch:?} should be rejected");
    }
}

/// SQL Mode 字符串规范化/解析与 TiDBX 发布版本转换应保持 Go 行为。
#[test]
fn charset_1_preserves_const_go_behaviour() {
    assert_eq!(
        FormatSQLModeStr("ansi,ANSI_QUOTES "),
        "REAL_AS_FLOAT,PIPES_AS_CONCAT,ANSI_QUOTES,IGNORE_SPACE,ONLY_FULL_GROUP_BY,ANSI"
    );
    let mode = GetSQLMode("ANSI_QUOTES,NO_ZERO_DATE").expect("known modes");
    assert_eq!(mode.0, ModeANSIQuotes.0 | ModeNoZeroDate.0);
    assert!(GetSQLMode("UNKNOWN_MODE").is_err());

    assert_eq!(
        BuildTiDBXReleaseVersion("v26.3.0-rc.1").unwrap(),
        "CLOUD.202603.0-rc.1"
    );
    for invalid in ["26.3.0", "v24.12.1", "v26.0.1", "v26.13.1"] {
        assert!(BuildTiDBXReleaseVersion(invalid).is_err(), "{invalid}");
    }
}
