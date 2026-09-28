// Copyright 2026 AsterSQL.

// # bindinfo：SQL 绑定（SQL Binding / Plan Binding）子系统
//
// 本 crate 实现"SQL 绑定"能力：把某条 SQL 语句（按其规范化后的"摘要"匹配）
// 与一份带有优化器提示（Hint）的 SQL 文本绑定在一起。当同样的语句再次执行时，
// 优化器会优先采用绑定中携带的提示来生成执行计划（执行计划：数据库把 SQL
// 翻译成的具体执行步骤，如选择哪个索引、用哪种连接算法等），从而在不修改
// 业务 SQL 的前提下稳定或修正执行计划，避免因统计信息波动等原因导致的
// 计划回退（plan regression）。
//
// 主要组成：
// - 绑定的内存表示与规范化逻辑（`binding` 模块）；
// - 自动捕获/自动绑定（`binding_auto` 模块）；
// - 绑定缓存，避免每次都访问系统表（`binding_cache` 模块）；
// - 全局绑定的加载与生命周期管理（`binding_handle` 模块）；
// - 创建/删除/启用等绑定操作（`binding_operator` 模块）；
// - 计划演进与计划生成，即为绑定探索、验证更优计划
//   （`binding_plan_evolution` / `binding_plan_generation` 模块）；
// - 会话级绑定管理（`session_handle` 模块）。
//
// 此外定义了整个子系统统一的错误类型 [`BindError`] 与结果别名 [`Result`]。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 绑定的核心数据结构与 SQL 规范化（normalization，把字面量参数化、
/// 统一大小写与空白，便于按"摘要"匹配语句）逻辑。
mod binding;
/// 自动绑定：根据运行时反馈自动捕获慢查询等语句并为其生成绑定。
mod binding_auto;
/// 绑定缓存：在内存中缓存已加载的绑定，减少对系统表的重复读取。
mod binding_cache;
/// 全局绑定句柄：负责从存储加载全局绑定并管理其增删改与过期回收。
mod binding_handle;
/// 绑定操作符：实现创建、删除、启用/禁用绑定等具体操作。
mod binding_operator;
/// 计划演进：在后台尝试为已绑定语句探索并验证更优的执行计划。
mod binding_plan_evolution;
/// 计划生成：为语句生成候选执行计划及对应的提示文本。
mod binding_plan_generation;
/// 会话级绑定句柄：管理仅对当前会话（连接）可见的绑定。
mod session_handle;
/// 子系统内部共享的工具函数。
mod utils;

/// 重新导出绑定核心类型，供外部 crate 直接使用。
pub use binding::*;
/// 重新导出自动绑定相关接口。
pub use binding_auto::*;
/// 重新导出绑定缓存相关接口。
pub use binding_cache::*;
/// 重新导出全局绑定句柄相关接口。
pub use binding_handle::*;
/// 重新导出绑定操作相关接口。
pub use binding_operator::*;
/// 重新导出计划演进相关接口。
pub use binding_plan_evolution::*;
/// 重新导出计划生成相关接口。
pub use binding_plan_generation::*;
/// 重新导出会话级绑定句柄相关接口。
pub use session_handle::*;
/// 重新导出内部工具函数。
pub use utils::*;

use std::fmt;

/// Errors returned by the binding subsystem.  Storage and optimizer adapters
/// keep their original error message so callers can expose actionable detail.
///
/// 绑定子系统统一的错误类型：内部仅包装一条错误消息字符串。来自存储层与
/// 优化器适配层的错误会保留其原始消息，便于调用方向用户展示可操作的细节。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindError(pub String);

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BindError {}

impl From<&str> for BindError {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for BindError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<serde_json::Error> for BindError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}

/// 绑定子系统统一的结果别名：错误类型固定为 [`BindError`]。
pub type Result<T> = std::result::Result<T, BindError>;

// 以下均为仅在测试构建（`cfg(test)`）时编译的单元测试模块。
#[cfg(test)]
mod binding_auto_test;
#[cfg(test)]
mod binding_cache_test;
#[cfg(test)]
mod binding_handle_test;
#[cfg(test)]
mod binding_operator_test;
#[cfg(test)]
mod binding_plan_evolution_test;
#[cfg(test)]
mod binding_plan_generation_test;
#[cfg(test)]
mod binding_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod session_handle_test;
#[cfg(test)]
mod utils_test;
