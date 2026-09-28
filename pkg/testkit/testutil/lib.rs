// Copyright 2026 AsterSQL.

// `testutil` crate 入口：为测试提供 codec / kv / MySQL 类型依赖重导出，
// 以及 handle、日志钩子与断言等辅助模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 编解码与 Datum 相关依赖的重导出。
pub mod codec {
    pub use codec_dependency::*;
}

/// 排序规则（collation）相关依赖的重导出。
pub mod collate {
    pub use codec_dependency::collate::*;
}

/// KV / Handle 相关依赖的重导出。
pub mod kv {
    pub use kv_dependency::*;
}

/// MySQL 常量与类型码相关依赖的重导出。
pub mod mysql {
    pub use mysql_dependency::r#const::*;
    pub use mysql_dependency::r#type::*;
}

/// Datum / 类型系统相关依赖的重导出。
pub mod types {
    pub use codec_dependency::types::*;
}

/// Handle 构造与分片掩码排序辅助（见 `handle.rs`）。
mod handle {
    use crate::{codec, kv, mysql, types};
    include!("handle.rs");
}
pub use handle::*;

/// tracing 日志捕获钩子（见 `loghook.rs`）。
mod loghook {
    include!("loghook.rs");
}
pub use loghook::*;

/// Datum / Handle / 无序切片等测试断言（见 `require.rs`）。
mod require {
    use crate::{collate, kv, types};
    include!("require.rs");
}
pub use require::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "require_test.rs"]
mod require_test;
