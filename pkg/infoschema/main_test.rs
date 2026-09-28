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

// InfoSchema 包级测试入口的可移植公共环境校验。

// `SetupForCommonTest` 在 Rust 侧的可观察契约是接受与 PingCAP zap 相同的
// `log_level` 值。实际 logger 安装由共享 testsetup crate 覆盖；本 crate 的依赖
// 边界只保留环境验证，且不引入 Go 专属的进程级测试 harness。
fn setup_for_common_test() {
    let Ok(level) = std::env::var("log_level") else {
        return;
    };
    if level.is_empty() {
        return;
    }
    assert!(
        matches!(
            level.to_ascii_lowercase().as_str(),
            "debug" | "info" | "warn" | "warning" | "error" | "dpanic" | "panic" | "fatal"
        ),
        "unrecognized log level {level:?}"
    );
}

#[test]
fn test_main_validates_common_environment() {
    setup_for_common_test();
}
