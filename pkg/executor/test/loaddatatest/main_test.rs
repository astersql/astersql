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

//! Rust counterpart of the package-level `TestMain` in `main_test.go`.
//!
//! 验证 LOAD DATA 包级测试环境的默认覆盖项及清理回调，与 Go `TestMain` 的行为保持一致。

use crate::LoadDataRuntime;

#[test]
fn test_main_applies_go_test_environment_overrides_and_cleanup() {
    // 这些默认值集中复刻 Go 测试入口安装的全局配置，避免各用例重复搭建包级环境。
    let mut runtime = LoadDataRuntime::default();
    assert_eq!(runtime.auto_id_step, 5_000);
    assert_eq!(runtime.slow_threshold_ms, 30_000);
    assert_eq!(runtime.async_commit_safe_window, 0);
    assert_eq!(runtime.async_commit_allowed_clock_drift, 0);
    assert!(runtime.allows_expression_index);
    assert!(runtime.failpoints_enabled);
    assert!(!runtime.cleanup_runs);
    assert!(!runtime.fix56408_store_cleanup_runs);
    assert!(!runtime.view_stopped);
    // 清理标记必须仅在显式执行回调后生效，用于守护测试环境的收尾流程。
    runtime.cleanup();
    assert!(runtime.cleanup_runs);
    assert!(runtime.fix56408_store_cleanup_runs);
    assert!(runtime.view_stopped);
}
