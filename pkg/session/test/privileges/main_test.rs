// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `privileges` 测试 harness 与 Go `TestMain` 语义对照。
//
// 上方保留 Go 测试主入口（AsyncCommit、failpoint、goleak）草稿；
// 可执行部分覆盖全局 TiKVClient 配置，并校验 session bootstrap 常量名。

use astersql_config::{get_global_config, update_global};
use astersql_session::bootstrap::{tidbClusterID, tidbDDLTableVersion};
use serial_test::serial;

/// 断言 privileges harness 可改写 TiKVClient 全局项，且 bootstrap 常量名与 Go 一致。
// 对应 TestMain：privileges 包测试入口会改写全局 TiKVClient 配置，并依赖 session bootstrap 常量名。
#[test]
#[serial(privileges_harness)]
fn privileges_harness_updates_global_tikv_client_and_exposes_bootstrap_names() {
    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();

    assert_eq!(tidbClusterID, "cluster_id");
    assert_eq!(tidbDDLTableVersion, "ddl_table_version");
}
