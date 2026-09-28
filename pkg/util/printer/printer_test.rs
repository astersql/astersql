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

// printer 核心 API 单测：GetPrintResult 校验、GetTiDBInfo / PrintTiDBInfo
// 在 Classic 与 NextGen 下的字段差异。

use crate::{GetPrintResult, GetTiDBInfo, PrintTiDBInfo, deploymode, kerneltype, mysql};
use serial_test::serial;
use tracing_test::traced_test;

/// 将 `&str` 切片转为 `Vec<String>`。
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// 覆盖列行不齐、正常表格、空数据与空列等 GetPrintResult 路径。
#[test]
fn test_print_result() {
    let mut cols = strings(&["col1", "col2", "col3"]);
    let mut datas = vec![strings(&["11"]), strings(&["21", "22", "23"])];
    let (result, ok) = GetPrintResult(&cols, &datas);
    assert!(!ok);
    assert_eq!("", result);

    datas = vec![strings(&["11", "12", "13"]), strings(&["21", "22", "23"])];
    let expect = "\n\
+------+------+------+\n\
| col1 | col2 | col3 |\n\
+------+------+------+\n\
| 11   | 12   | 13   |\n\
| 21   | 22   | 23   |\n\
+------+------+------+\n";
    let (result, ok) = GetPrintResult(&cols, &datas);
    assert!(ok);
    assert_eq!(&expect[1..], result);

    datas = Vec::new();
    let (result, ok) = GetPrintResult(&cols, &datas);
    assert!(!ok);
    assert_eq!("", result);

    cols = Vec::new();
    let (result, ok) = GetPrintResult(&cols, &datas);
    assert!(!ok);
    assert_eq!("", result);
}

/// 按当前内核类型断言 GetTiDBInfo 中的 Kernel Type 与 Release Version。
#[test]
#[serial]
fn test_get_tidb_info() {
    let info = GetTiDBInfo();
    if kerneltype::IsNextGen() {
        assert!(info.contains("\nKernel Type: Next Generation"));
        let normalized =
            mysql::NormalizeTiDBReleaseVersionForNextGen(unsafe { mysql::TiDBReleaseVersion });
        let expected_release_version = mysql::BuildTiDBXReleaseVersion(&normalized).unwrap();
        assert!(info.contains(&format!("Release Version: {expected_release_version}")));
    } else {
        assert!(info.contains("\nKernel Type: Classic"));
        assert!(info.contains(&format!("Release Version: {}", unsafe {
            mysql::TiDBReleaseVersion
        })));
    }
    assert!(!info.contains("TiDB Component Version:"));
}

/// 校验 PrintTiDBInfo 日志：NextGen 应含 component_version 与 deploy_mode。
#[traced_test]
#[test]
#[serial]
fn test_print_tidb_info() {
    deploymode::set_for_test("premium");
    PrintTiDBInfo();
    assert!(logs_contain("Welcome to TiDB."));
    if kerneltype::IsNextGen() {
        let expected =
            mysql::NormalizeTiDBReleaseVersionForNextGen(unsafe { mysql::TiDBReleaseVersion });
        assert!(logs_contain(&format!("component_version={expected}")));
        assert!(logs_contain("deploy_mode=premium"));
    } else {
        assert!(!logs_contain("component_version="));
        assert!(!logs_contain("deploy_mode="));
    }
}
