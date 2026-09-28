// Copyright 2026 AsterSQL.

// 行编解码（rowcodec）crate 入口：新格式行字节的编码、解码与公共依赖再导出。
//
// 对应 Go `pkg/util/rowcodec`。通过 `include!` 组合 `common`/`row`/`encoder`/`decoder`，
// 并再导出 chunk、codec、kv、types 等依赖，供 TiKV 行值与 Datum/Chunk 互转。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

/// 列式结果集 Chunk 依赖再导出。
pub mod chunk {
    pub use chunk_dependency::*;
}
/// 底层 codec（Datum 编解码）依赖再导出。
pub mod codec {
    pub use codec_dependency::*;
}
/// 共享错误类型再导出。
pub mod errors {
    pub use codec_dependency::errors::*;
}
/// 内部断言 / 测试探针依赖再导出。
pub mod intest {
    pub use intest_dependency::*;
}
/// 内核类型（classic / next-gen）判断再导出。
pub mod kerneltype {
    pub use kerneltype_dependency::*;
}
/// KV 键与 Handle 抽象再导出。
pub mod kv {
    pub use kv_dependency::*;
}
/// 表模型常量：特殊列 ID（句柄、行校验和、提交时间戳）。
pub mod model {
    pub use model_dependency::ColumnInfo;
    /// 额外句柄列 ID（负值，不落真实列定义）。
    pub const ExtraHandleID: i64 = -1;
    /// 额外行校验和列 ID。
    pub const ExtraRowChecksumID: i64 = -4;
    /// 额外提交时间戳列 ID（MVCC 提交版本）。
    pub const ExtraCommitTSID: i64 = -5;
}
/// MySQL 类型常量再导出。
pub mod mysql {
    pub use codec_dependency::mysql::*;
}
/// 时区类型：用 `chrono_tz::Tz` 对齐 Go `*time.Location`。
pub mod time {
    pub type Location = chrono_tz::Tz;
    pub const UTC: Location = chrono_tz::UTC;
}
/// 字段类型与 Datum 相关再导出。
pub mod types {
    pub use types_dependency::*;
    pub use types_metadata_dependency::{NeedRestoredData, NeedRestoredDataWithCollate};
    pub use types_scalar_dependency::StrictContext;
    pub const ZeroCoreTime: types_dependency::CoreTime = types_dependency::CoreTime(0);
}

#[cfg(test)]
/// 排序规则依赖（仅测试）。
pub mod collate {
    pub use collate_dependency::*;
}

#[cfg(test)]
/// 表键编解码依赖（仅测试）。
pub mod tablecodec {
    pub use tablecodec_dependency::*;
}

#[cfg(test)]
/// 本 crate 错误类型别名（仅测试）。
pub mod rowcodec_errors {
    pub use crate::errors::*;
}

/// 实现体：include 同包源文件，并提供测试用旧行转码。
mod rowcodec_impl {
    use crate::{chunk, codec, errors, intest, kerneltype, kv, model, mysql, time, types};

    include!("common.rs");
    include!("row.rs");
    include!("encoder.rs");
    include!("decoder.rs");

    #[cfg(test)]
    /// 旧格式（colID+Datum 成对）转新 rowcodec；已是 `CodecVer` 则原样返回。
    pub fn encode_from_old_row(
        encoder: &mut Encoder,
        loc: Option<&time::Location>,
        mut old_row: &[u8],
        mut buf: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        // 已是新格式：直接复制，避免二次编码。
        if old_row.first() == Some(&CodecVer) {
            return Ok(old_row.to_vec());
        }

        encoder.reset();
        // 旧格式按 (column_id, datum) 成对 DecodeOne 推进。
        while old_row.len() > 1 {
            let (remaining, column_id) =
                codec::DecodeOne(old_row).map_err(|err| err.to_string())?;
            old_row = remaining;
            let (remaining, datum) = codec::DecodeOne(old_row).map_err(|err| err.to_string())?;
            old_row = remaining;
            encoder.appendColVal(column_id.GetInt64(), datum);
        }
        let (num_cols, not_null_idx) = encoder.reformatCols();
        encoder
            .encodeRowCols(loc, num_cols, not_null_idx)
            .map_err(|err| err.to_string())?;
        buf.clear();
        Ok(encoder.row.toBytes(buf))
    }
}

pub use rowcodec_impl::*;

/// 对外命名空间，与 Go 包名 `rowcodec` 对齐。
pub mod rowcodec {
    pub use crate::rowcodec_impl::*;
}

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;

#[cfg(test)]
#[path = "common_1_aster_unit_test.rs"]
mod common_1_aster_unit_test;

#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;

#[cfg(test)]
#[path = "encoder_test.rs"]
mod encoder_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "rowcodec_test.rs"]
mod rowcodec_test;
