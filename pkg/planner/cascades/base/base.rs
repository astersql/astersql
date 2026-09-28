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

// Cascades 优化器基础哈希与相等性接口。
//
// Cascades 是基于 memo（等价类记忆化）的规则优化框架；本模块提供 Hash64 /
// Equals / HashEquals，用于表达式与算子在 memo 中的去重与冲突二次确认。

// 本文件由 pkg/planner/cascades/base/base.go 迁移而来，保留接口声明顺序。

use std::any::Any;

/// Hash64 对应 Go 的同名接口。
/// 实现者应把对象自底向上的有损摘要写入共享 Hasher，而不是返回紧凑字节数组。
pub trait Hash64 {
    /// 对应 Go `Hash64(h Hasher)`；动态引用保留接口参数语义，并允许原地更新哈希器。
    #[allow(non_snake_case)]
    fn Hash64(&self, h: &mut dyn Hasher);
}

/// Equals 对应 Go 的冲突后二次相等性检查接口。
pub trait Equals {
    // / Go 参数类型为 `any`；这里以 `dyn Any` 保留运行时类型判断入口。
    /// Go 参数类型为 `any`；这里以 `dyn Any` 保留运行时类型判断入口。
    #[allow(non_snake_case)]
    fn Equals(&self, other: &dyn Any) -> bool;
}

/// HashEquals 对应同时嵌入 Hash64 与 Equals 的 Go 组合接口。
pub trait HashEquals: Hash64 + Equals {}

// Go 中满足两个嵌入接口的方法集即可隐式实现 HashEquals；此 blanket impl 保留该语义。
impl<T> HashEquals for T where T: Hash64 + Equals + ?Sized {}
