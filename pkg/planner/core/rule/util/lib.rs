// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 逻辑优化规则（Logical Opt Rule）公用工具包入口。
//
// 再导出 `misc` 中的列/表达式替换、最大一行条件检查、谓词简化钩子注册，
// 以及后序构建键信息（Key Info，唯一键/主键等）的门户函数；测试文件单独挂接。

#![allow(non_snake_case)]

/// 杂项工具实现（列替换、谓词钩子、BuildKeyInfo 等）。
mod misc;
pub use misc::*;

#[cfg(test)]
#[path = "misc_aster_unit_test.rs"]
/// misc 模块的 Aster 单元测试。
mod misc_aster_unit_test;

#[cfg(test)]
#[path = "misc_test.rs"]
mod misc_test;
