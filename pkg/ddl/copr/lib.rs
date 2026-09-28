// Copyright 2026 AsterSQL.

// DDL Coprocessor（协处理器）辅助 crate 的入口模块。
//
// 背景说明：在 TiDB 体系中，DDL（数据定义语言，如 CREATE/ALTER TABLE）在
// 添加索引等场景需要把"读取行数据并回填索引"的任务下推到存储层的
// Coprocessor（协处理器，运行在 TiKV 上、可就近扫描 Region 数据的计算组件）
// 执行。本 crate 提供构造这类下推请求所需的上下文信息（列元数据、
// 表信息布局等）。
//
// 本文件仅负责 crate 级配置与子模块的组织、导出：
// - `copr_ctx`：Coprocessor 上下文（`CopContext`）的核心实现；
// - `copr_ctx_test`：仅在测试编译时包含的单元测试模块。

// crate 级 lint 豁免：代码由 Go(TiDB) 机械迁移而来，
// 保留了 Go 风格的命名（驼峰类型名、下划线不规范等）与暂未使用的代码，
// 因此关闭对应的编译告警。
#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]
/// Coprocessor 上下文实现模块：定义构建下推请求所需的表/索引列信息。
mod copr_ctx;
// 将 copr_ctx 内的公共项（如 CopContext 等）直接从 crate 根重新导出，
// 使外部使用者无需感知内部模块路径。
pub use copr_ctx::*;

/// `copr_ctx` 模块的单元测试，仅在 `cargo test` 编译配置下参与构建。
#[cfg(test)]
mod copr_ctx_test;
