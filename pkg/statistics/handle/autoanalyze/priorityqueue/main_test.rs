// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 优先队列包级测试入口与后台轮询间隔常量校验。
//
// Go 侧 `TestMain` 通过 `testsetup.SetupForCommonTest` 初始化全局测试环境，并用
// 关闭并 join。此处保留可执行的包级常量契约校验。
use crate::{
    DML_CHANGES_FETCH_INTERVAL, LAST_ANALYSIS_DURATION_REFRESH_INTERVAL,
    MUST_RETRY_JOB_REQUEUE_INTERVAL,
};

/// 校验 DML 变更拉取、must-retry 重入队、上次分析时长刷新三类后台间隔秒数。
#[test]
fn queue_background_intervals_match_go_defaults() {
    // 与 Go 常量保持一致：2min / 5min / 10min。
    assert_eq!(120, DML_CHANGES_FETCH_INTERVAL.as_secs());
    assert_eq!(300, MUST_RETRY_JOB_REQUEUE_INTERVAL.as_secs());
    assert_eq!(600, LAST_ANALYSIS_DURATION_REFRESH_INTERVAL.as_secs());
}
