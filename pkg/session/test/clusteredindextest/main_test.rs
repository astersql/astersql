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

// `clusteredindextest` 测试 harness 与 Go `TestMain` 语义对照。
//
// `_GO_DRAFT_ARCHIVE` 保留 Go 侧缩短 schema lease、清零 AsyncCommit 窗口、
// failpoint 与 goleak 的启动流程草稿；可执行测试校验 schema lease 缩短与
// bootstrap 常量名，以及全局 TiKV 客户端配置可被测试入口覆盖。
//
// Schema lease（schema 租约）：DDL 后其它 session 感知新 schema 的最大等待时间；
// 缩短可加速聚簇索引相关测试中的 schema 传播。

/// 归档 Go `TestMain` 草稿字符串，不参与运行，仅供对照迁移语义。
use std::time::Duration;

use astersql_config::{get_global_config, update_global};
use astersql_session::bootstrap::{tidbClusterID, tidbDDLTableVersion};
use astersql_sessionctx_vardef::{GetSchemaLease, SetSchemaLease};

/// 校验 harness 可将 schema lease 缩至 20ms，并清零 Async Commit 窗口，同时暴露 bootstrap 常量名。
// 对应 TestMain：clusteredindextest 入口会把 schema lease 缩短到 20ms，并清零
// TiKVClient AsyncCommit 的安全窗口和允许时钟漂移。
#[test]
fn clusteredindextest_harness_shortens_schema_lease_and_exposes_bootstrap_names() {
    // 先缩短再恢复，避免污染其它用例的 schema lease。
    let previous = GetSchemaLease();
    SetSchemaLease(Duration::from_millis(20));
    assert_eq!(GetSchemaLease(), Duration::from_millis(20));
    SetSchemaLease(previous);

    // Go TestMain 清零 AsyncCommit 的安全窗口和允许时钟漂移；两项都必须可观察。
    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
    let tikv_client = &get_global_config().tikv_client;
    assert_eq!(tikv_client.async_commit.safe_window, 0);
    assert_eq!(tikv_client.async_commit.allowed_clock_drift, 0);
    restore();

    // 与 Go bootstrap 变量名保持一致，供后续 DDL/集群元数据测试引用。
    assert_eq!(tidbClusterID, "cluster_id");
    assert_eq!(tidbDDLTableVersion, "ddl_table_version");
}
