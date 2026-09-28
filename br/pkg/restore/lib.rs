// Copyright 2026 AsterSQL.

//! BR restore 子系统根入口：聚合 stubs、导入模式切换、杂项与 Restorer。
//! 对应 Go `br/pkg/restore` 包面；子模块经 `#[path]` 挂载，测试同样
//! 隔离挂载，避免与实现混编。对外符号扁平再导出，供上层 restore 流程直接引用。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 本地桩：隔离尚未完整迁入的依赖，保持编译可达。
#[path = "stubs.rs"]
pub mod stubs;

// 导入模式切换：在 normal/import 间切换 TiKV，加速 SST 灌入。
#[path = "import_mode_switcher.rs"]
pub mod import_mode_switcher;

// 杂项工具：校验、路径与 restore 辅助函数。
#[path = "misc.rs"]
pub mod misc;

// Restorer 主流程：编排文件加载、写入与收尾。
#[path = "restorer.rs"]
pub mod restorer;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;

#[cfg(test)]
#[path = "import_mode_switcher_test.rs"]
mod import_mode_switcher_test;

#[cfg(test)]
#[path = "misc_test.rs"]
mod misc_test;

#[cfg(test)]
#[path = "restorer_test.rs"]
mod restorer_test;

// 扁平再导出：ImportModeSwitcher / Restorer / misc 工具与桩类型一并暴露。
pub use import_mode_switcher::*;
pub use misc::*;
pub use restorer::*;
pub use stubs::*;
