// Copyright 2026 AsterSQL.

// codec 包入口：聚合整数/浮点/字节/DECIMAL 编解码，并声明依赖桩模块与测试模块。
//
// 对应 Go `pkg/util/codec`。本文件用 `include!` 嵌入各实现源文件，同时提供
// `errors`/`types`/`mysql` 等迁移期兼容命名空间；测试通过 `#[path]` 挂到 crate。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

/// 错误类型再导出与 Go 风格 `Errorf` 构造。
pub mod errors {
    pub use astersql_errors::*;
    /// 用消息字符串构造 SharedError（对齐 Go `errors.Errorf` 的简易用法）。
    pub fn Errorf(message: impl Into<String>) -> SharedError {
        New(message.into())
    }
}

/// encoding/binary 风格变长整数：PutUvarint/PutVarint 与解码。
pub mod binary {
    /// uvarint 最大字节数（64 位值最多 10 字节）。
    pub const MaxVarintLen64: usize = 10;
    /// 将无符号变长整数写入 buffer，返回写入字节数。
    pub fn PutUvarint(buffer: &mut [u8], mut value: u64) -> usize {
        let mut index = 0;
        while value >= 0x80 {
            buffer[index] = value as u8 | 0x80;
            value >>= 7;
            index += 1;
        }
        buffer[index] = value as u8;
        index + 1
    }
    /// ZigZag 编码有符号整数后再写 uvarint。
    pub fn PutVarint(buffer: &mut [u8], value: i64) -> usize {
        PutUvarint(buffer, ((value as u64) << 1) ^ ((value >> 63) as u64))
    }
    /// 解码 uvarint；溢出返回负计数值，不足返回 (0,0)。
    pub fn Uvarint(buffer: Vec<u8>) -> (u64, isize) {
        let mut value = 0u64;
        for (index, byte) in buffer.into_iter().enumerate() {
            if index == MaxVarintLen64 {
                return (0, -((index + 1) as isize));
            }
            if byte < 0x80 {
                if index == MaxVarintLen64 - 1 && byte > 1 {
                    return (0, -((index + 1) as isize));
                }
                return (value | ((byte as u64) << (index * 7)), (index + 1) as isize);
            }
            value |= ((byte & 0x7f) as u64) << (index * 7);
        }
        (0, 0)
    }
    /// 解码 ZigZag varint。
    pub fn Varint(buffer: Vec<u8>) -> (i64, isize) {
        let (unsigned, count) = Uvarint(buffer);
        ((unsigned >> 1) as i64 ^ -((unsigned & 1) as i64), count)
    }
}

/// 类型系统再导出：MyDecimal、表达式类型标签、JSON/向量 peek 等。
pub mod types {
    pub use types_crate::*;
    pub use types_decimal::mydecimal::{
        DecimalBinSize, DecimalPeak, NewDecFromFloatForTest, NewDecFromInt, NewDecFromStringForTest,
    };
    pub use types_field::{
        ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
        ETVectorFloat32,
    };
    pub use types_json_functions::PeekBytesAsJSON;
    pub use types_scalar::StrictContext;
    pub use types_vector::PeekBytesAsVectorFloat32;
    /// 零值 CoreTime 常量。
    pub const ZeroCoreTime: CoreTime = CoreTime(0);
    /// 对齐 Go types.IsTypeFloat：只接受 FLOAT，DOUBLE 不属于此谓词。
    pub fn IsTypeFloat(tp: u8) -> bool {
        tp == crate::mysql::TypeFloat
    }
    /// 判断字段类型是否为时间类类型。
    pub fn IsTypeTime(tp: u8) -> bool {
        types_field::IsTypeTime(tp)
    }
}
/// MySQL 常量与类型码再导出。
pub mod mysql {
    pub use mysql_crate::r#const::*;
    pub use mysql_crate::r#type::*;
}
/// terror 错误分类再导出。
pub mod terror {
    pub use terror_crate::*;
}
/// base 工具再导出。
pub mod base {
    pub use base_crate::base::*;
}
/// chunk 列式数据再导出。
pub mod chunk {
    pub use chunk_crate::*;
}
/// 排序规则（collation）再导出。
pub mod collate {
    pub use collate_crate::*;
}
/// hack 工具再导出。
pub mod hack {
    pub use hack_crate::hack::*;
}
/// intest 测试辅助再导出。
pub mod intest {
    pub use intest_crate::*;
}
/// 日志工具再导出。
pub mod logutil {
    pub use logutil_crate::log::*;
}
/// 尺寸常量再导出。
pub mod size {
    pub use size_crate::*;
}
/// 时区/时长桩：用 chrono_tz 模拟 Go time.Location。
pub mod time {
    /// 时区类型别名。
    pub type Location = chrono_tz::Tz;
    /// UTC 时区常量。
    pub const UTC: Location = chrono_tz::UTC;
    /// 透传时长数值（迁移期桩）。
    pub fn Duration(value: i64) -> i64 {
        value
    }
}

/// 整数编解码实现。
mod number;
#[cfg(test)]
mod number_test;
pub use number::*;
/// 浮点编解码实现。
mod float;
pub use float::*;
/// 字节编解码：通过 include! 嵌入 bytes.rs。
mod bytes_codec {
    use crate::{binary, errors, number::*};
    include!("bytes.rs");
}
pub use bytes_codec::*;
/// DECIMAL 编解码：通过 include! 嵌入 decimal.rs。
mod decimal {
    use crate::{errors, mysql, types};
    include!("decimal.rs");
}
pub use decimal::*;
/// 主 codec 逻辑：通过 include! 嵌入 codec.rs。
mod codec {
    use crate::{
        base, binary, bytes_codec::*, chunk, collate, decimal::*, errors, float::*, hack, intest,
        logutil, mysql, number::*, size, terror, time, types,
    };
    include!("codec.rs");
}
pub use codec::*;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;

#[cfg(test)]
#[path = "bytes_1_aster_unit_test.rs"]
mod bytes_1_aster_unit_test;

#[cfg(test)]
#[path = "bytes_test.rs"]
mod bytes_test;

#[cfg(test)]
#[path = "codec_test.rs"]
mod codec_test;

#[cfg(test)]
#[path = "collation_test.rs"]
mod collation_test;

#[cfg(test)]
#[path = "decimal_test.rs"]
mod decimal_test;

#[cfg(test)]
#[path = "float_2_aster_unit_test.rs"]
mod float_2_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
