// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! 事务 meta 键与 write-CF 值的编解码。
//! 对应 Go `br/pkg/stream/meta_kv.go`：日志备份/恢复在重写 meta RawKV
//! 时需拆出 Key/Field/Ts，并按 TiKV write 列族布局解析与回写。
//! 布局对齐 tikv `txn_types::Write`；短值、GC fence、txnSource 等可选
//! 后缀按 flag 顺序编码，未知 flag 终止解析以保持向前兼容。

use crate::stubs::LightningPhysicalImportTxnSource;
use crate::stubs::errors::Error;
use crate::stubs::errors::berrors;
use crate::stubs::{codec, kv, meta, tablecodec};

/// 事务 meta 键的三元组视图：业务 Key、Field 与提交时间戳。
/// 与 Go `RawMetaKey` 字段一一对应，供 filter/rewrite 原地改写后重编码。
pub struct RawMetaKey {
    pub Key: Vec<u8>,
    pub Field: Vec<u8>,
    pub Ts: u64,
}

/// 从事务编码的 meta 键解出 `RawMetaKey`。
/// 顺序：`DecodeBytes` → `DecodeMetaKey` → 余量上 `DecodeUintDesc` 取 Ts；
/// 任一步失败则原样上抛，与 Go `ParseTxnMetaKeyFrom` 一致。
pub fn ParseTxnMetaKeyFrom(txnKey: &[u8]) -> Result<RawMetaKey, Error> {
    let (less_buff, raw_key) = codec::DecodeBytes(txnKey, None).map_err(Error::new)?;
    let (key, field) = tablecodec::DecodeMetaKey(&raw_key).map_err(Error::new)?;
    let (_, ts) = codec::DecodeUintDesc(&less_buff).map_err(Error::new)?;
    Ok(RawMetaKey {
        Key: key,
        Field: field,
        Ts: ts,
    })
}

/// 从 table meta 事务键解析所属 DB ID。
/// 先走完整 meta 键解析，再对业务 Key 调用 `meta::ParseDBKey`。
pub fn ParseDBIDFromTableKey(key: Vec<u8>) -> Result<i64, Error> {
    let raw_meta_key = ParseTxnMetaKeyFrom(&key)?;
    meta::ParseDBKey(&raw_meta_key.Key).map_err(Error::new)
}

impl RawMetaKey {
    /// 更新业务 Key（如 DB/table 重映射后）。
    pub fn UpdateKey(&mut self, key: Vec<u8>) {
        self.Key = key;
    }

    /// 更新 Field（如字段级重写）。
    pub fn UpdateField(&mut self, field: Vec<u8>) {
        self.Field = field;
    }

    /// 更新时间戳（通常保持原提交 Ts，少数路径需对齐）。
    pub fn UpdateTS(&mut self, ts: u64) {
        self.Ts = ts;
    }

    /// 按解析逆序编码回事务键：EncodeMetaKey → EncodeBytes → EncodeUintDesc。
    /// 与 Go `EncodeMetaKey` 字节布局一致，保证 rewrite 后可被下游再次解析。
    pub fn EncodeMetaKey(&self) -> kv::Key {
        let raw_key = tablecodec::EncodeMetaKey(&self.Key, &self.Field);
        let encoded_key = codec::EncodeBytes(Vec::new(), &raw_key);
        codec::EncodeUintDesc(encoded_key, self.Ts)
    }
}

/// write-CF 首字节写入类型，与 TiKV WriteType 字节常量对齐。
pub type WriteType = u8;

/// Lock 记录。
pub const WriteTypeLock: WriteType = b'L';
/// Rollback 记录。
pub const WriteTypeRollback: WriteType = b'R';
/// Delete 记录。
pub const WriteTypeDelete: WriteType = b'D';
/// Put 记录。
pub const WriteTypePut: WriteType = b'P';

/// 校验并返回合法 WriteType；非法字节标注 `ErrInvalidArgument`。
pub fn WriteTypeFrom(t: u8) -> Result<WriteType, Error> {
    match t {
        WriteTypeDelete | WriteTypeLock | WriteTypePut | WriteTypeRollback => Ok(t),
        _ => Err(Error::new(format!("invalid write type:{}", t as char))
            .Annotatef(berrors::ErrInvalidArgument)),
    }
}

// 可选后缀 flag，与 tikv write 编码一致；顺序影响 EncodeTo 输出。
const flagShortValuePrefix: u8 = b'v';
const flagOverlappedRollback: u8 = b'R';
const flagGCFencePrefix: u8 = b'F';
const flagLastChangePrefix: u8 = b'l';
const flagTxnSourcePrefix: u8 = b'S';

/// write 列族值的结构化表示。
/// 对应 Go `RawWriteCFValue` / TiKV Write：类型+startTs 为定长头，
/// 其后按 flag 串联 shortValue、overlapped rollback、gcFence、
/// lastChange、txnSource；用于日志恢复时改写 shortValue 后原样回编。
#[derive(Clone, Debug, Default)]
pub struct RawWriteCFValue {
    t: WriteType,
    startTs: u64,
    shortValue: Vec<u8>,
    hasOverlappedRollback: bool,
    hasGCFence: bool,
    gcFence: u64,
    lastChangeTs: u64,
    versionsToLastChange: u64,
    txnSource: u64,
}

impl RawWriteCFValue {
    /// 按 TiKV write 布局解码；长度不足 9 视为非法输入。
    /// 未知 flag 直接 break，避免把后续未知扩展误读为当前字段。
    pub fn ParseFrom(&mut self, mut data: &[u8]) -> Result<(), Error> {
        if data.len() < 9 {
            return Err(
                Error::new(format!("invalid input value, len:{}", data.len()))
                    .Annotatef(berrors::ErrInvalidArgument),
            );
        }

        self.t = WriteTypeFrom(data[0])?;
        let (remain, ts) = codec::DecodeUvarint(&data[1..]).map_err(Error::new)?;
        data = remain;
        self.startTs = ts;

        // 逐 flag 消费可选后缀；与 Go 的 `l_for` 循环语义一致。
        while !data.is_empty() {
            match data[0] {
                flagShortValuePrefix => {
                    // flag + 1 字节长度；再校验 value 体是否完整。
                    if data.len() < 2 {
                        return Err(Error::new(format!(
                            "insufficient data for short value prefix, need at least 2 bytes but only have {}",
                            data.len()
                        ))
                        .Annotatef(berrors::ErrInvalidArgument));
                    }
                    let vlen = data[1] as usize;
                    let required_len = vlen + 2;
                    if data.len() < required_len {
                        return Err(Error::new(format!(
                            "insufficient data for short value, need {required_len} bytes but only have {}",
                            data.len()
                        ))
                        .Annotatef(berrors::ErrInvalidArgument));
                    }
                    self.shortValue = data[2..required_len].to_vec();
                    data = &data[required_len..];
                }
                flagOverlappedRollback => {
                    // 单字节标记，无载荷。
                    self.hasOverlappedRollback = true;
                    data = &data[1..];
                }
                flagGCFencePrefix => {
                    // 重叠回滚场景记录的下一版本栅栏，见 TiKV Write::gc_fence。
                    self.hasGCFence = true;
                    let decoded = codec::DecodeUint(&data[1..]).map_err(|_| {
                        Error::new("decode gc fence failed").Annotate(berrors::ErrInvalidArgument)
                    })?;
                    data = decoded.0;
                    self.gcFence = decoded.1;
                }
                flagLastChangePrefix => {
                    // lastChangeTs 为定长 uint，versionsToLastChange 为 uvarint。
                    // versions>0 且 lastChangeTs==0 表示此前无 PUT/DELETE。
                    let decoded_last = codec::DecodeUint(&data[1..]).map_err(|_| {
                        Error::new("decode last change ts failed")
                            .Annotate(berrors::ErrInvalidArgument)
                    })?;
                    data = decoded_last.0;
                    self.lastChangeTs = decoded_last.1;

                    let decoded_versions = codec::DecodeUvarint(data).map_err(|_| {
                        Error::new("decode versions to last change failed")
                            .Annotate(berrors::ErrInvalidArgument)
                    })?;
                    data = decoded_versions.0;
                    self.versionsToLastChange = decoded_versions.1;
                }
                flagTxnSourcePrefix => {
                    // 事务来源位图，恢复路径可 OR 物理导入标记。
                    let decoded = codec::DecodeUvarint(&data[1..]).map_err(|_| {
                        Error::new("decode txn source failed").Annotate(berrors::ErrInvalidArgument)
                    })?;
                    data = decoded.0;
                    self.txnSource = decoded.1;
                }
                // 未知 flag：停止解析，保留已读字段。
                _ => break,
            }
        }
        Ok(())
    }

    /// 是否为 rollback 记录。
    pub fn IsRollback(&self) -> bool {
        self.GetWriteType() == WriteTypeRollback
    }

    /// 是否为 delete 记录。
    pub fn IsDelete(&self) -> bool {
        self.GetWriteType() == WriteTypeDelete
    }

    /// 是否为 put 记录。
    pub fn IsPut(&self) -> bool {
        self.GetWriteType() == WriteTypePut
    }

    /// shortValue 非空时表示值内联在 write-CF，无需再读 default-CF。
    pub fn HasShortValue(&self) -> bool {
        !self.shortValue.is_empty()
    }

    /// 取出 shortValue 副本，供 meta 内容重写。
    pub fn GetShortValue(&self) -> Vec<u8> {
        self.shortValue.clone()
    }

    /// 原地替换 shortValue（例如 filter 改写后的 meta 字节）。
    pub fn UpdateShortValue(&mut self, value: Vec<u8>) {
        self.shortValue = value;
    }

    /// 标记事务来自 Lightning 物理导入，与 Go 对 `LightningPhysicalImportTxnSource` 的 OR 一致。
    pub fn MarkPhysicalImportTxnSource(&mut self) {
        self.txnSource |= LightningPhysicalImportTxnSource;
    }

    /// 返回 startTs（事务开始时间戳）。
    pub fn GetStartTs(&self) -> u64 {
        self.startTs
    }

    /// 返回 write 类型字节。
    pub fn GetWriteType(&self) -> u8 {
        self.t
    }

    /// 按与 ParseFrom 对称的顺序编码；仅输出已设置的可选后缀。
    /// lastChange 在 ts 或 versions 任一非零时写出，对齐 Go `EncodeTo`。
    pub fn EncodeTo(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(9);
        data.push(self.t);
        data = codec::EncodeUvarint(data, self.startTs);

        if !self.shortValue.is_empty() {
            data.push(flagShortValuePrefix);
            data.push(self.shortValue.len() as u8);
            data.extend_from_slice(&self.shortValue);
        }
        if self.hasOverlappedRollback {
            data.push(flagOverlappedRollback);
        }
        if self.hasGCFence {
            data.push(flagGCFencePrefix);
            data = codec::EncodeUint(data, self.gcFence);
        }
        if self.lastChangeTs > 0 || self.versionsToLastChange > 0 {
            data.push(flagLastChangePrefix);
            data = codec::EncodeUint(data, self.lastChangeTs);
            data = codec::EncodeUvarint(data, self.versionsToLastChange);
        }
        if self.txnSource > 0 {
            data.push(flagTxnSourcePrefix);
            data = codec::EncodeUvarint(data, self.txnSource);
        }
        data
    }
}
