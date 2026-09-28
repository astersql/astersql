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

// 逻辑/物理算子共用的基础 Plan 实现 crate。
//
// 导出 `Plan` 结构体及其 ID、统计信息、Explain 标识等元数据方法；
// 逻辑计划（logical plan）与物理计划（physical plan）均内嵌该基类。

#![allow(non_snake_case, non_upper_case_globals)]

mod plan;
pub use plan::*;

#[cfg(test)]
#[path = "plan_aster_unit_test.rs"]
mod plan_aster_unit_test;

#[cfg(test)]
#[path = "plan_test.rs"]
mod plan_test;
