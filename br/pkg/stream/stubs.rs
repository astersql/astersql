// Copyright 2026 AsterSQL.
//! Local stand-ins for Storage, backuppb, codec, tablecodec, meta, kv, and domain boundaries.
//!
//! 流备份包本地桩/适配边界：对齐 Go 侧多包依赖的最小可测子集。
//! 提供 Storage、backuppb、codec/tablecodec、meta、model、errors 等占位实现。
//! 能力边界：JSON 代替 protobuf、MemStorage 代替远端对象存储、codec 为 TiKV 兼容子集。
//! 注释标明“桩”与真实依赖差异；勿将未实现路径描述为生产可用。
//! 供 stream_mgr/table_mapping/stream_metas 单测与编译链接，不替代完整 PD/TiKV 栈。

//! EncodeBytes 循环含空末组，保证与 TiKV 可比较编码一致。
//! Migration/Metadata Marshal 使用 serde_json，与生产 protobuf 字节不兼容。
//! IsMetaDBKey 仅检查 mDB 前缀，复杂编码场景需真实 utils。
//! MemStorage 非线程安全之外的语义保证：内部 Mutex 保护 map。
//! StreamBackupTaskInfo getter 对齐 proto 生成代码调用习惯。
//! DeleteSpan/MetaEdit getter 供 migration 合并逻辑只读访问。
//! tablecodec::PrefixNext 用于构造左闭右开观察范围上界。
//! LightningPhysicalImportTxnSource 仅占位，本包不解释事务源位图。
//! CIStr 的 L 字段默认空，解析侧通常只用 O。
//! Job/DelRangeArg 供 DDL 相关测试占位，流路径可不使用。
//! berrors 常量为 Annotate 上下文标签，非 errno。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 上游集群对象 ID。
pub type UpstreamID = i64;
pub type DownstreamID = i64;

/// Lightning 物理导入事务源标记位，对齐 Go 常量。
pub const LightningPhysicalImportTxnSource: u64 = 1 << 16;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 表简要信息：名称 + 分区 ID 列表（meta 解析用）。
pub struct TableSimpleInfo {
    pub Name: String,
    // 逻辑表下的分区 ID；无分区则为空。
    pub PartitionIds: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 单表上下游替换：表 ID、分区/索引映射与过滤标记。
pub struct TableReplace {
    pub Name: String,
    pub TableID: DownstreamID,
    // 分区上游→下游。
    pub PartitionMap: HashMap<UpstreamID, DownstreamID>,
    pub IndexMap: HashMap<UpstreamID, DownstreamID>,
    // 被 filter 排除时为真。
    pub FilteredOut: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 单库上下游替换，内含 TableMap。
pub struct DBReplace {
    pub Name: String,
    pub DbID: DownstreamID,
    // 库内表映射。
    pub TableMap: HashMap<UpstreamID, TableReplace>,
    pub FilteredOut: bool,
    // 下游复用已有库时为真。
    pub Reused: bool,
}

/// 构造空分区/索引映射的 TableReplace。
pub fn NewTableReplace(name: String, newID: DownstreamID) -> TableReplace {
    TableReplace {
        Name: name,
        TableID: newID,
        ..Default::default()
    }
}

/// 构造空 TableMap 的 DBReplace。
pub fn NewDBReplace(name: String, newID: DownstreamID) -> DBReplace {
    DBReplace {
        Name: name,
        DbID: newID,
        ..Default::default()
    }
}

// 轻量错误类型与 berrors 字符串常量；非完整 br 错误体系。
pub mod errors {
    #[derive(Clone, Debug, PartialEq, Eq)]
    /// 仅承载消息的桩错误。
    pub struct Error {
        pub msg: String,
    }

    impl Error {
        /// 从消息构造。
        pub fn new(msg: impl Into<String>) -> Self {
            Self { msg: msg.into() }
        }

        /// 对齐 Go Errorf 命名。
        pub fn Errorf(msg: impl Into<String>) -> Self {
            Self::new(msg)
        }

        /// 前置上下文：`ctx: msg`。
        pub fn Annotate(self, ctx: impl Into<String>) -> Self {
            Self {
                msg: format!("{}: {}", ctx.into(), self.msg),
            }
        }

        /// Annotate 别名。
        pub fn Annotatef(self, ctx: impl Into<String>) -> Self {
            self.Annotate(ctx)
        }

        /// 桩：原样返回，无堆栈增强。
        pub fn Trace(err: Self) -> Self {
            err
        }
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.msg)
        }
    }

    impl std::error::Error for Error {}

    // 与 Go berrors 名称对齐的字符串标签，非枚举错误码。
    pub mod berrors {
        pub const ErrInvalidArgument: &str = "invalid argument";
        /// 恢复 rewrite/ID 映射非法。
        pub const ErrRestoreInvalidRewrite: &str = "restore invalid rewrite";
        pub const ErrUnknown: &str = "unknown";
        /// migration 版本不受支持。
        pub const ErrMigrationVersionNotSupported: &str = "migration version not supported";
    }
}

// KV 键值与范围的最小类型，对齐 kv.Key / KeyRange。
pub mod kv {
    pub type Key = Vec<u8>;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 键值对条目。
    pub struct Entry {
        pub Key: Key,
        pub Value: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 左闭右开键范围。
    pub struct KeyRange {
        pub StartKey: Key,
        pub EndKey: Key,
    }
}

// TiKV memcomparable 编码子集：Bytes/Uint/Uvarint 与降序 Uint。
// 用于 meta key 编解码测试；非完整 tikv/client-go codec。
pub mod codec {
    const ENC_GROUP_SIZE: usize = 8;
    // marker = 0xff - pad_count。
    const ENC_MARKER: u8 = 0xff;
    // 填充字节。
    const ENC_PAD: u8 = 0x0;
    const maxVarintLen64: usize = 10;

    /// 按组编码字节串，末组补零并写 marker。
    pub fn EncodeBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
        let d_len = data.len();
        for idx in (0..=d_len).step_by(ENC_GROUP_SIZE) {
            let remain = d_len.saturating_sub(idx);
            let pad_count = if remain >= ENC_GROUP_SIZE {
                b.extend_from_slice(&data[idx..idx + ENC_GROUP_SIZE]);
                0
            } else {
                let pad_count = ENC_GROUP_SIZE - remain;
                b.extend_from_slice(&data[idx..]);
                b.extend(vec![ENC_PAD; pad_count]);
                pad_count
            };
            let marker = ENC_MARKER - pad_count as u8;
            b.push(marker);
        }
        b
    }

    /// 解码 EncodeBytes 结果；返回剩余输入与解码缓冲。
    pub fn DecodeBytes(
        mut b: &[u8],
        mut buf: Option<Vec<u8>>,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut out = buf.take().unwrap_or_default();
        out.clear();
        loop {
            if b.len() < ENC_GROUP_SIZE + 1 {
                return Err("insufficient bytes to decode value".into());
            }
            let group_bytes = &b[..ENC_GROUP_SIZE + 1];
            let group = &group_bytes[..ENC_GROUP_SIZE];
            let marker = group_bytes[ENC_GROUP_SIZE];
            let pad_count = ENC_MARKER.wrapping_sub(marker);
            // marker 非法：填充数超出组大小。
            if pad_count > ENC_GROUP_SIZE as u8 {
                return Err(format!("invalid marker byte, group bytes {group_bytes:?}"));
            }
            let real_group_size = ENC_GROUP_SIZE - pad_count as usize;
            out.extend_from_slice(&group[..real_group_size]);
            b = &b[ENC_GROUP_SIZE + 1..];
            if pad_count != 0 {
                // 末组填充必须为 0。
                for v in &group[real_group_size..] {
                    if *v != ENC_PAD {
                        return Err(format!("invalid padding byte, group bytes {group_bytes:?}"));
                    }
                }
                break;
            }
        }
        Ok((b.to_vec(), out))
    }

    /// 大端 8 字节编码。
    pub fn EncodeUint(mut b: Vec<u8>, v: u64) -> Vec<u8> {
        b.extend_from_slice(&v.to_be_bytes());
        b
    }

    /// 大端解码；长度不足报错。
    pub fn DecodeUint(b: &[u8]) -> Result<(&[u8], u64), String> {
        if b.len() < 8 {
            return Err("insufficient bytes to decode value".into());
        }
        let v = u64::from_be_bytes(b[0..8].try_into().unwrap());
        Ok((&b[8..], v))
    }

    /// 降序编码：存 !v，使较大 TS 排序更前。
    pub fn EncodeUintDesc(mut b: Vec<u8>, v: u64) -> Vec<u8> {
        b.extend_from_slice(&(!v).to_be_bytes());
        b
    }

    /// 降序解码：再取反还原。
    pub fn DecodeUintDesc(b: &[u8]) -> Result<(&[u8], u64), String> {
        if b.len() < 8 {
            return Err("insufficient bytes to decode value".into());
        }
        let v = u64::from_be_bytes(b[0..8].try_into().unwrap());
        Ok((&b[8..], !v))
    }

    // 解析 uvarint 前缀，返回 (值, 消费字节数)。
    fn decodeUvarintPrefix(b: &[u8]) -> Result<(u64, usize), String> {
        let mut value = 0u64;
        let mut shift = 0u32;
        for (index, byte) in b.iter().copied().enumerate() {
            if index == maxVarintLen64 - 1 && byte > 1 {
                return Err("value larger than 64 bits".into());
            }
            if byte < 0x80 {
                return Ok((value | u64::from(byte) << shift, index + 1));
            }
            value |= u64::from(byte & 0x7f) << shift;
            shift += 7;
        }
        Err("insufficient bytes to decode value".into())
    }

    /// 标准 protobuf 风格 uvarint 编码。
    pub fn EncodeUvarint(mut b: Vec<u8>, v: u64) -> Vec<u8> {
        let mut value = v;
        while value >= 0x80 {
            b.push(value as u8 | 0x80);
            value >>= 7;
        }
        b.push(value as u8);
        b
    }

    /// 解码 uvarint 并返回剩余切片。
    pub fn DecodeUvarint(b: &[u8]) -> Result<(&[u8], u64), String> {
        let (value, length) = decodeUvarintPrefix(b)?;
        Ok((&b[length..], value))
    }
}

// 表/meta 键编码桩：m 前缀 hash meta、t 前缀表 record。
pub mod tablecodec {
    use super::codec;

    // meta 键前缀。
    const META_PREFIX: &[u8] = b"m";
    const HASH_DATA: u64 = b'h' as u64;

    /// 编码 meta hash 数据键：m + enc(key) + h + enc(field)。
    pub fn EncodeMetaKey(key: &[u8], field: &[u8]) -> Vec<u8> {
        let mut ek = Vec::with_capacity(META_PREFIX.len() + key.len() + field.len() + 16);
        ek.extend_from_slice(META_PREFIX);
        ek = codec::EncodeBytes(ek, key);
        ek = codec::EncodeUint(ek, HASH_DATA);
        codec::EncodeBytes(ek, field)
    }

    /// 解码 meta 键，校验前缀与 hash 标志。
    pub fn DecodeMetaKey(ek: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        if !ek.starts_with(META_PREFIX) {
            return Err("invalid encoded hash data key prefix".into());
        }
        let (remain, key) = codec::DecodeBytes(&ek[META_PREFIX.len()..], None)?;
        let (remain, tp) = codec::DecodeUint(&remain)?;
        if tp != HASH_DATA {
            return Err(format!("invalid encoded hash data key flag {}", tp as u8));
        }
        let (_, field) = codec::DecodeBytes(&remain, None)?;
        Ok((key, field))
    }

    /// 表 record 前缀：EncodeTablePrefix + "_r"。
    pub fn GenTableRecordPrefix(table_id: i64) -> Vec<u8> {
        let mut key = EncodeTablePrefix(table_id);
        key.extend_from_slice(b"_r");
        key
    }

    /// `t` + 有序 int 编码的 table_id。
    pub fn EncodeTablePrefix(table_id: i64) -> Vec<u8> {
        let mut key = vec![b't'];
        encode_int(&mut key, table_id);
        key
    }

    // 符号位翻转后大端，保证有符号整数可比较。
    fn encode_int(buf: &mut Vec<u8>, v: i64) {
        let u = (v as u64) ^ (1u64 << 63);
        buf.extend_from_slice(&u.to_be_bytes());
    }

    /// 字典序下一前缀；全 0xff 时追加 0 字节。
    pub fn PrefixNext(key: &[u8]) -> Vec<u8> {
        let mut buf = key.to_vec();
        let mut i = key.len() as isize - 1;
        while i >= 0 {
            let idx = i as usize;
            buf[idx] = buf[idx].wrapping_add(1);
            if buf[idx] != 0 {
                break;
            }
            i -= 1;
        }
        if i == -1 {
            buf = key.to_vec();
            buf.push(0);
        }
        buf
    }
}

// TiDB meta 键名桩：DB/Table/IID/TID/SID/TARID 前缀解析。
// 字符串格式 `Prefix:id`，非真实二进制 meta codec。
pub mod meta {
    const M_DB_PREFIX: &str = "DB";
    const M_TABLE_PREFIX: &str = "Table";
    const M_INC_ID_PREFIX: &str = "IID";
    const M_TABLE_ID_PREFIX: &str = "TID";
    const M_SEQUENCE_PREFIX: &str = "SID";
    const M_RANDOM_ID_PREFIX: &str = "TARID";

    // 统一 `prefix:id` 编码。
    fn prefixed_key(prefix: &str, id: i64) -> Vec<u8> {
        format!("{prefix}:{id}").into_bytes()
    }

    // 校验前缀并解析十进制 id。
    fn parse_prefixed_key(key: &[u8], prefix: &str) -> Result<i64, String> {
        let s = std::str::from_utf8(key).map_err(|e| e.to_string())?;
        let expected = format!("{prefix}:");
        if !s.starts_with(&expected) {
            return Err(format!("fail to parse {prefix} key"));
        }
        s[expected.len()..]
            .parse::<i64>()
            .map_err(|e| e.to_string())
    }

    /// 编码 DB 键。
    pub fn DBkey(db_id: i64) -> Vec<u8> {
        prefixed_key(M_DB_PREFIX, db_id)
    }

    /// 解析 DB 键；前缀不对则失败。
    pub fn ParseDBKey(dbkey: &[u8]) -> Result<i64, String> {
        if !IsDBkey(dbkey) {
            return Err("fail to parse dbKey".into());
        }
        parse_prefixed_key(dbkey, M_DB_PREFIX)
    }

    /// 是否 DB: 前缀。
    pub fn IsDBkey(db_key: &[u8]) -> bool {
        db_key.starts_with(format!("{M_DB_PREFIX}:").as_bytes())
    }

    /// 编码 Table 键。
    pub fn TableKey(table_id: i64) -> Vec<u8> {
        prefixed_key(M_TABLE_PREFIX, table_id)
    }

    /// 解析 Table 键。
    pub fn ParseTableKey(table_key: &[u8]) -> Result<i64, String> {
        if !IsTableKey(table_key) {
            return Err("fail to parse tableKey".into());
        }
        parse_prefixed_key(table_key, M_TABLE_PREFIX)
    }

    /// 是否 Table: 前缀。
    pub fn IsTableKey(table_key: &[u8]) -> bool {
        table_key.starts_with(format!("{M_TABLE_PREFIX}:").as_bytes())
    }

    /// 自增 ID 键。
    pub fn AutoIncrementIDKey(table_id: i64) -> Vec<u8> {
        prefixed_key(M_INC_ID_PREFIX, table_id)
    }

    /// 是否 IID: 前缀。
    pub fn IsAutoIncrementIDKey(key: &[u8]) -> bool {
        key.starts_with(format!("{M_INC_ID_PREFIX}:").as_bytes())
    }

    /// 解析自增 ID 键。
    pub fn ParseAutoIncrementIDKey(key: &[u8]) -> Result<i64, String> {
        if !IsAutoIncrementIDKey(key) {
            return Err("fail to parse autoIncrementKey".into());
        }
        parse_prefixed_key(key, M_INC_ID_PREFIX)
    }

    /// Auto Table ID 键。
    pub fn AutoTableIDKey(table_id: i64) -> Vec<u8> {
        prefixed_key(M_TABLE_ID_PREFIX, table_id)
    }

    /// 是否 TID: 前缀。
    pub fn IsAutoTableIDKey(key: &[u8]) -> bool {
        key.starts_with(format!("{M_TABLE_ID_PREFIX}:").as_bytes())
    }

    /// 解析 Auto Table ID 键。
    pub fn ParseAutoTableIDKey(key: &[u8]) -> Result<i64, String> {
        if !IsAutoTableIDKey(key) {
            return Err("fail to parse autoTableKey".into());
        }
        parse_prefixed_key(key, M_TABLE_ID_PREFIX)
    }

    /// Sequence 键。
    pub fn SequenceKey(sequence_id: i64) -> Vec<u8> {
        prefixed_key(M_SEQUENCE_PREFIX, sequence_id)
    }

    /// 是否 SID: 前缀。
    pub fn IsSequenceKey(key: &[u8]) -> bool {
        key.starts_with(format!("{M_SEQUENCE_PREFIX}:").as_bytes())
    }

    /// 解析 Sequence 键。
    pub fn ParseSequenceKey(key: &[u8]) -> Result<i64, String> {
        if !IsSequenceKey(key) {
            return Err("fail to parse sequenceKey".into());
        }
        parse_prefixed_key(key, M_SEQUENCE_PREFIX)
    }

    /// AutoRandom 表 ID 键。
    pub fn AutoRandomTableIDKey(table_id: i64) -> Vec<u8> {
        prefixed_key(M_RANDOM_ID_PREFIX, table_id)
    }

    /// 是否 TARID: 前缀。
    pub fn IsAutoRandomTableIDKey(key: &[u8]) -> bool {
        key.starts_with(format!("{M_RANDOM_ID_PREFIX}:").as_bytes())
    }

    /// 解析 AutoRandom 表 ID 键。
    pub fn ParseAutoRandomTableIDKey(key: &[u8]) -> Result<i64, String> {
        if !IsAutoRandomTableIDKey(key) {
            return Err("fail to parse autoRandomTableKey".into());
        }
        parse_prefixed_key(key, M_RANDOM_ID_PREFIX)
    }
}

// meta txn 键与系统库名判断辅助。
pub mod utils {
    use super::{codec, meta, tablecodec};

    /// meta key + 降序 ts，模拟 txn KV 编码。
    pub fn EncodeTxnMetaKey(key: &[u8], field: &[u8], ts: u64) -> Vec<u8> {
        let k = tablecodec::EncodeMetaKey(key, field);
        let txn_key = codec::EncodeBytes(Vec::new(), &k);
        codec::EncodeUintDesc(txn_key, ts)
    }

    /// 粗判 meta DB 键：`mDB` 前缀（桩级简化）。
    pub fn IsMetaDBKey(key: &[u8]) -> bool {
        key.starts_with(b"mDB")
    }

    /// 系统库/临时库名，恢复时通常过滤。
    pub fn IsSysOrTempSysDB(name: &str) -> bool {
        matches!(
            name,
            "mysql" | "sys" | "INFORMATION_SCHEMA" | "PERFORMANCE_SCHEMA" | "__TiDB_TEMP_DB"
        )
    }
}

// DBInfo/TableInfo 等 JSON 模型桩，供 meta 值解析。
// 字段集为测试所需子集，非完整 TiDB model。
pub mod model {
    use super::TableSimpleInfo;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 大小写信息字符串：O 原始，L 小写。
    pub struct CIStr {
        #[serde(rename = "O")]
        pub O: String,
        #[serde(default, rename = "L")]
        pub L: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 库信息 JSON。
    pub struct DBInfo {
        #[serde(default)]
        pub ID: i64,
        #[serde(default)]
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 分区定义（仅 ID）。
    pub struct PartitionDefinition {
        #[serde(default)]
        pub ID: i64,
        #[serde(default)]
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 分区列表。
    pub struct PartitionInfo {
        #[serde(default)]
        pub Definitions: Vec<PartitionDefinition>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// TiFlash 副本计数桩。
    pub struct TiFlashReplicaInfo {
        #[serde(default)]
        pub Count: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// TTL 配置桩字段。
    pub struct TTLInfo {
        #[serde(default)]
        pub ColumnName: CIStr,
        #[serde(default)]
        pub IntervalExprStr: String,
        #[serde(default)]
        pub IntervalTimeUnit: i32,
        #[serde(default)]
        pub Enable: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 表信息 JSON 子集。
    pub struct TableInfo {
        #[serde(default)]
        pub ID: i64,
        #[serde(default)]
        pub Name: CIStr,
        #[serde(default)]
        pub Partition: Option<PartitionInfo>,
        #[serde(default)]
        pub TiFlashReplica: Option<TiFlashReplicaInfo>,
        #[serde(default)]
        pub TTLInfo: Option<TTLInfo>,
    }

    impl TableInfo {
        /// 取分区信息引用。
        pub fn GetPartitionInfo(&self) -> Option<&PartitionInfo> {
            self.Partition.as_ref()
        }
    }

    /// JSON → (tableId, TableSimpleInfo)；收集分区 ID。
    pub fn table_simple_from_value(value: &[u8]) -> Result<(i64, TableSimpleInfo), String> {
        let table_info: TableInfo = serde_json::from_slice(value).map_err(|e| e.to_string())?;
        let mut partition_ids = Vec::new();
        if let Some(partitions) = table_info.GetPartitionInfo() {
            for def in &partitions.Definitions {
                partition_ids.push(def.ID);
            }
        }
        Ok((
            table_info.ID,
            TableSimpleInfo {
                Name: table_info.Name.O,
                PartitionIds: partition_ids,
            },
        ))
    }

    /// JSON → 库原始名 O。
    pub fn db_name_from_value(value: &[u8]) -> Result<String, String> {
        let db_info: DBInfo = serde_json::from_slice(value).map_err(|e| e.to_string())?;
        Ok(db_info.Name.O)
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// GC delete range 参数桩。
    pub struct DelRangeArg {
        pub TableID: i64,
        pub ElemID: i64,
        pub StartKey: String,
        pub EndKey: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// DDL Job 桩（含 DelRange）。
    pub struct Job {
        pub ID: i64,
        pub NeedGC: bool,
        pub DelRangeArgs: Vec<DelRangeArg>,
    }
}

// backuppb 消息的 serde JSON 桩；非真正 protobuf 编解码。
// 字段命名保持 Go/proto 风格以便与测试向量对齐。
pub mod backuppb {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[repr(i32)]
    /// migration 格式版本。
    pub enum MigrationVersion {
        #[default]
        M0 = 0,
        M1 = 1,
        M2 = 2,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 上下游 ID 对。
    pub struct IDMap {
        pub UpstreamId: i64,
        pub DownstreamId: i64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// PITR 表级映射。
    pub struct PitrTableMap {
        pub Name: String,
        pub IdMap: IDMap,
        #[serde(default)]
        pub Partitions: Vec<IDMap>,
        #[serde(default)]
        pub FilteredOut: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// PITR 库级映射。
    pub struct PitrDBMap {
        pub Name: String,
        pub IdMap: IDMap,
        #[serde(default)]
        pub Tables: Vec<PitrTableMap>,
        #[serde(default)]
        pub FilteredOut: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 单个日志数据文件描述。
    pub struct DataFileInfo {
        pub Path: String,
        #[serde(default)]
        pub StartKey: Vec<u8>,
        #[serde(default)]
        pub EndKey: Vec<u8>,
        #[serde(default)]
        pub MinTs: u64,
        #[serde(default)]
        pub MaxTs: u64,
        #[serde(default)]
        pub ResolvedTs: u64,
        #[serde(default)]
        pub Length: u64,
        #[serde(default)]
        pub MinBeginTsInDefaultCf: u64,
        #[serde(default)]
        pub NumberOfEntries: i64,
        #[serde(default)]
        pub Cf: String,
        #[serde(default)]
        pub Sha256: Vec<u8>,
        #[serde(default)]
        pub IsMeta: bool,
    }

    impl DataFileInfo {
        /// 取校验和切片。
        pub fn GetSha256(&self) -> &[u8] {
            &self.Sha256
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[repr(i32)]
    /// Metadata 版本：V1 Files / V2 FileGroups。
    pub enum MetaVersion {
        #[default]
        V1 = 0,
        V2 = 1,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[repr(i32)]
    /// 压缩类型；由 MetadataHelper 执行真实 ZSTD 解码。
    pub enum CompressionType {
        #[default]
        UNKNOWN = 0,
        ZSTD = 1,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// V2 文件组。
    pub struct DataFileGroup {
        #[serde(default)]
        pub Path: String,
        #[serde(default)]
        pub MinTs: u64,
        #[serde(default)]
        pub MaxTs: u64,
        #[serde(default)]
        pub MinResolvedTs: u64,
        #[serde(default)]
        pub Length: u64,
        #[serde(default)]
        pub DataFilesInfo: Vec<DataFileInfo>,
    }

    impl DataFileGroup {
        /// 组路径。
        pub fn GetFileGroupsPath(&self) -> &str {
            &self.Path
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 流备份 meta 文件内容。
    pub struct Metadata {
        #[serde(default)]
        pub MinTs: u64,
        #[serde(default)]
        pub MaxTs: u64,
        #[serde(default)]
        pub ResolvedTs: u64,
        #[serde(default)]
        pub StoreId: i64,
        #[serde(default)]
        pub MetaVersion: MetaVersion,
        #[serde(default)]
        pub FileGroups: Vec<DataFileGroup>,
        #[serde(default)]
        pub Files: Vec<DataFileInfo>,
    }

    impl Metadata {
        /// 取 FileGroups。
        pub fn GetFileGroups(&self) -> &[DataFileGroup] {
            &self.FileGroups
        }
        /// JSON 序列化（桩，非 protobuf）。
        pub fn Marshal(&self) -> Result<Vec<u8>, String> {
            serde_json::to_vec(self).map_err(|e| format!("{e}"))
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 逻辑删除区间。
    pub struct DeleteSpan {
        #[serde(default)]
        pub Offset: u64,
        #[serde(default)]
        pub Length: u64,
    }

    impl DeleteSpan {
        /// 偏移。
        pub fn GetOffset(&self) -> u64 {
            self.Offset
        }
        /// 长度。
        pub fn GetLength(&self) -> u64 {
            self.Length
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 单文件上的逻辑删除跨度集合。
    pub struct DeleteSpansOfFile {
        pub Path: String,
        #[serde(default)]
        pub Spans: Vec<DeleteSpan>,
        #[serde(default)]
        pub WholeFileLength: u64,
    }

    impl DeleteSpansOfFile {
        /// 文件路径。
        pub fn GetPath(&self) -> &str {
            &self.Path
        }
        /// 跨度列表。
        pub fn GetSpans(&self) -> &[DeleteSpan] {
            &self.Spans
        }
        /// 原文件长度。
        pub fn GetWholeFileLength(&self) -> u64 {
            self.WholeFileLength
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// migration 对 meta 的编辑描述。
    pub struct MetaEdit {
        pub Path: String,
        #[serde(default)]
        pub DeletePhysicalFiles: Vec<String>,
        #[serde(default)]
        pub DeleteLogicalFiles: Vec<DeleteSpansOfFile>,
        #[serde(default)]
        pub DestructSelf: bool,
    }

    impl MetaEdit {
        pub fn GetPath(&self) -> &str {
            &self.Path
        }
        /// 物理删除文件列表。
        pub fn GetDeletePhysicalFiles(&self) -> &[String] {
            &self.DeletePhysicalFiles
        }
        /// 逻辑删除列表。
        pub fn GetDeleteLogicalFiles(&self) -> &[DeleteSpansOfFile] {
            &self.DeleteLogicalFiles
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 压缩产物引用（ArtifactsHash）。
    pub struct Compaction {
        #[serde(default)]
        pub ArtifactsHash: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 流备份 migration 记录。
    pub struct Migration {
        #[serde(default)]
        pub Version: MigrationVersion,
        #[serde(default)]
        pub Creator: String,
        #[serde(default)]
        pub EditMeta: Vec<MetaEdit>,
        #[serde(default)]
        pub Compactions: Vec<Compaction>,
        #[serde(default)]
        pub TruncatedTo: u64,
        #[serde(default)]
        pub DestructPrefix: Vec<String>,
        #[serde(default)]
        pub IngestedSstPaths: Vec<String>,
    }

    impl Migration {
        /// meta 编辑列表。
        pub fn GetEditMeta(&self) -> &[MetaEdit] {
            &self.EditMeta
        }
        /// 压缩列表。
        pub fn GetCompactions(&self) -> &[Compaction] {
            &self.Compactions
        }
        /// 截断到的 TS。
        pub fn GetTruncatedTo(&self) -> u64 {
            self.TruncatedTo
        }
        /// 待销毁前缀。
        pub fn GetDestructPrefix(&self) -> &[String] {
            &self.DestructPrefix
        }
        /// 已 ingest 的 SST 路径。
        pub fn GetIngestedSstPaths(&self) -> &[String] {
            &self.IngestedSstPaths
        }

        /// JSON 反序列化。
        pub fn Unmarshal(data: &[u8]) -> Result<Self, String> {
            serde_json::from_slice(data).map_err(|e| format!("{e}"))
        }

        pub fn Marshal(&self) -> Result<Vec<u8>, String> {
            serde_json::to_vec(self).map_err(|e| format!("{e}"))
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 流备份任务错误信息。
    pub struct StreamBackupError {
        pub ErrorCode: String,
        pub ErrorMessage: String,
        #[serde(default)]
        pub HappenAt: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 存储 URI 桩。
    pub struct StorageBackend {
        pub Uri: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 任务信息：名、TS、filter、storage。
    pub struct StreamBackupTaskInfo {
        pub Name: String,
        #[serde(default)]
        pub StartTs: u64,
        #[serde(default)]
        pub EndTs: u64,
        #[serde(default)]
        pub TableFilter: Vec<String>,
        #[serde(default)]
        pub Storage: Option<StorageBackend>,
    }

    impl StreamBackupTaskInfo {
        /// 任务名。
        pub fn GetName(&self) -> &str {
            &self.Name
        }
        /// 起始 TS。
        pub fn GetStartTs(&self) -> u64 {
            self.StartTs
        }
        /// 结束 TS。
        pub fn GetEndTs(&self) -> u64 {
            self.EndTs
        }
        /// 表过滤规则副本。
        pub fn GetTableFilter(&self) -> Vec<String> {
            self.TableFilter.clone()
        }
        /// 可选存储后端。
        pub fn GetStorage(&self) -> Option<&StorageBackend> {
            self.Storage.as_ref()
        }
    }
}

/// Object storage boundary for stream search / metadata loading.
/// 对象存储边界：读写/列举/删除；默认实现覆盖 Exists/批量删/Rename/WalkDir。
/// 真实实现应对接 S3/GCS/本地；此处由 MemStorage 满足单测。
pub trait Storage: Send + Sync {
    fn ReadFile(&self, path: &str) -> Result<Vec<u8>, String>;
    /// 写入/覆盖文件。
    fn WriteFile(&self, path: &str, data: &[u8]) -> Result<(), String>;
    fn ListFiles(&self, sub_dir: &str) -> Result<Vec<(String, i64)>, String>;
    /// 默认：ReadFile 成功即存在。
    fn FileExists(&self, path: &str) -> Result<bool, String> {
        Ok(self.ReadFile(path).is_ok())
    }
    /// 删除单文件。
    fn DeleteFile(&self, path: &str) -> Result<(), String>;
    fn DeleteFiles(&self, paths: &[String]) -> Result<(), String> {
        for p in paths {
            self.DeleteFile(p)?;
        }
        Ok(())
    }
    /// 读-写-删模拟 rename（非原子）。
    fn Rename(&self, from: &str, to: &str) -> Result<(), String> {
        let data = self.ReadFile(from)?;
        self.WriteFile(to, &data)?;
        self.DeleteFile(from)
    }
    /// 默认等同 ListFiles。
    fn WalkDir(&self, sub_dir: &str) -> Result<Vec<(String, i64)>, String> {
        self.ListFiles(sub_dir)
    }
}

#[derive(Default, Clone)]
/// 内存对象存储：HashMap 路径→字节，供单测。
pub struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemStorage {
    /// 空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 直接插入，跳过 WriteFile 校验。
    pub fn insert(&self, path: impl Into<String>, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.into(), data);
    }
}

// MemStorage 的 Storage 实现：路径前缀过滤列举。
impl Storage for MemStorage {
    fn ReadFile(&self, path: &str) -> Result<Vec<u8>, String> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("file not found: {path}"))
    }

    fn WriteFile(&self, path: &str, data: &[u8]) -> Result<(), String> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }

    /// 前缀匹配后按路径排序，保证测试稳定。
    fn ListFiles(&self, sub_dir: &str) -> Result<Vec<(String, i64)>, String> {
        let mut paths: Vec<_> = self
            .files
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.starts_with(sub_dir))
            .map(|(p, v)| (p.clone(), v.len() as i64))
            .collect();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(paths)
    }

    fn DeleteFile(&self, path: &str) -> Result<(), String> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }
}

// 日志桩：吞掉消息，避免测试依赖全局 logger。
pub mod log {
    pub fn Info(_msg: &str) {}
    /// 警告日志空实现。
    pub fn Warn(_msg: &str) {}
    pub fn Debug(_msg: &str) {}
}
