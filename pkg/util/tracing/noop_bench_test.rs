// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// tracing 无操作（noop）路径的有界压力测试。
//
// 对应 Go `noop_bench_test.go` 中四个 benchmark。稳定版 Rust 没有内置
// benchmark harness，因此用固定迭代次数的 `#[test]` 覆盖相同 noop 调用路径；
// 真实性能测量仍由 Go benchmark 负责。

use crate::util::{ChildSpanFromContxt, Context, SpanFromContext};

// Stable Rust has no built-in benchmark harness. These bounded tests execute
// the same no-op paths as the four Go benchmarks; performance remains the Go
// benchmark's responsibility.
/// 对应 BenchmarkNoopLogKV：对 noop span 反复记录结构化日志。
#[test]
fn benchmark_noop_log_kv() {
    // 无 tracing 上下文时 SpanFromContext 返回 noop span。
    let span = SpanFromContext(&Context::background());
    assert!(span.is_noop());
    for _ in 0..128 {
        span.log_kv("event", "noop is finished");
    }
    // Go noopSpan.LogKV 不会把日志键值错当成 baggage 持久化。
    assert_eq!(span.baggage_item("event"), None);
}

/// 对应 BenchmarkNoopLogKVWithF：写入前先 format 字符串，模拟带格式化的日志路径。
#[test]
fn benchmark_noop_log_kv_with_f() {
    let span = SpanFromContext(&Context::background());
    for _ in 0..128 {
        let message = format!("this is format {}", "noop is finished");
        span.log_kv("event", &message);
    }
    assert_eq!(span.baggage_item("event"), None);
}

/// 对应 BenchmarkSpanFromContext：反复从空 Context 取 span，应始终为 noop。
#[test]
fn benchmark_span_from_context() {
    let ctx = Context::background();
    for _ in 0..128 {
        assert!(SpanFromContext(&ctx).is_noop());
    }
}

/// 对应 BenchmarkChildFromContext：从空 Context 创建子 span，应返回 noop 且不改 Context。
#[test]
fn benchmark_child_from_context() {
    let ctx = Context::background();
    for _ in 0..128 {
        let (child, returned_ctx) = ChildSpanFromContxt(ctx.clone(), "child");
        assert!(child.is_noop());
        // same_instance：返回的 Context 与输入共享同一 Arc，未派生新值。
        assert!(returned_ctx.same_instance(&ctx));
    }
}
