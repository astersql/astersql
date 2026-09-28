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

// SEM 配置解析与校验单元测试。
//
// 覆盖：合法/非法/空 JSON 解析，以及 `validateSEMConfig` 对版本、系统变量与 SQL 规则的校验。

use std::io::Write;

use serial_test::serial;

/// 将配置字符串写入临时文件后调用 `parseSEMConfigFromFile`，模拟从路径加载。
fn parseSEMConfig(configStr: &str) -> Result<Config, String> {
    let mut tempFile = tempfile::NamedTempFile::new()
        .map_err(|err| format!("failed to create temp SEM config file: {err}"))?;
    tempFile
        .write_all(configStr.as_bytes())
        .map_err(|err| format!("failed to write temp SEM config file: {err}"))?;
    tempFile
        .flush()
        .map_err(|err| format!("failed to flush temp SEM config file: {err}"))?;
    parseSEMConfigFromFile(tempFile.path().to_str().expect("temp path must be UTF-8"))
}

// TestParseConfigWithDifferentFormat 对应 Go 的表驱动测试：覆盖合法 JSON、非法 JSON 和空 JSON。
// 子测试名称、输入和 wantErr 结构按原顺序保留。
/// 表驱动：合法配置、截断 JSON、空对象三种格式的解析结果。
#[test]
fn test_parse_config_with_different_format() {
    struct TestCase {
        name: &'static str,
        config: &'static str,
        wantErr: bool,
    }

    let cases = vec![
        TestCase {
            name: "valid config",
            config: r#"{
                "version": "1.0",
                "tidb_version": "v6.0.0",
                "restricted_databases": ["mysql", "test"],
                "restricted_tables": [
                    {
                        "schema": "test",
                        "name": "t1",
                        "hidden": false,
                        "columns": [
                            {"name": "c1", "hidden": true, "value": "default"}
                        ]
                    }
                ],
                "restricted_variables": [
                    {"name": "autocommit", "hidden": false, "readonly": true, "value": "1"}
                ],
                "restricted_privileges": ["SUPER"],
                "restricted_sql": {
                    "sql": ["DROP DATABASE"],
                    "rule": ["no_drop"]
                }
            }"#,
            wantErr: false,
        },
        TestCase {
            name: "invalid JSON",
            config: r#"{"version": "1.0", "tidb_version": "v6.0.0","#,
            wantErr: true,
        },
        TestCase {
            name: "empty JSON",
            config: r#"{}"#,
            wantErr: false,
        },
        // encoding/json accepts null for non-pointer fields and leaves their
        // zero values unchanged.
        TestCase {
            name: "null values",
            config: r#"{
                "version": null,
                "tidb_version": null,
                "restricted_databases": null,
                "restricted_tables": null,
                "restricted_variables": null,
                "restricted_status_variables": null,
                "restricted_privileges": null,
                "restricted_sql": null,
                "restricted_hints": null
            }"#,
            wantErr: false,
        },
        TestCase {
            name: "nested null values",
            config: r#"{
                "restricted_tables": [{
                    "schema": null,
                    "name": null,
                    "hidden": null,
                    "columns": [{"name": null, "hidden": null, "value": null}]
                }],
                "restricted_variables": [{
                    "name": null,
                    "hidden": null,
                    "readonly": null,
                    "value": null
                }],
                "restricted_sql": {"sql": null, "rule": null}
            }"#,
            wantErr: false,
        },
        // Go json.Decoder.Decode consumes exactly one JSON value and does not
        // require EOF, so a second value is left unread rather than rejected.
        TestCase {
            name: "trailing JSON value",
            config: r#"{"version":"1.0"} {"version":"2.0"}"#,
            wantErr: false,
        },
    ];

    for tc in cases {
        let result = parseSEMConfig(tc.config);
        if tc.wantErr {
            assert!(result.is_err(), "{}: expected error but got none", tc.name);
        } else {
            assert!(
                result.is_ok(),
                "{}: unexpected error: {:?}",
                tc.name,
                result.err()
            );
        }
    }
}

// TestValidateConfig 对应 Go 的配置校验表驱动测试。
// 它会修改 mysql.TiDBReleaseVersion 这个包级全局变量，仅保留测试前置条件。
/// 表驱动：校验 TiDB 版本下限、未知系统变量、带 Value 但非只读、未知 SQL 规则等错误路径。
#[test]
#[serial]
fn test_validate_config() {
    // 固定发行版本并注册 autocommit，保证校验路径可复现。
    unsafe { mysql::r#const::TiDBReleaseVersion = "v9.0.0" };
    variable::UnregisterSysVar(vardef::AutoCommit);
    variable::RegisterSysVar(variable::SysVar {
        Name: vardef::AutoCommit.to_owned(),
        Value: vardef::On.to_owned(),
        Scope: vardef::ScopeGlobal,
        ..Default::default()
    });

    struct TestCase {
        name: &'static str,
        config: &'static str,
        errMsg: &'static str,
    }

    let cases = vec![
        TestCase {
            name: "valid config",
            config: r#"{
                "version": "1.0",
                "tidb_version": "v6.0.0",
                "restricted_variables": [
                    {"name": "autocommit", "hidden": false, "readonly": true, "value": ""}
                ]
            }"#,
            errMsg: "",
        },
        TestCase {
            name: "invalid TiDB version",
            config: r#"{
                "version": "1.0",
                "tidb_version": "v99.0.0"
            }"#,
            errMsg: "current TiDB version",
        },
        TestCase {
            name: "unknown variable",
            config: r#"{
                "version": "1.0",
                "tidb_version": "v6.0.0",
                "restricted_variables": [
                    {"name": "invalid_var", "hidden": false, "readonly": true, "value": "1"}
                ]
            }"#,
            errMsg: "restricted variable invalid_var is not a valid system variable",
        },
        TestCase {
            name: "invalid value for variable",
            config: r#"{
                "version": "1.0",
                "tidb_version": "v6.0.0",
                "restricted_variables": [
                    {"name": "autocommit", "hidden": false, "readonly": true, "value": "1"}
                ]
            }"#,
            errMsg: "restricted variable autocommit has a value set, but it is not a readonly variable",
        },
        TestCase {
            name: "invalid restricted SQL rule",
            config: r#"{
                "version": "1.0",
                "tidb_version": "v6.0.0",
                "restricted_sql": {
                    "sql": ["DROP DATABASE"],
                    "rule": ["unknown_rule"]
                }
            }"#,
            errMsg: "unknown SQL rule: unknown_rule",
        },
    ];

    for tc in cases {
        let semConfig = parseSEMConfig(tc.config)
            .unwrap_or_else(|err| panic!("{}: failed to parse SEM config: {err}", tc.name));
        let err = validateSEMConfig(&semConfig);
        if err.is_err() && tc.errMsg.is_empty() {
            panic!("{}: expected no error, but got: {:?}", tc.name, err.err());
        }
        if err.is_ok() && !tc.errMsg.is_empty() {
            panic!("{}: expected error {:?}, but got none", tc.name, tc.errMsg);
        }
        if let Err(err) = err {
            assert!(
                err.contains(tc.errMsg),
                "{}: expected error to contain {:?}, but got: {}",
                tc.name,
                tc.errMsg,
                err
            );
        }
    }
    variable::UnregisterSysVar(vardef::AutoCommit);
}
