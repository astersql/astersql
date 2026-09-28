// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! RawKV 批量写入客户端，对齐 Go `rawkv_client.go`。
//! 面向 restore 的非事务 RawKV 路径：缓冲去重后 BatchPut 到 TiKV。
//! PD/TiKV 边界用本地 trait 抽象（darwin-safe，无 kvproto/grpcio）。
//! 关键约束：同逻辑键去重保留更大 originTs，避免 resolved_ts 下重复键 panic。
//! Close/PutRest 语义与 Go 一致；默认 dialer 未配置时显式报错。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 与 Go `WithCustomTimeoutOption(10*time.Second)` 对齐的 PD/gRPC 超时。
pub const RAWKV_CUSTOM_TIMEOUT: Duration = Duration::from_secs(10);

pub type Result<T> = std::result::Result<T, Error>;

// 轻量错误类型：对齐 Go pingcap/errors 的文案透传，无堆栈依赖。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    // Trace 在本地模式为恒等；保留符号以便调用点与 Go 一致。
    pub fn Trace(err: Self) -> Self {
        err
    }

    // Errorf 兼容 Go errors.Errorf 调用习惯。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// 近似 Go `context.Context` 的取消令牌；Background/TODO 均可被 cancel。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    // 对应 context.Background：默认可取消容器。
    pub fn Background() -> Self {
        Self::default()
    }

    // 对应 context.TODO：语义同 Background，便于占位调用。
    pub fn TODO() -> Self {
        Self::default()
    }

    // 写入取消原因；后续 Err() 可观测。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    // 返回已记录的取消错误（若有）。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }
}

/// 替代 `tikv/client-go` Security：TLS CA/Cert/Key 透传给 dialer。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Security {
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
}

/// 替代 `rawkv.RawOption`：本包仅使用列族 setter。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawOption {
    pub column_family: Option<String>,
}

/// 对应 `rawkv.SetColumnFamily`，BatchPut 时附带目标 CF。
pub fn SetColumnFamily(cf: impl Into<String>) -> RawOption {
    RawOption {
        column_family: Some(cf.into()),
    }
}

/// RawKV 客户端接口：对齐 Go rawkv.Client 的 Get/Put/Batch*/Close。
pub trait RawkvClient: Send + Sync {
    fn Get(&self, ctx: &Context, key: &[u8], options: &[RawOption]) -> Result<Vec<u8>>;
    fn Put(&self, ctx: &Context, key: &[u8], value: &[u8], options: &[RawOption]) -> Result<()>;
    fn BatchGet(
        &self,
        ctx: &Context,
        keys: &[Vec<u8>],
        options: &[RawOption],
    ) -> Result<Vec<Vec<u8>>>;
    fn BatchPut(
        &self,
        ctx: &Context,
        keys: &[Vec<u8>],
        values: &[Vec<u8>],
        options: &[RawOption],
    ) -> Result<()>;
    fn Close(&self) -> Result<()>;
}

/// PD 拨号抽象：替换 tikv rawkv.NewClient，便于测试注入假客户端。
pub trait PdRawkvDialer: Send + Sync {
    fn NewClient(
        &self,
        ctx: &Context,
        pd_addrs: &[String],
        security: &Security,
        timeout: Duration,
    ) -> Result<Arc<dyn RawkvClient>>;
}

// 默认 dialer：未注入真实 PD 时显式失败，避免静默连网。
fn default_pd_dialer() -> Arc<dyn PdRawkvDialer> {
    Arc::new(UnconfiguredPdDialer)
}

// 未配置桩：返回明确错误，供单元测试断言边界。
struct UnconfiguredPdDialer;

impl PdRawkvDialer for UnconfiguredPdDialer {
    fn NewClient(
        &self,
        _ctx: &Context,
        _pd_addrs: &[String],
        _security: &Security,
        _timeout: Duration,
    ) -> Result<Arc<dyn RawkvClient>> {
        // darwin-safe 本地模式：无真实 PD/TiKV 依赖。
        Err(Error::new(
            "rawkv PD dialer not configured (darwin-safe local trait mode)",
        ))
    }
}

/// 创建 RawKV 客户端；超时固定为 [`RAWKV_CUSTOM_TIMEOUT`]。
/// 对齐 Go：`rawkv.NewClient(..., opt.WithCustomTimeoutOption(10*time.Second))`。
pub fn NewRawkvClient(
    ctx: &Context,
    pdAddrs: &[String],
    security: &Security,
) -> Result<Arc<dyn RawkvClient>> {
    NewRawkvClientWithDialer(ctx, pdAddrs, security, default_pd_dialer().as_ref())
}

/// 同 [`NewRawkvClient`]，但可注入 PdRawkvDialer（测试/本地 trait 模式）。
/// 始终传入 [`RAWKV_CUSTOM_TIMEOUT`]，与 Go 自定义超时选项对齐。
pub fn NewRawkvClientWithDialer(
    ctx: &Context,
    pdAddrs: &[String],
    security: &Security,
    dialer: &dyn PdRawkvDialer,
) -> Result<Arc<dyn RawkvClient>> {
    dialer.NewClient(ctx, pdAddrs, security, RAWKV_CUSTOM_TIMEOUT)
}

// 缓冲条目：保留完整带 TS 的 key/value，以及用于去重比较的 originTs。
struct KVPair {
    ts: u64,
    key: Vec<u8>,
    value: Vec<u8>,
}

/// 批量 RawKV 写入器：缓冲后按 cap 刷盘；非线程安全。
/// 用 TruncateTS 后的逻辑键做 map 去重，防止 resolved_ts 开启时重复键导致 TiKV panic。
pub struct RawKVBatchClient {
    // 目标列族名，随 BatchPut options 下发。
    cf: String,
    cap: i32,
    // size 统计不同逻辑键数量（覆盖写不递增）。
    size: i32,
    // use map to remove duplicate entry, cause duplicate entry will make tikv panic when
    // resolved_ts enabled.
    // see https://github.com/tikv/tikv/blob/a401f78bc86f7e6ea6a55ad9f453ae31be835b55/components/resolved_ts/src/cmd.rs#L204
    // 键为 TruncateTS(key)；值保留更大 ts 的原始 KV。
    kvs: HashMap<Vec<u8>, KVPair>,
    rawkvClient: Arc<dyn RawkvClient>,
}

/// 构造批量客户端；batchCount 为触发 BatchPut 的逻辑键容量。
pub fn NewRawKVBatchClient(rawkvClient: Arc<dyn RawkvClient>, batchCount: i32) -> RawKVBatchClient {
    RawKVBatchClient {
        cf: String::new(),
        cap: batchCount,
        size: 0,
        kvs: HashMap::new(),
        rawkvClient,
    }
}

impl RawKVBatchClient {
    /// 关闭底层 RawkvClient；忽略 Close 错误以匹配 Go 侧“尽力关闭”。
    pub fn Close(&self) {
        let _ = self.rawkvClient.Close();
    }

    /// 设置后续 BatchPut 使用的列族（default/write 等）。
    pub fn SetColumnFamily(&mut self, columnFamily: impl Into<String>) {
        self.cf = columnFamily.into();
    }

    /// 缓冲写入：满批则 BatchPut；同逻辑键仅在 originTs 更大时覆盖。
    pub fn Put(&mut self, ctx: &Context, key: &[u8], value: &[u8], originTs: u64) -> Result<()> {
        // 逻辑键去掉末尾 8 字节 TS，与 restore utils.TruncateTS 一致。
        let k = TruncateTS(key);
        let sk = k;
        if let Some(v) = self.kvs.get(&sk) {
            // 已有条目：仅当新 ts 更大才替换，旧版本不得覆盖新版本。
            if v.ts < originTs {
                self.kvs.insert(
                    sk,
                    KVPair {
                        ts: originTs,
                        key: key.to_vec(),
                        value: value.to_vec(),
                    },
                );
            }
        } else {
            // 新逻辑键：size+1；覆盖写不增加 size。
            self.kvs.insert(
                sk,
                KVPair {
                    ts: originTs,
                    key: key.to_vec(),
                    value: value.to_vec(),
                },
            );
            self.size += 1;
        }

        // 达到容量：刷盘并记录 batch size / 耗时直方图。
        if self.size >= self.cap {
            let mut keys = Vec::with_capacity(self.kvs.len());
            let mut values = Vec::with_capacity(self.kvs.len());
            for kv in self.kvs.values() {
                keys.push(kv.key.clone());
                values.push(kv.value.clone());
            }
            let start = Instant::now();
            let err =
                self.rawkvClient
                    .BatchPut(ctx, &keys, &values, &[SetColumnFamily(self.cf.clone())]);
            metrics::RawKVBatchPutBatchSize_Observe(&self.cf, self.kvs.len() as f64);
            metrics::RawKVBatchPutDurationSeconds_Observe(&self.cf, start.elapsed().as_secs_f64());
            if let Err(err) = err {
                // 失败不 reset，保留缓冲以便上层重试/观测。
                return Err(Error::Trace(err));
            }

            self.reset();
        }
        Ok(())
    }

    /// 刷出残余缓冲；size==0 时为 no-op。
    pub fn PutRest(&mut self, ctx: &Context) -> Result<()> {
        // 与 Put 满批路径相同：BatchPut + 指标 + 成功后 reset。
        if self.size > 0 {
            let mut keys = Vec::with_capacity(self.kvs.len());
            let mut values = Vec::with_capacity(self.kvs.len());
            for kv in self.kvs.values() {
                keys.push(kv.key.clone());
                values.push(kv.value.clone());
            }
            let start = Instant::now();
            let err =
                self.rawkvClient
                    .BatchPut(ctx, &keys, &values, &[SetColumnFamily(self.cf.clone())]);
            metrics::RawKVBatchPutBatchSize_Observe(&self.cf, self.kvs.len() as f64);
            metrics::RawKVBatchPutDurationSeconds_Observe(&self.cf, start.elapsed().as_secs_f64());
            if let Err(err) = err {
                return Err(Error::Trace(err));
            }

            self.reset();
        }
        Ok(())
    }

    // 清空缓冲与计数，供下次批次使用。
    fn reset(&mut self) {
        self.kvs = HashMap::new();
        self.size = 0;
    }
}

/// 内联 `restore/utils.TruncateTS`：去掉 key 末尾 8 字节时间戳。
/// 短于 8 字节则原样返回，避免越界。
fn TruncateTS(key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return Vec::new();
    }
    if key.len() < 8 {
        return key.to_vec();
    }
    key[..key.len() - 8].to_vec()
}

/// 轻量 metrics 桩：收集 BatchPut 观测值，无 prometheus 依赖。
pub mod metrics {
    use std::sync::Mutex;

    #[derive(Clone, Debug, Default)]
    pub struct Observation {
        pub cf: String,
        pub value: f64,
    }

    static BATCH_SIZE: Mutex<Vec<Observation>> = Mutex::new(Vec::new());
    static DURATION: Mutex<Vec<Observation>> = Mutex::new(Vec::new());

    // 记录某 CF 一次 BatchPut 的条目数。
    pub fn RawKVBatchPutBatchSize_Observe(cf: &str, value: f64) {
        BATCH_SIZE.lock().unwrap().push(Observation {
            cf: cf.to_string(),
            value,
        });
    }

    // 记录某 CF 一次 BatchPut 的耗时（秒）。
    pub fn RawKVBatchPutDurationSeconds_Observe(cf: &str, value: f64) {
        DURATION.lock().unwrap().push(Observation {
            cf: cf.to_string(),
            value,
        });
    }

    // 测试辅助：取出并清空 batch size 观测。
    pub fn take_batch_size_observations() -> Vec<Observation> {
        std::mem::take(&mut *BATCH_SIZE.lock().unwrap())
    }

    // 测试辅助：取出并清空耗时观测。
    pub fn take_duration_observations() -> Vec<Observation> {
        std::mem::take(&mut *DURATION.lock().unwrap())
    }

    // 测试前清空，避免用例间污染。
    pub fn clear_observations() {
        BATCH_SIZE.lock().unwrap().clear();
        DURATION.lock().unwrap().clear();
    }
}
