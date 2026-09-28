// Copyright 2026 AsterSQL.
//! Local stand-ins for kvproto / kv / util-redact used by logutil without
//! pulling kvproto/grpcio on darwin arm64.
//!
//! 本地桩：在无完整 kvproto/grpcio 的环境下支撑 logutil 编译与测试。
//! 包含 redact（NeedRedact/Key/Value）、kv.KeyRange，以及 brpb/import_sstpb/metapb 子集。
//! 这些类型仅模拟 getter/setter 形状，不是完整 protobuf 实现；勿当作生产编解码。
//! 脱敏开关为进程内 AtomicBool，仅测试可切换。

use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// 脱敏桩：对齐 util/redact 的 NeedRedact/Key/Value 行为子集。
// redact stand-ins (astersql-util-redact Key/Value/NeedRedact)
// ---------------------------------------------------------------------------

// 全局脱敏开关；默认关闭，避免影响普通日志测试。
static NEED_REDACT: AtomicBool = AtomicBool::new(false);

/// 测试专用：切换脱敏模式（SeqCst 保证跨线程可见）。
/// Test helper: toggle redact mode used by [`NeedRedact`].
pub fn set_need_redact_for_test(on: bool) {
    NEED_REDACT.store(on, Ordering::SeqCst);
}

/// 当前是否需要脱敏；logging 的 Redact/RedactAny 会查询它。
pub fn NeedRedact() -> bool {
    NEED_REDACT.load(Ordering::SeqCst)
}

/// 字符串值脱敏：开启时返回 `?`，否则原样。
pub fn Value(arg: &str) -> String {
    // 开启脱敏：掩码为问号。
    if NeedRedact() {
        "?".to_string()
    } else {
        arg.to_string()
    }
}

/// 键脱敏：开启时 `?`，否则大写十六进制（注意与 logging hex 小写不同）。
pub fn Key(key: &[u8]) -> String {
    // 键路径同样掩码。
    if NeedRedact() {
        return "?".to_string();
    }
    // 预分配 2*len，逐字节写两位大写 hex。
    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key {
        encoded.push_str(&format!("{byte:02X}"));
    }
    encoded
}

// ---------------------------------------------------------------------------
// kv 键范围桩：仅承载字节，供 StringifyRange 等转换。
// kv.Key / kv.KeyRange stand-ins
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
/// 字节键包装，对齐 tidb/pkg/kv.Key 的最小形态。
pub struct KvKey(pub Vec<u8>);

// 方便按切片传递给 RedactKey/hex。
impl AsRef<[u8]> for KvKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 半开区间 [StartKey, EndKey)，字段名保持 Go 风格。
pub struct KeyRange {
    pub StartKey: KvKey,
    pub EndKey: KvKey,
}

// ---------------------------------------------------------------------------
// kvproto 子集桩：字段私有 + get_/set_，形状贴近 protobuf 生成代码。
// kvproto stand-ins (protobuf-codec shaped getters/setters)
// ---------------------------------------------------------------------------

/// 命名空间对齐 `kvproto::...`，供 logging 与测试 `use crate::kvproto`。
pub mod kvproto {
    /// backuppb 子集：File 与 StreamBackupTaskInfo。
    pub mod brpb {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// 备份文件元数据桩；字段覆盖 logging::FileMarshaler 所需。
        pub struct File {
            // 文件名。
            name: String,
            // Column Family。
            cf: String,
            // 内容校验。
            sha256: Vec<u8>,
            // 覆盖范围起点。
            start_key: Vec<u8>,
            // 覆盖范围终点。
            end_key: Vec<u8>,
            // 起始版本。
            start_version: u64,
            // 结束版本。
            end_version: u64,
            // KV 条数。
            total_kvs: u64,
            // KV 字节合计。
            total_bytes: u64,
            // CRC64 xor。
            crc64xor: u64,
            // 文件大小。
            size: u64,
        }

        impl File {
            /// 默认空 File。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读文件名。
            pub fn get_name(&self) -> &str {
                &self.name
            }
            /// 写文件名。
            pub fn set_name(&mut self, v: String) {
                self.name = v;
            }
            /// 读 CF。
            pub fn get_cf(&self) -> &str {
                &self.cf
            }
            /// 写 CF。
            pub fn set_cf(&mut self, v: String) {
                self.cf = v;
            }
            /// 读 sha256 字节。
            pub fn get_sha256(&self) -> &[u8] {
                &self.sha256
            }
            /// 写 sha256。
            pub fn set_sha256(&mut self, v: Vec<u8>) {
                self.sha256 = v;
            }
            /// 读 start_key。
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            /// 写 start_key。
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            /// 读 end_key。
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            /// 写 end_key。
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
            /// 读 start_version。
            pub fn get_start_version(&self) -> u64 {
                self.start_version
            }
            /// 写 start_version。
            pub fn set_start_version(&mut self, v: u64) {
                self.start_version = v;
            }
            /// 读 end_version。
            pub fn get_end_version(&self) -> u64 {
                self.end_version
            }
            /// 写 end_version。
            pub fn set_end_version(&mut self, v: u64) {
                self.end_version = v;
            }
            /// 读 total_kvs。
            pub fn get_total_kvs(&self) -> u64 {
                self.total_kvs
            }
            /// 写 total_kvs。
            pub fn set_total_kvs(&mut self, v: u64) {
                self.total_kvs = v;
            }
            /// 读 total_bytes。
            pub fn get_total_bytes(&self) -> u64 {
                self.total_bytes
            }
            /// 写 total_bytes。
            pub fn set_total_bytes(&mut self, v: u64) {
                self.total_bytes = v;
            }
            /// 读 crc64xor。
            pub fn get_crc64xor(&self) -> u64 {
                self.crc64xor
            }
            /// 写 crc64xor。
            pub fn set_crc64xor(&mut self, v: u64) {
                self.crc64xor = v;
            }
            /// 读 size。
            pub fn get_size(&self) -> u64 {
                self.size
            }
            /// 写 size。
            pub fn set_size(&mut self, v: u64) {
                self.size = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// 日志备份任务信息桩，供 StreamBackupTaskInfo 字段编码。
        pub struct StreamBackupTaskInfo {
            // 任务名。
            name: String,
            // 起始 TS。
            start_ts: u64,
            // 结束 TS。
            end_ts: u64,
            // 表过滤表达式列表。
            table_filter: Vec<String>,
        }

        impl StreamBackupTaskInfo {
            /// 默认空任务信息。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读任务名。
            pub fn get_name(&self) -> &str {
                &self.name
            }
            /// 写任务名。
            pub fn set_name(&mut self, v: String) {
                self.name = v;
            }
            /// 读 start_ts。
            pub fn get_start_ts(&self) -> u64 {
                self.start_ts
            }
            /// 写 start_ts。
            pub fn set_start_ts(&mut self, v: u64) {
                self.start_ts = v;
            }
            /// 读 end_ts。
            pub fn get_end_ts(&self) -> u64 {
                self.end_ts
            }
            /// 写 end_ts。
            pub fn set_end_ts(&mut self, v: u64) {
                self.end_ts = v;
            }
            /// 读表过滤列表。
            pub fn get_table_filter(&self) -> &[String] {
                &self.table_filter
            }
            /// 整表替换过滤列表。
            pub fn set_table_filter(&mut self, v: Vec<String>) {
                self.table_filter = v;
            }
            /// 可变借用过滤列表以便 push。
            pub fn mut_table_filter(&mut self) -> &mut Vec<String> {
                &mut self.table_filter
            }
        }
    }

    /// import_sstpb 子集：Range / RewriteRule / SstMeta。
    pub mod import_sstpb {
        use super::metapb::RegionEpoch;

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// SST 键范围桩。
        pub struct Range {
            // 范围起点。
            start: Vec<u8>,
            // 范围终点。
            end: Vec<u8>,
        }

        impl Range {
            pub fn new() -> Self {
                Self::default()
            }
            /// 读 Range.start。
            pub fn get_start(&self) -> &[u8] {
                &self.start
            }
            /// 写 Range.start。
            pub fn set_start(&mut self, v: Vec<u8>) {
                self.start = v;
            }
            /// 读 Range.end。
            pub fn get_end(&self) -> &[u8] {
                &self.end
            }
            /// 写 Range.end。
            pub fn set_end(&mut self, v: Vec<u8>) {
                self.end = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// 键前缀重写规则桩；前缀以原始字节保存，日志侧再 hex。
        pub struct RewriteRule {
            // 旧 key 前缀。
            old_key_prefix: Vec<u8>,
            // 新 key 前缀。
            new_key_prefix: Vec<u8>,
            // 重写后时间戳。
            new_timestamp: u64,
        }

        impl RewriteRule {
            pub fn new() -> Self {
                Self::default()
            }
            /// 读旧前缀。
            pub fn get_old_key_prefix(&self) -> &[u8] {
                &self.old_key_prefix
            }
            /// 写旧前缀。
            pub fn set_old_key_prefix(&mut self, v: Vec<u8>) {
                self.old_key_prefix = v;
            }
            /// 读新前缀。
            pub fn get_new_key_prefix(&self) -> &[u8] {
                &self.new_key_prefix
            }
            /// 写新前缀。
            pub fn set_new_key_prefix(&mut self, v: Vec<u8>) {
                self.new_key_prefix = v;
            }
            /// 读新时间戳。
            pub fn get_new_timestamp(&self) -> u64 {
                self.new_timestamp
            }
            /// 写新时间戳。
            pub fn set_new_timestamp(&mut self, v: u64) {
                self.new_timestamp = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// SST 元数据桩：覆盖 BriefSSTMetas/SSTMeta 编码所需字段。
        pub struct SstMeta {
            // CF 名。
            cf_name: String,
            // 右端是否排他。
            end_key_exclusive: bool,
            // CRC32。
            crc32: u32,
            length: u64,
            // 所属 Region。
            region_id: u64,
            // Region epoch 快照。
            region_epoch: RegionEpoch,
            // 键范围。
            range: Range,
            // SST UUID 原始字节。
            uuid: Vec<u8>,
            // SST 内 KV 数。
            total_kvs: u64,
            // SST 内 KV 字节。
            total_bytes: u64,
        }

        impl SstMeta {
            pub fn new() -> Self {
                Self::default()
            }
            /// 读 CF 名。
            pub fn get_cf_name(&self) -> &str {
                &self.cf_name
            }
            /// 写 CF 名。
            pub fn set_cf_name(&mut self, v: String) {
                self.cf_name = v;
            }
            /// 读右开标记。
            pub fn get_end_key_exclusive(&self) -> bool {
                self.end_key_exclusive
            }
            /// 写右开标记。
            pub fn set_end_key_exclusive(&mut self, v: bool) {
                self.end_key_exclusive = v;
            }
            /// 读 CRC32。
            pub fn get_crc32(&self) -> u32 {
                self.crc32
            }
            /// 写 CRC32。
            pub fn set_crc32(&mut self, v: u32) {
                self.crc32 = v;
            }
            /// 读 length。
            pub fn get_length(&self) -> u64 {
                self.length
            }
            /// 写 length。
            pub fn set_length(&mut self, v: u64) {
                self.length = v;
            }
            /// 读 region_id。
            pub fn get_region_id(&self) -> u64 {
                self.region_id
            }
            /// 写 region_id。
            pub fn set_region_id(&mut self, v: u64) {
                self.region_id = v;
            }
            /// 读 region_epoch。
            pub fn get_region_epoch(&self) -> &RegionEpoch {
                &self.region_epoch
            }
            /// 写 region_epoch。
            pub fn set_region_epoch(&mut self, v: RegionEpoch) {
                self.region_epoch = v;
            }
            /// 读 range。
            pub fn get_range(&self) -> &Range {
                &self.range
            }
            /// 写 range。
            pub fn set_range(&mut self, v: Range) {
                self.range = v;
            }
            /// 可变借用 range。
            pub fn mut_range(&mut self) -> &mut Range {
                &mut self.range
            }
            /// 读 uuid 字节。
            pub fn get_uuid(&self) -> &[u8] {
                &self.uuid
            }
            /// 写 uuid。
            pub fn set_uuid(&mut self, v: Vec<u8>) {
                self.uuid = v;
            }
            /// 读 SstMeta.total_kvs。
            pub fn get_total_kvs(&self) -> u64 {
                self.total_kvs
            }
            /// 写 SstMeta.total_kvs。
            pub fn set_total_kvs(&mut self, v: u64) {
                self.total_kvs = v;
            }
            /// 读 SstMeta.total_bytes。
            pub fn get_total_bytes(&self) -> u64 {
                self.total_bytes
            }
            /// 写 SstMeta.total_bytes。
            pub fn set_total_bytes(&mut self, v: u64) {
                self.total_bytes = v;
            }
        }
    }

    /// metapb 子集：RegionEpoch / Peer / Region。
    pub mod metapb {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// Region 版本对：conf_ver + version。
        pub struct RegionEpoch {
            // 配置版本。
            conf_ver: u64,
            // 数据版本。
            version: u64,
        }

        impl RegionEpoch {
            pub fn new() -> Self {
                Self::default()
            }
            /// 读 conf_ver。
            pub fn get_conf_ver(&self) -> u64 {
                self.conf_ver
            }
            /// 写 conf_ver。
            pub fn set_conf_ver(&mut self, v: u64) {
                self.conf_ver = v;
            }
            /// 读 version。
            pub fn get_version(&self) -> u64 {
                self.version
            }
            /// 写 version。
            pub fn set_version(&mut self, v: u64) {
                self.version = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// Raft peer：id + store_id。
        pub struct Peer {
            id: u64,
            // 所在 store。
            store_id: u64,
        }

        impl Peer {
            pub fn new() -> Self {
                Self::default()
            }
            /// 读 peer id。
            pub fn get_id(&self) -> u64 {
                self.id
            }
            pub fn set_id(&mut self, v: u64) {
                self.id = v;
            }
            /// 读 store_id。
            pub fn get_store_id(&self) -> u64 {
                self.store_id
            }
            /// 写 store_id。
            pub fn set_store_id(&mut self, v: u64) {
                self.store_id = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// Region 桩：范围、epoch 与 peers 列表。
        pub struct Region {
            // peer id。
            id: u64,
            // Region 起点。
            start_key: Vec<u8>,
            // Region 终点。
            end_key: Vec<u8>,
            // 当前 epoch。
            region_epoch: RegionEpoch,
            // peer 列表。
            peers: Vec<Peer>,
        }

        impl Region {
            pub fn new() -> Self {
                Self::default()
            }
            /// 读 Region id。
            pub fn get_id(&self) -> u64 {
                self.id
            }
            /// 写 peer id。
            pub fn set_id(&mut self, v: u64) {
                self.id = v;
            }
            /// 读 start_key。
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            /// 写 start_key。
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            /// 读 end_key。
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            /// 写 end_key。
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
            /// 读 epoch。
            pub fn get_region_epoch(&self) -> &RegionEpoch {
                &self.region_epoch
            }
            /// 写 epoch。
            pub fn set_region_epoch(&mut self, v: RegionEpoch) {
                self.region_epoch = v;
            }
            /// 读 peers。
            pub fn get_peers(&self) -> &[Peer] {
                &self.peers
            }
            /// 可变借用 peers 以便 push。
            pub fn mut_peers(&mut self) -> &mut Vec<Peer> {
                &mut self.peers
            }
            /// 整表替换 peers。
            pub fn set_peers(&mut self, v: Vec<Peer>) {
                self.peers = v;
            }
        }
    }
}
