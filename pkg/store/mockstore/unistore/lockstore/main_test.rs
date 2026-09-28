// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// lockstore 包测试入口配置（对应 Go `TestMain`）。
//
// lockstore 是 unistore 中存放悲观锁（pessimistic lock）等键值对的内存存储。
// lib.rs 在 libtest 启动前安装公共测试环境，本文件验证初始化时序与错误传播。
// 原生读者线程在 lockstore_test.rs 和 migration_aster_unit_test.rs 中逐一 join
// 并传播 panic。

#[test]
fn common_setup_runs_before_filtered_tests() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "main_test::configured_level_probe"])
        .env("log_level", "debug")
        .env("LOCKSTORE_SETUP_PROBE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn configured_level_probe() {
    if std::env::var_os("LOCKSTORE_SETUP_PROBE").is_some() {
        assert_eq!(testsetup::configured_log_level().to_string(), "DEBUG");
    }
}

#[test]
fn invalid_log_level_fails_even_without_selected_tests() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "no_such_lockstore_test"])
        .env("log_level", "invalid-lockstore-level")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("applyOSLogLevel failed"));
}
