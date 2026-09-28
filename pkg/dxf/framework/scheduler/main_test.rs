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

// 调度框架测试入口的 Rust 对照检查。
//
// Go 版 `TestMain` 会执行进程级测试初始化、缩短轮询间隔，并配置 goroutine
// 泄漏检查；Rust 测试使用标准测试运行器和确定性的内存边界，无需复刻这些全局副作用。

#[test]
// 确认没有测试覆盖值时，并发任务数读取接口仍返回框架的默认上限。
fn test_main_equivalent_defaults() {
    assert_eq!(
        crate::GetMaxConcurrentTask(),
        crate::DEFAULT_MAX_CONCURRENT_TASKS
    );
}
