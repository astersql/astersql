// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Local stand-ins for codec / PD / TiKV / HTTP boundaries (darwin-safe; no
//! kv/domain/kvproto/grpcio).
//!
//! 本文件为 key-locked 测试提供本地桩：codec/PD/TiKV/HTTP/TLS/配置边界。
//! 有意不依赖 kvproto/grpcio，保证 darwin 可编译；不是生产实现。
//! 能力边界：StubHttpClient 拒绝真实 GET；TLS 仅校验 CA 非空；全局配置进程内 Mutex。
//! 算法对齐：memcomparable EncodeBytes/DecodeBytes、EncodeInt 符号位翻转、ComposeTS。
//! 供 locker/parity 注入假依赖，勿把桩路径描述为已接通真实集群。

use std::cell::Cell;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 本测试 crate 统一 Result，错误类型为轻量 `Error`。
pub type Result<T> = std::result::Result<T, Error>;

/// 轻量错误：字符串消息，模拟 pingcap/errors 的 Trace/Annotate 链。
/// 非完整错误栈；仅满足测试断言与文案拼接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// 人类可读消息，可被 Annotate 前缀叠加。
    pub msg: String,
}

impl Error {
    /// 从任意字符串构造。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 对齐 Go `errors.Errorf` 命名的构造别名。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }

    /// Trace 桩：当前不附加栈，原样返回以便调用点保留。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// 以 `ctx: msg` 形式注解上下文，对齐 Go Annotate。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
        }
    }

    /// Annotatef 与 Annotate 同语义（格式化已在调用方完成）。
    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self::Annotate(err, ctx)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

// --- context ---
// 上下文桩：支持取消传播；WithTimeout 不真实计时，退化为 WithCancel。

/// 可取消上下文；`Done`/`Err` 读取共享取消原因。
#[derive(Clone, Default)]
pub struct Context {
    /// 取消原因；Some 表示已结束。
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    /// 空背景上下文，对齐 `context.Background`。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 超时桩：忽略 duration，行为等同 WithCancel（不自动超时）。
    pub fn WithTimeout(parent: &Self, _timeout: Duration) -> (Self, CancelFunc) {
        Self::WithCancel(parent)
    }

    /// 派生子上下文并返回 CancelFunc；继承父已有错误。
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            cancelled: Arc::new(Mutex::new(parent.Err())),
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

    /// 当前错误副本；None 表示仍活跃。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }

    /// 是否已取消/超时（本桩无真实超时）。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

/// 取消句柄；`cancel` 写入固定 "context canceled" 文案。
#[derive(Clone)]
pub struct CancelFunc {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl CancelFunc {
    /// 触发取消；消费 self 防止重复语义纠缠。
    pub fn cancel(self) {
        *self.cancelled.lock().unwrap() = Some(Error::new("context canceled"));
    }
}

// --- codec (memcomparable EncodeBytes / DecodeBytes) ---
// memcomparable 编解码：与 pkg/util/codec 及 Go locker 使用方式对齐。

pub mod codec {
    use super::{Error, Result};

    /// 每组 8 字节数据 + 1 字节 marker。
    const ENC_GROUP_SIZE: usize = 8;
    /// marker 基值；实际 marker = ENC_MARKER - pad_count。
    const ENC_MARKER: u8 = 0xff;
    /// 填充字节必须为 0。
    const ENC_PAD: u8 = 0x0;

    /// Matches `pkg/util/codec.EncodeBytes`.
    /// 按 8 字节分组追加，不足填 0，末尾写 marker；保证字典序可比较。
    pub fn EncodeBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
        let d_len = data.len();
        let realloc_size = (d_len / ENC_GROUP_SIZE + 1) * (ENC_GROUP_SIZE + 1);
        b.reserve(realloc_size);

        let mut idx = 0;
        while idx <= d_len {
            let remain = d_len - idx;
            let pad_count = if remain >= ENC_GROUP_SIZE {
                b.extend_from_slice(&data[idx..idx + ENC_GROUP_SIZE]);
                0usize
            } else {
                let pad_count = ENC_GROUP_SIZE - remain;
                b.extend_from_slice(&data[idx..]);
                b.extend(std::iter::repeat_n(ENC_PAD, pad_count));
                pad_count
            };
            let marker = ENC_MARKER - pad_count as u8;
            b.push(marker);
            idx += ENC_GROUP_SIZE;
        }
        b
    }

    /// Matches `pkg/util/codec.DecodeBytes`.
    /// 返回 (剩余输入, 解码输出)；校验 marker/padding，失败返回明确错误。
    pub fn DecodeBytes(mut b: &[u8], buf: Option<Vec<u8>>) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut out = buf.unwrap_or_else(|| Vec::with_capacity(b.len()));
        out.clear();
        loop {
            if b.len() < ENC_GROUP_SIZE + 1 {
                return Err(Error::new("insufficient bytes to decode value"));
            }
            let group_bytes = &b[..ENC_GROUP_SIZE + 1];
            let group = &group_bytes[..ENC_GROUP_SIZE];
            let marker = group_bytes[ENC_GROUP_SIZE];
            let pad_count = ENC_MARKER.wrapping_sub(marker);
            if pad_count as usize > ENC_GROUP_SIZE {
                return Err(Error::Errorf(format!(
                    "invalid marker byte, group bytes {group_bytes:?}"
                )));
            }
            let real_group_size = ENC_GROUP_SIZE - pad_count as usize;
            out.extend_from_slice(&group[..real_group_size]);
            b = &b[ENC_GROUP_SIZE + 1..];
            if pad_count != 0 {
                for v in &group[real_group_size..] {
                    if *v != ENC_PAD {
                        return Err(Error::Errorf(format!(
                            "invalid padding byte, group bytes {group_bytes:?}"
                        )));
                    }
                }
                break;
            }
        }
        Ok((b.to_vec(), out))
    }
}

// --- number / tablecodec / kv handle ---
// 整数与表记录键编码：符号位翻转保证有序，前缀形状对齐 tablecodec。

/// 有符号整数编码用的符号掩码（最高位翻转）。
const SIGN_MASK: u64 = 0x8000_0000_0000_0000;
/// 表键前缀字节 't'。
const TABLE_PREFIX: &[u8] = b"t";
/// 记录段分隔 "_r"。
const RECORD_PREFIX_SEP: &[u8] = b"_r";

/// 追加 8 字节大端有序整数（异或 SIGN_MASK）。
pub fn EncodeInt(mut b: Vec<u8>, v: i64) -> Vec<u8> {
    let u = (v as u64) ^ SIGN_MASK;
    b.extend_from_slice(&u.to_be_bytes());
    b
}

pub mod tablecodec {
    use super::{EncodeInt, RECORD_PREFIX_SEP, TABLE_PREFIX};

    /// GenTableRecordPrefix composes record prefix with tableID: "t[tableID]_r".
    /// 生成表记录前缀：`t` + EncodeInt(tableID) + `_r`。
    pub fn GenTableRecordPrefix(table_id: i64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + 8 + 2);
        buf.extend_from_slice(TABLE_PREFIX);
        buf = EncodeInt(buf, table_id);
        buf.extend_from_slice(RECORD_PREFIX_SEP);
        buf
    }

    /// EncodeRecordKey appends an int handle encoding onto the record prefix.
    /// 在前缀后追加行 ID 的有序整数编码。
    pub fn EncodeRecordKey(record_prefix: &[u8], row_id: i64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(record_prefix.len() + 8);
        buf.extend_from_slice(record_prefix);
        EncodeInt(buf, row_id)
    }
}

pub mod kv {
    /// IntHandle stand-in (Go `kv.IntHandle`).
    /// 整型句柄桩，仅占位对齐类型名。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct IntHandle(pub i64);
}

// --- metapb / router ---

pub mod metapb {
    /// Region 元数据精简字段：Id 与键范围。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Region {
        pub Id: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }
}

pub mod router {
    use super::metapb;

    /// 路由层 Region 包装，内含 Meta。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Region {
        pub Meta: metapb::Region,
    }
}

pub mod opt {
    /// Placeholder for PD GetRegion options.
    /// PD GetRegion 选项占位，当前无字段。
    #[derive(Clone, Debug, Default)]
    pub struct GetRegionOption;
}

// --- PD client ---
// PD 客户端 trait：测试实现可录制参数/注入失败。

/// PD 客户端最小面：region 查询与取 TS；非完整 pd.Client。
pub trait PdClient: Send + Sync {
    /// 按键查 region。
    fn GetRegion(
        &self,
        ctx: &Context,
        key: &[u8],
        opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>>;

    /// 查前一个 region。
    fn GetPrevRegion(
        &self,
        ctx: &Context,
        key: &[u8],
        opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>>;

    /// 按 region id 查询。
    fn GetRegionByID(
        &self,
        ctx: &Context,
        region_id: u64,
        opts: &[opt::GetRegionOption],
    ) -> Result<Option<router::Region>>;

    /// 扫一段 region；end 空表示无上界。
    fn ScanRegions(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
        limit: i32,
        opts: &[opt::GetRegionOption],
    ) -> Result<Vec<Option<router::Region>>>;

    /// 取物理/逻辑时间戳。
    fn GetTS(&self, ctx: &Context) -> Result<(i64, i64)>;
}

// --- oracle ---

pub mod oracle {
    /// 物理时间左移位数，对齐 tikv oracle。
    pub const PhysicalShiftBits: i64 = 18;

    /// 合成 startTs：physical<<18 | logical。
    pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
        ((physical as u64) << (PhysicalShiftBits as u64)) | (logical as u64)
    }
}

// --- TiKV RPC / storage boundaries ---

pub mod kvrpcpb {
    /// 变更操作；测试仅需 Put。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Op {
        Put = 1,
    }

    /// Prewrite mutation 三元组。
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Mutation {
        pub Op: Op,
        pub Key: Vec<u8>,
        pub Value: Vec<u8>,
    }

    /// Prewrite 请求精简字段。
    #[derive(Clone, Debug)]
    pub struct PrewriteRequest {
        pub Mutations: Vec<Mutation>,
        pub PrimaryLock: Vec<u8>,
        pub StartVersion: u64,
        pub LockTtl: u64,
    }

    /// Prewrite 响应占位（成功路径可为空结构）。
    #[derive(Clone, Debug, Default)]
    pub struct PrewriteResponse {}

    /// Region/Key 错误消息载体。
    #[derive(Clone, Debug, Default)]
    pub struct Error {
        pub message: String,
    }

    impl Error {
        /// 对齐 Go `Error.String()`。
        pub fn String(&self) -> String {
            self.message.clone()
        }
    }
}

pub mod tikvrpc {
    use super::kvrpcpb;

    /// RPC 命令枚举；本场景仅 Prewrite。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Cmd {
        Prewrite,
    }

    /// 封装命令与 Prewrite 载荷。
    #[derive(Clone, Debug)]
    pub struct Request {
        pub cmd: Cmd,
        pub prewrite: kvrpcpb::PrewriteRequest,
    }

    /// 构造 Request，对齐 `tikvrpc.NewRequest`。
    pub fn NewRequest(cmd: Cmd, prewrite: kvrpcpb::PrewriteRequest) -> Request {
        Request { cmd, prewrite }
    }

    /// 响应：可选 region_error 与 Prewrite Resp。
    #[derive(Clone, Debug, Default)]
    pub struct Response {
        pub region_error: Option<kvrpcpb::Error>,
        pub Resp: Option<kvrpcpb::PrewriteResponse>,
    }

    impl Response {
        /// 取出 region 错误；包装为 Result 以对齐 Go 签名。
        pub fn GetRegionError(&self) -> Result<Option<kvrpcpb::Error>, super::Error> {
            Ok(self.region_error.clone())
        }
    }
}

/// Region 版本三元组，用于 SendReq 路由。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionVerID {
    pub Id: u64,
    pub ConfVer: u64,
    pub Ver: u64,
}

/// 键所在 region 定位结果（含半开区间）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyLocation {
    pub Region: RegionVerID,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

/// 退避器桩：计数 sleeps，超过 64 次将错误 Trace 返回。
/// 不真实 sleep，避免单测变慢。
pub struct Backoffer {
    pub ctx: Context,
    pub max_sleep_ms: i32,
    pub sleeps: Mutex<i32>,
}

impl Backoffer {
    /// 创建退避器；max_sleep_ms 仅占位对齐 Go 参数。
    pub fn new(ctx: Context, max_sleep_ms: i32) -> Self {
        Self {
            ctx,
            max_sleep_ms,
            sleeps: Mutex::new(0),
        }
    }

    /// 记录一次退避；超限返回 Trace(err)。
    pub fn Backoff(&self, _reason: &str, err: Error) -> Result<()> {
        let mut sleeps = self.sleeps.lock().unwrap();
        *sleeps += 1;
        if *sleeps > 64 {
            return Err(Error::Trace(err));
        }
        Ok(())
    }
}

/// 工厂函数，对齐 Go `retry.NewBackoffer`。
pub fn NewBackoffer(ctx: Context, max_sleep_ms: i32) -> Backoffer {
    Backoffer::new(ctx, max_sleep_ms)
}

/// region miss 退避原因常量。
pub fn BoRegionMiss() -> &'static str {
    "regionMiss"
}

/// Region 缓存最小接口：按键定位。
pub trait RegionCache: Send + Sync {
    fn LocateKey(&self, bo: &Backoffer, key: &[u8]) -> Result<KeyLocation>;
}

/// TiKV Storage 最小面：取 cache + 发送 RPC。
/// 真实网络由测试注入实现；默认无内建集群。
pub trait Storage: Send + Sync {
    fn GetRegionCache(&self) -> &dyn RegionCache;
    fn SendReq(
        &self,
        bo: &Backoffer,
        req: tikvrpc::Request,
        region: RegionVerID,
        timeout: Duration,
    ) -> Result<tikvrpc::Response>;
}

// --- HTTP / TLS / config / model boundaries ---

/// schema JSON 中的表信息；仅反序列化 ID。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct TableInfo {
    #[serde(rename = "id", alias = "ID")]
    pub ID: i64,
}

/// HTTP GET 抽象，便于注入 MockHttp。
pub trait HttpClient: Send + Sync {
    fn DoGet(&self, ctx: &Context, url: &str) -> Result<(u16, Vec<u8>)>;
}

/// Default HTTP client stand-in: network boundary is not exercised in unit tests.
/// 默认 HTTP 桩：拒绝真实网络，提示注入 HttpClient。
#[derive(Default)]
pub struct StubHttpClient;

impl HttpClient for StubHttpClient {
    /// 始终错误，防止误连外网。
    fn DoGet(&self, _ctx: &Context, url: &str) -> Result<(u16, Vec<u8>)> {
        Err(Error::new(format!(
            "HTTP boundary stub: GET {url} (inject HttpClient for real network)"
        )))
    }
}

/// TLS 材料路径三元组；不加载真实证书文件内容。
#[derive(Clone, Debug, Default)]
pub struct TLSConfig {
    pub CA: String,
    pub Cert: String,
    pub Key: String,
}

impl TLSConfig {
    /// 仅校验 CA 非空；不建立 TLS 连接。
    pub fn ToTLSConfig(&self) -> Result<()> {
        if self.CA.is_empty() {
            return Err(Error::new("empty CA"));
        }
        Ok(())
    }
}

/// 全局 TiDB 集群 TLS 配置快照字段。
#[derive(Clone, Debug, Default)]
pub struct TidbConfig {
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
}

/// 进程内全局配置槽；非线程跨进程共享。
static GLOBAL_CONFIG: Mutex<Option<TidbConfig>> = Mutex::new(None);

/// 新建默认配置。
pub fn NewConfig() -> TidbConfig {
    TidbConfig::default()
}

/// 写入全局配置。
pub fn StoreGlobalConfig(cfg: TidbConfig) {
    *GLOBAL_CONFIG.lock().unwrap() = Some(cfg);
}

/// 取出并清空全局配置（测试断言副作用）。
pub fn TakeGlobalConfig() -> Option<TidbConfig> {
    GLOBAL_CONFIG.lock().unwrap().take()
}

// --- lightweight RNG matching math/rand.Intn usage ---
// 轻量可种子 RNG，对齐测试中对 math/rand.Intn 的使用。

thread_local! {
    /// 每线程 xorshift 状态。
    static RNG: Cell<u64> = const { Cell::new(0x4d59_5df4_d0f3_3173) };
}

/// 固定种子，保证 Prewrite Value 可复现。
pub fn seed_rng(seed: u64) {
    RNG.with(|c| c.set(seed));
}

fn next_u64() -> u64 {
    RNG.with(|c| {
        // xorshift64*
        // 线程局部推进状态并乘常数打散。
        let mut x = c.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        c.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    })
}

/// Matches Go `rand.Intn(n)` for n > 0.
/// 返回 `[0,n)`；n<=0 断言失败。
pub fn Intn(n: i32) -> i32 {
    assert!(n > 0);
    (next_u64() % n as u64) as i32
}
