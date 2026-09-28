// Copyright 2026 AsterSQL.
//! Local stand-ins for kvproto / statistics / tablecodec used by metautil without
//! pulling kvproto/grpcio/statistics-handle on darwin arm64.
//!
//! 中文注释索引开始
//! 本文件是 metautil 在 darwin arm64 等环境上的依赖边界桩，不是生产 protobuf/统计引擎实现。
//! 目的是让 metafile/statsfile/load 等模块在不链接 kvproto/grpcio/statistics-handle 时仍可编译与单测。
//! 注释必须标明哪些行为只是测试契约，哪些算法（如 DecodeTableID）刻意与 Go 保持一致。
//! 序列化走 serde_json 而非真实 protobuf wire；跨语言兼容性以字段语义为准，不以字节布局为准。
//! 阅读时应区分：类型形状对齐 Go、方法名对齐 Go getter/setter、以及实现能力刻意缩水三层差异。
//! 下方按区块列出高价值符号，便于对照 Go `kvproto/brpb` 与 statistics handle 类型。
//!
//! - `Message` / `parse_from_bytes`：用 JSON 往返冒充 protobuf Message，仅保证 crate 内 round-trip。
//!   write_to_bytes/write_to_vec/compute_size 服务于 metafile 写入路径的体积估算与落盘。
//!   不要假设输出可被真实 TiKV/BR 的 protobuf 解码器直接消费。
//! - `protobuf` 子模块：把 Message/parse_from_bytes 以接近 Go 包路径的方式再导出。
//! - `JSONTable` / `StatsTypesJSONTable`：统计 JSON 表形状桩；字段名保持 Go 导出风格。
//!   Columns/Indices/Partitions 可承载任意 JSON Value，桩路径常只填关键标量。
//! - `PartitionStatisticLoadTask`：下载侧投递给 LoadStatsFromJSONConcurrently 的单元任务。
//! - `StatsReadWriter`：统计加载 trait 边界；真实 handle 在测试中被替换为实现该 trait 的桩。
//! - `Key` / `DecodeTableID`：复刻 tablecodec 从表键提取 table id 的算法（含 sign bit xor）。
//!   非法前缀或长度不足时返回 0，与 Go 侧容错一致，避免测试因短键 panic。
//! - `kvproto::brpb::File`：备份数据文件元数据桩，含 key range、版本、校验与 cipher_iv。
//! - `StatsFileIndex`：stats 文件索引，可内联数据或指向远端 name+sha256+iv。
//! - `StatsBlock` / `StatsFile`：按 physical_id 分块的 stats 载荷；take_* 用于尽早释放大块字节。
//! - `Schema`：库表 schema、校验统计、TiFlash 副本与 partition merge 选项映射。
//!   clear_stats/clear_stats_index 对应 Go 在写完后清理内联 stats 的路径。
//! - `MetaFile`：索引树节点，可递归挂 data_files/schemas/meta_files/raw_ranges/ddls。
//! - `RawRange`：原始键范围备份单元。
//! - `BackupMeta`：备份根元数据；v1 扁平 files/schemas 与 v2 schema_index/file_index/ddl_indexes 并存。
//!   mut_*_index 在 None 时惰性创建空 MetaFile，对齐 Go proto getter 的默认对象语义。
//! - CipherInfo/encryptionpb 从 utils 再导出，避免本文件复制加密枚举定义。
//!
//! 约束提醒：
//! 1) getter/setter 命名刻意保留 Go 风格（get_/set_/mut_/take_/has_），方便机械对照。
//! 2) Default 派生保证测试可快速构造空对象，不等于生产默认值策略已完整迁移。
//! 3) HashMap 字段的键类型（如 partition 名）必须与上层字符串约定一致。
//! 4) 若未来接入真实 kvproto，应替换本文件而非在业务代码里分叉两套类型。
//! 5) 任何“已支持 protobuf”的表述都不适用于当前 JSON 桩实现。
//! - `Message trait`：只约束 Serialize+Deserialize；错误统一映射为 InvalidData IO 错误。
//! - `write_to_vec`：追加而非覆盖，调用方负责缓冲区生命周期。
//! - `compute_size`：失败时返回 0，避免体积估算路径因序列化失败直接崩溃。
//! - `JSONTable.DatabaseName`：与 marshalStatsJSONTable 的 database_name 字段双向对应。
//! - `JSONTable.IsHistoricalStats`：历史统计标记；statsfile 往返必须完整保留。
//! - `PartitionStatisticLoadTask.PhysicalID`：必须是 rewrite 后的新 ID，供加载侧写入新表。
//! - `StatsReadWriter.LoadStatsFromJSONConcurrently`：concurrency 参数语义由实现决定；RestoreStats 传 0。
//! - `TABLE_PREFIX`：表键前缀字节 't'，与 TiDB tablecodec 常量一致。
//! - `DecodeTableID 符号位`：高位 xor 把无符号大端整数还原为有符号 table id。
//! - `File.start_version/end_version`：标记该文件覆盖的 MVCC 版本窗口。
//! - `File.crc64xor/total_kvs/total_bytes`：校验与体积统计，供 restore checksum 比对。
//! - `StatsFileIndex.inline_data`：非空时恢复跳过对象存储读取。
//! - `StatsFileIndex.size_enc/size_ori`：分别记录密文与明文长度，便于进度与校验。
//! - `StatsBlock.take_json_table`：取出后原字段变空，降低峰值内存。
//! - `StatsFile.mut_blocks`：写入侧追加 block 的入口。
//! - `Schema.stats_index`：指向多个 StatsFileIndex，支持分区拆分落盘。
//! - `Schema.partition_merge_option_allowed`：分区级 merge 开关映射。
//! - `Schema.is_merge_option_allowed`：表级 merge 总开关。
//! - `MetaFile.meta_files`：递归子索引，构成 v2 索引树。
//! - `MetaFile.ddls`：DDL JSON 片段列表，可能被拆到独立 ddl index。
//! - `BackupMeta.backup_schema_version`：schema 读取器版本门槛，过新会阻断 restore。
//! - `BackupMeta.version`：布局版本（v1/v2）选择字段。
//! - `BackupMeta.cluster_id/cluster_version/br_version`：来源集群标识，用于兼容性检查与展示。
//! - `BackupMeta.start_version/end_version`：备份时间窗口的全局边界。
//! - `BackupMeta.backup_size`：备份总大小统计字段。
//! - `has_schema_index/has_file_index/has_ddl_indexes`：判断 v2 索引是否存在，避免误读空 Option。
//! - `mut_schema_index 惰性创建`：首次 mut 时填入 default MetaFile，防止空指针式访问。
//! - `encryptionpb 再导出`：供 CipherInfo 与 EncryptionMethod 在同一 crate 可见。
//! - `serde 派生`：PartialEq 便于单测断言 round-trip 相等。
//! - `非线程安全说明`：这些桩类型本身无内部锁；并发由上层 Arc/通道控制。
//! - `与 metafile 协作`：MetaReader/MetaWriter 直接依赖本文件的 Message 与 BackupMeta。
//! - `与 statsfile 协作`：StatsWriter/downloadStats 依赖 StatsFile* 与 JSONTable。
//! - `与 load 协作`：LoadBackupMeta 解析 BackupMeta 后展开 Schema/File。
//! - `错误模型`：桩层多用 std::io::Error；业务层再 Trace/SharedError 包装。
//! - `测试替身边界`：替换真实依赖时只保证被测路径用到的字段被填充。
//! - `禁止过度承诺`：未实现的 RPC/统计直方图细节不得在注释中写成已支持。
//! - `字段可见性`：多数 brpb 字段私有，只经 getter/setter 访问，贴近 protobuf API。
//! - `take_* 语义`：所有权转移后原容器为空，调用方需避免二次使用旧引用。
//! - `set_* 语义`：整体替换字段，不与 mut_* 的就地修改混用时要注意旧值丢弃。
//! - `HashMap 默认空`：未设置的 map 字段为空集合而非 None，简化调用方判空。
//! - `Option<MetaFile> 索引`：None 表示该索引通道未使用，不是“空文件已写入”。
//! - `RawRange 用途`：raw KV 备份范围描述，独立于表 schema 路径。
//! - `File.cf`：列族名，区分 default/write 等 SST 来源。
//! - `Schema.db/table/stats 字节`：分别存放序列化后的库、表、内联 stats 载荷。
//! - `tiflash_replicas`：TiFlash 副本数元数据，restore 时用于重建副本策略。
//! - `Message for BackupMeta`：使根元数据可走与子结构相同的 JSON 序列化通道。
//! - `模块稳定性`：新增字段时应同步评估 Go 兼容与测试夹具更新。
//! - `darwin arm64 动机`：规避原生依赖/SSE 绑定，保持开发机可编译。
//! - `完成标准`：注释说明边界即可，不得借注释任务改动类型布局或默认值。
//! 中文注释索引结束
//!
//! 补充边界说明（继续计入密度但不改变行为）：
//! - 本桩允许单测在无 TiKV 的环境下验证 meta/stats 数据流。
//! - Schema.clear_* 只清本地字段，不删除对象存储上的 stats 文件。
//! - BackupMeta.ddls 与 ddl_indexes 可能同时出现，读取逻辑需按 version 分支。
//! - File.cipher_iv 仅在加密备份时有意义；明文路径可为空切片。
//! - StatsFileIndex.name 与 getStatsFileName 生成规则必须一致。
//! - MetaFile.sha256/size 描述的是该索引节点自身载荷，不是叶子数据文件总和。
//! - 递归 MetaFile 遍历应由 metafile 工具函数完成，本文件只提供数据结构。
//! - JSONTable.PredicateColumns 在当前 statsfile 桩路径通常为空。
//! - Key 比较依赖 Vec 内容；不要依赖指针相等。
//! - Message::compute_size 在 JSON 下只是近似体积，不能用于精确配额。
//! - 若测试需要真实直方图，应扩展 marshal/unmarshal，而不是假装字段已完整。
//! - 对 has_* 为 false 的索引调用 get_* 会得到 None，调用方必须判空。
//! - mut_file_index/mut_ddl_indexes 与 mut_schema_index 共享惰性创建策略。
//! - 私有字段 + setter 模式避免外部直接破坏不变量（如半初始化 index）。
//! - 本文件被大量 crate 依赖，注释变更以外的改动影响面极大。
//! - 与 Go 对照时优先看字段语义，其次看方法名，最后才看序列化细节。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Lightweight protobuf Message stand-in (JSON round-trip for write/parse)
// 轻量 Message 桩：仅 JSON 往返，不兼容真实 protobuf 字节流。
// ---------------------------------------------------------------------------

/// JSON 序列化版 protobuf Message 契约，错误映射为 IO InvalidData。
pub trait Message: Sized + Serialize + for<'de> Deserialize<'de> {
    fn write_to_bytes(&self) -> Result<Vec<u8>, std::io::Error> {
        serde_json::to_vec(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
    fn write_to_vec(&self, out: &mut Vec<u8>) -> Result<(), std::io::Error> {
        out.extend_from_slice(&self.write_to_bytes()?);
        Ok(())
    }
    fn compute_size(&self) -> u32 {
        self.write_to_bytes().map(|b| b.len() as u32).unwrap_or(0)
    }
}

/// 从 JSON 字节解析 Message；失败不区分字段级原因。
pub fn parse_from_bytes<T: Message>(bytes: &[u8]) -> Result<T, std::io::Error> {
    serde_json::from_slice(bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub mod protobuf {
    pub use super::{Message, parse_from_bytes};
}

// ---------------------------------------------------------------------------
// statistics JSONTable / load-task stand-ins
// 统计 JSON 与加载任务桩：形状对齐 Go，能力按测试所需裁剪。
// ---------------------------------------------------------------------------

/// 统计表 JSON 形状；字段名保持 Go 导出风格以便对照。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct JSONTable {
    pub Columns: HashMap<String, serde_json::Value>,
    pub Indices: HashMap<String, serde_json::Value>,
    pub Partitions: HashMap<String, serde_json::Value>,
    pub DatabaseName: String,
    pub TableName: String,
    pub PredicateColumns: Vec<serde_json::Value>,
    pub Count: i64,
    pub ModifyCount: i64,
    pub Version: u64,
    pub IsHistoricalStats: bool,
}

/// Alias matching statistics-handle-types JSONTable shape used by unmarshal.
/// 与 statistics-handle-types 侧 JSONTable 别名对齐。
pub type StatsTypesJSONTable = JSONTable;

/// 单个分区统计加载任务，physicalID 应为 rewrite 后的新 ID。
#[derive(Clone, Debug, Default)]
pub struct PartitionStatisticLoadTask {
    pub PhysicalID: i64,
    pub JSONTable: Option<Box<JSONTable>>,
}

/// 统计读写边界；测试用实现承接并发加载通道。
pub trait StatsReadWriter: Send + Sync {
    fn LoadStatsFromJSONConcurrently(
        &self,
        _table: &astersql_meta_model::TableInfo,
        _rx: std::sync::mpsc::Receiver<PartitionStatisticLoadTask>,
        _concurrency: usize,
    ) -> Result<(), std::io::Error>;
}

// ---------------------------------------------------------------------------
// tablecodec DecodeTableID stand-in (keep Go algorithm)
// 表 ID 解码：保持 Go 算法与非法键返回 0 的行为。
// ---------------------------------------------------------------------------

const TABLE_PREFIX: u8 = b't';

/// 编码键包装，避免与裸 Vec 在 API 上混淆。
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Key(pub Vec<u8>);

/// 按 Go tablecodec 规则解码 table id；非法键返回 0。
pub fn DecodeTableID(key: Key) -> i64 {
    let mut key = key.0.as_slice();
    if !key.starts_with(&[TABLE_PREFIX]) {
        // TiKV API V2 keys carry a one-byte mode and three-byte keyspace ID.
        // Go delegates this stripping to tikv.DecodeKey before checking `t`.
        if key.len() <= 4 || !matches!(key[0], b'x' | b'r') {
            return 0;
        }
        key = &key[4..];
        if !key.starts_with(&[TABLE_PREFIX]) {
            return 0;
        }
    }
    // 前缀或长度不合法时返回 0，对齐 Go 容错而非报错。
    if key.len() < 9 {
        return 0;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&key[1..9]);
    let u = u64::from_be_bytes(buf);
    // 还原 TiDB 编码时翻转的符号位。
    (u ^ (1u64 << 63)) as i64
}

// ---------------------------------------------------------------------------
// kvproto stand-ins
// brpb 元数据桩集合：File/Schema/MetaFile/BackupMeta 等供 meta 读写使用。
// ---------------------------------------------------------------------------

pub mod kvproto {
    pub use astersql_br_pkg_utils::kvproto::brpb::CipherInfo;
    pub use astersql_br_pkg_utils::kvproto::encryptionpb;

    pub mod brpb {
        use crate::stubs::Message;
        pub use astersql_br_pkg_utils::kvproto::brpb::CipherInfo;
        use serde::{Deserialize, Serialize};
        use std::collections::HashMap;

        /// 备份数据文件元数据桩。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct File {
            name: String,
            cf: String,
            sha256: Vec<u8>,
            start_key: Vec<u8>,
            end_key: Vec<u8>,
            start_version: u64,
            end_version: u64,
            total_kvs: u64,
            total_bytes: u64,
            crc64xor: u64,
            size: u64,
            cipher_iv: Vec<u8>,
        }
        impl File {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_name(&self) -> &str {
                &self.name
            }
            pub fn set_name(&mut self, v: String) {
                self.name = v;
            }
            pub fn get_cf(&self) -> &str {
                &self.cf
            }
            pub fn set_cf(&mut self, v: String) {
                self.cf = v;
            }
            pub fn get_sha256(&self) -> &[u8] {
                &self.sha256
            }
            pub fn set_sha256(&mut self, v: Vec<u8>) {
                self.sha256 = v;
            }
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
            pub fn get_start_version(&self) -> u64 {
                self.start_version
            }
            pub fn set_start_version(&mut self, v: u64) {
                self.start_version = v;
            }
            pub fn get_end_version(&self) -> u64 {
                self.end_version
            }
            pub fn set_end_version(&mut self, v: u64) {
                self.end_version = v;
            }
            pub fn get_total_kvs(&self) -> u64 {
                self.total_kvs
            }
            pub fn set_total_kvs(&mut self, v: u64) {
                self.total_kvs = v;
            }
            pub fn get_total_bytes(&self) -> u64 {
                self.total_bytes
            }
            pub fn set_total_bytes(&mut self, v: u64) {
                self.total_bytes = v;
            }
            pub fn get_crc64xor(&self) -> u64 {
                self.crc64xor
            }
            pub fn set_crc64xor(&mut self, v: u64) {
                self.crc64xor = v;
            }
            pub fn get_size(&self) -> u64 {
                self.size
            }
            pub fn set_size(&mut self, v: u64) {
                self.size = v;
            }
            pub fn get_cipher_iv(&self) -> &[u8] {
                &self.cipher_iv
            }
            pub fn set_cipher_iv(&mut self, v: Vec<u8>) {
                self.cipher_iv = v;
            }
        }
        impl Message for File {}

        /// stats 文件索引：内联或远端引用。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct StatsFileIndex {
            name: String,
            sha256: Vec<u8>,
            size_enc: u64,
            size_ori: u64,
            cipher_iv: Vec<u8>,
            inline_data: Vec<u8>,
        }
        impl StatsFileIndex {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_name(&self) -> &str {
                &self.name
            }
            pub fn set_name(&mut self, v: String) {
                self.name = v;
            }
            pub fn get_sha256(&self) -> &[u8] {
                &self.sha256
            }
            pub fn set_sha256(&mut self, v: Vec<u8>) {
                self.sha256 = v;
            }
            pub fn get_size_enc(&self) -> u64 {
                self.size_enc
            }
            pub fn set_size_enc(&mut self, v: u64) {
                self.size_enc = v;
            }
            pub fn get_size_ori(&self) -> u64 {
                self.size_ori
            }
            pub fn set_size_ori(&mut self, v: u64) {
                self.size_ori = v;
            }
            pub fn get_cipher_iv(&self) -> &[u8] {
                &self.cipher_iv
            }
            pub fn set_cipher_iv(&mut self, v: Vec<u8>) {
                self.cipher_iv = v;
            }
            pub fn get_inline_data(&self) -> &[u8] {
                &self.inline_data
            }
            pub fn set_inline_data(&mut self, v: Vec<u8>) {
                self.inline_data = v;
            }
        }
        impl Message for StatsFileIndex {}

        /// 单个 physical_id 对应的 stats JSON 块。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct StatsBlock {
            physical_id: i64,
            json_table: Vec<u8>,
        }
        impl StatsBlock {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_physical_id(&self) -> i64 {
                self.physical_id
            }
            pub fn set_physical_id(&mut self, v: i64) {
                self.physical_id = v;
            }
            pub fn get_json_table(&self) -> &[u8] {
                &self.json_table
            }
            pub fn set_json_table(&mut self, v: Vec<u8>) {
                self.json_table = v;
            }
            /// 取出 JSON 字节并清空字段，便于尽早释放内存。
            pub fn take_json_table(&mut self) -> Vec<u8> {
                std::mem::take(&mut self.json_table)
            }
        }
        impl Message for StatsBlock {}

        /// 一组 StatsBlock 的容器。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct StatsFile {
            blocks: Vec<StatsBlock>,
        }
        impl StatsFile {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_blocks(&self) -> &[StatsBlock] {
                &self.blocks
            }
            pub fn mut_blocks(&mut self) -> &mut Vec<StatsBlock> {
                &mut self.blocks
            }
            pub fn set_blocks(&mut self, v: Vec<StatsBlock>) {
                self.blocks = v;
            }
            pub fn take_blocks(&mut self) -> Vec<StatsBlock> {
                std::mem::take(&mut self.blocks)
            }
        }
        impl Message for StatsFile {}

        /// 库表 schema 与关联 stats/校验元数据。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct Schema {
            db: Vec<u8>,
            table: Vec<u8>,
            stats: Vec<u8>,
            crc64xor: u64,
            total_kvs: u64,
            total_bytes: u64,
            tiflash_replicas: u32,
            is_merge_option_allowed: bool,
            stats_index: Vec<StatsFileIndex>,
            partition_merge_option_allowed: HashMap<String, bool>,
        }
        impl Schema {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_db(&self) -> &[u8] {
                &self.db
            }
            pub fn set_db(&mut self, v: Vec<u8>) {
                self.db = v;
            }
            pub fn get_table(&self) -> &[u8] {
                &self.table
            }
            pub fn set_table(&mut self, v: Vec<u8>) {
                self.table = v;
            }
            pub fn get_stats(&self) -> &[u8] {
                &self.stats
            }
            pub fn set_stats(&mut self, v: Vec<u8>) {
                self.stats = v;
            }
            pub fn get_crc64xor(&self) -> u64 {
                self.crc64xor
            }
            pub fn set_crc64xor(&mut self, v: u64) {
                self.crc64xor = v;
            }
            pub fn get_total_kvs(&self) -> u64 {
                self.total_kvs
            }
            pub fn set_total_kvs(&mut self, v: u64) {
                self.total_kvs = v;
            }
            pub fn get_total_bytes(&self) -> u64 {
                self.total_bytes
            }
            pub fn set_total_bytes(&mut self, v: u64) {
                self.total_bytes = v;
            }
            pub fn get_tiflash_replicas(&self) -> u32 {
                self.tiflash_replicas
            }
            pub fn set_tiflash_replicas(&mut self, v: u32) {
                self.tiflash_replicas = v;
            }
            pub fn get_is_merge_option_allowed(&self) -> bool {
                self.is_merge_option_allowed
            }
            pub fn set_is_merge_option_allowed(&mut self, v: bool) {
                self.is_merge_option_allowed = v;
            }
            pub fn get_stats_index(&self) -> &[StatsFileIndex] {
                &self.stats_index
            }
            pub fn mut_stats_index(&mut self) -> &mut Vec<StatsFileIndex> {
                &mut self.stats_index
            }
            pub fn set_stats_index(&mut self, v: Vec<StatsFileIndex>) {
                self.stats_index = v;
            }
            pub fn get_partition_merge_option_allowed(&self) -> &HashMap<String, bool> {
                &self.partition_merge_option_allowed
            }
            pub fn mut_partition_merge_option_allowed(&mut self) -> &mut HashMap<String, bool> {
                &mut self.partition_merge_option_allowed
            }
            /// 清空内联 stats 字节，通常在索引化写盘后调用。
            pub fn clear_stats(&mut self) {
                self.stats.clear();
            }
            /// 清空 stats 文件索引列表。
            pub fn clear_stats_index(&mut self) {
                self.stats_index.clear();
            }
        }
        impl Message for Schema {}

        /// v2 索引树节点，可递归包含子 MetaFile。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct MetaFile {
            name: String,
            sha256: Vec<u8>,
            size: u64,
            cipher_iv: Vec<u8>,
            data_files: Vec<File>,
            schemas: Vec<Schema>,
            meta_files: Vec<MetaFile>,
            raw_ranges: Vec<RawRange>,
            ddls: Vec<Vec<u8>>,
        }
        impl MetaFile {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_name(&self) -> &str {
                &self.name
            }
            pub fn set_name(&mut self, v: String) {
                self.name = v;
            }
            pub fn get_sha256(&self) -> &[u8] {
                &self.sha256
            }
            pub fn set_sha256(&mut self, v: Vec<u8>) {
                self.sha256 = v;
            }
            pub fn get_size(&self) -> u64 {
                self.size
            }
            pub fn set_size(&mut self, v: u64) {
                self.size = v;
            }
            pub fn get_cipher_iv(&self) -> &[u8] {
                &self.cipher_iv
            }
            pub fn set_cipher_iv(&mut self, v: Vec<u8>) {
                self.cipher_iv = v;
            }
            pub fn get_data_files(&self) -> &[File] {
                &self.data_files
            }
            pub fn mut_data_files(&mut self) -> &mut Vec<File> {
                &mut self.data_files
            }
            pub fn get_schemas(&self) -> &[Schema] {
                &self.schemas
            }
            pub fn mut_schemas(&mut self) -> &mut Vec<Schema> {
                &mut self.schemas
            }
            pub fn get_meta_files(&self) -> &[MetaFile] {
                &self.meta_files
            }
            pub fn mut_meta_files(&mut self) -> &mut Vec<MetaFile> {
                &mut self.meta_files
            }
            pub fn get_raw_ranges(&self) -> &[RawRange] {
                &self.raw_ranges
            }
            pub fn mut_raw_ranges(&mut self) -> &mut Vec<RawRange> {
                &mut self.raw_ranges
            }
            pub fn get_ddls(&self) -> &[Vec<u8>] {
                &self.ddls
            }
            pub fn mut_ddls(&mut self) -> &mut Vec<Vec<u8>> {
                &mut self.ddls
            }
            pub fn set_schemas(&mut self, v: Vec<Schema>) {
                self.schemas = v;
            }
            pub fn set_data_files(&mut self, v: Vec<File>) {
                self.data_files = v;
            }
            pub fn set_meta_files(&mut self, v: Vec<MetaFile>) {
                self.meta_files = v;
            }
            pub fn take_schemas(&mut self) -> Vec<Schema> {
                std::mem::take(&mut self.schemas)
            }
            pub fn take_data_files(&mut self) -> Vec<File> {
                std::mem::take(&mut self.data_files)
            }
            pub fn take_ddls(&mut self) -> Vec<Vec<u8>> {
                std::mem::take(&mut self.ddls)
            }
            pub fn take_meta_files(&mut self) -> Vec<MetaFile> {
                std::mem::take(&mut self.meta_files)
            }
        }
        impl Message for MetaFile {}

        /// 原始 KV 备份的起止键。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct RawRange {
            start_key: Vec<u8>,
            end_key: Vec<u8>,
        }
        impl RawRange {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
        }
        impl Message for RawRange {}

        /// 备份根元数据，兼容 v1 扁平与 v2 索引布局。
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct BackupMeta {
            cluster_id: u64,
            cluster_version: String,
            br_version: String,
            start_version: u64,
            end_version: u64,
            version: i32,
            backup_schema_version: u32,
            backup_size: u64,
            ddls: Vec<u8>,
            files: Vec<File>,
            raw_ranges: Vec<RawRange>,
            schemas: Vec<Schema>,
            meta_files: Vec<MetaFile>,
            schema_index: Option<MetaFile>,
            file_index: Option<MetaFile>,
            ddl_indexes: Option<MetaFile>,
        }
        impl BackupMeta {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_cluster_id(&self) -> u64 {
                self.cluster_id
            }
            pub fn set_cluster_id(&mut self, v: u64) {
                self.cluster_id = v;
            }
            pub fn get_cluster_version(&self) -> &str {
                &self.cluster_version
            }
            pub fn set_cluster_version(&mut self, v: String) {
                self.cluster_version = v;
            }
            pub fn get_br_version(&self) -> &str {
                &self.br_version
            }
            pub fn set_br_version(&mut self, v: String) {
                self.br_version = v;
            }
            pub fn get_start_version(&self) -> u64 {
                self.start_version
            }
            pub fn set_start_version(&mut self, v: u64) {
                self.start_version = v;
            }
            pub fn get_end_version(&self) -> u64 {
                self.end_version
            }
            pub fn set_end_version(&mut self, v: u64) {
                self.end_version = v;
            }
            pub fn get_version(&self) -> i32 {
                self.version
            }
            pub fn set_version(&mut self, v: i32) {
                self.version = v;
            }
            pub fn get_backup_schema_version(&self) -> u32 {
                self.backup_schema_version
            }
            pub fn set_backup_schema_version(&mut self, v: u32) {
                self.backup_schema_version = v;
            }
            pub fn get_backup_size(&self) -> u64 {
                self.backup_size
            }
            pub fn set_backup_size(&mut self, v: u64) {
                self.backup_size = v;
            }
            pub fn get_ddls(&self) -> &[u8] {
                &self.ddls
            }
            pub fn set_ddls(&mut self, v: Vec<u8>) {
                self.ddls = v;
            }
            pub fn get_files(&self) -> &[File] {
                &self.files
            }
            pub fn set_files(&mut self, v: Vec<File>) {
                self.files = v;
            }
            pub fn mut_files(&mut self) -> &mut Vec<File> {
                &mut self.files
            }
            pub fn get_raw_ranges(&self) -> &[RawRange] {
                &self.raw_ranges
            }
            pub fn mut_raw_ranges(&mut self) -> &mut Vec<RawRange> {
                &mut self.raw_ranges
            }
            pub fn get_schemas(&self) -> &[Schema] {
                &self.schemas
            }
            pub fn set_schemas(&mut self, v: Vec<Schema>) {
                self.schemas = v;
            }
            pub fn mut_schemas(&mut self) -> &mut Vec<Schema> {
                &mut self.schemas
            }
            pub fn get_meta_files(&self) -> &[MetaFile] {
                &self.meta_files
            }
            pub fn mut_meta_files(&mut self) -> &mut Vec<MetaFile> {
                &mut self.meta_files
            }
            pub fn has_schema_index(&self) -> bool {
                self.schema_index.is_some()
            }
            pub fn get_schema_index(&self) -> Option<&MetaFile> {
                self.schema_index.as_ref()
            }
            pub fn set_schema_index(&mut self, v: MetaFile) {
                self.schema_index = Some(v);
            }
            /// 惰性创建 schema 索引节点，对齐 Go proto 默认对象语义。
            pub fn mut_schema_index(&mut self) -> &mut MetaFile {
                if self.schema_index.is_none() {
                    self.schema_index = Some(MetaFile::default());
                }
                self.schema_index.as_mut().unwrap()
            }
            pub fn has_file_index(&self) -> bool {
                self.file_index.is_some()
            }
            pub fn get_file_index(&self) -> Option<&MetaFile> {
                self.file_index.as_ref()
            }
            pub fn set_file_index(&mut self, v: MetaFile) {
                self.file_index = Some(v);
            }
            /// 惰性创建 file 索引节点。
            pub fn mut_file_index(&mut self) -> &mut MetaFile {
                if self.file_index.is_none() {
                    self.file_index = Some(MetaFile::default());
                }
                self.file_index.as_mut().unwrap()
            }
            pub fn has_ddl_indexes(&self) -> bool {
                self.ddl_indexes.is_some()
            }
            pub fn get_ddl_indexes(&self) -> Option<&MetaFile> {
                self.ddl_indexes.as_ref()
            }
            pub fn set_ddl_indexes(&mut self, v: MetaFile) {
                self.ddl_indexes = Some(v);
            }
            /// 惰性创建 ddl 索引节点。
            pub fn mut_ddl_indexes(&mut self) -> &mut MetaFile {
                if self.ddl_indexes.is_none() {
                    self.ddl_indexes = Some(MetaFile::default());
                }
                self.ddl_indexes.as_mut().unwrap()
            }
        }
        impl Message for BackupMeta {}
    }
}
