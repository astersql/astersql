// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 对应 Go `seqtest` 包进程入口语义：共享库上的会话隔离与预编译查询烟测。
//
// `Session::peer` 共享底层 `Database`，会话本地状态（事务、prepared）相互独立；
// 本测试验证 peer 可见已提交 schema/数据，并能 prepare/execute/close cursor。

use astersql_config::{get_global_config, restore_func, update_global};

use crate::{Session, Value, row};

/// 对应 Go `config.UpdateGlobal`：测试进程中禁用 AsyncCommit 时间窗口。
fn configure_seqtest_harness() {
    update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

#[test]
fn test_main_matches_go_process_setup() {
    let restore = restore_func();
    update_global(|config| {
        config.tikv_client.async_commit.safe_window = 1;
        config.tikv_client.async_commit.allowed_clock_drift = 2;
    });

    astersql_testkit_testsetup::SetupForCommonTest();
    configure_seqtest_harness();

    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();
}

/// 在共享数据库上创建 peer 会话，执行预编译 SELECT 并关闭游标。
#[test]
fn test_main_initializes_isolated_shared_sessions() {
    let mut first = Session::default();
    first
        .create_table("config", &["id", "safe_window"])
        .unwrap();
    first
        .insert("config", row(&[("safe_window", Value::Int(0))]))
        .unwrap();
    // peer：共享同一 Database，模拟同进程多连接。
    let mut second = first.peer();
    let statement = second
        .prepare_select("config", &["safe_window"], None)
        .unwrap();
    let mut result = second.execute_prepared(statement, &[]).unwrap();
    assert_eq!(result.next().unwrap(), Some(vec![Value::Int(0)]));
    result.close();
    assert!(result.is_closed());
}
