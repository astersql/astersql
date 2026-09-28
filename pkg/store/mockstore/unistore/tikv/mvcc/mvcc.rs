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

// MVCC 锁结构、用户元与额外事务状态键编解码。
//
// 定义锁头（LockHdr）固定布局、锁体序列化/反序列化，以及 DBUserMeta
// （startTS/commitTS）与 Rollback/Op_Lock 使用的额外状态键编解码。

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code
)]

use crate::codec;
use kvproto::kvrpcpb::{LockInfo, Op};
use protobuf::ProtobufEnum;

/// 文档常量：锁二进制默认使用小端序（与 Go encoding/binary 一致）。
pub const defaultEndian: &str = "binary.LittleEndian";

/// 数据库用户元数据：编码 startTS 与 commitTS。
// DBUserMeta is the user meta used in DB.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DBUserMeta(pub Vec<u8>);

/// 将字节解码为锁；Primary/Value 拷贝，异步提交时 Secondaries 也拷贝。
// DecodeLock decodes data to lock, the primary and value is copied, the secondaries are copied if async commit is enabled.
pub fn DecodeLock(data: &[u8]) -> Lock {
    let header = LockHdr::from_go_bytes(data);
    let lockBuf = &data[mvccLockHdrSize..];
    let primary_len = header.PrimaryLen as usize;
    let primary = lockBuf[..primary_len].to_vec();
    let mut cursor = primary_len;
    // 异步提交时按 SecondaryNum 依次读取长度前缀的次键。
    let mut secondaries = Vec::with_capacity(header.SecondaryNum as usize);
    if header.SecondaryNum > 0 {
        for _ in 0..header.SecondaryNum {
            let keyLen =
                u16::from_le_bytes(lockBuf[cursor..cursor + 2].try_into().unwrap()) as usize;
            cursor += 2;
            secondaries.push(lockBuf[cursor..cursor + keyLen].to_vec());
            cursor += keyLen;
        }
    }
    Lock {
        LockHdr: header,
        Primary: primary,
        Value: lockBuf[cursor..].to_vec(),
        Secondaries: secondaries,
    }
}

/// MVCC 锁固定头：时间戳、TTL、操作类型与异步提交相关字段。
// LockHdr holds fixed size fields for mvcc Lock.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LockHdr {
    pub StartTS: u64,
    pub ForUpdateTS: u64,
    pub MinCommitTS: u64,
    pub TTL: u32,
    pub Op: u8,
    pub HasOldVer: bool,
    pub PrimaryLen: u16,
    pub UseAsyncCommit: bool,
    pub SecondaryNum: u32,
}

impl LockHdr {
    /// 按 Go 小端布局从字节解析锁头。
    fn from_go_bytes(data: &[u8]) -> Self {
        assert!(data.len() >= mvccLockHdrSize, "invalid MVCC lock header");
        Self {
            StartTS: u64::from_le_bytes(data[0..8].try_into().unwrap()),
            ForUpdateTS: u64::from_le_bytes(data[8..16].try_into().unwrap()),
            MinCommitTS: u64::from_le_bytes(data[16..24].try_into().unwrap()),
            TTL: u32::from_le_bytes(data[24..28].try_into().unwrap()),
            Op: data[28],
            HasOldVer: data[29] != 0,
            PrimaryLen: u16::from_le_bytes(data[30..32].try_into().unwrap()),
            UseAsyncCommit: data[32] != 0,
            SecondaryNum: u32::from_le_bytes(data[36..40].try_into().unwrap()),
        }
    }

    /// 按 Go 小端布局将锁头写入缓冲（含 3 字节填充）。
    fn write_go_bytes(&self, data: &mut [u8]) {
        data[0..8].copy_from_slice(&self.StartTS.to_le_bytes());
        data[8..16].copy_from_slice(&self.ForUpdateTS.to_le_bytes());
        data[16..24].copy_from_slice(&self.MinCommitTS.to_le_bytes());
        data[24..28].copy_from_slice(&self.TTL.to_le_bytes());
        data[28] = self.Op;
        data[29] = u8::from(self.HasOldVer);
        data[30..32].copy_from_slice(&self.PrimaryLen.to_le_bytes());
        data[32] = u8::from(self.UseAsyncCommit);
        data[33..36].fill(0);
        data[36..40].copy_from_slice(&self.SecondaryNum.to_le_bytes());
    }
}

/// 锁头固定大小（字节），与 Go `mvccLockHdrSize` 对齐。
pub const mvccLockHdrSize: usize = 40;

/// MVCC 锁：固定头 + Primary +（可选）Secondaries + Value。
// Lock is the structure for MVCC lock.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lock {
    pub LockHdr: LockHdr,
    pub Primary: Vec<u8>,
    pub Value: Vec<u8>,
    pub Secondaries: Vec<Vec<u8>>,
}

impl Lock {
    /// 将锁序列化为二进制（对齐 Go BinaryMarshaler）。
    // MarshalBinary implements encoding.BinaryMarshaler interface.
    pub fn MarshalBinary(&self) -> Vec<u8> {
        let lockLen = mvccLockHdrSize + self.Primary.len() + self.Value.len();
        let mut length = lockLen;
        // 次键按 u16 长度前缀依次追加。
        if self.LockHdr.SecondaryNum > 0 {
            for secondaryKey in &self.Secondaries {
                length += 2;
                length += secondaryKey.len();
            }
        }

        let mut buf = vec![0; length];
        self.LockHdr.write_go_bytes(&mut buf[..mvccLockHdrSize]);
        let mut cursor = mvccLockHdrSize;
        buf[cursor..cursor + self.Primary.len()].copy_from_slice(&self.Primary);
        cursor += self.Primary.len();

        if self.LockHdr.SecondaryNum > 0 {
            for secondaryKey in &self.Secondaries {
                let keyLen = secondaryKey.len() as u16;
                buf[cursor..cursor + 2].copy_from_slice(&keyLen.to_le_bytes());
                cursor += 2;
                buf[cursor..cursor + secondaryKey.len()].copy_from_slice(secondaryKey);
                cursor += secondaryKey.len();
            }
        }

        buf[cursor..cursor + self.Value.len()].copy_from_slice(&self.Value);
        buf
    }

    /// 转为 RPC 可见的 LockInfo。
    // ToLockInfo converts an mvcc Lock to kvrpcpb.LockInfo.
    pub fn ToLockInfo(&self, key: &[u8]) -> LockInfo {
        LockInfo {
            primary_lock: self.Primary.clone(),
            lock_version: self.LockHdr.StartTS,
            key: key.to_vec(),
            lock_ttl: self.LockHdr.TTL as u64,
            lock_type: Op::from_i32(self.LockHdr.Op as i32).unwrap_or_default(),
            lock_for_update_ts: self.LockHdr.ForUpdateTS,
            use_async_commit: self.LockHdr.UseAsyncCommit,
            min_commit_ts: self.LockHdr.MinCommitTS,
            secondaries: self.Secondaries.clone().into(),
            ..Default::default()
        }
    }

    /// 生成与 Go 风格接近的调试字符串。
    // String implements fmt.Stringer for Lock.
    pub fn String(&self) -> String {
        let lock_type = Op::from_i32(self.LockHdr.Op as i32)
            .map(|op| format!("{op:?}"))
            .unwrap_or_else(|| self.LockHdr.Op.to_string());
        format!(
            "Lock {{ Type: {}, StartTS: {},  ForUpdateTS: {}, Primary: {}, UseAsyncCommit: {} }}",
            lock_type,
            self.LockHdr.StartTS,
            self.LockHdr.ForUpdateTS,
            hex::encode(&self.Primary),
            self.LockHdr.UseAsyncCommit,
        )
    }
}

/// 锁用户元单字节取值。
// UserMeta value for lock.
pub const LockUserMetaNoneByte: u8 = 0;
pub const LockUserMetaDeleteByte: u8 = 2;

/// 锁用户元字节切片常量。
// UserMeta byte slices for lock.
pub const LockUserMetaNone: &[u8] = &[LockUserMetaNoneByte];
pub const LockUserMetaDelete: &[u8] = &[LockUserMetaDeleteByte];

/// 从键尾 8 字节解码时间戳（降序编码）。
// DecodeKeyTS decodes the TS in a key.
pub fn DecodeKeyTS(buf: &[u8]) -> u64 {
    let tsBin = &buf[buf.len() - 8..];
    match codec::DecodeUintDesc(tsBin) {
        Ok((_remain, ts)) => ts,
        Err(err) => panic!("{:?}", err),
    }
}

/// 用 startTS/commitTS 构造 16 字节用户元。
// NewDBUserMeta creates a new DBUserMeta.
pub fn NewDBUserMeta(startTS: u64, commitTS: u64) -> DBUserMeta {
    let mut m = vec![0; 16];
    m[..8].copy_from_slice(&startTS.to_le_bytes());
    m[8..].copy_from_slice(&commitTS.to_le_bytes());
    DBUserMeta(m)
}

impl DBUserMeta {
    /// 读取提交时间戳。
    // CommitTS reads the commitTS from the DBUserMeta.
    pub fn CommitTS(&self) -> u64 {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&self.0[8..16]);
        u64::from_le_bytes(bytes)
    }

    /// 读取开始时间戳。
    // StartTS reads the startTS from the DBUserMeta.
    pub fn StartTS(&self) -> u64 {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&self.0[..8]);
        u64::from_le_bytes(bytes)
    }
}

/// 编码额外事务状态键（仅用于 Rollback 与 Op_Lock）。
// EncodeExtraTxnStatusKey encodes a extra transaction status key.
// It is only used for Rollback and Op_Lock.
pub fn EncodeExtraTxnStatusKey(key: &[u8], startTS: u64) -> Vec<u8> {
    let b = key.to_vec();
    let mut ret = codec::EncodeUintDesc(b, startTS);
    // 首字节 +1，与普通用户键命名空间隔离。
    ret[0] = ret[0].wrapping_add(1);
    ret
}

/// 解码额外事务状态键，还原原始用户键。
// DecodeExtraTxnStatusKey decodes a extra transaction status key.
pub fn DecodeExtraTxnStatusKey(extraKey: &[u8]) -> Option<Vec<u8>> {
    if extraKey.len() <= 9 {
        return None;
    }
    let mut key = extraKey[..extraKey.len() - 8].to_vec();
    key[0] = key[0].wrapping_sub(1);
    Some(key)
}
