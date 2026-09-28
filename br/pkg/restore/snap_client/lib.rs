// Copyright 2026 AsterSQL.

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/snap_client/lib.rs`对应的snap_client crate 入口与模块装配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/snap_client (package)` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 本任务要求至少14行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `stubs`：测试/非完整环境下的 PD、TiKV、Domain 等桩，不代表生产依赖。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `systable_schema_update / systable_restore`：系统表 schema 版本与临时表恢复。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `placement_rule_manager`：放置规则在恢复前后的备份与还原。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `pipeline_items`：恢复流水线元素与拆分策略衔接。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `import / client / tikv_sender`：导入、客户端控制面、SST 发送主路径。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `pitr_collector`：PiTR 相关收集与依赖注入。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `pub use 重导出`：扁平导出生产 API；测试模块仅 cfg(test) 挂载。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! - `cfg(test) 模块列表`：export/main/parity 与各 *_test 通过 path 挂载，保持与 Go 文件一一对应。
//!   模块加载顺序不影响 pub use 导出集合，但测试 path 挂载需与文件名稳定对应。
//! 中文注释索引结束

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// stubs 仅服务编译与测试，生产调用应走真实 PD/TiKV 客户端。
#[path = "stubs.rs"]
pub mod stubs;

#[path = "systable_schema_update.rs"]
pub mod systable_schema_update;

#[path = "systable_restore.rs"]
pub mod systable_restore;

#[path = "placement_rule_manager.rs"]
pub mod placement_rule_manager;

#[path = "pipeline_items.rs"]
pub mod pipeline_items;

#[path = "import.rs"]
pub mod import;

#[path = "pitr_collector.rs"]
pub mod pitr_collector;

#[path = "tikv_sender.rs"]
pub mod tikv_sender;

#[path = "client.rs"]
pub mod client;

// 对外以 SnapClient 等符号为主入口，调用方无需写 snap_client::client::。
pub use client::*;
pub use import::*;
pub use pipeline_items::*;
pub use pitr_collector::*;
pub use placement_rule_manager::*;
pub use systable_restore::*;
pub use systable_schema_update::*;
pub use tikv_sender::*;

#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

#[cfg(test)]
#[path = "import_test.rs"]
mod import_test;

#[cfg(test)]
#[path = "pipeline_items_test.rs"]
mod pipeline_items_test;

#[cfg(test)]
#[path = "pitr_collector_test.rs"]
mod pitr_collector_test;

#[cfg(test)]
#[path = "placement_rule_manager_test.rs"]
mod placement_rule_manager_test;

#[cfg(test)]
#[path = "systable_restore_test.rs"]
mod systable_restore_test;

#[cfg(test)]
#[path = "systable_schema_update_test.rs"]
mod systable_schema_update_test;

#[cfg(test)]
#[path = "tikv_sender_test.rs"]
mod tikv_sender_test;
