// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// naming 包迁移回归测试：对照 Go 侧合法/非法名称、长度边界与错误文案。
//
// Keyspace（键空间）是物理集群内逻辑隔离单元；本文件验证名称校验与 Go 行为一致。

use super::{Check, CheckKeyspaceName, CheckWithMaxLen};

/// 合法服务范围名（字母数字、连字符、下划线、空串、全连字符）应通过 Check。
#[test]
fn migration_accepts_the_same_scope_names_as_go() {
    for name in ["789z-_", "scope1", "", "-----"] {
        assert!(Check(name).is_ok(), "{name:?} should be valid");
    }
}

/// 非法字符、空白、换行与非 ASCII（如中文）应被拒绝。
#[test]
fn migration_rejects_invalid_characters_and_unicode() {
    for name in ["789z-_)", "contains space", "line\nfeed", "数据库"] {
        assert!(Check(name).is_err(), "{name:?} should be invalid");
    }
}

/// 校验 scope 默认上限 64 与 keyspace 名称上限 20 的边界。
#[test]
fn migration_enforces_scope_and_keyspace_boundaries() {
    assert!(Check(&"a".repeat(64)).is_ok());
    assert!(Check(&"a".repeat(65)).is_err());
    assert!(CheckKeyspaceName(&"k".repeat(20)).is_ok());
    assert!(CheckKeyspaceName(&"k".repeat(21)).is_err());
}

/// 错误消息文本须与 Go fmt.Errorf 文案逐字一致，便于跨语言对照。
#[test]
fn migration_preserves_go_error_text() {
    assert_eq!(
        CheckWithMaxLen("bad!", 3),
        Err("the value 'bad!' is invalid. It must be 3 characters or fewer and consist only of letters (a-z, A-Z), numbers (0-9), hyphens (-), and underscores (_)".to_owned())
    );
}

/// 负的 maxLen 会使正则非法；对齐 Go MustCompile 的 panic 语义。
#[test]
#[should_panic]
fn migration_negative_max_len_panics_like_go_must_compile() {
    let _ = CheckWithMaxLen("", -1);
}
