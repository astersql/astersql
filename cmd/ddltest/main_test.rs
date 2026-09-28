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

//! Go-equivalent of `cmd/ddltest/main_test.go` TestMain setup.
//! 对齐 Go 版 `TestMain` 的公共测试入口，确保测试环境初始化和日志准备链路可用。

use astersql_cmd_ddltest::stubs::{LOG_LEVEL, setup_test_main};

/// TestMain — Go `TestMain` (common setup + logger + goleak ignore list).
/// 这里不覆盖全部 goleak 细节，只验证 Rust 侧共享初始化桩已连通且日志级别已注入。
#[test]
fn test_main_setup() {
    assert!(!LOG_LEVEL.is_empty());
    setup_test_main().expect("TestMain setup");
}
