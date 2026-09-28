// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Go-equivalent export helpers from `export_test.go` (same-package test accessors).
//!
//! Go `export_test.go` re-exports private blocklist symbols for `restore_test`.
//! Rust production already exposes the same aliases on `misc`; this module keeps
//! the Go export surface available to sibling test modules.
//!
//! 模块职责：把 Go `export_test.go` 的同包导出面接到 Rust，供兄弟测试访问 blocklist
//! 前缀/解析/反序列化别名；生产代码已在 `misc` 公开，此处只保证测试侧符号对称。
//! 约束：不新增业务逻辑，仅 re-export 与别名一致性断言。
//!
//! 为同包测试暴露 log-restore 表 ID 黑名单文件前缀与解析别名。
//! 大小写两套符号与 Go export_test 双名习惯对齐。
//! 不包含 Unmarshal 行为测试，仅锁定导出面与文件名解析。

#![allow(unused_imports)]

pub use crate::misc::{
    LogRestoreTableIDBlocklistFilePrefix, ParseLogRestoreTableIDsBlocklistFileName,
    UnmarshalLogRestoreTableIDsBlocklistFile, logRestoreTableIDBlocklistFilePrefix,
    parseLogRestoreTableIDsBlocklistFileName, unmarshalLogRestoreTableIDsBlocklistFile,
};

/// 验证导出别名与 Go export_test 路径一致：大小写别名同值，文件名解析对称。
#[test]
fn test_export_aliases_match_go_export_test() {
    // 公开常量与内部别名必须同值。
    // 前缀路径变更会破坏已有黑名单对象布局。
    // PascalCase 与 camelCase 别名必须指向同一前缀常量。
    assert_eq!(
        LogRestoreTableIDBlocklistFilePrefix,
        logRestoreTableIDBlocklistFilePrefix
    );
    assert_eq!(
        LogRestoreTableIDBlocklistFilePrefix,
        // 与 Go 常量字面量字节级一致，路径变更会破坏外部已有文件布局。
        "v1/log_restore_tables_blocklists"
    );
    // 全 F hex 文件名解析为 u64::MAX；大小写 API 结果一致。
    // 全 F 十六进制表示 u64::MAX 的 commit/start ts 边界样例。
    let (c, s, ok) =
        ParseLogRestoreTableIDsBlocklistFileName("RFFFFFFFFFFFFFFFF_SFFFFFFFFFFFFFFFF.meta");
    assert!(ok);
    assert_eq!(c, u64::MAX);
    assert_eq!(s, u64::MAX);
    // 小写别名解析结果必须与大写入口完全相同。
    let (c2, s2, ok2) =
        parseLogRestoreTableIDsBlocklistFileName("RFFFFFFFFFFFFFFFF_SFFFFFFFFFFFFFFFF.meta");
    assert!(ok2);
    assert_eq!((c, s), (c2, s2));
}
