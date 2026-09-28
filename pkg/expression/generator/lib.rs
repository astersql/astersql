// Copyright 2026 AsterSQL.

// 表达式向量化内置函数的 Go 源码生成器集合。
//
// 对应 Go 目录 `pkg/expression/generator`：用模板按类型签名展开
// `vecEval*` 实现与基准/单元测试桩。各子模块彼此独立，生成结果通常写入
// `builtin_*_vec_generated.go`；本 crate 本身不参与运行时求值。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 线程安全相关内置函数的向量化生成逻辑。
pub mod builtin_threadsafe;
/// 比较类（如 GE/LE/EQ）向量化函数生成器。
pub mod compare_vec;
/// 控制流类（如 IF/CASE/IFNULL）向量化函数生成器。
pub mod control_vec;

/// control_vec 与 Go 生成结果的对抗一致性回归测试。
#[cfg(test)]
#[path = "control_vec_test.rs"]
mod control_vec_test;
/// 其它类（当前主要为 IN）向量化函数生成器。
pub mod other_vec;
/// other_vec 与 Go 模板的对抗一致性回归测试。
#[cfg(test)]
#[path = "other_vec_test.rs"]
mod other_vec_test;
/// 字符串类（当前主要为 FIELD）向量化函数生成器。
pub mod string_vec;
/// string_vec 与 Go 生成器写盘语义的对抗一致性回归测试。
#[cfg(test)]
#[path = "string_vec_test.rs"]
mod string_vec_test;
/// 时间类（ADDTIME/SUBTIME/TIMEDIFF/ADDDATE/SUBDATE）向量化函数生成器。
pub mod time_vec;

/// builtin_threadsafe 生成器的迁移单元测试。
#[cfg(test)]
#[path = "builtin_threadsafe_1_aster_unit_test.rs"]
mod builtin_threadsafe_1_aster_unit_test;

/// builtin_threadsafe 与 Go AST/format.Source 的对抗一致性回归测试。
#[cfg(test)]
#[path = "builtin_threadsafe_test.rs"]
mod builtin_threadsafe_test;

/// compare_vec 与 Go 模板的对抗一致性回归测试。
#[cfg(test)]
#[path = "compare_vec_test.rs"]
mod compare_vec_test;

/// time_vec 签名矩阵、模板渲染与双文件写出的迁移单元测试。
#[cfg(test)]
#[path = "time_vec_2_aster_unit_test.rs"]
mod time_vec_2_aster_unit_test;
