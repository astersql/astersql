// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `bootstraptest2` 测试 harness 与 Go `TestMain` 语义对照。
//
// 可执行测试校验升级相关超时与 `mysql.tidb` 变量名常量。
use astersql_session::bootstrap::{internalSQLTimeout, tidbClusterID, tidbDDLTableVersion};
use astersql_testkit_testmain::{TestingM, WrapTestingM};

/// Rust 的内建测试运行器没有 Go `TestMain` 的同名入口；以这一最小 runner 驱动与 Go
/// 等价的 testmain 包装器，验证回调不会吞掉套件退出码。
struct FixedExitCode(i32);

impl TestingM for FixedExitCode {
    fn run(&self) -> i32 {
        self.0
    }
}

/// 断言升级 harness 使用的超时与 `cluster_id` / `ddl_table_version` 变量名。
#[test]
fn bootstrap_harness_exposes_canonical_upgrade_timeout_and_variable_names() {
    assert_eq!(internalSQLTimeout.as_secs(), 75);
    assert_eq!(tidbClusterID, "cluster_id");
    assert_eq!(tidbDDLTableVersion, "ddl_table_version");
}

/// 对应 Go TestMain 的 `testmain.WrapTestingM` 收尾路径。
#[test]
fn bootstrap_harness_preserves_runner_exit_code_after_cleanup_callback() {
    let runner = FixedExitCode(17);
    let wrapped = WrapTestingM(&runner, Some(Box::new(|exit_code| exit_code)));
    assert_eq!(wrapped.run(), 17);
}
