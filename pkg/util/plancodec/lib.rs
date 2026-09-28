// Copyright 2026 AsterSQL.

// `util/plancodec` crate 入口：执行计划编码/解码与二进制计划展示。
//
// 对应 Go `pkg/util/plancodec`。提供类型 ID 映射、文本/压缩编码、
// 以及基于 tipb Explain 协议的二进制计划解码（类似 EXPLAIN ANALYZE 输出）。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    static_mut_refs
)]

extern crate self as plancodec_dependency;

/// 存储引擎类型（TiKV/TiFlash/TiDB）的本地桩，供任务类型编码使用。
pub mod kv {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[repr(u8)]
    /// 计划任务所在存储类型；`UnSpecified` 表示未指定。
    pub enum StoreType {
        TiKV = 0,
        TiFlash = 1,
        TiDB = 2,
        UnSpecified = 255,
    }
    impl StoreType {
        /// 返回存储类型的小写名称字符串。
        pub fn Name(self) -> &'static str {
            match self {
                Self::TiKV => "tikv",
                Self::TiFlash => "tiflash",
                Self::TiDB => "tidb",
                Self::UnSpecified => "unspecified",
            }
        }
    }
}
/// 内存字节数格式化辅助（用于计划展示中的内存列）。
pub mod memory {
    /// 将字节数格式化为带单位的可读字符串（Bytes/KB/MB/GB）。
    pub fn FormatBytes(num_bytes: i64) -> String {
        const KB: i64 = 1 << 10;
        const MB: i64 = 1 << 20;
        const GB: i64 = 1 << 30;
        if num_bytes <= KB {
            return format!("{} Bytes", num_bytes);
        }
        let (unit, suffix) = if num_bytes > GB {
            (GB, "GB")
        } else if num_bytes > MB {
            (MB, "MB")
        } else {
            (KB, "KB")
        };
        let value = num_bytes as f64 / unit as f64;
        let decimals = if num_bytes % unit == 0 {
            0
        } else if value < 10.0 {
            2
        } else {
            1
        };
        format!("{:.*} {}", decimals, value, suffix)
    }
}
/// 文本树绘制依赖的再导出。
pub mod texttree {
    pub use texttree_dependency::*;
}
mod id;
#[path = "../../types/explain_format.rs"]
/// EXPLAIN 输出格式相关类型（从 `pkg/types` 挂入）。
pub mod types;
/// 再导出计划类型字符串与 ID 映射 API。
pub use id::*;
mod codec {
    use crate::tipb;
    include!("codec.rs");
}
/// 再导出计划文本编解码与压缩 API。
pub use codec::*;
mod binary_plan_decode {
    use crate::tipb;
    include!("binary_plan_decode.rs");
}
/// 再导出二进制计划解码 API。
pub use binary_plan_decode::*;
pub use protobuf;
#[allow(warnings)]
/// tipb Explain 相关 protobuf 生成代码。
pub mod tipb {
    include!(concat!(env!("OUT_DIR"), "/explain.rs"));
}

#[cfg(test)]
#[path = "binary_plan_decode_test.rs"]
mod binary_plan_decode_test;
#[cfg(test)]
#[path = "codec_test.rs"]
mod codec_test;
#[cfg(test)]
#[path = "id_test.rs"]
mod id_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充的 plancodec 单元测试。
mod migration_aster_unit_test;
