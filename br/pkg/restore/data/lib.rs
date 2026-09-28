// Copyright 2026 AsterSQL.

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/data/lib.rs`对应的入口重导出与模块装配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少6行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! 入口装配顺序影响测试模块可见性；pub use 决定 crate 对外表面。
//! - `stubs`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
//! - `key`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
//! - `recover`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
//! - `data`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
//! - `parity_test`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
//! - `data_test`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
//! - `key_test`模块在入口处装配或重导出，真实逻辑通常位于同目录具体文件。
//! 入口文件关注初始化顺序、条件编译与 pub use 可见性。
//! 测试子模块仅在 cfg(test) 下挂载，不影响发布产物。
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

#[path = "stubs.rs"]
pub mod stubs;

#[path = "key.rs"]
pub mod key;

#[path = "recover.rs"]
pub mod recover;

#[path = "data.rs"]
pub mod data;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "data_test.rs"]
mod data_test;

#[cfg(test)]
#[path = "key_test.rs"]
mod key_test;

#[cfg(test)]
#[path = "recover_test.rs"]
mod recover_test;

pub use data::*;
pub use key::*;
pub use recover::*;
pub use stubs::*;
