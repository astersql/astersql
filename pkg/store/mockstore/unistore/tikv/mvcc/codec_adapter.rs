// Copyright 2026 AsterSQL.

// MVCC 编解码适配层：复用 util/codec 的数字与字节编码。
//
// 将 `number`/`bytes` 编解码实现 include 进本 crate，并再导出
// Encode/Decode 系列函数，供锁值与 Write CF 编码使用。

/// 与 Go `encoding/binary` 对齐的常量容器。
pub mod binary {
    /// 64 位变长整数（varint）编码的最大字节数。
    pub const MaxVarintLen64: usize = 10;
}

// 引入 util/codec 的整数编解码实现。
mod number {
    include!("../../../../../util/codec/number.rs");
}

// 引入紧凑字节编码，并绑定本模块的 binary/number。
mod bytes_codec {
    use super::{binary, number::*};
    use crate::errors;
    include!("../../../../../util/codec/bytes.rs");
}

/// 再导出 MVCC 锁/写编码所需的编解码入口。
pub use bytes_codec::EncodeCompactBytes;
pub use number::{DecodeUintDesc, DecodeUvarint, EncodeUint, EncodeUintDesc, EncodeUvarint};
