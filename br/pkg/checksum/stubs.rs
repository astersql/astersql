// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
//
// Local stand-ins for kv / distsql / tipb / tablecodec / ranger / metautil / utils
// boundaries (darwin-safe; no heavy workspace deps).

//! 本地桩：替代 kv/distsql/tipb/tablecodec/ranger/metautil 边界，供 checksum 执行器编译与测试。
//!
//! 模块职责：提供足够的类型、键前缀编码与 tipb 子集编解码，而非完整 TiKV 客户端。
//! 约束：能力为占位/适配；不得把缺失的真实 store/RPC 描述成已实现。
//! 数据流：TableInfo → RequestBuilder → Marshal(Data) → Client.Send → Response 流。
//! Go 对齐：符号名与错误文案（含历史拼写 parition）尽量保持一致。
//! 测试钩子：SKIP_BACKOFF_SLEEP 与 CHECKSUM_RETRY_ERR 仅影响测试路径。
//! 重试：WithRetry + ChecksumBackoffStrategy 对应 Go utils.WithRetry/checksum 退避。
//! 编解码：ChecksumRequest/Response/RewriteRule 手写 wire，字段号与 tipb 对齐。
//! 键空间：EncodeInt / GenTableRecordPrefix / EncodeTableIndexPrefix 复刻 tablecodec。
//! RequestBuilder 延迟错误：Marshal 失败记入 err，Build 时一次返回。
//! DistSQLChecksum 仅薄封装 Client.Send，拒绝 nil Response。
//! GetPartitionByName 服务分区表 rewrite；找不到名与无分区是两条错误路径。

use std::cell::Cell;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// 统一 Result；错误为本文件轻量 Error 桩。
pub type Result<T> = std::result::Result<T, Error>;

/// 轻量错误：仅 msg；Trace/Annotate 模拟 Go 包装，非完整 pingcap/errors。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

// Error 辅助：new/Trace/Annotate，供调用方包装上下文。
impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// Go errors.Trace 占位：当前原样返回。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// 前缀注解为 `ctx: msg`。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Cancellation token approximating Go context.Context for cancel checks.
/// 以共享 Option<Error> 近似 Go context 取消；非异步取消传播全实现。
#[derive(Clone, Default)]
pub struct Context {
    // None 表示未取消；Some 为取消原因。
    cancelled: Arc<Mutex<Option<Error>>>,
    // 父链保留动态取消传播，不只是 WithCancel 时的快照。
    parent: Option<Arc<Context>>,
}

// Context API 面刻意缩小到 checksum 取消检查所需。
impl Context {
    /// 对应 context.Background。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 对应 context.TODO。
    pub fn TODO() -> Self {
        Self::default()
    }

    /// 派生可取消子上下文，并保留父链以传播后续取消。
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            cancelled: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent.clone())),
        };
        let cancel = CancelFunc {
            cancelled: child.cancelled.clone(),
        };
        (child, cancel)
    }

    /// 写入取消原因。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// 已取消则返回原因副本。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.parent.as_ref().and_then(|parent| parent.Err()))
    }

    /// 是否已取消。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

/// WithCancel 句柄；cancel 写入固定 "context canceled"。
#[derive(Clone)]
pub struct CancelFunc {
    cancelled: Arc<Mutex<Option<Error>>>,
}

// 消费 self 的 cancel，与 Go cancel func 一次性调用习惯接近。
impl CancelFunc {
    pub fn cancel(self) {
        *self.cancelled.lock().unwrap() = Some(Error::new("context canceled"));
    }
}

// 对齐 Go DefDistSQLScanConcurrency，默认并发回落值。
pub const DefDistSQLScanConcurrency: u32 = 15;

// checksum 重试次数上限。
pub const ChecksumRetryTime: i32 = 8;
// 初始退避 1s，NextBackoff 中倍增。
pub const ChecksumWaitInterval: Duration = Duration::from_secs(1);
// 文档常量；构造策略时实际 max 取 Go 默认 10s。
pub const ChecksumMaxWaitInterval: Duration = Duration::from_secs(30);

thread_local! {
    // 为 true 时压缩墙钟，不改变尝试次数逻辑。
    /// When true, `WithRetry` skips sleeping (parity tests).
    // 线程局部：仅测试进程内有效。
    static SKIP_BACKOFF_SLEEP: Cell<bool> = const { Cell::new(false) };

    // 对应 Go failpoint checksumRetryErr。
    /// Failpoint stand-in for `checksumRetryErr` (consume-once).
    // 与 inject/take 配对使用。
    static CHECKSUM_RETRY_ERR: Cell<bool> = const { Cell::new(false) };
}

/// 测试开关：跳过 WithRetry 内 sleep，不改重试语义。
pub fn set_skip_backoff_sleep(skip: bool) {
    SKIP_BACKOFF_SLEEP.set(skip);
}

/// 设置 checksumRetryErr 注入标志。
pub fn inject_checksum_retry_err(enable: bool) {
    CHECKSUM_RETRY_ERR.set(enable);
}

/// 取出并清除注入标志（consume-once）。
pub fn take_checksum_retry_err() -> bool {
    CHECKSUM_RETRY_ERR.replace(false)
}

/// ast.CIStr 桩：O 原文，L 小写。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CIStr {
    // O：原始大小写；L：小写副本。
    pub O: String,
    pub L: String,
}

// 构造时固化 L，后续分区名比较只读 L。
impl CIStr {
    /// 由字符串构造，L 立即小写化。
    pub fn new(s: impl Into<String>) -> Self {
        let O = s.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

// Display 输出 O，保持用户可见原名。
impl std::fmt::Display for CIStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.O)
    }
}

// Public schema 状态；非此值索引不参与 checksum。
pub const StatePublic: i32 = 5;

/// 索引元数据子集，足够展开 Index 扫描。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexInfo {
    pub ID: i64,
    pub Name: CIStr,
    pub State: i32,
}

/// 单分区定义：ID + 名称。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionDefinition {
    pub ID: i64,
    pub Name: CIStr,
}

/// 分区定义列表；None 表示非分区表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionInfo {
    pub Definitions: Vec<PartitionDefinition>,
}

// 分区查找失败返回 0，由 GetPartitionByName 转成 Err。
impl PartitionInfo {
    /// 按小写名查 ID；未找到返回 0。
    pub fn GetPartitionIDByName(&self, name_l: &str) -> i64 {
        self.Definitions
            .iter()
            .find(|d| d.Name.L == name_l)
            .map(|d| d.ID)
            .unwrap_or(0)
    }
}

/// 表元数据最小子集；IsCommonHandle 影响范围编码路径。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableInfo {
    pub ID: i64,
    pub Name: CIStr,
    // 仅 Public 索引会展开请求。
    pub Indices: Vec<IndexInfo>,
    // Some 表示分区表。
    pub Partition: Option<PartitionInfo>,
    // clustered PK / common handle。
    pub IsCommonHandle: bool,
}

/// metautil.Table 桩，供 SetOldTable 生成 rewrite Rule。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaTable {
    // 旧表或元数据中的 TableInfo。
    pub Info: TableInfo,
}

/// 请求来源标记，透传到 kv.Request。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestSource {
    // 是否内部请求来源。
    pub RequestSourceInternal: bool,
    // 来源类型字符串。
    pub RequestSourceType: String,
    // 显式覆盖的来源类型。
    pub ExplicitRequestSourceType: String,
}

// kv.ReqTypeChecksum。
pub const ReqTypeChecksum: i64 = 105;
// 优先级常量，对齐 kv.Priority*。
pub const PriorityNormal: i32 = 0;
// checksum 默认低优先级。
pub const PriorityLow: i32 = 1;
// 高优先级占位，默认路径不用。
pub const PriorityHigh: i32 = 2;

/// 半开区间 [StartKey, EndKey)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyRange {
    // 范围下界（含）。
    pub StartKey: Vec<u8>,
    // 范围上界（不含）。
    pub EndKey: Vec<u8>,
}

/// 多段范围；桩中 FirstPartitionRange 返回全部。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyRanges {
    pub ranges: Vec<KeyRange>,
}

// 分区首段 API 在桩中退化为全量 ranges 视图。
impl KeyRanges {
    /// Go 按分区取首段；此处简化为整个 ranges。
    pub fn FirstPartitionRange(&self) -> &[KeyRange] {
        &self.ranges
    }
}

/// kv.Request 子集；Data 为 ChecksumRequest 序列化结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    // 请求类型；checksum 为 ReqTypeChecksum。
    pub Tp: i64,
    // 快照时间戳。
    pub StartTs: u64,
    // tipb.ChecksumRequest 编码字节。
    pub Data: Vec<u8>,
    // checksum 固定 true，避免污染 block cache。
    pub NotFillCache: bool,
    // 扫描并发。
    pub Concurrency: i32,
    // 通常为 PriorityLow。
    pub Priority: i32,
    // 资源组，可空。
    pub ResourceGroupName: String,
    // 来源透传。
    pub RequestSource: RequestSource,
    // 扫描键范围集合。
    pub KeyRanges: KeyRanges,
}

/// 会话变量桩：BackOffWeight / killed。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Variables {
    // 由 Executor.SetBackoffWeight 注入。
    pub BackOffWeight: i32,
    // 预留 kill 标志，桩路径未强依赖。
    pub killed: u32,
}

/// 构造 Variables，BackOffWeight 默认 0。
pub fn NewVariables(killed: &u32) -> Variables {
    Variables {
        BackOffWeight: 0,
        killed: *killed,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i32)]
/// tipb.ChecksumScanOn：Table=0 / Index=1。
pub enum ChecksumScanOn {
    #[default]
    Table = 0,
    Index = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i32)]
/// tipb.ChecksumAlgorithm：仅 Crc64_Xor。
pub enum ChecksumAlgorithm {
    #[default]
    Crc64_Xor = 0,
}

/// 键前缀重写：OldPrefix → NewPrefix。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumRewriteRule {
    // 旧键前缀。
    pub OldPrefix: Vec<u8>,
    // 新键前缀。
    pub NewPrefix: Vec<u8>,
}

/// tipb.ChecksumRequest 子集，Marshal 进 Request.Data。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumRequest {
    // 表或索引扫描。
    pub ScanOn: ChecksumScanOn,
    // 校验算法。
    pub Algorithm: ChecksumAlgorithm,
    // rewrite 时存在。
    pub Rule: Option<ChecksumRewriteRule>,
}

/// tipb.ChecksumResponse；Checksum XOR，Kvs/Bytes 累加。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumResponse {
    // XOR 聚合校验值。
    pub Checksum: u64,
    // 键值对计数累加。
    pub TotalKvs: u64,
    // 字节数累加。
    pub TotalBytes: u64,
}

// 手写 protobuf：字段 2=scan_on，3=algorithm，4=rule；scan_on 即使为 0 也写。
impl ChecksumRequest {
    /// 序列化为 tipb 兼容字节。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        // 字段号与 tipb.ChecksumRequest 对齐。
        // field 2 scan_on (enum) — write even when 0 to keep explicit Go assignment visible in tests
        write_tag(&mut out, 2, 0);
        write_varint(&mut out, self.ScanOn as u64);
        // algorithm 当前固定解析为 Crc64_Xor。
        // field 3 algorithm
        write_tag(&mut out, 3, 0);
        write_varint(&mut out, self.Algorithm as u64);
        if let Some(rule) = &self.Rule {
            let nested = rule.Marshal()?;
            write_tag(&mut out, 4, 2);
            write_varint(&mut out, nested.len() as u64);
            out.extend_from_slice(&nested);
        }
        Ok(out)
    }

    /// 跳过未知字段；短读报错。
    pub fn Unmarshal(data: &[u8]) -> Result<Self> {
        let mut msg = Self::default();
        let mut i = 0;
        while i < data.len() {
            let (tag, ni) = read_varint(data, i)?;
            i = ni;
            let field = (tag >> 3) as u32;
            let wt = (tag & 7) as u8;
            match (field, wt) {
                (2, 0) => {
                    let (v, ni) = read_varint(data, i)?;
                    i = ni;
                    msg.ScanOn = if v == 1 {
                        ChecksumScanOn::Index
                    } else {
                        ChecksumScanOn::Table
                    };
                }
                (3, 0) => {
                    let (_v, ni) = read_varint(data, i)?;
                    i = ni;
                    msg.Algorithm = ChecksumAlgorithm::Crc64_Xor;
                }
                (4, 2) => {
                    let (len, ni) = read_varint(data, i)?;
                    i = ni;
                    let end = checked_length_end(data, i, len, "short checksum rule")?;
                    // 嵌套 length-delimited rule 消息。
                    msg.Rule = Some(ChecksumRewriteRule::Unmarshal(&data[i..end])?);
                    i = end;
                }
                (_, 0) => {
                    let (_v, ni) = read_varint(data, i)?;
                    i = ni;
                }
                (_, 2) => {
                    let (len, ni) = read_varint(data, i)?;
                    i = checked_length_end(data, ni, len, "short length-delimited field")?;
                }
                // 未识别 wire type 直接失败，避免静默丢字段。
                _ => return Err(Error::new(format!("unsupported wire type {wt}"))),
            }
        }
        Ok(msg)
    }
}

// 字段 1=OldPrefix，2=NewPrefix（bytes）。
impl ChecksumRewriteRule {
    /// 编码 Old/New 前缀。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        if !self.OldPrefix.is_empty() {
            write_tag(&mut out, 1, 2);
            write_varint(&mut out, self.OldPrefix.len() as u64);
            out.extend_from_slice(&self.OldPrefix);
        }
        if !self.NewPrefix.is_empty() {
            write_tag(&mut out, 2, 2);
            write_varint(&mut out, self.NewPrefix.len() as u64);
            out.extend_from_slice(&self.NewPrefix);
        }
        Ok(out)
    }

    /// field1→OldPrefix，field2→NewPrefix。
    pub fn Unmarshal(data: &[u8]) -> Result<Self> {
        let mut msg = Self::default();
        let mut i = 0;
        while i < data.len() {
            let (tag, ni) = read_varint(data, i)?;
            i = ni;
            let field = (tag >> 3) as u32;
            let wt = (tag & 7) as u8;
            match (field, wt) {
                (1, 2) | (2, 2) => {
                    let (len, ni) = read_varint(data, i)?;
                    i = ni;
                    let end = checked_length_end(data, i, len, "short prefix bytes")?;
                    let bytes = data[i..end].to_vec();
                    i = end;
                    if field == 1 {
                        msg.OldPrefix = bytes;
                    } else {
                        msg.NewPrefix = bytes;
                    }
                }
                (_, 0) => {
                    let (_v, ni) = read_varint(data, i)?;
                    i = ni;
                }
                (_, 2) => {
                    let (len, ni) = read_varint(data, i)?;
                    i = checked_length_end(data, ni, len, "short length-delimited field")?;
                }
                _ => return Err(Error::new(format!("unsupported wire type {wt}"))),
            }
        }
        Ok(msg)
    }
}

// 字段 1/2/3；零值省略写入（类 omitempty）。
impl ChecksumResponse {
    /// 非零字段才写入。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        if self.Checksum != 0 {
            write_tag(&mut out, 1, 0);
            write_varint(&mut out, self.Checksum);
        }
        if self.TotalKvs != 0 {
            write_tag(&mut out, 2, 0);
            write_varint(&mut out, self.TotalKvs);
        }
        if self.TotalBytes != 0 {
            write_tag(&mut out, 3, 0);
            write_varint(&mut out, self.TotalBytes);
        }
        Ok(out)
    }

    /// 填充 Checksum/TotalKvs/TotalBytes。
    pub fn Unmarshal(data: &[u8]) -> Result<Self> {
        let mut msg = Self::default();
        let mut i = 0;
        while i < data.len() {
            let (tag, ni) = read_varint(data, i)?;
            i = ni;
            let field = (tag >> 3) as u32;
            let wt = (tag & 7) as u8;
            match (field, wt) {
                (1, 0) => {
                    let (v, ni) = read_varint(data, i)?;
                    i = ni;
                    msg.Checksum = v;
                }
                (2, 0) => {
                    let (v, ni) = read_varint(data, i)?;
                    i = ni;
                    msg.TotalKvs = v;
                }
                (3, 0) => {
                    let (v, ni) = read_varint(data, i)?;
                    i = ni;
                    msg.TotalBytes = v;
                }
                (_, 0) => {
                    let (_v, ni) = read_varint(data, i)?;
                    i = ni;
                }
                (_, 2) => {
                    let (len, ni) = read_varint(data, i)?;
                    i = checked_length_end(data, ni, len, "short length-delimited field")?;
                }
                _ => return Err(Error::new(format!("unsupported wire type {wt}"))),
            }
        }
        Ok(msg)
    }
}

// protobuf key = field<<3 | wire_type。
fn write_tag(out: &mut Vec<u8>, field: u32, wire_type: u8) {
    write_varint(out, ((field as u64) << 3) | (wire_type as u64));
}

// protobuf varint 编码。
fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
            out.push(b);
        } else {
            out.push(b);
            break;
        }
    }
}

// Validate protobuf length-delimited bounds without lossy casts or addition overflow.
fn checked_length_end(data: &[u8], start: usize, len: u64, message: &str) -> Result<usize> {
    let len = usize::try_from(len).map_err(|_| Error::new(message))?;
    let end = start.checked_add(len).ok_or_else(|| Error::new(message))?;
    if end > data.len() {
        return Err(Error::new(message));
    }
    Ok(end)
}

// 解码 varint；截断或溢出报错。
fn read_varint(data: &[u8], mut i: usize) -> Result<(u64, usize)> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if i >= data.len() {
            return Err(Error::new("truncated varint"));
        }
        let b = data[i];
        i += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok((result, i));
        }
        shift += 7;
        if shift > 63 {
            return Err(Error::new("varint overflow"));
        }
    }
}

// tablecodec 前缀：t / _r / _i 与符号位掩码。
const TABLE_PREFIX: &[u8] = b"t";
const RECORD_PREFIX_SEP: &[u8] = b"_r";
const INDEX_PREFIX_SEP: &[u8] = b"_i";
// 符号位翻转掩码，保证有序编码。
const SIGN_MASK: u64 = 0x8000_0000_0000_0000;

/// 有符号整数可比较编码：异或 SIGN_MASK 后大端 8 字节。
pub fn EncodeInt(mut b: Vec<u8>, v: i64) -> Vec<u8> {
    // 与 tablecodec.EncodeInt 相同变换。
    let u = (v as u64) ^ SIGN_MASK;
    b.extend_from_slice(&u.to_be_bytes());
    b
}

/// 表记录前缀 `t{id}_r`。
pub fn GenTableRecordPrefix(table_id: i64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 8 + 2);
    buf.extend_from_slice(TABLE_PREFIX);
    buf = EncodeInt(buf, table_id);
    buf.extend_from_slice(RECORD_PREFIX_SEP);
    buf
}

/// 索引前缀 `t{tid}_i{idx}`。
pub fn EncodeTableIndexPrefix(table_id: i64, idx_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(1 + 8 + 2 + 8);
    key.extend_from_slice(TABLE_PREFIX);
    key = EncodeInt(key, table_id);
    key.extend_from_slice(INDEX_PREFIX_SEP);
    EncodeInt(key, idx_id)
}

/// ranger 范围桩；由 RequestBuilder 加表/索引前缀。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangerRange {
    pub Low: Vec<u8>,
    pub High: Vec<u8>,
}

/// 整型 handle 全范围；本 crate 的 checksum 路径传入 signed=false。
pub fn FullIntRange(_is_unsigned: bool) -> Vec<RangerRange> {
    let low = EncodeInt(Vec::new(), i64::MIN);
    let high = prefix_next(EncodeInt(Vec::new(), i64::MAX));
    vec![RangerRange {
        Low: low,
        High: high,
    }]
}

/// Go `kv.Key.PrefixNext`: increment the last non-0xff byte; if every byte is
/// 0xff, keep the input and append 0x00.
fn prefix_next(mut key: Vec<u8>) -> Vec<u8> {
    for index in (0..key.len()).rev() {
        key[index] = key[index].wrapping_add(1);
        if key[index] != 0 {
            return key;
        }
    }
    key.fill(0xff);
    key.push(0);
    key
}

/// common-handle 非空全范围：MinNotNull(0x01) 到 MaxValue 的开上界(0xfb)。
pub fn FullNotNullRange() -> Vec<RangerRange> {
    vec![RangerRange {
        Low: vec![0x01],
        High: vec![0xfb],
    }]
}

/// 含空值全范围：Null(0x00) 到 MaxValue 的开上界(0xfb)。
pub fn FullRange() -> Vec<RangerRange> {
    vec![RangerRange {
        Low: vec![0x00],
        High: vec![0xfb],
    }]
}

/// distsql.RequestBuilder 桩；err 置位后 setter 短路，Build 返回错误。
#[derive(Default)]
pub struct RequestBuilder {
    // 正在组装的请求。
    pub Request: Request,
    // 延迟错误；Build 时 take。
    err: Option<Error>,
}

impl RequestBuilder {
    /// 范围加表记录前缀；is_common_handle 在桩中未分支。
    pub fn SetHandleRanges(
        &mut self,
        _dctx: Option<()>,
        tid: i64,
        _is_common_handle: bool,
        ranges: Vec<RangerRange>,
    ) -> &mut Self {
        // 每段 Low/High 都拼到同一表前缀后。
        let prefix = GenTableRecordPrefix(tid);
        self.Request.KeyRanges = KeyRanges {
            ranges: ranges
                .into_iter()
                .map(|r| {
                    let mut start = prefix.clone();
                    start.extend_from_slice(&r.Low);
                    let mut end = prefix.clone();
                    end.extend_from_slice(&r.High);
                    KeyRange {
                        StartKey: start,
                        EndKey: end,
                    }
                })
                .collect(),
        };
        self
    }

    /// 范围加索引前缀。
    pub fn SetIndexRanges(
        &mut self,
        _dctx: Option<()>,
        tid: i64,
        idx_id: i64,
        ranges: Vec<RangerRange>,
    ) -> &mut Self {
        // 索引前缀含 tid 与 idx_id 两级编码。
        let prefix = EncodeTableIndexPrefix(tid, idx_id);
        self.Request.KeyRanges = KeyRanges {
            ranges: ranges
                .into_iter()
                .map(|r| {
                    let mut start = prefix.clone();
                    start.extend_from_slice(&r.Low);
                    let mut end = prefix.clone();
                    end.extend_from_slice(&r.High);
                    KeyRange {
                        StartKey: start,
                        EndKey: end,
                    }
                })
                .collect(),
        };
        self
    }

    /// 设置快照 StartTs。
    pub fn SetStartTS(&mut self, start_ts: u64) -> &mut Self {
        self.Request.StartTs = start_ts;
        self
    }

    /// Marshal 正文到 Data，并设 Tp=Checksum、NotFillCache。
    pub fn SetChecksumRequest(&mut self, checksum: &ChecksumRequest) -> &mut Self {
        if self.err.is_none() {
            self.Request.Tp = ReqTypeChecksum;
            // Marshal 失败写入 self.err，避免部分更新 Request。
            match checksum.Marshal() {
                Ok(data) => {
                    self.Request.Data = data;
                    self.Request.NotFillCache = true;
                }
                Err(e) => self.err = Some(e),
            }
        }
        self
    }

    /// 写入并发度。
    pub fn SetConcurrency(&mut self, concurrency: i32) -> &mut Self {
        self.Request.Concurrency = concurrency;
        self
    }

    /// 资源组名透传。
    pub fn SetResourceGroupName(&mut self, name: &str) -> &mut Self {
        self.Request.ResourceGroupName = name.to_string();
        self
    }

    /// 绑定 RequestSource。
    pub fn SetRequestSource(&mut self, source: RequestSource) -> &mut Self {
        self.Request.RequestSource = source;
        self
    }

    /// 返回累积错误或克隆 Request。
    pub fn Build(&mut self) -> Result<Request> {
        if let Some(err) = self.err.take() {
            return Err(err);
        }
        Ok(self.Request.clone())
    }
}

/// kv.Response 流接口桩。
pub trait Response: Send {
    // None 表示流结束。
    fn NextRaw(&mut self, ctx: &Context) -> Result<Option<Vec<u8>>>;
    // Close 错误可覆盖已成功读取的结果（由 executor 处理）。
    fn Close(&mut self) -> Result<()>;
}

/// kv.Client 发送接口桩；真实行为由测试实现提供。
pub trait Client: Send + Sync {
    fn Send(
        &self,
        ctx: &Context,
        req: &Request,
        vars: &Variables,
    ) -> Result<Option<Box<dyn Response>>>;
}

/// 对齐 distsql.Checksum：Send 后要求非 nil Response。
pub fn DistSQLChecksum(
    ctx: &Context,
    client: &dyn Client,
    req: &Request,
    vars: &Variables,
) -> Result<Box<dyn Response>> {
    let resp = client.Send(ctx, req, vars)?;
    match resp {
        Some(r) => Ok(r),
        None => Err(Error::new("client returns nil response")),
    }
}

/// 退避策略：下次等待与剩余尝试次数。
pub trait BackoffStrategy: Send {
    fn NextBackoff(&mut self, err: &Error) -> Duration;
    fn RemainingAttempts(&self) -> i32;
}

/// checksum 指数退避，封顶 max_delay_time。
pub struct ChecksumBackoffStrategy {
    // 剩余尝试；每次 NextBackoff 减一。
    remaining_attempts: i32,
    // 当前退避基数，倍增用。
    delay_time: Duration,
    // 封顶；构造时为 10s。
    max_delay_time: Duration,
}

/// 重试=ChecksumRetryTime，初始 1s，max 取 Go 默认 10s（非 30s 常量）。
pub fn NewChecksumBackoffStrategy() -> Box<dyn BackoffStrategy> {
    // 与 Go 注释一致：显式使用 10s 封顶而非 ChecksumMaxWaitInterval。
    // Go NewChecksumBackoffStrategy does not set WithMaxDelayTime, so the
    // NewBackoffStrategy default maxDelayTime (10s) applies.
    Box::new(ChecksumBackoffStrategy {
        remaining_attempts: ChecksumRetryTime,
        delay_time: ChecksumWaitInterval,
        max_delay_time: Duration::from_secs(10),
    })
}

impl BackoffStrategy for ChecksumBackoffStrategy {
    // 始终可重试（Go alwaysTrueFunc）；先倍增再扣减剩余次数。
    fn NextBackoff(&mut self, _err: &Error) -> Duration {
        // 延迟倍增后与 max 取较小者返回。
        // always-retry path for checksum (matches Go alwaysTrueFunc / doBackoff)
        self.delay_time = self.delay_time.saturating_mul(2);
        self.remaining_attempts -= 1;
        if self.delay_time > self.max_delay_time {
            self.max_delay_time
        } else {
            self.delay_time
        }
    }

    // 供 WithRetry 循环条件读取。
    fn RemainingAttempts(&self) -> i32 {
        self.remaining_attempts
    }
}

/// 剩余次数内重试；ctx.Done 或耗尽则合并错误返回；可跳过 sleep。
pub fn WithRetry<F>(
    ctx: &Context,
    mut retryable: F,
    mut backoff: Box<dyn BackoffStrategy>,
) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    // 收集每次失败，最终 join 或中途因 ctx.Done 返回。
    let mut errors: Vec<Error> = Vec::new();
    while backoff.RemainingAttempts() > 0 {
        match retryable() {
            Ok(()) => return Ok(()),
            Err(err) => {
                errors.push(err);
                // 取消优先于继续退避。
                if ctx.Done() {
                    return Err(join_errors(&errors));
                }
                let d = backoff.NextBackoff(errors.last().unwrap());
                // 测试可关闭真实 sleep。
                if !SKIP_BACKOFF_SLEEP.get() {
                    thread::sleep(d);
                }
            }
        }
    }
    Err(join_errors(&errors))
}

// 多次错误以 "; " 拼接。
fn join_errors(errors: &[Error]) -> Error {
    if errors.is_empty() {
        return Error::new("retry failed with no errors collected");
    }
    Error::new(
        errors
            .iter()
            .map(|e| e.msg.as_str())
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// 按名解析分区 ID；无分区错误保留历史拼写 "parition"。
pub fn GetPartitionByName(table_info: &TableInfo, name: &CIStr) -> Result<i64> {
    // 非分区表：Go 兼容错误模板。
    let Some(partition) = table_info.Partition.as_ref() else {
        // 保留 Go 文案，含 parition 拼写。
        return Err(Error::new(format!(
            "the table {}[id={}] does not have parition",
            table_info.Name.O, table_info.ID
        )));
    };
    let part_id = partition.GetPartitionIDByName(&name.L);
    // >0 命中；0 表示未找到。
    if part_id > 0 {
        return Ok(part_id);
    }
    // 有分区定义但名称不匹配。
    Err(Error::new(format!(
        "partition is not found in the table {}[id={}]",
        table_info.Name.O, table_info.ID
    )))
}
