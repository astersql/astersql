// Copyright 2026 AsterSQL.

// `tablecodec` crate：表键值编解码（对应 Go `pkg/tablecodec`）。
//
// 负责将逻辑表行/索引映射为 TiKV 键值：行键前缀 `t{table_id}`、记录/索引后缀，
// 以及 Datum（类型化单元格值）序列化。本文件聚合依赖并 `include!` 主实现。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

/// 错误类型再导出（dbterror）。
pub mod errors {
    pub use dbterror_dependency::errors::*;
}
/// 底层 codec 与共享 Error 别名。
pub mod codec {
    pub use codec_dependency::*;
    pub type Error = codec_dependency::errors::SharedError;
}
/// 排序规则（collation）依赖。
pub mod collate {
    pub use collate_dependency::*;
}
/// canonical KV 类型与句柄契约。
pub mod kv {
    pub use structure_dependency::kv::*;
}
/// 内存感知 Map 等 hack ABI。
pub mod hack {
    pub use hack_dependency::map_abi::{MemAwareMap, NewMemAwareMap};
}
/// 尺寸估算依赖。
pub mod size {
    pub use size_dependency::*;
}
/// Datum/标量类型与严格上下文。
pub mod types {
    pub use datum_dependency::*;
    pub use types_etc_dependency::{NeedRestoredData, NeedRestoredDataWithCollate};
    pub use types_scalar::StrictContext;
    /// 零值 CoreTime。
    pub const ZeroCoreTime: datum_dependency::CoreTime = datum_dependency::CoreTime(0);
}
/// 行编解码（旧/新行格式）实现，include util/rowcodec。
pub mod rowcodec {
    use crate::{chunk, codec, intest, kerneltype, kv, model, mysql, time, types};
    mod errors {
        pub use codec_dependency::errors::*;
    }
    include!("../util/rowcodec/common.rs");
    include!("../util/rowcodec/row.rs");
    include!("../util/rowcodec/encoder.rs");
    include!("../util/rowcodec/decoder.rs");
}
/// Chunk（列式批处理缓冲）依赖。
pub mod chunk {
    pub use chunk_dependency::*;
}
/// 内部测试辅助依赖。
pub mod intest {
    pub use intest_dependency::*;
}
/// 内核类型依赖。
pub mod kerneltype {
    pub use kerneltype_dependency::*;
}
/// canonical 表/列/索引元数据模型。
pub mod model {
    pub use model_group_1::*;
}
/// 字符集依赖。
pub mod charset {
    pub use charset_dependency::*;
}
/// MySQL 协议/类型相关再导出。
pub mod mysql {
    pub use codec_dependency::mysql::*;
}
/// 数据库错误类型。
pub mod dbterror {
    pub use dbterror_dependency::dbterror::*;
}
/// MySQL 风格错误号。
pub mod errno {
    pub use dbterror_dependency::errno::*;
}
/// terror 错误框架。
pub mod terror {
    pub use dbterror_dependency::terror::*;
}
/// structure 编解码依赖。
pub mod structure {
    pub use structure_dependency::*;
}
/// 字符串工具。
pub mod stringutil {
    pub use stringutil_dependency::string_util::*;
}
/// 时区 Location 与 Duration 占位。
pub mod time {
    /// 时区类型别名。
    pub type Location = chrono_tz::Tz;
    /// UTC 时区常量。
    pub const UTC: Location = chrono_tz::UTC;
    /// 时长占位：原样返回（对齐 Go 迁移桩）。
    pub fn Duration(value: i64) -> i64 {
        value
    }
}

// 主编解码实现（tablecodec.rs）。
include!("tablecodec.rs");

/// 行索引编解码子模块。
#[path = "rowindexcodec/lib.rs"]
pub mod rowindexcodec;

// 基准与单元测试模块。
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "tablecodec_test.rs"]
mod tablecodec_test;

#[cfg(test)]
#[path = "tablecodec_1_aster_unit_test.rs"]
mod tablecodec_1_aster_unit_test;
