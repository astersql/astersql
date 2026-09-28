// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 对照 Go const_test：校验 SQLMode 位值、版本分隔符与 TiDB-X 版本串转换。
//
// SQL mode 是会话级兼容开关位掩码，影响解析器/执行器对引号、严格模式等行为；
// 版本分隔符参与 PD 对 ServerVersion 的拼接与反向解析，不得改动字面量。

// 本文件对照 pkg/parser/mysql/const_test.go 迁移，保留 Go 测试结构与行为。
// 校验内存中的 MySQL 协议常量、SQL mode 位值和版本字符串转换。
use crate::r#const::*;

// test_sql_mode 对应 Go 的 TestSQLMode，逐项固定 SQLMode 与 MySQL binlog query-event 文档中的数值。
#[test]
fn test_sql_mode() {
    // ref https://dev.mysql.com/doc/internals/en/query-event.html#q-sql-mode-code,
    // 下列位值必须与 binlog Query event 中的 sql_mode 字段编码一致。
    let hard_code: &[(SQLMode, i64)] = &[
        (ModeRealAsFloat, 0x00000001),
        (ModePipesAsConcat, 0x00000002),
        (ModeANSIQuotes, 0x00000004),
        (ModeIgnoreSpace, 0x00000008),
        (ModeNotUsed, 0x00000010),
        (ModeOnlyFullGroupBy, 0x00000020),
        (ModeNoUnsignedSubtraction, 0x00000040),
        (ModeNoDirInCreate, 0x00000080),
        (ModePostgreSQL, 0x00000100),
        (ModeOracle, 0x00000200),
        (ModeMsSQL, 0x00000400),
        (ModeDb2, 0x00000800),
        (ModeMaxdb, 0x00001000),
        (ModeNoKeyOptions, 0x00002000),
        (ModeNoTableOptions, 0x00004000),
        (ModeNoFieldOptions, 0x00008000),
        (ModeMySQL323, 0x00010000),
        (ModeMySQL40, 0x00020000),
        (ModeANSI, 0x00040000),
        (ModeNoAutoValueOnZero, 0x00080000),
        (ModeNoBackslashEscapes, 0x00100000),
        (ModeStrictTransTables, 0x00200000),
        (ModeStrictAllTables, 0x00400000),
        (ModeNoZeroInDate, 0x00800000),
        (ModeNoZeroDate, 0x01000000),
        (ModeInvalidDates, 0x02000000),
        (ModeErrorForDivisionByZero, 0x04000000),
        (ModeTraditional, 0x08000000),
        (ModeNoAutoCreateUser, 0x10000000),
        (ModeHighNotPrecedence, 0x20000000),
        (ModeNoEngineSubstitution, 0x40000000),
        (ModePadCharToFullLength, 0x80000000),
    ];

    for (code, value) in hard_code {
        // Go 使用 require.Equal(t, ca.value, int(ca.code))；SQLMode 在 Rust 里是 newtype。
        assert_eq!(*value, code.0);
    }
}

// test_version_separator 对应 Go 的 TestVersionSeparator，防止 PD 解析 ServerVersion 的分隔符被改动。
#[test]
fn test_version_separator() {
    // DO NOT change the value of VersionSeparator.
    assert_eq!("-TiDB-", VersionSeparator);
}

// test_build_ti_dbx_release_version 对应 Go 的 TestBuildTiDBXReleaseVersion。
// 它验证 next-gen release/server 版本格式，并保留 invalid version 的错误分支检查。
#[test]
fn test_build_ti_dbx_release_version() {
    let tidb_x_version =
        BuildTiDBXReleaseVersion("v26.3.0").expect("BuildTiDBXReleaseVersion should succeed");
    assert_eq!("CLOUD.202603.0", tidb_x_version);

    let tidb_x_version = BuildTiDBXReleaseVersion("v26.3.0-xxx")
        .expect("BuildTiDBXReleaseVersion should keep prerelease");
    assert_eq!("CLOUD.202603.0-xxx", tidb_x_version);

    let server_version =
        BuildTiDBXServerVersion("v26.3.0").expect("BuildTiDBXServerVersion should succeed");
    assert_eq!("8.0.11-TiDB-CLOUD.202603.0", server_version);

    let server_version = BuildTiDBXServerVersion("v26.3.0-xxx")
        .expect("BuildTiDBXServerVersion should keep prerelease");
    assert_eq!("8.0.11-TiDB-CLOUD.202603.0-xxx", server_version);

    for ver in ["26.1.1", "v26xxxx", "v24.1.1", "v26.0.1", "v26.13.1"] {
        let err =
            BuildTiDBXReleaseVersion(ver).expect_err("invalid TiDB release version should fail");
        assert!(err.to_string().contains("invalid TiDB release version"));
    }
}

// test_normalize_ti_db_release_version_for_next_gen 对应 Go 的 TestNormalizeTiDBReleaseVersionForNextGen。
// 第一个断言覆盖 classic 占位版本到 next-gen 占位版本的特殊改写，第二个断言覆盖普通版本原样返回。
#[test]
fn test_normalize_ti_db_release_version_for_next_gen() {
    assert_eq!(
        "v26.3.0-this-is-a-placeholder",
        NormalizeTiDBReleaseVersionForNextGen("v8.4.0-this-is-a-placeholder")
    );
    assert_eq!("v26.3.0", NormalizeTiDBReleaseVersionForNextGen("v26.3.0"));
}
