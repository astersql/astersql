// Copyright 2026 AsterSQL.
//! Local stand-ins for kvproto / kv / tablecodec / sqlexec used by utils without
//!
//! utils 本地桩：在不链接完整 kvproto/gRPC 的环境下提供编译期替身。
//! 覆盖 kv 上下文、取消令牌、受限 SQL、tablecodec 元键编解码与 brpb 消息壳。
//! 多数类型仅为字段载体与 getter/setter，不实现真实 RPC 或存储语义。
//! EncodeMetaKey/DecodeMetaKey 走 util-codec，算法与 Go tablecodec 对齐。
//! 调用方不得假设 ExecRestrictedSQL/RunInNewTxn 已接入真实 TiDB/TiKV。
//! 本文件目标是让 utils 及依赖方在 darwin/arm64 等环境可编译与单测。
//! pulling kvproto/grpcio on darwin arm64.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use astersql_errors::SharedError;
use astersql_parser_types::FieldType;
use astersql_util_codec::{DecodeBytes, DecodeUint, EncodeBytes, EncodeUint, EncodedBytesLength};

pub use astersql_br_pkg_logutil::{KeyRange, KvKey};

// ---------------------------------------------------------------------------
// kv stand-ins
// KV 事务/存储最小替身：供配置读写等路径通过类型检查，无真实提交。
// ---------------------------------------------------------------------------

pub const InternalTxnBR: &str = "br";

fn shared_meta_transaction() -> astersql_meta::kv::Transaction {
    static META_TRANSACTION: OnceLock<astersql_meta::kv::Transaction> = OnceLock::new();
    META_TRANSACTION
        .get_or_init(astersql_meta::kv::Transaction::default)
        .clone()
}

#[derive(Clone, Debug, Default)]
/// 轻量 KV 上下文壳；cancelled 供将来扩展，当前透传为主。
pub struct KvContext {
    cancelled: Arc<AtomicBool>,
}

impl KvContext {
    /// 构造占位上下文（Go context.TODO 对偶）。
    pub fn todo() -> Self {
        Self::default()
    }
    /// 构造后台上下文（Go context.Background 对偶）。
    pub fn Background() -> Self {
        Self::default()
    }
}

/// 标记内部来源类型；桩实现原样返回 ctx。
pub fn WithInternalSourceType(ctx: KvContext, _source: &str) -> KvContext {
    ctx
}

/// 存储抽象桩；name 默认空，真实实现由上层注入。
pub trait Storage: Send + Sync {
    fn name(&self) -> &str {
        ""
    }

    /// Returns a transaction backed by this stand-in cluster's shared meta state.
    fn meta_transaction(&self) -> astersql_meta::kv::Transaction {
        shared_meta_transaction()
    }
}

/// 事务抽象桩；同时携带当前存储对应的 meta 事务。
pub trait Transaction: Send {
    fn set_option(&mut self, _opt: &str) {}

    fn meta_transaction(&mut self) -> astersql_meta::kv::Transaction {
        shared_meta_transaction()
    }
}

/// 在共享内存 meta 事务上同步执行闭包；不连接真实 KV，也不重试。
pub fn RunInNewTxn(
    _ctx: &KvContext,
    _storage: &dyn Storage,
    _retry: bool,
    mut f: impl FnMut(&KvContext, &mut dyn Transaction) -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    struct EmptyTxn {
        meta_transaction: astersql_meta::kv::Transaction,
    }
    impl Transaction for EmptyTxn {
        fn meta_transaction(&mut self) -> astersql_meta::kv::Transaction {
            self.meta_transaction.clone()
        }
    }
    let ctx = KvContext::todo();
    let mut txn = EmptyTxn {
        meta_transaction: _storage.meta_transaction(),
    };
    f(&ctx, &mut txn)
}

// ---------------------------------------------------------------------------
// sqlexec context stand-in (CancellationToken-shaped)
// 取消令牌形态的 Context：共享 AtomicBool，child_token 共享同一取消位。
// ---------------------------------------------------------------------------

pub mod context {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex, Weak};
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct State {
        cancelled: AtomicBool,
        wait_lock: Mutex<()>,
        wait_changed: Condvar,
        children: Mutex<Vec<Weak<State>>>,
    }

    #[derive(Clone, Debug, Default)]
    /// 可取消上下文；clone 共享状态，child 独立取消并接受父级向下传播。
    pub struct Context {
        state: Arc<State>,
    }

    impl Context {
        /// 默认构造空实例。
        pub fn new() -> Self {
            Self::default()
        }
        /// 查询是否已取消。
        pub fn is_cancelled(&self) -> bool {
            self.state.cancelled.load(Ordering::SeqCst)
        }
        /// 置位取消标志，唤醒等待方语义由调用方解释。
        pub fn cancel(&self) {
            cancel_state(&self.state);
        }
        /// 等待取消或超时；返回 true 表示等待期间（或此前）已取消。
        pub fn wait_cancelled_timeout(&self, timeout: Duration) -> bool {
            if self.is_cancelled() {
                return true;
            }
            let guard = self
                .state
                .wait_lock
                .lock()
                .expect("context wait lock poisoned");
            if self.is_cancelled() {
                return true;
            }
            let _ = self
                .state
                .wait_changed
                .wait_timeout(guard, timeout)
                .expect("context wait lock poisoned");
            self.is_cancelled()
        }
        /// 派生子令牌；父级取消向下传播，子级取消不影响父级。
        pub fn child_token(&self) -> Self {
            let child_state = Arc::new(State::default());
            let mut children = self
                .state
                .children
                .lock()
                .expect("context children poisoned");
            children.retain(|child| child.strong_count() > 0);
            children.push(Arc::downgrade(&child_state));
            if self.is_cancelled() {
                cancel_state(&child_state);
            }
            Self { state: child_state }
        }
    }

    fn cancel_state(state: &Arc<State>) {
        let children = {
            let _guard = state.wait_lock.lock().expect("context wait lock poisoned");
            if state.cancelled.swap(true, Ordering::SeqCst) {
                return;
            }
            state.wait_changed.notify_all();
            state
                .children
                .lock()
                .expect("context children poisoned")
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for child in children {
            cancel_state(&child);
        }
    }
}

// ---------------------------------------------------------------------------
// RestrictedSQLExecutor / ResultField / Row stand-ins
// 受限 SQL 执行器与结果行桩：列值以字符串细胞存储，非完整 Datum 体系。
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
/// 结果列信息壳，仅携带 FieldType。
pub struct ColumnInfo {
    pub FieldType: FieldType,
}

#[derive(Clone, Debug, Default)]
/// SQL 结果字段；column 可选以模拟缺失元数据。
pub struct ResultField {
    pub column: Option<ColumnInfo>,
}

#[derive(Clone, Debug, Default)]
/// 简化 Datum：内部仅 String，非完整 TiDB Datum。
pub struct Datum(String);

impl Datum {
    /// 返回内部字符串副本。
    pub fn ToString(&self) -> Result<String, SharedError> {
        Ok(self.0.clone())
    }
}

#[derive(Clone, Debug, Default)]
/// 结果行：字符串细胞模拟各列，供配置查询解析。
pub struct Row {
    cells: Vec<String>,
}

impl Row {
    /// 由字符串列构造行。
    pub fn from_cells(cells: Vec<String>) -> Self {
        Self { cells }
    }

    /// 按索引写入细胞，必要时扩容。
    pub fn set_cell(&mut self, idx: usize, value: String) {
        if idx >= self.cells.len() {
            self.cells.resize(idx + 1, String::new());
        }
        self.cells[idx] = value;
    }

    /// 按列索引取 Datum；越界返回空串。
    pub fn GetDatum(&self, idx: usize, _ft: &FieldType) -> Datum {
        Datum(self.cells.get(idx).cloned().unwrap_or_default())
    }
}

/// 跨语言错误别名，模拟 Go error 接口返回。
pub type GoError = Box<dyn std::error::Error + Send + Sync>;

pub trait RestrictedSQLExecutor {
    fn ExecRestrictedSQL(
        &mut self,
        _ctx: &context::Context,
        _opts: Vec<()>,
        _sql: &str,
        _args: Vec<Box<dyn Any>>,
    ) -> Result<(Vec<Row>, Vec<ResultField>), GoError>;
}

// ---------------------------------------------------------------------------
// tablecodec EncodeMetaKey / DecodeMetaKey (keep Go algorithm via util-codec)
// 元数据 hash 键编解码；前缀 m + key + flag h + field，错误文案对齐 Go。
// ---------------------------------------------------------------------------

const META_PREFIX: &[u8] = b"m";
// hash 数据 flag（ASCII `h`），编解码必须一致。
const HASH_DATA: u64 = b'h' as u64;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
/// 已编码的表/元键包装，AsRef 暴露原始字节。
pub struct TableKey(pub Vec<u8>);

impl AsRef<[u8]> for TableKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// 编码 meta hash 键：m || key || h || field。
pub fn EncodeMetaKey(key: &[u8], field: &[u8]) -> TableKey {
    let mut ek = Vec::with_capacity(
        META_PREFIX.len() + EncodedBytesLength(key.len()) + 8 + EncodedBytesLength(field.len()),
    );
    ek.extend_from_slice(META_PREFIX);
    ek = EncodeBytes(ek, key);
    ek = EncodeUint(ek, HASH_DATA);
    ek = EncodeBytes(ek, field);
    TableKey(ek)
}

/// 解码 meta hash 键；前缀或 flag 不符返回 InvalidData。
pub fn DecodeMetaKey(ek: TableKey) -> Result<(Vec<u8>, Vec<u8>), SharedError> {
    if !ek.0.starts_with(META_PREFIX) {
        return Err(SharedError::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid encoded hash data key prefix",
        )));
    }
    let (remain, key) = DecodeBytes(&ek.0[1..], None).map_err(SharedError::new)?;
    let (remain, tp) = DecodeUint(remain).map_err(SharedError::new)?;
    if tp != HASH_DATA {
        return Err(SharedError::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid encoded hash data key flag {}", tp as u8 as char),
        )));
    }
    let (_remain, field) = DecodeBytes(remain, None).map_err(SharedError::new)?;
    Ok((key, field))
}

// ---------------------------------------------------------------------------
// kvproto stand-ins
// kvproto 子集桩：encryption / metapb.Store / brpb 备份元数据消息壳。
// ---------------------------------------------------------------------------

pub mod kvproto {
    // 加密方法枚举命名空间桩。
    pub mod encryptionpb {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum EncryptionMethod {
            #[default]
            Unknown = 0,
            Plaintext = 1,
            Aes128Ctr = 2,
            Aes192Ctr = 3,
            Aes256Ctr = 4,
        }
    }

    // Store 元数据命名空间桩。
    pub mod metapb {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum StoreState {
            #[default]
            Up = 0,
            Offline = 1,
            Tombstone = 2,
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        /// Store 标签键值对桩。
        pub struct StoreLabel {
            key: String,
            // 标签值字段。
            value: String,
        }

        impl StoreLabel {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取标签键。
            pub fn get_key(&self) -> &str {
                &self.key
            }
            /// 写入标签键。
            pub fn set_key(&mut self, v: String) {
                self.key = v;
            }
            /// 读取标签值。
            pub fn get_value(&self) -> &str {
                &self.value
            }
            /// 写入标签值。
            pub fn set_value(&mut self, v: String) {
                self.value = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// metapb.Store 子集桩，供拨号与监视读取地址/状态。
        pub struct Store {
            id: u64,
            // 主地址。
            address: String,
            status_address: String,
            // peer 地址。
            peer_address: String,
            state: StoreState,
            // 最近心跳。
            last_heartbeat: i64,
            labels: Vec<StoreLabel>,
        }

        impl Store {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取 store id。
            pub fn get_id(&self) -> u64 {
                self.id
            }
            /// 写入 store id。
            pub fn set_id(&mut self, v: u64) {
                self.id = v;
            }
            /// 读取主服务地址。
            pub fn get_address(&self) -> &str {
                &self.address
            }
            /// 写入主服务地址。
            pub fn set_address(&mut self, v: String) {
                self.address = v;
            }
            /// 读取 status/HTTP 地址。
            pub fn get_status_address(&self) -> &str {
                &self.status_address
            }
            /// 写入 status/HTTP 地址。
            pub fn set_status_address(&mut self, v: String) {
                self.status_address = v;
            }
            /// 读取 peer 地址（拨号优先）。
            pub fn get_peer_address(&self) -> &str {
                &self.peer_address
            }
            /// 写入 peer 地址。
            pub fn set_peer_address(&mut self, v: String) {
                self.peer_address = v;
            }
            /// 读取 store 运行态。
            pub fn get_state(&self) -> StoreState {
                self.state
            }
            /// 写入 store 运行态。
            pub fn set_state(&mut self, v: StoreState) {
                self.state = v;
            }
            /// 读取最近心跳时间戳。
            pub fn get_last_heartbeat(&self) -> i64 {
                self.last_heartbeat
            }
            /// 写入最近心跳时间戳。
            pub fn set_last_heartbeat(&mut self, v: i64) {
                self.last_heartbeat = v;
            }
            /// 只读访问标签列表。
            pub fn get_labels(&self) -> &[StoreLabel] {
                &self.labels
            }
            /// 可变借用标签列表。
            pub fn mut_labels(&mut self) -> &mut Vec<StoreLabel> {
                &mut self.labels
            }
            /// 整体替换标签列表。
            pub fn set_labels(&mut self, v: Vec<StoreLabel>) {
                self.labels = v;
            }
        }
    }

    // BR protobuf 消息命名空间桩（无编码实现）。
    pub mod brpb {
        use super::encryptionpb::EncryptionMethod;

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 备份数据文件描述消息壳。
        pub struct File {
            name: String,
            // 列族。
            cf: String,
            sha256: Vec<u8>,
            // 起始键。
            start_key: Vec<u8>,
            end_key: Vec<u8>,
            // 起始 TS。
            start_version: u64,
            end_version: u64,
            // KV 条数。
            total_kvs: u64,
            total_bytes: u64,
            // CRC 校验。
            crc64xor: u64,
            size: u64,
            // IV。
            cipher_iv: Vec<u8>,
        }

        impl File {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取备份文件名。
            pub fn get_name(&self) -> &str {
                &self.name
            }
            /// 写入备份文件名。
            pub fn set_name(&mut self, v: String) {
                self.name = v;
            }
            /// 读取列族名。
            pub fn get_cf(&self) -> &str {
                &self.cf
            }
            /// 写入列族名。
            pub fn set_cf(&mut self, v: String) {
                self.cf = v;
            }
            /// 读取内容校验摘要。
            pub fn get_sha256(&self) -> &[u8] {
                &self.sha256
            }
            /// 写入内容校验摘要。
            pub fn set_sha256(&mut self, v: Vec<u8>) {
                self.sha256 = v;
            }
            /// 读取范围起始键。
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            /// 写入范围起始键。
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            /// 读取范围结束键。
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            /// 写入范围结束键。
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
            /// 读取起始版本（TS）。
            pub fn get_start_version(&self) -> u64 {
                self.start_version
            }
            /// 写入起始版本（TS）。
            pub fn set_start_version(&mut self, v: u64) {
                self.start_version = v;
            }
            /// 读取结束版本（TS）。
            pub fn get_end_version(&self) -> u64 {
                self.end_version
            }
            /// 写入结束版本（TS）。
            pub fn set_end_version(&mut self, v: u64) {
                self.end_version = v;
            }
            /// 读取键值条数统计。
            pub fn get_total_kvs(&self) -> u64 {
                self.total_kvs
            }
            /// 写入键值条数统计。
            pub fn set_total_kvs(&mut self, v: u64) {
                self.total_kvs = v;
            }
            /// 读取字节量统计。
            pub fn get_total_bytes(&self) -> u64 {
                self.total_bytes
            }
            /// 写入字节量统计。
            pub fn set_total_bytes(&mut self, v: u64) {
                self.total_bytes = v;
            }
            /// 读取 CRC64 xor 校验。
            pub fn get_crc64xor(&self) -> u64 {
                self.crc64xor
            }
            /// 写入 CRC64 xor 校验。
            pub fn set_crc64xor(&mut self, v: u64) {
                self.crc64xor = v;
            }
            /// 读取文件大小字段。
            pub fn get_size(&self) -> u64 {
                self.size
            }
            /// 写入文件大小字段。
            pub fn set_size(&mut self, v: u64) {
                self.size = v;
            }
            /// 读取加密 IV。
            pub fn get_cipher_iv(&self) -> &[u8] {
                &self.cipher_iv
            }
            /// 写入加密 IV。
            pub fn set_cipher_iv(&mut self, v: Vec<u8>) {
                self.cipher_iv = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 备份加密参数（算法+密钥）消息壳。
        pub struct CipherInfo {
            cipher_type: EncryptionMethod,
            // 密钥。
            cipher_key: Vec<u8>,
        }

        impl CipherInfo {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取加密算法枚举。
            pub fn get_cipher_type(&self) -> EncryptionMethod {
                self.cipher_type
            }
            /// 写入加密算法枚举。
            pub fn set_cipher_type(&mut self, v: EncryptionMethod) {
                self.cipher_type = v;
            }
            /// 读取加密密钥材料。
            pub fn get_cipher_key(&self) -> &[u8] {
                &self.cipher_key
            }
            /// 写入加密密钥材料。
            pub fn set_cipher_key(&mut self, v: Vec<u8>) {
                self.cipher_key = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// BR RPC 错误消息壳，含 KV/Region/ClusterId 标志。
        pub struct Error {
            msg: String,
            // KV 错误标志。
            kv_error: bool,
            region_error: bool,
            // 集群 ID 错误标志。
            cluster_id_error: bool,
        }

        impl Error {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取错误消息文本。
            pub fn get_msg(&self) -> &str {
                &self.msg
            }
            /// 写入错误消息文本。
            pub fn set_msg(&mut self, v: String) {
                self.msg = v;
            }
            /// 是否标记为 KV 错误。
            pub fn has_kv_error(&self) -> bool {
                self.kv_error
            }
            /// 设置 KV 错误标志。
            pub fn set_kv_error(&mut self, v: bool) {
                self.kv_error = v;
            }
            /// 是否标记为 Region 错误。
            pub fn has_region_error(&self) -> bool {
                self.region_error
            }
            /// 设置 Region 错误标志。
            pub fn set_region_error(&mut self, v: bool) {
                self.region_error = v;
            }
            /// 是否标记为集群 ID 错误。
            pub fn has_cluster_id_error(&self) -> bool {
                self.cluster_id_error
            }
            /// 设置集群 ID 错误标志。
            pub fn set_cluster_id_error(&mut self, v: bool) {
                self.cluster_id_error = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// raw KV 备份范围消息壳。
        pub struct RawRange {
            start_key: Vec<u8>,
            // 结束键。
            end_key: Vec<u8>,
            cf: String,
        }

        impl RawRange {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取范围起始键。
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            /// 写入范围起始键。
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            /// 读取范围结束键。
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            /// 写入范围结束键。
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
            /// 读取列族名。
            pub fn get_cf(&self) -> &str {
                &self.cf
            }
            /// 写入列族名。
            pub fn set_cf(&mut self, v: String) {
                self.cf = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 库表 schema 与统计载荷消息壳。
        pub struct Schema {
            db: Vec<u8>,
            // 表元数据字节。
            table: Vec<u8>,
            stats: Vec<u8>,
            // CRC 校验。
            crc64xor: u64,
            total_kvs: u64,
            // 字节数。
            total_bytes: u64,
            tiflash_replicas: u32,
            // 是否允许 merge。
            is_merge_option_allowed: bool,
        }

        impl Schema {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取库名序列化字节。
            pub fn get_db(&self) -> &[u8] {
                &self.db
            }
            /// 写入库名序列化字节。
            pub fn set_db(&mut self, v: Vec<u8>) {
                self.db = v;
            }
            /// 读取表结构序列化字节。
            pub fn get_table(&self) -> &[u8] {
                &self.table
            }
            /// 写入表结构序列化字节。
            pub fn set_table(&mut self, v: Vec<u8>) {
                self.table = v;
            }
            /// 读取统计信息字节。
            pub fn get_stats(&self) -> &[u8] {
                &self.stats
            }
            /// 写入统计信息字节。
            pub fn set_stats(&mut self, v: Vec<u8>) {
                self.stats = v;
            }
            /// 读取 CRC64 xor 校验。
            pub fn get_crc64xor(&self) -> u64 {
                self.crc64xor
            }
            /// 写入 CRC64 xor 校验。
            pub fn set_crc64xor(&mut self, v: u64) {
                self.crc64xor = v;
            }
            /// 读取键值条数统计。
            pub fn get_total_kvs(&self) -> u64 {
                self.total_kvs
            }
            /// 写入键值条数统计。
            pub fn set_total_kvs(&mut self, v: u64) {
                self.total_kvs = v;
            }
            /// 读取字节量统计。
            pub fn get_total_bytes(&self) -> u64 {
                self.total_bytes
            }
            /// 写入字节量统计。
            pub fn set_total_bytes(&mut self, v: u64) {
                self.total_bytes = v;
            }
            /// 读取 TiFlash 副本数。
            pub fn get_tiflash_replicas(&self) -> u32 {
                self.tiflash_replicas
            }
            /// 写入 TiFlash 副本数。
            pub fn set_tiflash_replicas(&mut self, v: u32) {
                self.tiflash_replicas = v;
            }
            /// 是否允许 merge 选项。
            pub fn get_is_merge_option_allowed(&self) -> bool {
                self.is_merge_option_allowed
            }
            /// 设置是否允许 merge 选项。
            pub fn set_is_merge_option_allowed(&mut self, v: bool) {
                self.is_merge_option_allowed = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 备份元数据根消息壳，聚合文件/范围/schema。
        pub struct BackupMeta {
            cluster_id: u64,
            // 集群版本。
            cluster_version: String,
            br_version: String,
            // 起始 TS。
            start_version: u64,
            end_version: u64,
            // 格式版本。
            version: i32,
            ddls: Vec<u8>,
            // 文件列表。
            files: Vec<File>,
            raw_ranges: Vec<RawRange>,
            // schema 列表。
            schemas: Vec<Schema>,
            is_raw_kv: bool,
            // 新排序规则。
            new_collations_enabled: String,
        }

        impl BackupMeta {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取备份集群 ID。
            pub fn get_cluster_id(&self) -> u64 {
                self.cluster_id
            }
            /// 写入备份集群 ID。
            pub fn set_cluster_id(&mut self, v: u64) {
                self.cluster_id = v;
            }
            /// 读取集群版本字符串。
            pub fn get_cluster_version(&self) -> &str {
                &self.cluster_version
            }
            /// 写入集群版本字符串。
            pub fn set_cluster_version(&mut self, v: String) {
                self.cluster_version = v;
            }
            /// 读取 BR 工具版本。
            pub fn get_br_version(&self) -> &str {
                &self.br_version
            }
            /// 写入 BR 工具版本。
            pub fn set_br_version(&mut self, v: String) {
                self.br_version = v;
            }
            /// 读取起始版本（TS）。
            pub fn get_start_version(&self) -> u64 {
                self.start_version
            }
            /// 写入起始版本（TS）。
            pub fn set_start_version(&mut self, v: u64) {
                self.start_version = v;
            }
            /// 读取结束版本（TS）。
            pub fn get_end_version(&self) -> u64 {
                self.end_version
            }
            /// 写入结束版本（TS）。
            pub fn set_end_version(&mut self, v: u64) {
                self.end_version = v;
            }
            /// 读取 BackupMeta 格式版本。
            pub fn get_version(&self) -> i32 {
                self.version
            }
            /// 写入 BackupMeta 格式版本。
            pub fn set_version(&mut self, v: i32) {
                self.version = v;
            }
            /// 读取 DDL 载荷。
            pub fn get_ddls(&self) -> &[u8] {
                &self.ddls
            }
            /// 写入 DDL 载荷。
            pub fn set_ddls(&mut self, v: Vec<u8>) {
                self.ddls = v;
            }
            /// 只读访问文件列表。
            pub fn get_files(&self) -> &[File] {
                &self.files
            }
            /// 整体替换文件列表。
            pub fn set_files(&mut self, v: Vec<File>) {
                self.files = v;
            }
            /// 可变借用文件列表。
            pub fn mut_files(&mut self) -> &mut Vec<File> {
                &mut self.files
            }
            /// 只读访问 raw 范围。
            pub fn get_raw_ranges(&self) -> &[RawRange] {
                &self.raw_ranges
            }
            /// 整体替换 raw 范围。
            pub fn set_raw_ranges(&mut self, v: Vec<RawRange>) {
                self.raw_ranges = v;
            }
            /// 可变借用 raw 范围。
            pub fn mut_raw_ranges(&mut self) -> &mut Vec<RawRange> {
                &mut self.raw_ranges
            }
            /// 只读访问 schema 列表。
            pub fn get_schemas(&self) -> &[Schema] {
                &self.schemas
            }
            /// 整体替换 schema 列表。
            pub fn set_schemas(&mut self, v: Vec<Schema>) {
                self.schemas = v;
            }
            /// 可变借用 schema 列表。
            pub fn mut_schemas(&mut self) -> &mut Vec<Schema> {
                &mut self.schemas
            }
            /// 是否为 raw KV 备份。
            pub fn get_is_raw_kv(&self) -> bool {
                self.is_raw_kv
            }
            /// 设置 raw KV 备份标志。
            pub fn set_is_raw_kv(&mut self, v: bool) {
                self.is_raw_kv = v;
            }
            /// 新排序规则开关字符串。
            pub fn get_new_collations_enabled(&self) -> &str {
                &self.new_collations_enabled
            }
            /// 写入新排序规则开关。
            pub fn set_new_collations_enabled(&mut self, v: String) {
                self.new_collations_enabled = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 备份键范围消息壳。
        pub struct BackupRange {
            start_key: Vec<u8>,
            // 结束键。
            end_key: Vec<u8>,
        }

        impl BackupRange {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取范围起始键。
            pub fn get_start_key(&self) -> &[u8] {
                &self.start_key
            }
            /// 写入范围起始键。
            pub fn set_start_key(&mut self, v: Vec<u8>) {
                self.start_key = v;
            }
            /// 读取范围结束键。
            pub fn get_end_key(&self) -> &[u8] {
                &self.end_key
            }
            /// 写入范围结束键。
            pub fn set_end_key(&mut self, v: Vec<u8>) {
                self.end_key = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 分片元文件消息壳，承载 data_files 与 DDL。
        pub struct MetaFile {
            data_files: Vec<File>,
            // raw 范围。
            raw_ranges: Vec<RawRange>,
            schemas: Vec<Schema>,
            // DDL 载荷。
            ddls: Vec<Vec<u8>>,
            backup_ranges: Vec<BackupRange>,
        }

        impl MetaFile {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 只读访问 MetaFile 数据文件。
            pub fn get_data_files(&self) -> &[File] {
                &self.data_files
            }
            /// 写入 MetaFile 数据文件。
            pub fn set_data_files(&mut self, v: Vec<File>) {
                self.data_files = v;
            }
            /// 可变借用 MetaFile 数据文件。
            pub fn mut_data_files(&mut self) -> &mut Vec<File> {
                &mut self.data_files
            }
            /// 只读访问 raw 范围。
            pub fn get_raw_ranges(&self) -> &[RawRange] {
                &self.raw_ranges
            }
            /// 整体替换 raw 范围。
            pub fn set_raw_ranges(&mut self, v: Vec<RawRange>) {
                self.raw_ranges = v;
            }
            /// 可变借用 raw 范围。
            pub fn mut_raw_ranges(&mut self) -> &mut Vec<RawRange> {
                &mut self.raw_ranges
            }
            /// 只读访问 schema 列表。
            pub fn get_schemas(&self) -> &[Schema] {
                &self.schemas
            }
            /// 整体替换 schema 列表。
            pub fn set_schemas(&mut self, v: Vec<Schema>) {
                self.schemas = v;
            }
            /// 可变借用 schema 列表。
            pub fn mut_schemas(&mut self) -> &mut Vec<Schema> {
                &mut self.schemas
            }
            /// 读取 DDL 载荷。
            pub fn get_ddls(&self) -> &[Vec<u8>] {
                &self.ddls
            }
            /// 写入 DDL 载荷。
            pub fn set_ddls(&mut self, v: Vec<Vec<u8>>) {
                self.ddls = v;
            }
            /// 可变借用 DDL 列表。
            pub fn mut_ddls(&mut self) -> &mut Vec<Vec<u8>> {
                &mut self.ddls
            }
            /// 只读访问备份范围。
            pub fn get_backup_ranges(&self) -> &[BackupRange] {
                &self.backup_ranges
            }
            /// 写入备份范围列表。
            pub fn set_backup_ranges(&mut self, v: Vec<BackupRange>) {
                self.backup_ranges = v;
            }
            /// 可变借用备份范围。
            pub fn mut_backup_ranges(&mut self) -> &mut Vec<BackupRange> {
                &mut self.backup_ranges
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 统计信息块（JSON 表 + 物理 ID）。
        pub struct StatsBlock {
            json_table: Vec<u8>,
            // 物理 ID。
            physical_id: i64,
        }

        impl StatsBlock {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 读取统计 JSON 表字节。
            pub fn get_json_table(&self) -> &[u8] {
                &self.json_table
            }
            /// 写入统计 JSON 表字节。
            pub fn set_json_table(&mut self, v: Vec<u8>) {
                self.json_table = v;
            }
            /// 读取物理表 ID。
            pub fn get_physical_id(&self) -> i64 {
                self.physical_id
            }
            /// 写入物理表 ID。
            pub fn set_physical_id(&mut self, v: i64) {
                self.physical_id = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        /// 统计文件，包含多个 StatsBlock。
        pub struct StatsFile {
            blocks: Vec<StatsBlock>,
        }

        impl StatsFile {
            /// 默认构造空实例。
            pub fn new() -> Self {
                Self::default()
            }
            /// 只读访问统计块列表。
            pub fn get_blocks(&self) -> &[StatsBlock] {
                &self.blocks
            }
            /// 写入统计块列表。
            pub fn set_blocks(&mut self, v: Vec<StatsBlock>) {
                self.blocks = v;
            }
            /// 可变借用统计块列表。
            pub fn mut_blocks(&mut self) -> &mut Vec<StatsBlock> {
                &mut self.blocks
            }
        }
    }
}
