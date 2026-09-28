// Copyright 2026 AsterSQL.

//! importer 的 crate 根只负责把 Go `cmd/importer` 对齐实现重新装配成一个可执行入口。
//! 真正的参数解析、建表 SQL 处理、数据生成、批量写入和统计加载流程都下沉在各个子模块中，
//! 这里不承载业务判断，重点是保持模块暴露关系与二进制入口稳定。
//! 二进制包装层和对齐测试都通过同一 crate 根复用 `entry::main()` 和共享桩实现。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

/// 配置解析与默认值集中在这里，保持与 Go `config.go` 的命令行契约一致。
#[path = "config.rs"]
pub mod config;

/// 测试数据生成逻辑单独拆分，避免入口层混入具体字段取值策略。
#[path = "data.rs"]
pub mod data;

/// 与 Go 版一致保留独立随机辅助模块，便于复用时间和数值生成逻辑。
#[path = "rand.rs"]
pub mod rand;

/// 统计信息加载独立成模块，入口只决定是否接入现有统计文件。
#[path = "stats.rs"]
pub mod stats;

/// DDL 解析与表结构装配在这里完成，供入口在真正导入前建立元数据视图。
#[path = "parser.rs"]
pub mod parser;

/// 数据库连接、DDL 执行和 INSERT 语句生成都通过该模块对外提供。
#[path = "db.rs"]
pub mod db;

/// 并发导入任务调度在这里实现，crate 根只负责暴露它给主流程使用。
#[path = "job.rs"]
pub mod job;

/// 真实的可执行主流程放在 `main.rs`，这里仅把 crate 根入口转发过去。
#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "db_test.rs"]
mod db_test;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "data_test.rs"]
mod data_test;

#[cfg(test)]
#[path = "parser_test.rs"]
mod parser_test;

#[cfg(test)]
#[path = "rand_test.rs"]
mod rand_test;

#[cfg(test)]
#[path = "stats_test.rs"]
mod stats_test;

/// Shared process entrypoint called by the binary wrapper.
/// 库根 `main` 只保留单跳转发，避免把装配层与真实导入流程耦合在一起。
/// 这样无论二进制运行还是 parity/test 侧复用入口，都会落到同一份 Go 对齐实现。
pub fn main() {
    entry::main();
}
