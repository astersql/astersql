// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Cascades 基础 Equals 接口形状的确定性测试。
//
// Go 原文件是 Benchmark；这里改成单元测试，验证泛型 Equals 与
// `dyn Any` 类型擦除路径（含断言失败分支）行为一致。

/// 测试用简易结构体，字段参与相等性比较。
#[allow(non_camel_case_types)]
struct testcase {
    a: i64,
    b: i64,
    c: String,
}

/// 泛型 Equals，对应 Go 的 `TestEquals[T]`。
trait TestEquals<T> {
    /// 与同类型（或引用）对象比较是否相等。
    fn EqualsT(&self, other: T) -> bool;
}

/// 类型擦除 Equals，对应 Go 接受 `any` 的比较入口。
trait TestEqualAny {
    /// 对 `dyn Any` 做 downcast 后再比较字段。
    fn EqualsAny(&self, other: &dyn std::any::Any) -> bool;
}

// Go embeds TestEquals[TestSuperEquals]. Keeping the method on this trait
// preserves that self-referential interface shape without an unusable Rust
// super-trait cycle.
/// 自引用 Equals 形状占位，避免 Rust 超 trait 循环。
#[allow(dead_code)]
trait TestSuperEquals {
    /// 与另一 `TestSuperEquals` 动态对象比较。
    fn EqualsT(&self, other: &dyn TestSuperEquals) -> bool;
}

impl TestEquals<&testcase> for testcase {
    fn EqualsT(&self, other: &testcase) -> bool {
        self.a == other.a && self.b == other.b && self.c == other.c
    }
}

impl TestEqualAny for testcase {
    fn EqualsAny(&self, other: &dyn std::any::Any) -> bool {
        other
            .downcast_ref::<testcase>()
            .is_some_and(|tc| self.a == tc.a && self.b == tc.b && self.c == tc.c)
    }
}

// BenchmarkEqualsT is represented as a deterministic test: it exercises the
// same runtime type assertion and then dispatches through TestEquals.
/// 验证类型断言后走泛型 Equals 的路径。
#[test]
fn benchmark_equals_t() {
    let tc1 = testcase {
        a: 1,
        b: 2,
        c: "3".to_owned(),
    };
    let tc2 = testcase {
        a: 1,
        b: 2,
        c: "3".to_owned(),
    };
    // 先擦除再 downcast，模拟 Go 的 interface{} 断言。
    let erased: &dyn std::any::Any = &tc2;
    let tc3 = erased
        .downcast_ref::<testcase>()
        .expect("testcase assertion");
    let equaler: &dyn TestEquals<&testcase> = &tc1;
    assert!(equaler.EqualsT(tc3));
}

// BenchmarkEqualsAny is represented as a deterministic test over the same
// type-erased call path, including the Go assertion-failure branch.
/// 验证 `EqualsAny` 在类型匹配与断言失败（非 testcase）两种分支。
#[test]
fn benchmark_equals_any() {
    let tc1 = testcase {
        a: 1,
        b: 2,
        c: "3".to_owned(),
    };
    let tc2 = testcase {
        a: 1,
        b: 2,
        c: "3".to_owned(),
    };
    let equaler: &dyn TestEqualAny = &tc1;
    assert!(equaler.EqualsAny(&tc2));
    // 非 testcase 类型 downcast 失败，应返回 false。
    assert!(!equaler.EqualsAny(&"not a testcase"));
}
