// Copyright 2026 AsterSQL.

// deploymode crate 的入口模块（lib.rs）。
//
// 本 crate 负责"部署模式"（deploy mode）相关的配置定义：
// 数据库可以以不同的部署形态运行（例如经典模式、下一代/云原生模式等），
// 部署模式决定了内核在存储、调度等方面启用的特性集合。
// 这里主要做子模块的组织与符号重导出（re-export），方便其他 crate
// 以统一路径引用部署模式相关的类型与函数。

// 允许非蛇形命名与非全大写全局常量：
// 代码由 Go(TiDB) 机械迁移而来，保留了 Go 风格的命名（如驼峰式函数名），
// 因此需要关闭 Rust 默认的命名规范 lint。
#![allow(non_snake_case, non_upper_case_globals)]

/// 内核类型（kernel type）子模块。
///
/// 将独立 crate `astersql_config_kerneltype` 的全部公开符号重导出到
/// 本 crate 的 `kerneltype` 路径下，保持与原 Go 包结构一致的引用方式。
/// 内核类型用于区分经典内核与下一代内核等不同内核实现。
pub mod kerneltype {
    pub use astersql_config_kerneltype::*;
}

/// 文档性说明模块（对应 Go 包中的 doc.go）。
pub mod doc;
/// 部署模式的核心实现模块，定义具体的模式类型与判断函数。
pub mod mode;
// 将 mode 模块的全部公开符号提升到 crate 根，调用方可直接使用。
pub use mode::*;

/// 兼容路径模块：以 `deploymode::deploymode::*` 的形式重导出 mode 模块内容，
/// 便于按原 Go 包名 `deploymode` 的层级进行引用。
pub mod deploymode {
    pub use crate::mode::*;
}

// 以下为测试模块：仅在 `cargo test`（cfg(test)）时编译，
// 通过 #[path] 属性指定测试源文件的相对路径。

// 迁移辅助单元测试（验证 Go->Rust 迁移行为一致性）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

// mode 模块的单元测试。
#[cfg(test)]
#[path = "mode_test.rs"]
mod mode_test;
