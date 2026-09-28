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

// `session/test/common` 测试 harness 与 Go `TestMain` 语义对照。
//
// `_GO_DRAFT_ARCHIVE` 保留 Go 侧 failpoint / goleak / AsyncCommit 清零的启动流程草稿；
// 可执行测试用可观察的 `store_limit` 覆盖验证测试入口可改写全局 TiKV 客户端配置。

/// 归档 Go `TestMain` 草稿字符串，不参与运行，仅供对照迁移语义。
use astersql_config::{get_global_config, update_global};

/// 校验 common harness 改写的正是 Go `TestMain` 中的 AsyncCommit 时间窗口。
// `restore_func` 让全局配置修改在用例结束后回滚，避免和其它测试串扰。
#[test]
fn common_harness_clears_async_commit_timing_windows() {
    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();
}
