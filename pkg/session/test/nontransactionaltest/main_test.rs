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

// `nontransactionaltest` 测试 harness 与 Go `TestMain` 语义对照。
//
// 上方 `_GO_DRAFT_ARCHIVE` 保留 Go 测试主入口（AsyncCommit 窗口清零、failpoint、goleak）草稿；
// 可执行部分验证其中的全局 AsyncCommit 配置语义。

use astersql_config::{get_global_config, update_global};

fn configure_nontransactionaltest_harness() {
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

/// 断言测试入口精确清零 Go `TestMain` 指定的 AsyncCommit 时间窗口。
#[test]
fn nontransactionaltest_harness_clears_async_commit_timing_windows() {
    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 1;
        conf.tikv_client.async_commit.allowed_clock_drift = 2;
    });
    configure_nontransactionaltest_harness();
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();
}
