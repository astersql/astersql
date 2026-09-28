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

// mathutil crate 根：整数/浮点工具、指数移动平均与 MySQL 兼容随机数。
//
// 按 feature/`intest` 条件挂载 assert 实现；测试模块通过 `#[path]` 引入。

#![allow(non_snake_case, non_upper_case_globals)]

// 断言实现：正式 crate 与 workspace 路径下按 feature 选择有/无断言变体。
#[cfg(any(test, feature = "intest", feature = "enableassert"))]
#[path = "../intest/assert.rs"]
mod assert;
#[path = "../intest/assert_common.rs"]
mod assert_common;
#[cfg(not(any(test, feature = "intest", feature = "enableassert")))]
#[path = "../intest/no_assert.rs"]
mod no_assert;

mod exponential_average;
mod math;
mod rand;

/// 导出 EMA 类型与构造函数。
pub use exponential_average::{ExponentialMovingAverage, NewExponentialMovingAverage};
/// 导出整数边界常量与常用数值工具函数。
pub use math::{
    Abs, Clamp, Divide2Batches, IntBits, IsFinite, MaxInt, MaxUint, MinInt, NextPowerOfTwo,
    StrLenOfInt64Fast, StrLenOfUint64Fast,
};
/// 导出 MySQL 兼容 RNG。
pub use rand::{MysqlRng, NewWithSeed, NewWithTime};

#[cfg(test)]
#[path = "exponential_average_test.rs"]
mod exponential_average_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "math_test.rs"]
mod math_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "rand_test.rs"]
mod rand_test;
