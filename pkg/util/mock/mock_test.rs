// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// mock 包上下文键值存取与构造开销的单元/基准测试。
//
// 对应 Go `pkg/util/mock` 中的 `TestContext` / `BenchmarkNewContext`：
// 验证 `Context` 的 Set/Get/Clear，以及反复 `NewContext` 的分配成本。

use std::fmt;
use std::hint::black_box;

use mock_crate::NewContext;

/// 测试用上下文键类型，包装 `i32` 并实现 `Display` 以满足键约束。
#[derive(Clone, Copy)]
struct ContextKeyType(i32);

impl fmt::Display for ContextKeyType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 键名对查找语义无关，固定输出便于稳定断言。
        let _ = self.0;
        formatter.write_str("mock_key")
    }
}

/// 固定的上下文键实例，供 SetValue/Value/ClearValue 共用。
const CONTEXT_KEY: ContextKeyType = ContextKeyType(0);

/// Direct counterpart of Go `TestContext`.
/// 对应 Go `TestContext`：写入、读取再清除同一键，确认值生命周期正确。
#[test]
fn test_context() {
    // 对齐 Go TestMain / 公共测试初始化边界。
    super::mock_main_test::setup_for_common_test();

    let mut ctx = NewContext();
    ctx.SetValue(CONTEXT_KEY, 1_i32);
    assert_eq!(ctx.Value::<i32>(CONTEXT_KEY).copied(), Some(1));

    ctx.ClearValue(CONTEXT_KEY);
    assert_eq!(ctx.Value::<i32>(CONTEXT_KEY), None);
}

/// Stable-Rust form of Go `BenchmarkNewContext`. The caller supplies the same
/// iteration count that Go's benchmark harness exposes as `b.N`.
/// 对应 Go `BenchmarkNewContext`：按调用方传入的迭代次数反复构造 Context，
/// 用 `black_box` 防止编译器优化掉分配。
#[allow(dead_code)]
pub fn benchmark_new_context(iterations: u64) {
    for _ in 0..iterations {
        black_box(NewContext());
    }
}
