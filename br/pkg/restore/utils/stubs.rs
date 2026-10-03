// Copyright 2026 AsterSQL.
//! Local stand-ins for backuppb, import_sstpb, model, metautil, tablecodec, codec,
//! logutil, and util used by restore utils without pulling kvproto/grpcio.
//!
//! 本文件是 restore utils 的桩/适配边界：用本地精简类型与编码函数替代
//! kvproto/grpcio/tidb 真实依赖，使 rewrite_rule 等逻辑可在无集群环境下单测。
//! 约束：只覆盖本包测试与算法所需字段/行为；未实现的 RPC、存储与完整编解码
//! 不得被注释描述为已支持。与 Go 对应包语义对齐处（memcmp 有序编码、半开前缀）
//! 在各子模块注释中标明。

use std::fmt;

/// 备份元数据文件替身：对齐 backuppb.File / DataFileInfo 的键范围字段。
pub mod backuppb {
    use super::AppliedFile;

    // SST 文件描述；Cf/Crc 等字段供校验与日志，可不参与重写匹配。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct File {
        pub Name: String,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub TotalBytes: u64,
        /// Physical SST size; flow control falls back to TotalBytes for old backups.
        pub Size_: u64,
        pub TotalKvs: u64,
        pub Cf: String,
        pub Crc64Xor: u64,
    }

    impl File {
        // 克隆返回，模拟 protobuf getter 的值语义。
        pub fn GetStartKey(&self) -> Vec<u8> {
            self.StartKey.clone()
        }

        // EndKey 同样值拷贝，避免借用跨越调用方生命周期。
        pub fn GetEndKey(&self) -> Vec<u8> {
            self.EndKey.clone()
        }

        // 文件名仅用于错误信息与日志，不参与前缀匹配。
        pub fn GetName(&self) -> &str {
            &self.Name
        }
    }

    // 统一 AppliedFile 接口，便于 GetRewriteRawKeys 泛型调用。
    impl AppliedFile for File {
        fn GetStartKey(&self) -> Vec<u8> {
            File::GetStartKey(self)
        }

        fn GetEndKey(&self) -> Vec<u8> {
            File::GetEndKey(self)
        }
    }

    // 日志备份 DataFileInfo：路径 + 已编码键范围。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DataFileInfo {
        pub Path: String,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }

    impl AppliedFile for DataFileInfo {
        // 日志文件键通常已是 EncodeBytes 后的形态。
        fn GetStartKey(&self) -> Vec<u8> {
            self.StartKey.clone()
        }

        // 与 StartKey 成对；空 End 表示开放上界时由调用方解释。
        fn GetEndKey(&self) -> Vec<u8> {
            self.EndKey.clone()
        }
    }
}

/// import_sstpb.RewriteRule 替身：仅保留键前缀重写与时间戳过滤字段。
pub mod import_sstpb {
    // NewTimestamp / Ignore* 供 SetTimeRangeFilter 写入；匹配逻辑主要看前后缀。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RewriteRule {
        pub OldKeyPrefix: Vec<u8>,
        pub NewKeyPrefix: Vec<u8>,
        pub NewTimestamp: u64,
        pub IgnoreAfterTimestamp: u64,
        pub IgnoreBeforeTimestamp: u64,
    }

    impl RewriteRule {
        // 匹配侧旧前缀；starts_with 比较用。
        pub fn GetOldKeyPrefix(&self) -> Vec<u8> {
            self.OldKeyPrefix.clone()
        }

        // 替换后新前缀；长度可与旧前缀不同，调用方需注意后缀切片。
        pub fn GetNewKeyPrefix(&self) -> Vec<u8> {
            self.NewKeyPrefix.clone()
        }
    }
}

/// model 精简：表/分区/索引 ID 与 CIStr，供 GetRewriteRules* 构造映射。
pub mod model {
    // O 为原始大小写，L 为小写；索引按名匹配时用 L。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct CIStr {
        pub O: String,
        pub L: String,
    }

    impl CIStr {
        // 构造时立刻生成小写副本，避免热路径重复 to_lowercase。
        pub fn new(name: impl Into<String>) -> Self {
            let O = name.into();
            let L = O.to_lowercase();
            Self { O, L }
        }
    }

    // 分区定义：恢复时旧/新分区 ID 成对写入重写规则。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PartitionDefinition {
        pub ID: i64,
        pub Name: CIStr,
    }

    // 分区列表容器；None 表示非分区表。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PartitionInfo {
        pub Definitions: Vec<PartitionDefinition>,
    }

    // 索引元数据：细粒度模式下按名对齐新旧 index ID。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct IndexInfo {
        pub ID: i64,
        pub Name: CIStr,
    }

    // Partition/Indices 用于细粒度 record/index 规则；粗粒度只看 ID。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct TableInfo {
        pub ID: i64,
        pub Partition: Option<PartitionInfo>,
        pub Indices: Vec<IndexInfo>,
    }
}

/// metautil.Table 占位：本包算法路径几乎不触达，仅满足类型引用。
pub mod metautil {
    // 空结构体占位，避免牵出真实 metautil 依赖图。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Table;
}

/// tablecodec 精简：表/行/索引前缀编码，对齐 TiDB memcomparable 整数布局。
pub mod tablecodec {
    // TiDB 表键固定以 't' 开头。
    const TABLE_PREFIX: u8 = b't';
    // 行记录与索引区的分隔字节，与 tablecodec 常量一致。
    const RECORD_PREFIX_SEP: &[u8] = b"_r";
    const INDEX_PREFIX_SEP: &[u8] = b"_i";

    // 符号位翻转后大端：保证有序比较与 Go codec.EncodeInt 一致。
    fn encode_int(buf: &mut Vec<u8>, v: i64) {
        let u = (v as u64) ^ (1u64 << 63);
        buf.extend_from_slice(&u.to_be_bytes());
    }

    // `t` + table_id：粗粒度表前缀。
    pub fn EncodeTablePrefix(table_id: i64) -> Vec<u8> {
        let mut key = Vec::with_capacity(9);
        key.push(TABLE_PREFIX);
        encode_int(&mut key, table_id);
        key
    }

    // Go 别名：与 EncodeTablePrefix 同义。
    pub fn GenTablePrefix(table_id: i64) -> Vec<u8> {
        EncodeTablePrefix(table_id)
    }

    // `t{id}_r`：行记录前缀，细粒度重写用。
    pub fn GenTableRecordPrefix(table_id: i64) -> Vec<u8> {
        let mut key = Vec::with_capacity(11);
        key.push(TABLE_PREFIX);
        encode_int(&mut key, table_id);
        key.extend_from_slice(RECORD_PREFIX_SEP);
        key
    }

    // `t{id}_i`：索引区前缀（尚不含 index_id）。
    pub fn GenTableIndexPrefix(table_id: i64) -> Vec<u8> {
        let mut key = Vec::with_capacity(11);
        key.push(TABLE_PREFIX);
        encode_int(&mut key, table_id);
        key.extend_from_slice(INDEX_PREFIX_SEP);
        key
    }

    // `t{id}_i{idx}`：完整索引前缀。
    pub fn EncodeTableIndexPrefix(table_id: i64, idx_id: i64) -> Vec<u8> {
        let mut key = GenTableIndexPrefix(table_id);
        encode_int(&mut key, idx_id);
        key
    }

    // 非法前缀或长度不足时返回 0，与 Go 测试容错一致（非 panic）。
    // Go tablecodec 会在普通表键匹配失败后尝试剥离 API V2 的
    // 1-byte mode + 3-byte keyspace ID 前缀，再解析内部表键。
    pub fn DecodeTableID(mut key: &[u8]) -> i64 {
        const API_V2_KEYSPACE_PREFIX_LEN: usize = 4;
        const API_V2_RAW_MODE_PREFIX: u8 = b'r';
        const API_V2_TXN_MODE_PREFIX: u8 = b'x';

        if key.first() != Some(&TABLE_PREFIX) {
            if key.len() < API_V2_KEYSPACE_PREFIX_LEN
                || !matches!(key[0], API_V2_RAW_MODE_PREFIX | API_V2_TXN_MODE_PREFIX)
            {
                return 0;
            }
            key = &key[API_V2_KEYSPACE_PREFIX_LEN..];
        }
        if key.is_empty() || key[0] != TABLE_PREFIX {
            return 0;
        }
        let rest = &key[1..];
        if rest.len() < 8 {
            return 0;
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&rest[..8]);
        let u = u64::from_be_bytes(buf);
        (u ^ (1u64 << 63)) as i64
    }

    // 字典序下一前缀；全 0xff 进位则在末尾追加 0，对齐 kv.NextKey 习惯。
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

/// codec 精简：EncodeInt / EncodeBytes / DecodeBytes，供日志键编解码单测。
pub mod codec {
    // memcomparable：每组 8 字节 + 1 marker。
    const ENC_GROUP_SIZE: usize = 8;
    const ENC_MARKER: u8 = 0xff;
    // 填充字节必须为 0，解码时严格校验。
    const ENC_PAD: u8 = 0x0;

    // 忽略可选缓冲参数，始终返回新 Vec，行为对调用方足够。
    pub fn EncodeInt(_buf: Option<Vec<u8>>, v: i64) -> Vec<u8> {
        let u = (v as u64) ^ (1u64 << 63);
        u.to_be_bytes().to_vec()
    }

    // 按 8 字节分组 + marker；最后一组用 pad 标记真实长度。
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

    // 返回 (剩余输入, 解码值)；marker/padding 非法时 Err，对齐 Go 错误语义。
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
            if pad_count > ENC_GROUP_SIZE as u8 {
                return Err(format!("invalid marker byte, group bytes {group_bytes:?}"));
            }
            let real_group_size = ENC_GROUP_SIZE - pad_count as usize;
            out.extend_from_slice(&group[..real_group_size]);
            b = &b[ENC_GROUP_SIZE + 1..];
            // pad_count!=0 表示最后一组，校验填充字节后结束。
            if pad_count != 0 {
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
}

/// util.ProtoV1Clone：真实路径会做 protobuf 深拷贝；此处 clone 即足够。
pub mod util {
    use super::import_sstpb::RewriteRule;

    // 测试环境无需 protobuf wire 往返，结构体 clone 即可。
    pub fn ProtoV1Clone(rule: &RewriteRule) -> RewriteRule {
        rule.clone()
    }
}

/// 密钥脱敏：hex 展示，避免日志明文。
pub mod redact {
    // 小写 hex，无 0x 前缀，便于与 Go redact.Key 字符串形态对照。
    pub fn Key(key: &[u8]) -> String {
        key.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// logutil 字段包装：真实实现写 zap Field；此处仅占位类型供签名对齐。
pub mod logutil {
    use super::backuppb::File;
    use super::import_sstpb::RewriteRule;

    pub struct KeyField<'a>(pub &'static str, pub &'a [u8]);
    pub struct FileField<'a>(pub &'a File);
    pub struct RewriteRuleField<'a>(pub &'a RewriteRule);

    // 构造命名键字段；真实路径会转成 zap.Field。
    pub fn Key<'a>(name: &'static str, key: &'a [u8]) -> KeyField<'a> {
        KeyField(name, key)
    }

    // 包装备份文件，供错误上下文附带文件名/范围。
    pub fn File(file: &File) -> FileField<'_> {
        FileField(file)
    }

    // 包装规则，便于日志打印 old/new 前缀。
    pub fn RewriteRule(rule: &RewriteRule) -> RewriteRuleField<'_> {
        RewriteRuleField(rule)
    }
}

/// 日志桩：Panic 真正 panic；其余级别空操作，避免测试依赖全局 logger。
pub mod log {
    // 对齐 Go log.Panic：不可返回，测试中用于不可达分支。
    pub fn Panic(_msg: &str) -> ! {
        panic!("log.Panic invoked")
    }

    // 以下级别故意空实现，避免单测初始化全局 logger。
    pub fn Error(_msg: &str) {}

    pub fn Warn(_msg: &str) {}

    pub fn Debug(_msg: &str) {}
}

/// 可被重写键范围的文件抽象：SST File 与日志 DataFileInfo 共用。
pub trait AppliedFile {
    // 文件覆盖范围下界（含）。
    fn GetStartKey(&self) -> Vec<u8>;
    // 文件覆盖范围上界（通常半开，由调用约定解释）。
    fn GetEndKey(&self) -> Vec<u8>;
}

// Display 用 redact 输出前后缀，便于断言与调试日志。
impl fmt::Display for import_sstpb::RewriteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{old:{}, new:{}}}",
            redact::Key(&self.OldKeyPrefix),
            redact::Key(&self.NewKeyPrefix)
        )
    }
}
