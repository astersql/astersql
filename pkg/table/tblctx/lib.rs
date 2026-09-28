// Copyright 2026 AsterSQL.

// 表变更上下文（`tblctx`）crate：为 DML 路径提供突变上下文、行编码缓冲与临时表处理边界。
//
// 本包对应 Go `pkg/table/tblctx`，通过再导出依赖（类型、编解码、表达式上下文等）
// 组装可独立编译的表侧上下文 API；核心逻辑见 `table` 与 `buffers` 模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_table_tblctx;

/// 共享错误类型再导出。
pub mod errors {
    pub use errctx_dependency::errors::*;
}
/// 错误/警告上下文（errctx）再导出。
pub mod errctx {
    pub use errctx_dependency::errctx::*;
}
/// MySQL/TiDB 类型与 Datum 再导出。
pub mod types {
    pub use tablecodec_dependency::types::*;
}
/// 编解码器相关再导出。
pub mod codec {
    pub use tablecodec_dependency::codec::*;
}
/// 排序规则（collation）再导出。
pub mod collate {
    pub use tablecodec_dependency::collate::*;
}
/// 行编解码（rowcodec）再导出。
pub mod rowcodec {
    pub use tablecodec_dependency::rowcodec::*;
}
/// 表键值编解码（tablecodec）再导出。
pub mod tablecodec {
    pub use tablecodec_dependency::*;
}
/// Chunk/行视图再导出。
pub mod chunk {
    pub use chunk_dependency::*;
}
/// 会话变量与写语句缓冲再导出。
pub mod variable {
    pub use variable_dependency::session::{TemporaryTableData, WriteStmtBufs};
    pub use variable_dependency::*;
}
/// 表工具（含临时表）再导出。
pub mod tableutil {
    pub use tableutil_dependency::*;
}
/// 元数据模型再导出。
pub mod model {
    pub use model_dependency::*;
}
/// 自增/行 ID 分配器再导出。
pub mod autoid {
    pub use autoid_dependency::*;
}
/// 语句上下文再导出。
pub mod stmtctx {
    pub use stmtctx_dependency::*;
}
/// 表达式上下文再导出。
pub mod exprctx {
    pub use exprctx_dependency::*;
}
/// 信息模式（infoschema）再导出。
pub mod infoschema {
    pub use infoschema_dependency::*;
}
/// 内部断言辅助再导出。
pub mod intest {
    pub use intest_dependency::*;
}
/// 时区/位置类型别名，供行编码路径使用。
pub mod time {
    pub type Location = chrono_tz::Tz;
    pub const UTC: Location = chrono_tz::UTC;
}
/// 精简 KV 边界：Handle/Key 与内存缓冲写入接口。
pub mod kv {
    pub use tablecodec_dependency::kv::{Handle, IntHandle, Key};

    /// 写入 MemBuffer 时的标志操作。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum FlagsOp {
        /// 假定键尚不存在（PresumeKeyNotExists）。
        SetPresumeKeyNotExists,
    }

    /// 事务内存缓冲写入接口（对应 Go MemBuffer）。
    pub trait MemBuffer {
        fn Set(&mut self, key: Key, value: Vec<u8>) -> Result<(), crate::errors::SharedError>;
        fn SetWithFlags(
            &mut self,
            key: Key,
            value: Vec<u8>,
            flags: &[FlagsOp],
        ) -> Result<(), crate::errors::SharedError>;
    }
}

mod table;
pub use table::*;
mod buffers;
pub use buffers::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "buffers_test.rs"]
mod buffers_test;

#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
