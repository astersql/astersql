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

// Aster 迁移补充单测：表格打印字节宽、Classic/NextGen/Enterprise 信息分支，
// 以及 PrintTiDBInfo 条件字段与配置日志。

use crate::{
    GetPrintResult, GetTiDBInfo, PrintTiDBInfo, config, deploymode, kerneltype, versioninfo,
};
use serial_test::serial;
use tracing_test::traced_test;

/// 将 `&str` 切片转为 `Vec<String>`，方便构造表格列/行。
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// 校验空输入失败，以及含多字节字符时表宽按字节长度对齐（与 Go 一致）。
#[test]
fn print_result_matches_go_validation_and_byte_width_behavior() {
    assert_eq!(GetPrintResult(&[], &[]), (String::new(), false));
    assert_eq!(
        GetPrintResult(&strings(&["col1", "col2"]), &[]),
        (String::new(), false)
    );
    assert_eq!(
        GetPrintResult(&strings(&["col1", "col2"]), &[strings(&["only-one"])]),
        (String::new(), false)
    );

    let (result, ok) = GetPrintResult(
        &strings(&["列", "value"]),
        &[strings(&["x", "短"]), strings(&["long", "z"])],
    );
    assert!(ok);
    assert_eq!(
        result,
        "+------+-------+\n\
         | 列  | value |\n\
         +------+-------+\n\
         | x    | 短   |\n\
         | long | z     |\n\
         +------+-------+\n"
    );
}

/// 分别断言 Classic Community 与 NextGen Enterprise 下 GetTiDBInfo 字段。
#[test]
#[serial]
fn tidb_info_matches_classic_nextgen_and_enterprise_branches() {
    config::set_for_test("unistore", true);
    versioninfo::set_for_test("Community", "hash", "branch", "2026-07-12", "");
    kerneltype::set_nextgen_for_test(false);
    let classic = GetTiDBInfo();
    assert!(classic.contains("Release Version: v8.4.0-this-is-a-placeholder"));
    assert!(classic.contains("Edition: Community\nGit Commit Hash: hash"));
    assert!(classic.contains("Check Table Before Drop: true\nStore: unistore"));
    assert!(classic.ends_with("\nKernel Type: Classic"));
    assert!(!classic.contains("TiDB Component Version:"));
    assert!(!classic.contains("Enterprise Extension Commit Hash:"));

    kerneltype::set_nextgen_for_test(true);
    versioninfo::set_for_test(
        "Enterprise",
        "hash",
        "branch",
        "2026-07-12",
        "extension-hash",
    );
    let nextgen = GetTiDBInfo();
    assert!(nextgen.contains("Release Version: CLOUD.202603.0"));
    assert!(nextgen.contains("Edition: Enterprise"));
    assert!(nextgen.contains("\nEnterprise Extension Commit Hash: extension-hash"));
    assert!(nextgen.ends_with("\nKernel Type: Next Generation"));
    assert!(!nextgen.contains("TiDB Component Version:"));
}

/// 校验 PrintTiDBInfo 在 Classic/NextGen 下输出的 tracing 字段与配置 JSON。
#[traced_test]
#[serial]
#[test]
fn print_tidb_info_emits_go_equivalent_conditional_fields_and_config() {
    config::set_for_test("tikv", false);
    versioninfo::set_for_test("Community", "abc123", "main", "now", "");
    kerneltype::set_nextgen_for_test(false);
    PrintTiDBInfo();
    assert!(logs_contain("Welcome to TiDB."));
    assert!(logs_contain("release_version=v8.4.0-this-is-a-placeholder"));
    assert!(logs_contain("kernel_type=Classic"));
    assert!(logs_contain("loaded config"));
    assert!(logs_contain("\"store\":\"tikv\""));
    assert!(!logs_contain("component_version="));
    assert!(!logs_contain("deploy_mode="));

    kerneltype::set_nextgen_for_test(true);
    deploymode::set_for_test("starter");
    PrintTiDBInfo();
    assert!(logs_contain("release_version=CLOUD.202603.0"));
    assert!(logs_contain(
        "component_version=v26.3.0-this-is-a-placeholder"
    ));
    assert!(logs_contain("deploy_mode=starter"));
    assert!(logs_contain("kernel_type=Next Generation"));
}
