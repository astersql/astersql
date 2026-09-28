//! 中文注释索引开始
//! 本文件负责`br/pkg/encryption/master_key/lib.rs`对应的入口重导出与模块装配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少9行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - 补充约束 1: `br/pkg/encryption/master_key/lib.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `br/pkg/encryption/master_key/lib.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]
#[path = "common.rs"]
pub mod common;
#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;
#[path = "file_backend.rs"]
pub mod file_backend;
#[cfg(test)]
#[path = "file_backend_test.rs"]
mod file_backend_test;
#[path = "kms_backend.rs"]
pub mod kms_backend;
#[cfg(test)]
#[path = "kms_backend_test.rs"]
mod kms_backend_test;
#[path = "master_key.rs"]
pub mod master_key;
#[path = "mem_backend.rs"]
pub mod mem_backend;
#[cfg(test)]
#[path = "mem_backend_test.rs"]
mod mem_backend_test;
#[path = "multi_master_key_backend.rs"]
pub mod multi_master_key_backend;
#[cfg(test)]
#[path = "multi_master_key_backend_test.rs"]
mod multi_master_key_backend_test;
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
#[path = "pb.rs"]
pub mod pb;
#[cfg(test)]
#[path = "pb_test.rs"]
mod pb_test;
pub use common::*;
pub use file_backend::*;
pub use kms_backend::*;
pub use master_key::*;
pub use mem_backend::*;
pub use multi_master_key_backend::*;
pub use pb::*;
