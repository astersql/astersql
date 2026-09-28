// Copyright 2026 AsterSQL.
//! Local stand-ins for backuppb / kvrpcpb / metautil / tablecodec / utils
//! used by rtree algorithms without pulling kvproto/grpcio.
//!
//! 本文件为 `br/pkg/rtree` 提供本地桩类型与 tablecodec 子集，避免拉入完整
//! protobuf/gRPC 依赖。语义对齐 Go 侧 backuppb.File、kvrpcpb.KeyRange、
//! metautil 校验和汇总，以及 tablecodec 的行/索引键编解码；仅覆盖 rtree
//! 合并与进度树所需路径，不是完整 codec 实现。

use std::fmt;

/// Minimal backup file metadata used by Range / checksum aggregation.
/// 备份文件元数据子集：供 Range 聚合与校验和累加使用，字段名保持 Go 风格。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct File {
    pub Name: String,
    pub Cf: String,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub TotalBytes: u64,
    pub TotalKvs: u64,
    pub Crc64Xor: u64,
}

impl File {
    /// 返回文件名；对应 Go `GetName` 访问器。
    pub fn GetName(&self) -> &str {
        &self.Name
    }
    /// 区间起始键（含）。
    pub fn GetStartKey(&self) -> &[u8] {
        &self.StartKey
    }
    /// 区间结束键（通常左闭右开语义由调用方解释）。
    pub fn GetEndKey(&self) -> &[u8] {
        &self.EndKey
    }
}

/// Key range shape matching kvrpcpb.KeyRange.
/// 与 kvrpcpb.KeyRange 同形的键区间，供展示与区间运算。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RpcKeyRange {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

/// Checksum aggregate matching metautil.ChecksumStats.
/// 校验和聚合：Crc64Xor 按文件异或，Kvs/Bytes 累加，对齐 metautil。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumStats {
    pub Crc64Xor: u64,
    pub TotalKvs: u64,
    pub TotalBytes: u64,
}

/// AppendDataFile marker matching metautil.AppendDataFile.
/// MetaWriter.Send 的 kind 常量：表示追加数据文件元信息。
pub const AppendDataFile: i32 = 1;

/// MetaWriter stand-in: records sent files for ProgressRangeTree.
/// 进度树写入桩：真实路径会落到 backup meta writer；此处仅抽象 Send。
pub trait MetaWriter: Send {
    fn Send(&self, files: &[File], kind: i32) -> Result<(), String>;
}

/// FreeListG stand-in; Rust BTreeMap does not need a freelist pool.
/// Go 用 freelist 复用节点；Rust BTreeMap 无需池化，保留类型仅满足 API 形状。
#[derive(Clone, Default)]
pub struct FreeListG<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> FreeListG<T> {
    /// 构造空 freelist 占位。
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

/// 汇总文件列表的 Crc64Xor / TotalKvs / TotalBytes，对齐 metautil.SummaryFiles。
pub fn SummaryFiles(files: &[File]) -> (u64, u64, u64) {
    let mut crc = 0u64;
    let mut kvs: u64 = 0;
    let mut bytes: u64 = 0;
    for f in files {
        crc ^= f.Crc64Xor;
        kvs = kvs.wrapping_add(f.TotalKvs);
        bytes = bytes.wrapping_add(f.TotalBytes);
    }
    (crc, kvs, bytes)
}

/// TiDB 表键前缀 `'t'`。
const TABLE_PREFIX: u8 = b't';
/// 行记录分隔 `"_r"`。
const RECORD_PREFIX_SEP: &[u8] = b"_r";
/// 索引分隔 `"_i"`。
const INDEX_PREFIX_SEP: &[u8] = b"_i";

/// 解码 memcomparable 有符号整数：大端 u64 后翻转符号位还原 i64。
fn decode_cmp_uint(data: &[u8]) -> Result<(i64, &[u8]), String> {
    if data.len() < 8 {
        return Err("insufficient bytes to decode int".into());
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&data[..8]);
    let u = u64::from_be_bytes(buf);
    // DecodeCmpUintToInt: flip the sign bit back.
    // 与 codec.DecodeCmpUintToInt 一致：最高位翻转以恢复有序编码前的符号。
    let v = (u ^ (1u64 << 63)) as i64;
    Ok((v, &data[8..]))
}

/// DecodeKeyHead matches pkg/tablecodec.DecodeKeyHead for un-keyspaced keys.
/// 解析未带 keyspace 前缀的表键头：返回 (table_id, index_id, is_record)。
/// 行键 index_id=0 且 is_record=true；索引键则带出 index_id。
pub fn DecodeKeyHead(key: &[u8]) -> Result<(i64, i64, bool), String> {
    if key.is_empty() || key[0] != TABLE_PREFIX {
        return Err(format!("invalid key - {key:?}"));
    }
    let rest = &key[1..];
    let (table_id, rest) = decode_cmp_uint(rest)?;
    if rest.starts_with(RECORD_PREFIX_SEP) {
        return Ok((table_id, 0, true));
    }
    if !rest.starts_with(INDEX_PREFIX_SEP) {
        return Err(format!("invalid key - {key:?}"));
    }
    let rest = &rest[INDEX_PREFIX_SEP.len()..];
    let (index_id, _) = decode_cmp_uint(rest)?;
    Ok((table_id, index_id, false))
}

/// Encode helpers for tests / NeedsMerge key construction.
/// 将 i64 编码为可比较的 u64（翻转符号位），供测试与 NeedsMerge 构键。
pub fn EncodeIntToCmpUint(v: i64) -> u64 {
    (v as u64) ^ (1u64 << 63)
}

/// 编码行键前缀：`t || cmp(table_id) || _r`。
pub fn EncodeRowKeyPrefix(table_id: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 8 + 2);
    out.push(TABLE_PREFIX);
    out.extend_from_slice(&EncodeIntToCmpUint(table_id).to_be_bytes());
    out.extend_from_slice(RECORD_PREFIX_SEP);
    out
}

/// 编码索引键前缀：`t || cmp(table_id) || _i || cmp(index_id)`。
pub fn EncodeIndexKeyPrefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 8 + 2 + 8);
    out.push(TABLE_PREFIX);
    out.extend_from_slice(&EncodeIntToCmpUint(table_id).to_be_bytes());
    out.extend_from_slice(INDEX_PREFIX_SEP);
    out.extend_from_slice(&EncodeIntToCmpUint(index_id).to_be_bytes());
    out
}

/// GenTableRecordPrefix matches `tablecodec.GenTableRecordPrefix`.
/// 与 Go GenTableRecordPrefix 对齐的行前缀生成。
pub fn GenTableRecordPrefix(table_id: i64) -> Vec<u8> {
    EncodeRowKeyPrefix(table_id)
}

/// EncodeRecordKey matches `tablecodec.EncodeRecordKey` for IntHandle.
/// IntHandle 行键：在 record_prefix 后追加 cmp(row_id)。
pub fn EncodeRecordKey(record_prefix: &[u8], row_id: i64) -> Vec<u8> {
    let mut out = record_prefix.to_vec();
    out.extend_from_slice(&EncodeIntToCmpUint(row_id).to_be_bytes());
    out
}

/// EncodeIndexSeekKey matches `tablecodec.EncodeIndexSeekKey`.
/// 索引 seek 键：前缀后追加已编码的索引列值。
pub fn EncodeIndexSeekKey(table_id: i64, idx_id: i64, encoded_value: &[u8]) -> Vec<u8> {
    let mut out = EncodeIndexKeyPrefix(table_id, idx_id);
    out.extend_from_slice(encoded_value);
    out
}

/// EncodeKeyspaceKey prepends `'x' || uint24(keyspace_id)` (TiKV API V2 / ModeTxn).
/// API V2 keyspace：前置 `'x'` + 24-bit keyspace_id，再拼接业务键。
pub fn EncodeKeyspaceKey(keyspace_id: u32, key: &[u8]) -> Vec<u8> {
    let mut out = vec![
        b'x',
        ((keyspace_id >> 16) & 0xff) as u8,
        ((keyspace_id >> 8) & 0xff) as u8,
        (keyspace_id & 0xff) as u8,
    ];
    out.extend_from_slice(key);
    out
}

/// 将键字节格式化为连续小写 hex，用于日志脱敏展示。
pub fn redact_key(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

impl fmt::Display for RpcKeyRange {
    /// 以 `[start, end)` 的 hex 形式打印区间。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}, {})",
            redact_key(&self.StartKey),
            redact_key(&self.EndKey)
        )
    }
}
