// Copyright 2019-present PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TiKV 兼容的 Write CF / Lock CF 值编解码。
//
// Write CF 记录已提交写类型与 startTS；Lock CF 将 MVCC 锁编码为 TiKV
// 字节布局（短值内联、长值分离，可选 forUpdateTS / minCommitTS 后缀）。

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code
)]

use crate::codec;
use kvproto::kvrpcpb::Op;
use thiserror::Error;

use crate::mvcc::Lock;

/// 写类型别名（单字节标记）。
// WriteType defines a write type.
pub type WriteType = u8;

/// Write CF 类型常量：Lock / Rollback / Delete / Put。
// WriteType.
pub const WriteTypeLock: WriteType = b'L';
pub const WriteTypeRollback: WriteType = b'R';
pub const WriteTypeDelete: WriteType = b'D';
pub const WriteTypePut: WriteType = b'P';

/// Write CF 解码结果：类型、开始时间戳与短值载荷。
// WriteCFValue represents a write CF value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WriteCFValue {
    pub Type: WriteType,
    pub StartTS: u64,
    pub ShortVal: Vec<u8>,
}

/// 非法 Write CF 值的错误消息常量。
pub const errInvalidWriteCFValue: &str = "invalid write CF value";

#[derive(Debug, Error, Eq, PartialEq)]
/// Write CF 解析错误：内容非法或编解码失败。
pub enum WriteCFValueError {
    #[error("invalid write CF value")]
    Invalid,
    #[error("{0}")]
    Codec(String),
}

/// 解析 Write CF 字节为 [`WriteCFValue`]。
// ParseWriteCFValue parses the []byte data and returns a WriteCFValue.
pub fn ParseWriteCFValue(data: &[u8]) -> Result<WriteCFValue, WriteCFValueError> {
    if data.is_empty() {
        return Err(WriteCFValueError::Invalid);
    }

    let mut wv = WriteCFValue {
        Type: data[0],
        ..Default::default()
    };
    // 仅接受已知写类型字节。
    match wv.Type {
        WriteTypePut | WriteTypeDelete | WriteTypeLock | WriteTypeRollback => {}
        _ => return Err(WriteCFValueError::Invalid),
    }

    let (shortVal, startTS) = codec::DecodeUvarint(&data[1..])
        .map_err(|err| WriteCFValueError::Codec(err.to_string()))?;
    wv.ShortVal = shortVal.to_vec();
    wv.StartTS = startTS;
    Ok(wv)
}

/// 短值 / forUpdateTS / minCommitTS 字段前缀。
pub const shortValuePrefix: u8 = b'v';
pub const forUpdatePrefix: u8 = b'f';
pub const minCommitTsPrefix: u8 = b'm';

/// 短值最大长度；超过则写入 default CF。
// ShortValueMaxLen defines max length of short value.
pub const ShortValueMaxLen: usize = 64;

/// 按 TiKV 格式编码 Write CF 值。
// EncodeWriteCFValue accepts a write cf parameters and return the encoded bytes data.
// Just like the tikv encoding form. See tikv/src/storage/mvcc/write.rs for more detail.
pub fn EncodeWriteCFValue(t: WriteType, startTs: u64, shortVal: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    data.push(t);
    data = codec::EncodeUvarint(data, startTs);
    if !shortVal.is_empty() {
        data.push(shortValuePrefix);
        data.push(shortVal.len() as u8);
        data.extend_from_slice(shortVal);
    }
    data
}

/// 编码 MVCC 锁为 Lock CF 值；长值时第二返回值为 default CF 载荷。
// EncodeLockCFValue encodes the mvcc lock and returns putLock value and putDefault value if exists.
pub fn EncodeLockCFValue(lock: &Lock) -> (Vec<u8>, Vec<u8>) {
    let mut data = Vec::new();
    // 按操作类型写入锁类型字节。
    match lock.LockHdr.Op {
        op if op == Op::Put as u8 => data.push(LockTypePut),
        op if op == Op::Del as u8 => data.push(LockTypeDelete),
        op if op == Op::Lock as u8 => data.push(LockTypeLock),
        op if op == Op::PessimisticLock as u8 => data.push(LockTypePessimistic),
        _ => panic!("invalid lock op"),
    }

    let mut longValue = Vec::new();
    data = codec::EncodeUvarint(
        codec::EncodeCompactBytes(data, &lock.Primary),
        lock.LockHdr.StartTS,
    );
    data = codec::EncodeUvarint(data, lock.LockHdr.TTL as u64);
    // 短值内联；超长值放到 longValue 供 default CF。
    if lock.Value.len() <= ShortValueMaxLen {
        if !lock.Value.is_empty() {
            data.push(shortValuePrefix);
            data.push(lock.Value.len() as u8);
            data.extend_from_slice(&lock.Value);
        }
    } else {
        longValue = lock.Value.clone();
    }

    // 可选后缀：悲观锁 forUpdateTS 与异步提交 minCommitTS。
    if lock.LockHdr.ForUpdateTS > 0 {
        data.push(forUpdatePrefix);
        data = codec::EncodeUint(data, lock.LockHdr.ForUpdateTS);
    }
    if lock.LockHdr.MinCommitTS > 0 {
        data.push(minCommitTsPrefix);
        data = codec::EncodeUint(data, lock.LockHdr.MinCommitTS);
    }
    (data, longValue)
}

/// 锁类型别名（单字节标记）。
// LockType defines a lock type.
// LockType 对应 Go 的 byte 别名。
pub type LockType = u8;

/// Lock CF 类型常量：Put / Delete / Lock / Pessimistic。
// LockType.
pub const LockTypePut: LockType = b'P';
pub const LockTypeDelete: LockType = b'D';
pub const LockTypeLock: LockType = b'L';
pub const LockTypePessimistic: LockType = b'S';
