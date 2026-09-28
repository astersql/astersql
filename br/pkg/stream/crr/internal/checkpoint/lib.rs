// Copyright 2026 AsterSQL.

//! CRR 内部检查点包入口：对齐 Go `br/pkg/stream/crr/internal/checkpoint`。
//! 职责：按固定顺序挂载 calculator/doc/progress/storage，再扁平导出公开 API。
//! 测试模块仅在 `cfg(test)` 下以 `#[path]` 挂载，避免与实现文件混编。
//! 初始化顺序：实现模块先于测试模块；`Store` 从 streamhelper 再导出以便依赖方统一导入。
//! 本文件不含业务逻辑；算法说明见 `doc`，主循环见 `calculator`。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 核心计算器与公开类型定义。
#[path = "calculator.rs"]
pub mod calculator;

// 包级算法文档（无可执行代码），对应 Go doc.go。
#[path = "doc.rs"]
pub mod doc;

// 进度观测与指标辅助。
#[path = "progress.rs"]
pub mod progress;

// 上游存储校验与增量 meta 扫描相关辅助。
#[path = "storage.rs"]
pub mod storage;

// Go/Rust 公开契约对照测试。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

// 对应 Go checkpoint_calculator_test.go 的单元场景。
#[cfg(test)]
#[path = "checkpoint_calculator_test.rs"]
mod checkpoint_calculator_test;

// 端到端风格集成测试（内存夹具）。
#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;

// progress.go 内部推进与取消语义单测。
#[cfg(test)]
#[path = "progress_test.rs"]
mod progress_test;

// storage.go 元数据加载与兼容语义单测。
#[cfg(test)]
#[path = "storage_test.rs"]
mod storage_test;

// 随机化集成场景，覆盖更多时序组合。
#[cfg(test)]
#[path = "randomized_integration_test.rs"]
mod randomized_integration_test;

// storage 内部细节单测。
#[cfg(test)]
#[path = "storage_internal_test.rs"]
mod storage_internal_test;

// 再导出 Store，调用方不必直接依赖 streamhelper 路径。
pub use astersql_br_pkg_streamhelper::Store;
// 扁平导出 calculator 公开符号（Config/Calculator/事件类型等）。
pub use calculator::*;
