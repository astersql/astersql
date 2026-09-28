// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/importer`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/importer`).
//!
//! 该入口文件只负责把 importer 子系统拆分成可复用的 Rust 模块，并维持与 Go
//! 包同名的大体导出面，避免上层调用方感知迁移中的文件拆分差异。
//! 中文注释集中说明每组模块在导入流水线中的职责，便于从 crate 根快速定位：
//! 配置与前置检查、分块编码、去重与校验、导入执行、元数据管理，以及 TiDB
//! 交互等能力都从这里统一暴露。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
// 占位实现放在最前面导出，给尚未完整迁移的依赖提供稳定符号。
pub use stubs::*;

#[path = "check_template.rs"]
mod check_template;
// 模板检查负责把外部配置约束转成 importer 可消费的校验规则。
pub use check_template::*;

#[path = "check_info.rs"]
mod check_info;
// 预检信息采集覆盖目标表、源文件与环境状态，为真正导入前的 fail-fast 提供输入。
pub use check_info::*;

#[path = "checksum_helper.rs"]
mod checksum_helper;
// 校验和辅助逻辑保持与 Go 版本相同的数据核对语义，但由独立模块承载细节。
pub use checksum_helper::*;

#[path = "chunk_process.rs"]
mod chunk_process;
// chunk 处理阶段把源数据切成可并发处理的工作单元，是导入主流水线的中段。
pub use chunk_process::*;

#[path = "dup_detect.rs"]
mod dup_detect;
// 重复键检测与导入执行解耦，便于在不同模式下复用本地或远端去重策略。
pub use dup_detect::*;

#[path = "get_pre_info.rs"]
mod get_pre_info;
// 预信息获取负责在正式导入前探测集群与目标表状态，决定后续策略分支。
pub use get_pre_info::*;

#[path = "import.rs"]
mod import;
// import 模块承载顶层导入编排，组合前置检查、编码、写入与收尾动作。
pub use import::*;

#[path = "meta_manager.rs"]
mod meta_manager;
// 元数据管理记录任务级与表级进度，使恢复、重试与排他检查拥有持久化依据。
pub use meta_manager::*;

#[path = "precheck.rs"]
mod precheck;
// precheck 对外暴露预检查入口，统一封装导入前必须满足的环境约束。
pub use precheck::*;

#[path = "precheck_impl.rs"]
mod precheck_impl;
// 具体预检查规则拆到实现模块，避免入口层混入大量规则细节。
pub use precheck_impl::*;

#[path = "table_import.rs"]
mod table_import;
// 表导入模块聚焦单表生命周期，协调编码、写入、校验和收尾状态更新。
pub use table_import::*;

#[path = "tidb.rs"]
mod tidb;
// TiDB 交互模块封装与目标集群协商 schema、DDL 与运行时状态的接口。
pub use tidb::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
// parity 测试集中验证 Rust 导出面与 Go 语义/符号命名的对齐情况。

#[cfg(test)]
#[path = "check_info_test.rs"]
mod check_info_test;

#[cfg(test)]
#[path = "check_template_test.rs"]
mod check_template_test;

#[cfg(test)]
#[path = "chunk_process_test.rs"]
mod chunk_process_test;

#[cfg(test)]
#[path = "dup_detect_test.rs"]
mod dup_detect_test;

#[cfg(test)]
#[path = "get_pre_info_test.rs"]
mod get_pre_info_test;

#[cfg(test)]
#[path = "import_test.rs"]
mod import_test;

#[cfg(test)]
#[path = "meta_manager_test.rs"]
mod meta_manager_test;

#[cfg(test)]
#[path = "precheck_impl_test.rs"]
mod precheck_impl_test;

#[cfg(test)]
#[path = "precheck_test.rs"]
mod precheck_test;

#[cfg(test)]
#[path = "table_import_test.rs"]
mod table_import_test;

#[cfg(test)]
#[path = "tidb_test.rs"]
mod tidb_test;
