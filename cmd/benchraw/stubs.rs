// Copyright 2026 AsterSQL.
//! Production adapters and test stand-ins for PD/TiKV/rawkv/flags/logging/HTTP boundaries.
//!
//! The default client factory uses `tikv-client` and the HTTP adapter binds a real socket.
//! In-memory clients and observable state remain available only as explicit test doubles.
//!
//! 为 `cmd/benchraw` 提供生产边界适配器与可注入的本地测试桩。
//! 生产路径使用真实 PD/TiKV RawKV 连接和 HTTP 监听；测试路径通过
//! 显式注入的内存客户端记录输入、暴露状态并注入失败。
//! 与 Go 版本对齐时，优先保持调用点看到的参数、返回值和失败分支语义一致。
//! 网络、证书、pprof 和 RawKV 集群能力均由生产适配器真实执行。

use std::env;
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pprof::protos::Message;

/// Error type standing in for TiDB / client-go errors at the benchraw boundary.
/// 统一承接 benchraw 触达外部依赖时可能返回的错误文本。
/// 这里不区分错误类别，只保留 Go 调用链最终会打印的消息内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 与 Go 侧 `errors.New`/包装后 `.Error()` 的文本载体保持一致。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 保留 Go 风格命名，便于翻译代码直接调用并生成相同错误文本。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Go `errors.Trace` — preserves the error for `terror.Log`.
/// Go 版本这里会保留堆栈包装；桩实现只要求把同一错误继续传给日志边界。
pub fn errors_trace(err: Error) -> Error {
    err
}

/// Go `terror.Log` — logs non-nil errors (recorded for tests).
/// 只在存在错误时追加记录，模拟 Go 中“忽略 nil、打印非 nil”的边界行为。
/// 记录落到全局缓冲区，方便测试断言后台 goroutine 是否上报过错误。
pub fn terror_log(err: Option<Error>) {
    if let Some(e) = err {
        log::error!("{}", e.msg);
        TERROR_LOGS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .push(e.msg);
    }
}

static TERROR_LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// 取出并清空已记录的 terror 日志，避免跨测试相互污染。
pub fn take_terror_logs() -> Vec<String> {
    TERROR_LOGS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .drain(..)
        .collect()
}

/// 语义上等价于“丢弃当前日志”，用于重置测试环境。
pub fn clear_terror_logs() {
    let _ = take_terror_logs();
}

/// Go `log.Fatal`: production exits with status 1; unit tests panic so assertions can catch it.
pub fn fatal(msg: impl AsRef<str>) -> ! {
    let msg = msg.as_ref();
    #[cfg(test)]
    panic!("{msg}");
    #[cfg(not(test))]
    {
        eprintln!("{msg}");
        std::process::exit(1);
    }
}

/// Go `log.Fatal("put failed", zap.Error(err))` message shape.
/// 拼接后的字符串形状要稳定，调用侧和测试都依赖这条报错文本。
pub fn fatal_put_failed(err: &Error) -> ! {
    fatal(format!("put failed: {}", err.Error()));
}

/// Go's default `flag.CommandLine` exits with status 2 for parse failures.
fn flag_error(msg: impl AsRef<str>) -> ! {
    let msg = msg.as_ref();
    #[cfg(test)]
    panic!("{msg}");
    #[cfg(not(test))]
    {
        eprintln!("{msg}");
        print_usage();
        std::process::exit(2);
    }
}

// --- zap / log level ---
// 只保留 benchraw 会设置的级别枚举，不引入完整 zap 依赖树。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Warn,
    Info,
    Debug,
    Error,
}

static CURRENT_LOG_LEVEL: OnceLock<Mutex<LogLevel>> = OnceLock::new();

struct BenchLogger;

impl log::Log for BenchLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static BENCH_LOGGER: BenchLogger = BenchLogger;

/// Go `log.SetLevel(zap.WarnLevel)`.
/// 保存最近一次设置的级别，供测试确认主流程是否按 Go 版本初始化日志。
pub fn set_log_level(level: LogLevel) {
    *CURRENT_LOG_LEVEL
        .get_or_init(|| Mutex::new(LogLevel::Warn))
        .lock()
        .unwrap() = level;

    let filter = match level {
        LogLevel::Warn => log::LevelFilter::Warn,
        LogLevel::Info => log::LevelFilter::Info,
        LogLevel::Debug => log::LevelFilter::Debug,
        LogLevel::Error => log::LevelFilter::Error,
    };
    let _ = log::set_logger(&BENCH_LOGGER);
    log::set_max_level(filter);
}

/// 默认值选择 Warn，与 Go 主函数中的初始化保持一致。
pub fn get_log_level() -> LogLevel {
    *CURRENT_LOG_LEVEL
        .get_or_init(|| Mutex::new(LogLevel::Warn))
        .lock()
        .unwrap()
}

// --- flag (Go `flag` package subset) ---
// 这里只覆盖 benchraw 实际声明的参数，不实现通用命令行解析器。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flags {
    pub data_cnt: i64,
    pub worker_cnt: i64,
    pub pd_addr: String,
    pub value_size: i64,
    pub ssl_ca: String,
    pub ssl_cert: String,
    pub ssl_key: String,
}

impl Default for Flags {
    /// 默认值必须与 Go 里的 `flag.*` 声明完全对齐，避免基准参数漂移。
    fn default() -> Self {
        Self {
            data_cnt: 1_000_000,
            worker_cnt: 100,
            pd_addr: "localhost:2379".to_string(),
            value_size: 5,
            ssl_ca: String::new(),
            ssl_cert: String::new(),
            ssl_key: String::new(),
        }
    }
}

/// Parse argv like Go `flag.Parse` for the flags benchraw defines.
/// 支持 `-k v` 与 `-k=v` 两种写法，匹配 Go `flag` 包的常见输入形式。
/// 未知参数和缺失取值会立即以 flag 错误中止，生产退出码与 Go 一样为 2。
pub fn parse_flags(args: &[String]) -> Option<Flags> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Go's flag package stops parsing at the first non-flag argument (and at "-").
        // Everything after that point is positional, even when it starts with a dash.
        if !a.starts_with('-') || a == "-" || a == "--" {
            break;
        }
        let (key, inline_val) = if let Some(rest) = a.strip_prefix('-') {
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            if let Some((k, v)) = rest.split_once('=') {
                (k, Some(v.to_string()))
            } else {
                (rest, None)
            }
        } else {
            unreachable!("non-flag arguments stop parsing above")
        };
        if key.is_empty() || key.starts_with('-') || key.starts_with('=') {
            flag_error(format!("bad flag syntax: {a}"));
        }

        // 解析顺序保持与 Go `flag.Parse` 接近：优先用内联值，否则消费下一个 argv。
        let take_val = |inline: Option<String>, idx: &mut usize| -> String {
            if let Some(v) = inline {
                return v;
            }
            *idx += 1;
            args.get(*idx)
                .cloned()
                .unwrap_or_else(|| flag_error(format!("flag needs an argument: -{key}")))
        };

        match key {
            "N" => {
                // 数据量直接控制总写入次数，非法数字需要尽早中止而不是静默回退。
                let v = take_val(inline_val, &mut i);
                flags.data_cnt = v
                    .parse()
                    .unwrap_or_else(|_| flag_error(format!("invalid value \"{v}\" for flag -N")));
            }
            "C" => {
                // 并发数决定分片粒度，保持与 Go 一样的整数解析失败路径。
                let v = take_val(inline_val, &mut i);
                flags.worker_cnt = v
                    .parse()
                    .unwrap_or_else(|_| flag_error(format!("invalid value \"{v}\" for flag -C")));
            }
            "pd" => flags.pd_addr = take_val(inline_val, &mut i),
            "V" => {
                // value 大小后续用于 `make([]byte, valueSize)` 的等价 Rust 分配。
                let v = take_val(inline_val, &mut i);
                flags.value_size = v
                    .parse()
                    .unwrap_or_else(|_| flag_error(format!("invalid value \"{v}\" for flag -V")));
            }
            "cacert" => flags.ssl_ca = take_val(inline_val, &mut i),
            "cert" => flags.ssl_cert = take_val(inline_val, &mut i),
            "key" => flags.ssl_key = take_val(inline_val, &mut i),
            "h" | "help" => {
                print_usage();
                return None;
            }
            _ => {
                flag_error(format!("flag provided but not defined: -{key}"));
            }
        }
        i += 1;
    }
    Some(flags)
}

/// Go `flag.PrintDefaults` for benchraw flags.
/// 文本布局尽量贴近 Go 标准库输出，便于脚本和人工比对帮助信息。
pub fn print_defaults() {
    let d = Flags::default();
    let mut out = io::stderr().lock();
    let _ = writeln!(
        out,
        "  -C int\n    \tconcurrent num (default {})",
        d.worker_cnt
    );
    let _ = writeln!(out, "  -N int\n    \tdata num (default {})", d.data_cnt);
    let _ = writeln!(
        out,
        "  -V int\n    \tvalue size in byte (default {})",
        d.value_size
    );
    let _ = writeln!(
        out,
        "  -cacert string\n    \tpath of file that contains list of trusted SSL CAs."
    );
    let _ = writeln!(
        out,
        "  -cert string\n    \tpath of file that contains X509 certificate in PEM format."
    );
    let _ = writeln!(
        out,
        "  -key string\n    \tpath of file that contains X509 key in PEM format."
    );
    let _ = writeln!(
        out,
        "  -pd string\n    \tpd address:localhost:2379 (default \"{}\")",
        d.pd_addr
    );
}

/// Default `flag.CommandLine` usage header followed by its registered defaults.
pub fn print_usage() {
    let program = env::args().next().unwrap_or_else(|| "benchraw".to_string());
    let _ = writeln!(io::stderr().lock(), "Usage of {program}:");
    print_defaults();
}

/// 主流程读取进程参数时会跳过 argv[0]，与 Go `flag.Parse` 的输入集合一致。
pub fn args_from_env() -> Vec<String> {
    env::args().skip(1).collect()
}

// --- config.Security ---
// 仅保留 rawkv.NewClient 真正读取到的 TLS 三元组。

/// Go `config.Security` subset used by rawkv.NewClient.
/// 字段名沿用 Go 导出字段，减少翻译代码在构造字面量时的改动。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Security {
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
}

// --- rawkv client ---
// benchraw 的核心外部依赖是 RawKV Put；这里围绕该调用建立可观测桩。

/// One recorded Put call.
/// 每次写入都同时保留 key 与 value，方便测试校验分片和负载内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutCall {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// Shared put log for recording clients / default stub.
/// 多个客户端实例可以共享同一日志，模拟 Go 中并发 worker 写向同一个集群。
#[derive(Clone, Default)]
pub struct PutLog {
    inner: Arc<Mutex<Vec<PutCall>>>,
}

impl PutLog {
    /// 创建独立日志，适合单测隔离使用。
    pub fn new() -> Self {
        Self::default()
    }

    /// 与另一个日志共享底层缓冲区，用于把默认工厂创建出的客户端聚合到全局观察点。
    pub fn with_shared(shared: &PutLog) -> Self {
        Self {
            inner: Arc::clone(&shared.inner),
        }
    }

    /// 记录一次 Put 调用，不做任何键值校验，因为真实校验属于 TiKV 边界。
    pub fn record(&self, key: Vec<u8>, value: Vec<u8>) {
        self.inner.lock().unwrap().push(PutCall { key, value });
    }

    /// 返回快照而不是借用，避免测试读取时持有锁影响并发写入。
    pub fn calls(&self) -> Vec<PutCall> {
        self.inner.lock().unwrap().clone()
    }

    /// 提供轻量统计接口，让测试无需复制整份日志即可断言写入次数。
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// 清空历史调用，通常在复用全局日志前执行。
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
}

/// RawKV client surface matching `rawkv.Client.Put`.
/// trait 只暴露 benchraw 真正用到的方法，避免把未实现能力误导成已支持。
pub trait RawKvClient: Send + Sync {
    fn Put(&self, key: Vec<u8>, value: Vec<u8>) -> Option<Error>;
}

/// Production RawKV adapter backed by the asynchronous TiKV Rust client.
struct TikvRawKvClient {
    client: tikv_client::RawClient,
    runtime: tokio::runtime::Runtime,
}

impl RawKvClient for TikvRawKvClient {
    fn Put(&self, key: Vec<u8>, value: Vec<u8>) -> Option<Error> {
        self.runtime
            .block_on(self.client.put(key, value))
            .err()
            .map(|err| Error::new(err.to_string()))
    }
}

/// In-memory RawKV test double that records Puts.
/// 该桩默认成功并记录写入；开启失败开关后则返回固定错误，模拟下游故障。
#[derive(Clone)]
pub struct StubRawKvClient {
    pub put_log: PutLog,
    pub fail_put: Arc<AtomicBool>,
    pub fail_put_msg: Arc<Mutex<String>>,
    pub pd_addrs: Vec<String>,
    pub security: Security,
}

impl StubRawKvClient {
    /// 创建独立客户端，保留传入的 PD 地址和 TLS 配置供测试回看。
    pub fn new(pd_addrs: Vec<String>, security: Security) -> Self {
        Self {
            put_log: PutLog::new(),
            fail_put: Arc::new(AtomicBool::new(false)),
            fail_put_msg: Arc::new(Mutex::new("put failed".to_string())),
            pd_addrs,
            security,
        }
    }

    /// 允许调用方注入共享日志，把多个客户端实例的写入归并到同一观察面。
    pub fn with_put_log(pd_addrs: Vec<String>, security: Security, put_log: PutLog) -> Self {
        Self {
            put_log,
            fail_put: Arc::new(AtomicBool::new(false)),
            fail_put_msg: Arc::new(Mutex::new("put failed".to_string())),
            pd_addrs,
            security,
        }
    }

    /// 用原子开关切换失败模式，避免并发测试额外依赖外层锁。
    pub fn set_fail_put(&self, fail: bool) {
        self.fail_put.store(fail, Ordering::SeqCst);
    }
}

impl RawKvClient for StubRawKvClient {
    fn Put(&self, key: Vec<u8>, value: Vec<u8>) -> Option<Error> {
        // 失败路径优先返回，不记录写入，和真实客户端“请求未成功即无副作用”的预期一致。
        if self.fail_put.load(Ordering::SeqCst) {
            return Some(Error::new(self.fail_put_msg.lock().unwrap().clone()));
        }
        // 成功路径只记录请求，不做网络调用。
        self.put_log.record(key, value);
        None
    }
}

/// Factory used by `batch_raw_put` / tests to inject clients.
/// 用工厂函数替代直接构造，便于在测试中验证参数透传或替换失败行为。
pub type ClientFactory = Arc<
    dyn Fn(Vec<String>, Security) -> std::result::Result<Arc<dyn RawKvClient>, Error> + Send + Sync,
>;

static NEW_CLIENT_FAIL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
// 最近一次建连参数单独缓存，便于验证 `parse_flags` 与 `split_pd_addrs` 的串联结果。
static LAST_NEW_CLIENT: OnceLock<Mutex<Option<(Vec<String>, Security)>>> = OnceLock::new();
// 默认路径把所有成功 Put 汇总到这里，测试无需持有具体客户端实例。
static GLOBAL_PUT_LOG: OnceLock<PutLog> = OnceLock::new();

/// 默认客户端共享的全局写入日志，对应“通过默认工厂创建”的那条主流程。
pub fn global_put_log() -> &'static PutLog {
    GLOBAL_PUT_LOG.get_or_init(PutLog::new)
}

/// Force next / all `new_client` calls to fail with the given message (tests).
/// 这里是全局故障注入点，用来模拟 PD/TiKV 建连阶段失败。
pub fn set_new_client_fail(msg: Option<String>) {
    *NEW_CLIENT_FAIL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = msg;
}

/// 记录最近一次建客户端的参数，便于验证 split 结果和 TLS 透传是否正确。
pub fn last_new_client_args() -> Option<(Vec<String>, Security)> {
    LAST_NEW_CLIENT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone()
}

/// 清理最近一次建连参数，避免前一个用例遗留状态干扰后续断言。
pub fn clear_last_new_client_args() {
    *LAST_NEW_CLIENT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = None;
}

/// In-memory `rawkv.NewClient` test double.
/// 先保存输入参数，便于对齐测试核对 Go 调用点的重要边界。
/// 若配置了失败消息，则直接返回错误，不创建客户端实例。
/// 成功时返回共享全局日志的客户端，让默认路径的所有 Put 都能被集中观察。
pub fn new_client(
    pd_addrs: Vec<String>,
    security: Security,
) -> std::result::Result<Arc<dyn RawKvClient>, Error> {
    *LAST_NEW_CLIENT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some((pd_addrs.clone(), security.clone()));

    if let Some(msg) = NEW_CLIENT_FAIL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone()
    {
        return Err(Error::new(msg));
    }

    let client =
        StubRawKvClient::with_put_log(pd_addrs, security, PutLog::with_shared(global_put_log()));
    Ok(Arc::new(client))
}

fn real_new_client(
    pd_addrs: Vec<String>,
    security: Security,
) -> std::result::Result<Arc<dyn RawKvClient>, Error> {
    let mut config = tikv_client::Config::default();
    if !security.ClusterSSLCA.is_empty()
        || !security.ClusterSSLCert.is_empty()
        || !security.ClusterSSLKey.is_empty()
    {
        if security.ClusterSSLCA.is_empty()
            || security.ClusterSSLCert.is_empty()
            || security.ClusterSSLKey.is_empty()
        {
            return Err(Error::new(
                "cacert, cert and key must all be provided for a TLS RawKV connection",
            ));
        }
        config = config.with_security(
            security.ClusterSSLCA,
            security.ClusterSSLCert,
            security.ClusterSSLKey,
        );
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| Error::new(format!("create RawKV runtime: {err}")))?;
    let client = runtime
        .block_on(tikv_client::RawClient::new_with_config(pd_addrs, config))
        .map_err(|err| Error::new(err.to_string()))?;
    Ok(Arc::new(TikvRawKvClient { client, runtime }))
}

/// Production factory backed by a real PD/TiKV RawKV client.
pub fn default_client_factory() -> ClientFactory {
    Arc::new(real_new_client)
}

/// Explicit in-memory factory for deterministic unit tests.
pub fn stub_client_factory() -> ClientFactory {
    Arc::new(|addrs, sec| new_client(addrs, sec))
}

// --- HTTP ListenAndServe (pprof) ---
// main 中只关心 pprof 监听是否被触发以及错误是否会送进 terror.Log。

// 记录每次监听尝试使用的地址，等价于观察 Go 里传给 `ListenAndServe` 的实参。
static HTTP_LISTEN_ADDRS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
// 失败消息由测试预置，模拟后台 pprof 服务启动失败。
static HTTP_LISTEN_FAIL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
// 使用原子计数替代真实 goroutine 生命周期，只表达“启动动作发生过几次”。
static HTTP_STARTED: AtomicUsize = AtomicUsize::new(0);

/// 注入监听失败结果，模拟 `http.ListenAndServe` 返回错误。
pub fn set_http_listen_fail(msg: Option<String>) {
    *HTTP_LISTEN_FAIL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = msg;
}

/// 取出并清空已记录的监听地址，便于断言 pprof 是否使用固定端口。
pub fn take_http_listen_addrs() -> Vec<String> {
    HTTP_LISTEN_ADDRS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .drain(..)
        .collect()
}

/// 统计监听尝试次数，帮助测试确认后台启动逻辑确实执行过。
pub fn http_start_count() -> usize {
    HTTP_STARTED.load(Ordering::SeqCst)
}

/// 一次性重置 HTTP 相关全局状态，避免后台监听桩的观测值跨测试累积。
pub fn reset_http_state() {
    let _ = take_http_listen_addrs();
    HTTP_STARTED.store(0, Ordering::SeqCst);
    set_http_listen_fail(None);
}

/// Go `http.ListenAndServe(addr, nil)` equivalent.
/// Unit tests retain an observable non-binding double; production binds a socket and serves
/// pprof-compatible index, command-line, symbol and CPU profile endpoints.
pub fn listen_and_serve(addr: &str) -> Option<Error> {
    #[cfg(test)]
    {
        return listen_and_serve_stub(addr);
    }
    #[cfg(not(test))]
    {
        listen_and_serve_real(addr)
    }
}

#[cfg(test)]
fn listen_and_serve_stub(addr: &str) -> Option<Error> {
    HTTP_STARTED.fetch_add(1, Ordering::SeqCst);
    HTTP_LISTEN_ADDRS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .push(addr.to_string());
    if let Some(msg) = HTTP_LISTEN_FAIL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone()
    {
        return Some(Error::new(msg));
    }
    let _ = Duration::from_millis(0);
    None
}

pub(crate) fn listen_and_serve_real(addr: &str) -> Option<Error> {
    let bind_addr = addr
        .strip_prefix(':')
        .map(|port| format!("0.0.0.0:{port}"))
        .unwrap_or_else(|| addr.to_string());
    let server = match tiny_http::Server::http(&bind_addr) {
        Ok(server) => server,
        Err(err) => return Some(Error::new(err.to_string())),
    };

    loop {
        let request = match server.recv() {
            Ok(request) => request,
            Err(err) => return Some(Error::new(err.to_string())),
        };
        let url = request.url().to_string();
        let (status, content_type, body) = pprof_response(&url);
        let content_type = tiny_http::Header::from_bytes("Content-Type", content_type)
            .expect("static pprof content type is a valid HTTP header");
        let response = tiny_http::Response::from_data(body)
            .with_status_code(status)
            .with_header(content_type);
        if let Err(err) = request.respond(response) {
            log::warn!("pprof response failed: {err}");
        }
    }
}

pub(crate) fn pprof_response(url: &str) -> (u16, &'static str, Vec<u8>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    match path {
        "/debug/pprof/" => (
            200,
            "text/html; charset=utf-8",
            b"<html><body><a href=\"profile\">profile</a><br><a href=\"cmdline\">cmdline</a><br><a href=\"symbol\">symbol</a></body></html>\n".to_vec(),
        ),
        "/debug/pprof/cmdline" => (
            200,
            "application/octet-stream",
            std::env::args().collect::<Vec<_>>().join("\0").into_bytes(),
        ),
        "/debug/pprof/symbol" => (200, "text/plain; charset=utf-8", b"num_symbols: 0\n".to_vec()),
        "/debug/pprof/profile" => match cpu_profile(profile_seconds(query)) {
            Ok(profile) => (200, "application/octet-stream", profile),
            Err(err) => (500, "text/plain; charset=utf-8", err.msg.into_bytes()),
        },
        "/debug/pprof/trace" => (
            501,
            "text/plain; charset=utf-8",
            b"runtime trace is not supported by the Rust profiler\n".to_vec(),
        ),
        _ => (404, "text/plain; charset=utf-8", b"not found\n".to_vec()),
    }
}

fn profile_seconds(query: &str) -> u64 {
    query
        .split('&')
        .find_map(|item| item.strip_prefix("seconds="))
        .and_then(|value| value.parse().ok())
        .unwrap_or(30)
}

fn cpu_profile(seconds: u64) -> std::result::Result<Vec<u8>, Error> {
    let guard = pprof::ProfilerGuard::new(100).map_err(|err| Error::new(err.to_string()))?;
    std::thread::sleep(Duration::from_secs(seconds));
    let report = guard
        .report()
        .build()
        .map_err(|err| Error::new(err.to_string()))?;
    let profile = report.pprof().map_err(|err| Error::new(err.to_string()))?;
    let mut body = Vec::new();
    profile
        .encode(&mut body)
        .map_err(|err| Error::new(err.to_string()))?;
    Ok(body)
}

/// Split PD address list like Go `strings.Split(pdAddr, ",")`.
/// 不做 trim 或过滤空串，严格贴近 Go `strings.Split` 的原始分割语义。
/// 例如 `"a,,b"` 会得到 `["a", "", "b"]`，这样才能和 Go 侧边界条件一致。
pub fn split_pd_addrs(pd: &str) -> Vec<String> {
    pd.split(',').map(|s| s.to_string()).collect()
}
