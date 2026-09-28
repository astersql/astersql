// Copyright 2026 AsterSQL.

// TiFlash 工具 crate 入口：会话变量依赖与副本读策略再导出。
//
// 通过 `sessionctx::vardef` 桥接变量定义；`include!` 嵌入
// `tiflash_replica_read`，并把副本读 API 公开给调用方。

/// 会话上下文相关依赖的命名空间，当前仅再导出 vardef。
pub mod sessionctx {
    /// 将 `vardef_dependency` 以 `vardef` 名称对外暴露。
    pub use vardef_dependency as vardef;
}

/// 内部模块：嵌入副本读策略实现文件。
mod tiflash_replica_read {
    use crate::sessionctx::vardef;

    include!("tiflash_replica_read.rs");
}

pub use tiflash_replica_read::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
