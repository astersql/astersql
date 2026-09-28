// Copyright 2026 AsterSQL.
//! `benchkv` 的运行时边界适配层：生产默认连接 canonical TiKV driver、
//! Prometheus 默认注册表、真实 HTTP server/client；测试必须显式选择内存后端。
//! 两类后端共享同一组 Begin/Set/Commit/Rollback、指标和响应体接口，使主流程
//! 保持 Go `cmd/benchkv/main.go` 的控制流，同时隔离测试中的外部服务和端口。

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use astersql_kv::Storage as KvStorage;
use prometheus::Encoder;

/// Error type standing in for TiDB / terror / pingcap errors at the benchkv boundary.
/// 对 Go 侧多套错误封装做统一替身。
/// 这里故意只保留字符串消息，因为 benchkv 只会读取 `Error()` 文本、
/// 打日志或在致命路径中直接退出，不依赖错误码、堆栈或分类信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 允许各个桩入口像 Go 一样按需快速拼出错误文本。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 保留 Go 风格命名，便于主流程和测试直接对照 `err.Error()` 写法。
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

/// Go `errors.Trace` — identity for stub errors.
/// Go 里这里通常会追加堆栈；桩版本保持恒等，确保错误值能继续沿调用链透传。
pub fn trace(err: Option<Error>) -> Option<Error> {
    err
}

/// Go `terror.MustNil` — panic (process-fatal) when `err` is present.
/// Rust 里用 panic 代替进程退出，既保留“致命失败立即中断”语义，
/// 又允许 parity test 通过 `catch_unwind` 抓住该分支。
pub fn must_nil(err: Option<Error>) {
    if let Some(e) = err {
        fatal(e.Error());
    }
}

/// Go `terror.Log` — record/log when err is present; no-op on None.
/// 这里只负责把文本落到内存日志池，不承担真正的日志格式化或输出后端职责。
pub fn terror_log(err: Option<Error>) {
    if let Some(e) = err {
        log_error("terror", e.Error());
    }
}

/// Go `terror.Call(fn)` — invoke and log any returned error.
/// 用于对齐 Go 里“执行清理回调，并在失败时只记日志不再覆盖主错误”的模式。
pub fn terror_call<F>(f: F)
where
    F: FnOnce() -> Option<Error>,
{
    terror_log(f());
}

/// Go `log.Fatal` — process exit semantics via panic (catchable in tests).
/// 选择 panic 而不是 `std::process::exit`，是为了让单元测试能验证致命分支。
pub fn fatal(msg: impl AsRef<str>) -> ! {
    panic!("{}", msg.as_ref());
}

/// Go `log.Fatal` production behavior: write the error and terminate with status 1.
pub fn fatal_process(msg: impl AsRef<str>) -> ! {
    eprintln!("{}", msg.as_ref());
    std::process::exit(1);
}

/// Logged messages for parity assertions.
/// 用全局 `OnceLock + Mutex` 保存日志文本，模拟“写过日志”这一可观测副作用。
/// benchkv 只关心某条错误是否被记录，不需要真实 logger 的异步刷盘或字段编码。
fn logged() -> &'static Mutex<Vec<String>> {
    static LOG: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(Vec::new()))
}

/// 测试读取并清空当前积累的日志，避免相邻场景互相污染。
pub fn take_logs() -> Vec<String> {
    std::mem::take(&mut *logged().lock().unwrap())
}

/// 在进入新场景前显式清理日志缓冲，等价于测试里重置观察窗口。
pub fn clear_logs() {
    logged().lock().unwrap().clear();
}

/// 统一的错误文本落盘入口，让不同 Go 风格包装最终收敛到同一份副作用。
pub fn log_error(context: &str, msg: &str) {
    eprintln!("{context}: {msg}");
    logged().lock().unwrap().push(format!("{context}: {msg}"));
}

/// Go `log.Error("function call errored", zap.Error(err), ...)`.
/// 只保留 benchkv 真正断言的上下文前缀和错误消息，省略 zap 结构化字段细节。
pub fn log_function_call_errored(err: &Error) {
    log_error("function call errored", err.Error());
}

/// benchkv 目前只需要 Error 级别，因此枚举刻意保持最小闭包。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
}

static LOG_LEVEL: OnceLock<Mutex<LogLevel>> = OnceLock::new();

/// Go `log.SetLevel(zap.ErrorLevel)`.
/// 保存最近一次设置结果，供入口函数和 parity test 校验主流程副作用。
pub fn set_log_level(level: LogLevel) {
    *LOG_LEVEL
        .get_or_init(|| Mutex::new(LogLevel::Error))
        .lock()
        .unwrap() = level;
}

/// 读取当前日志级别，帮助测试验证入口是否把默认级别压到 error。
/// 这里也让日志配置从“只写不读”变成可断言的显式状态。
pub fn current_log_level() -> LogLevel {
    *LOG_LEVEL
        .get_or_init(|| Mutex::new(LogLevel::Error))
        .lock()
        .unwrap()
}

// --- flag (Go `flag` package subset) ---
// 这里只实现 benchkv 自己声明的四个 flag。
// 目标是保持 CLI 契约兼容，而不是复刻 Go `flag` 包的全部解析规则。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flags {
    /// `-N` data num
    pub data_cnt: i64,
    /// `-C` concurrent num
    pub worker_cnt: i64,
    /// `-pd` PD address
    pub pd_addr: String,
    /// `-V` value size in byte
    pub value_size: i64,
}

impl Default for Flags {
    /// 默认值必须与 Go `var` 代码块一致，否则无参启动和文档都会发生漂移。
    fn default() -> Self {
        Self {
            data_cnt: 1_000_000,
            worker_cnt: 400,
            pd_addr: "localhost:2379".to_string(),
            value_size: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlagParseError {
    Help,
    Message(String),
}

/// Parse argv like Go `flag.Parse` without applying `ExitOnError` process policy.
pub fn try_parse_flags(args: &[String]) -> Result<Flags, FlagParseError> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Go flag.Parse 在 `--` 或第一个非 flag 参数处停止，不会继续解析后续参数。
        if a == "--" || a == "-" || !a.starts_with('-') {
            break;
        }
        // 去掉一个或两个前导短横线，允许 `-pd` 和 `--pd` 都落到同一分支。
        let (key, inline_val) = if let Some(rest) = a.strip_prefix('-') {
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            if let Some((k, v)) = rest.split_once('=') {
                (k, Some(v.to_string()))
            } else {
                (rest, None)
            }
        } else {
            unreachable!("non-flag arguments stop parsing above");
        };

        // 闭包统一处理“内联值优先，否则消费下一个 argv”这一规则。
        let take_val =
            |inline: Option<String>, idx: &mut usize| -> Result<String, FlagParseError> {
                if let Some(v) = inline {
                    return Ok(v);
                }
                *idx += 1;
                args.get(*idx).cloned().ok_or_else(|| {
                    FlagParseError::Message(format!("flag needs an argument: -{key}"))
                })
            };

        match key {
            "N" => {
                let v = take_val(inline_val, &mut i)?;
                flags.data_cnt = parse_go_int(&v).map_err(|_| {
                    FlagParseError::Message(format!("invalid value \"{v}\" for flag -N"))
                })?;
            }
            "C" => {
                let v = take_val(inline_val, &mut i)?;
                flags.worker_cnt = parse_go_int(&v).map_err(|_| {
                    FlagParseError::Message(format!("invalid value \"{v}\" for flag -C"))
                })?;
            }
            "pd" => flags.pd_addr = take_val(inline_val, &mut i)?,
            "V" => {
                let v = take_val(inline_val, &mut i)?;
                flags.value_size = parse_go_int(&v).map_err(|_| {
                    FlagParseError::Message(format!("invalid value \"{v}\" for flag -V"))
                })?;
            }
            "h" | "help" => return Err(FlagParseError::Help),
            _ => {
                return Err(FlagParseError::Message(format!(
                    "flag provided but not defined: -{key}"
                )));
            }
        }
        i += 1;
    }
    Ok(flags)
}

/// Test-friendly parser: preserve fatal semantics as a catchable panic.
pub fn parse_flags(args: &[String]) -> Flags {
    match try_parse_flags(args) {
        Ok(flags) => flags,
        Err(FlagParseError::Help) => fatal("flag: help requested"),
        Err(FlagParseError::Message(message)) => fatal(message),
    }
}

/// Production `flag.CommandLine` policy: help exits 0; parse errors exit 2.
pub fn parse_flags_or_exit(args: &[String]) -> Flags {
    match try_parse_flags(args) {
        Ok(flags) => flags,
        Err(FlagParseError::Help) => {
            print_usage();
            std::process::exit(0);
        }
        Err(FlagParseError::Message(message)) => {
            eprintln!("{message}");
            print_usage();
            std::process::exit(2);
        }
    }
}

/// Parse an `int` flag with Go's `strconv.ParseInt(value, 0, IntSize)` rules.
fn parse_go_int(value: &str) -> Result<i64, ()> {
    let (negative, unsigned) = match value.as_bytes().first() {
        Some(b'+') => (false, &value[1..]),
        Some(b'-') => (true, &value[1..]),
        Some(_) => (false, value),
        None => return Err(()),
    };
    if unsigned.is_empty() {
        return Err(());
    }

    let (radix, digits) = if unsigned.starts_with("0b") || unsigned.starts_with("0B") {
        (2, &unsigned[2..])
    } else if unsigned.starts_with("0o") || unsigned.starts_with("0O") {
        (8, &unsigned[2..])
    } else if unsigned.starts_with("0x") || unsigned.starts_with("0X") {
        (16, &unsigned[2..])
    } else if let Some(rest) = unsigned.strip_prefix('0') {
        (8, rest)
    } else {
        (10, unsigned)
    };

    if value.contains('_') && !go_integer_underscores_are_valid(value) {
        return Err(());
    }
    let normalized: String = digits.chars().filter(|c| *c != '_').collect();
    let magnitude = if normalized.is_empty() && unsigned == "0" {
        0
    } else {
        u64::from_str_radix(&normalized, radix).map_err(|_| ())?
    };

    if negative {
        if magnitude == (i64::MAX as u64) + 1 {
            Ok(i64::MIN)
        } else {
            i64::try_from(magnitude).map(|v| -v).map_err(|_| ())
        }
    } else {
        i64::try_from(magnitude).map_err(|_| ())
    }
}

/// Match the underscore placement accepted by Go integer literals.
fn go_integer_underscores_are_valid(value: &str) -> bool {
    let unsigned = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    let prefix_len = if unsigned.starts_with("0b")
        || unsigned.starts_with("0B")
        || unsigned.starts_with("0o")
        || unsigned.starts_with("0O")
        || unsigned.starts_with("0x")
        || unsigned.starts_with("0X")
    {
        2
    } else {
        0
    };
    let mut previous_was_digit_or_prefix = prefix_len == 2;
    for (index, ch) in unsigned.char_indices() {
        if index < prefix_len {
            continue;
        }
        if ch == '_' {
            if !previous_was_digit_or_prefix {
                return false;
            }
            previous_was_digit_or_prefix = false;
        } else {
            previous_was_digit_or_prefix = ch.is_ascii_alphanumeric();
        }
    }
    previous_was_digit_or_prefix
}

/// Go `flag.PrintDefaults` for benchkv flags.
/// 输出文本保持与 Go 帮助信息同一组字段和顺序，方便人工对照命令行帮助。
pub fn print_defaults() {
    let mut out = io::stderr().lock();
    print_defaults_to(&mut out);
}

fn print_usage() {
    let program = env::args()
        .next()
        .unwrap_or_else(|| "astersql-cmd-benchkv".to_owned());
    let mut out = io::stderr().lock();
    let _ = writeln!(out, "Usage of {program}:");
    print_defaults_to(&mut out);
}

fn print_defaults_to(out: &mut dyn Write) {
    let d = Flags::default();
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
        "  -pd string\n    \tpd address:localhost:2379 (default \"{}\")",
        d.pd_addr
    );
}

/// 真实二进制入口从环境采集参数，测试则可直接绕过该函数传显式切片。
pub fn args_from_env() -> Vec<String> {
    env::args().skip(1).collect()
}

// --- TiKV driver / storage / transaction ---
// 这一段不是通用 KV 实现，而是为 `benchkv` 压测循环提供最小事务表面。
// 核心关注点是 Begin/Set/Commit/Rollback 的时序、副作用计数和可注入失败。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriverMode {
    Stub,
    Production,
}

#[derive(Clone, Debug)]
pub struct TiKVDriver {
    mode: DriverMode,
}

impl Default for TiKVDriver {
    fn default() -> Self {
        Self {
            mode: DriverMode::Production,
        }
    }
}

impl TiKVDriver {
    pub fn stub() -> Self {
        Self {
            mode: DriverMode::Stub,
        }
    }

    pub fn is_production(&self) -> bool {
        self.mode == DriverMode::Production
    }

    /// Go `driver.TiKVDriver.Open`.
    pub fn Open(&self, path: &str) -> (Storage, Option<Error>) {
        if path.is_empty() {
            return (Storage::default(), Some(Error::new("empty tikv path")));
        }
        match self.mode {
            DriverMode::Stub => (Storage::stub(path), None),
            DriverMode::Production => {
                let mut driver = astersql_store_driver::TiKVDriver::default();
                match driver.Open(path) {
                    Ok(store) => (Storage::production(path, store), None),
                    Err(error) => (Storage::default(), Some(Error::new(error.to_string()))),
                }
            }
        }
    }
}

/// 被 `Storage` 与 `Transaction` 共享的内部状态。
/// 所有统计信息都放在这里，便于测试在事务结束后直接观察副作用。
#[derive(Debug, Default)]
struct StorageInner {
    pub begins: u64,
    pub sets: Vec<(Vec<u8>, Vec<u8>)>,
    pub commits: u64,
    pub rollbacks: u64,
    pub fail_begin: bool,
    pub fail_commit_every: Option<u64>,
    pub fail_set: bool,
    pub next_commit_id: u64,
}

/// Opaque storage handle (Go `kv.Storage`).
/// 对外看起来像 Go 的存储句柄，内部则共享一份可加锁状态，便于多线程压测。
#[derive(Clone, Debug)]
enum StorageBackend {
    Stub(Arc<Mutex<StorageInner>>),
    Production(astersql_store_driver::TikvStore),
}

#[derive(Clone, Debug)]
pub struct Storage {
    pub path: String,
    backend: StorageBackend,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            path: String::new(),
            backend: StorageBackend::Stub(Arc::new(Mutex::new(StorageInner::default()))),
        }
    }
}

impl Storage {
    fn stub(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            backend: StorageBackend::Stub(Arc::new(Mutex::new(StorageInner::default()))),
        }
    }

    fn production(path: &str, store: astersql_store_driver::TikvStore) -> Self {
        Self {
            path: path.to_owned(),
            backend: StorageBackend::Production(store),
        }
    }

    pub fn is_production(&self) -> bool {
        matches!(self.backend, StorageBackend::Production(_))
    }

    /// Go `store.Begin()`.
    /// 每次开始事务都会先递增 begin 计数，这样即使后续人为注入失败，
    /// 测试仍能看到“调用方确实尝试过开启事务”。
    pub fn Begin(&self) -> (Transaction, Option<Error>) {
        match &self.backend {
            StorageBackend::Stub(inner) => {
                let mut g = inner.lock().unwrap();
                g.begins += 1;
                if g.fail_begin {
                    return (Transaction::invalid(), Some(Error::new("begin failed")));
                }
                let commit_id = g.next_commit_id;
                g.next_commit_id += 1;
                let fail_commit = matches!(
                    g.fail_commit_every,
                    Some(n) if n > 0 && (commit_id + 1) % n == 0
                );
                let fail_set = g.fail_set;
                (
                    Transaction::stub(Arc::clone(inner), fail_commit, fail_set),
                    None,
                )
            }
            StorageBackend::Production(store) => match KvStorage::Begin(store, &[]) {
                Ok(transaction) => (Transaction::production(transaction), None),
                Err(error) => (Transaction::invalid(), Some(Error::new(error.to_string()))),
            },
        }
    }

    /// 以下访问器都只暴露 benchkv 测试真正需要的可观测统计。
    pub fn begins(&self) -> u64 {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().begins,
            StorageBackend::Production(_) => 0,
        }
    }

    /// 返回已成功提交的键值写入，用来验证分片键空间和写入载荷。
    pub fn sets(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().sets.clone(),
            StorageBackend::Production(_) => Vec::new(),
        }
    }

    /// 成功提交次数只在 `Commit` 成功后增加，不把失败尝试计入吞吐。
    pub fn commits(&self) -> u64 {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().commits,
            StorageBackend::Production(_) => 0,
        }
    }

    /// 回滚计数帮助测试确认提交失败后是否走到了补偿路径。
    pub fn rollbacks(&self) -> u64 {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().rollbacks,
            StorageBackend::Production(_) => 0,
        }
    }

    /// 允许测试把所有 Begin 调用都打成失败，模拟致命启动路径。
    pub fn set_fail_begin(&self, v: bool) {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().fail_begin = v,
            StorageBackend::Production(_) => panic!("failure injection requires stub storage"),
        }
    }

    /// Fail every N-th commit attempt (1-based cadence via commit_id+1).
    /// 使用 1-based 节奏与人类阅读的“第 N 次失败”一致，避免测试心算偏移。
    pub fn set_fail_commit_every(&self, n: Option<u64>) {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().fail_commit_every = n,
            StorageBackend::Production(_) => panic!("failure injection requires stub storage"),
        }
    }

    /// 让 `Set` 直接失败，用来验证“记日志但仍继续控制流”的分支。
    pub fn set_fail_set(&self, v: bool) {
        match &self.backend {
            StorageBackend::Stub(inner) => inner.lock().unwrap().fail_set = v,
            StorageBackend::Production(_) => panic!("failure injection requires stub storage"),
        }
    }
}

enum TransactionBackend {
    Stub {
        store: Arc<Mutex<StorageInner>>,
        pending: Option<(Vec<u8>, Vec<u8>)>,
        fail_commit: bool,
        fail_set: bool,
    },
    Production(Box<dyn astersql_kv::Transaction>),
    Invalid,
}

pub struct Transaction {
    backend: TransactionBackend,
    pub committed: bool,
    pub rolled_back: bool,
}

impl fmt::Debug for Transaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transaction")
            .field("committed", &self.committed)
            .field("rolled_back", &self.rolled_back)
            .finish_non_exhaustive()
    }
}

impl Transaction {
    fn stub(store: Arc<Mutex<StorageInner>>, fail_commit: bool, fail_set: bool) -> Self {
        Self {
            backend: TransactionBackend::Stub {
                store,
                pending: None,
                fail_commit,
                fail_set,
            },
            committed: false,
            rolled_back: false,
        }
    }

    fn production(transaction: Box<dyn astersql_kv::Transaction>) -> Self {
        Self {
            backend: TransactionBackend::Production(transaction),
            committed: false,
            rolled_back: false,
        }
    }

    fn invalid() -> Self {
        Self {
            backend: TransactionBackend::Invalid,
            committed: false,
            rolled_back: false,
        }
    }

    /// Go `txn.Set`.
    /// 这里先把写入挂到 `pending`，只有 Commit 成功后才真正落入存储统计。
    /// 这样可以精确模拟“提交失败不会留下成功写入”的外部效果。
    pub fn Set(&mut self, key: &[u8], value: &[u8]) -> Option<Error> {
        match &mut self.backend {
            TransactionBackend::Stub {
                pending, fail_set, ..
            } => {
                if *fail_set {
                    return Some(Error::new("set failed"));
                }
                *pending = Some((key.to_vec(), value.to_vec()));
                None
            }
            TransactionBackend::Production(transaction) => transaction
                .Set(astersql_kv::Key(key.to_vec()), value.to_vec())
                .err()
                .map(|error| Error::new(error.to_string())),
            TransactionBackend::Invalid => Some(Error::new("invalid transaction")),
        }
    }

    /// Go `txn.Commit(ctx)`.
    /// 提交失败时只返回错误，不偷偷写入数据，也不增加成功提交计数。
    pub fn Commit(&mut self) -> Option<Error> {
        match &mut self.backend {
            TransactionBackend::Stub {
                store,
                pending,
                fail_commit,
                ..
            } => {
                if *fail_commit {
                    return Some(Error::new("commit failed"));
                }
                let mut g = store.lock().unwrap();
                if let Some(kv) = pending.take() {
                    g.sets.push(kv);
                }
                g.commits += 1;
                self.committed = true;
                None
            }
            TransactionBackend::Production(transaction) => {
                match transaction.Commit(&astersql_kv::Context::default()) {
                    Ok(()) => {
                        self.committed = true;
                        None
                    }
                    Err(error) => Some(Error::new(error.to_string())),
                }
            }
            TransactionBackend::Invalid => Some(Error::new("invalid transaction")),
        }
    }

    /// Go `txn.Rollback()`.
    /// 回滚会清空待写缓存，并留下计数，便于测试确认失败分支确实清理过状态。
    pub fn Rollback(&mut self) -> Option<Error> {
        match &mut self.backend {
            TransactionBackend::Stub { store, pending, .. } => {
                let mut g = store.lock().unwrap();
                g.rollbacks += 1;
                self.rolled_back = true;
                *pending = None;
                None
            }
            TransactionBackend::Production(transaction) => match transaction.Rollback() {
                Ok(()) => {
                    self.rolled_back = true;
                    None
                }
                Err(error) => Some(Error::new(error.to_string())),
            },
            TransactionBackend::Invalid => Some(Error::new("invalid transaction")),
        }
    }
}

// --- prometheus subset ---
// benchkv 只需要能累计计数、记录观测值并导出一小段文本。
// 因此这里不实现真实注册表、收集器接口或 bucket 聚合算法。

/// Go `prometheus.ExponentialBuckets(0.0005, 2, 13)`.
/// 直接返回 bucket 边界列表，供测试验证与 Go 默认配置一致。
pub fn exponential_buckets(start: f64, factor: f64, count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count);
    let mut v = start;
    for _ in 0..count {
        out.push(v);
        v *= factor;
    }
    out
}

/// 以 label 拼接键为索引的简化计数器族。
/// 对 benchkv 来说只会用到单个 `"txn"` label，因此字符串键已足够表达。
#[derive(Clone, Debug, Default)]
pub struct CounterVec {
    name: String,
    counts: Arc<Mutex<HashMap<String, u64>>>,
    production: Option<prometheus::CounterVec>,
}

impl CounterVec {
    /// 保留指标名，主要为了让类型语义更接近真实 prometheus 对象。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            counts: Arc::new(Mutex::new(HashMap::new())),
            production: None,
        }
    }

    fn production(name: impl Into<String>, counter: prometheus::CounterVec) -> Self {
        Self {
            name: name.into(),
            counts: Arc::new(Mutex::new(HashMap::new())),
            production: Some(counter),
        }
    }

    /// Go `WithLabelValues` 会返回一个绑定了 label 的句柄；这里复现相同用法。
    /// label 顺序会直接编码进 key，因此调用方必须保持与 Go 相同的维度顺序。
    pub fn WithLabelValues(&self, labels: &[&str]) -> Counter {
        let key = labels.join(",");
        Counter {
            name: self.name.clone(),
            key,
            counts: Arc::clone(&self.counts),
            production: self
                .production
                .as_ref()
                .map(|counter| counter.with_label_values(labels)),
        }
    }

    /// 读取某组 label 的当前计数，供 parity test 直接断言外部结果。
    pub fn get(&self, labels: &[&str]) -> u64 {
        let key = labels.join(",");
        *self.counts.lock().unwrap().get(&key).unwrap_or(&0)
    }

    /// Return `None` until Go's metric vector has created this label child.
    fn get_existing(&self, labels: &[&str]) -> Option<u64> {
        let key = labels.join(",");
        self.counts.lock().unwrap().get(&key).copied()
    }

    /// 汇总所有 label 的总量，适合需要忽略细分维度时使用。
    pub fn total(&self) -> u64 {
        self.counts.lock().unwrap().values().sum()
    }
}

/// 绑定到特定 label 集合后的单个计数器句柄。
#[derive(Clone, Debug)]
pub struct Counter {
    name: String,
    key: String,
    counts: Arc<Mutex<HashMap<String, u64>>>,
    production: Option<prometheus::Counter>,
}

impl Counter {
    /// 与 Go `Inc` 一样只做自增，不暴露任意加值接口，保持 benchkv 当前需求最小化。
    /// 内部通过互斥锁串行化更新，足以支撑压测线程并发下的统计一致性。
    pub fn Inc(&self) {
        let _ = &self.name;
        *self
            .counts
            .lock()
            .unwrap()
            .entry(self.key.clone())
            .or_insert(0) += 1;
        if let Some(counter) = &self.production {
            counter.inc();
        }
    }
}

/// 简化版直方图族，按 label 保存原始观测值序列。
/// 不预先聚合 bucket，是因为 benchkv 现有测试只关心观测次数与导出占位文本。
#[derive(Clone, Debug, Default)]
pub struct HistogramVec {
    name: String,
    observations: Arc<Mutex<HashMap<String, Vec<f64>>>>,
    pub buckets: Vec<f64>,
    production: Option<prometheus::HistogramVec>,
}

impl HistogramVec {
    /// bucket 定义会保留下来，确保默认配置仍可被外部检查。
    pub fn new(name: impl Into<String>, buckets: Vec<f64>) -> Self {
        Self {
            name: name.into(),
            observations: Arc::new(Mutex::new(HashMap::new())),
            buckets,
            production: None,
        }
    }

    fn production(
        name: impl Into<String>,
        buckets: Vec<f64>,
        histogram: prometheus::HistogramVec,
    ) -> Self {
        Self {
            name: name.into(),
            observations: Arc::new(Mutex::new(HashMap::new())),
            buckets,
            production: Some(histogram),
        }
    }

    /// 返回绑定了 label 的直方图句柄，调用方式与 prometheus 客户端一致。
    /// 与计数器一样，label 组合被折叠成单个字符串键，足够表达 benchkv 的单维场景。
    pub fn WithLabelValues(&self, labels: &[&str]) -> Histogram {
        let key = labels.join(",");
        Histogram {
            name: self.name.clone(),
            key,
            observations: Arc::clone(&self.observations),
            production: self
                .production
                .as_ref()
                .map(|histogram| histogram.with_label_values(labels)),
        }
    }

    /// 直接暴露原始观测值，便于测试按顺序或数量检查采样结果。
    pub fn observations(&self, labels: &[&str]) -> Vec<f64> {
        let key = labels.join(",");
        self.observations
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// benchkv 导出时只需要总样本数，因此这里聚合各 label 的向量长度。
    pub fn count(&self) -> usize {
        self.observations
            .lock()
            .unwrap()
            .values()
            .map(|v| v.len())
            .sum()
    }
}

/// 单个 label 绑定后的直方图句柄。
#[derive(Clone, Debug)]
pub struct Histogram {
    name: String,
    key: String,
    observations: Arc<Mutex<HashMap<String, Vec<f64>>>>,
    production: Option<prometheus::Histogram>,
}

impl Histogram {
    /// 记录原始观测值而非即时做 bucket 累计，保持实现简单且足够测试导出路径。
    /// 这样测试既能检查样本总数，也能在需要时回看每次观测的具体秒数。
    pub fn Observe(&self, v: f64) {
        let _ = &self.name;
        self.observations
            .lock()
            .unwrap()
            .entry(self.key.clone())
            .or_default()
            .push(v);
        if let Some(histogram) = &self.production {
            histogram.observe(v);
        }
    }
}

/// Shared metric registry used by Init + /metrics text.
/// 把 benchkv 关心的三组指标打包到一起，方便 `Init` 与 `/metrics` 共享同一状态。
#[derive(Clone, Debug)]
pub struct Metrics {
    pub txn_counter: CounterVec,
    pub txn_rolledback_counter: CounterVec,
    pub txn_durations: HistogramVec,
    registered: Arc<AtomicBool>,
    production: bool,
}

impl Default for Metrics {
    /// 默认名称、帮助文本语义和 bucket 配置全部与 Go `main.go` 对齐。
    fn default() -> Self {
        Self {
            txn_counter: CounterVec::new("tikv_txn_total"),
            txn_rolledback_counter: CounterVec::new("tikv_txn_failed_total"),
            txn_durations: HistogramVec::new(
                "tikv_txn_durations_histogram_seconds",
                exponential_buckets(0.0005, 2.0, 13),
            ),
            registered: Arc::new(AtomicBool::new(false)),
            production: false,
        }
    }
}

impl Metrics {
    pub fn production() -> Self {
        let total = prometheus::CounterVec::new(
            prometheus::Opts::new("total", "Counter of txns.")
                .namespace("tikv")
                .subsystem("txn"),
            &["type"],
        )
        .expect("valid txn counter options");
        let failed = prometheus::CounterVec::new(
            prometheus::Opts::new("failed_total", "Counter of rolled back txns.")
                .namespace("tikv")
                .subsystem("txn"),
            &["type"],
        )
        .expect("valid rollback counter options");
        let buckets = exponential_buckets(0.0005, 2.0, 13);
        let durations = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(
                "durations_histogram_seconds",
                "Txn latency distributions.",
            )
            .namespace("tikv")
            .subsystem("txn")
            .buckets(buckets.clone()),
            &["type"],
        )
        .expect("valid txn histogram options");
        Self {
            txn_counter: CounterVec::production("tikv_txn_total", total),
            txn_rolledback_counter: CounterVec::production("tikv_txn_failed_total", failed),
            txn_durations: HistogramVec::production(
                "tikv_txn_durations_histogram_seconds",
                buckets,
                durations,
            ),
            registered: Arc::new(AtomicBool::new(false)),
            production: true,
        }
    }

    pub fn is_production(&self) -> bool {
        self.production
    }

    /// 真实 prometheus 会向全局注册表注册收集器；这里仅记一个布尔位作为副作用。
    pub fn must_register_all(&self) {
        if self.registered.swap(true, Ordering::SeqCst) {
            fatal("duplicate metrics collector registration");
        }
        if self.production {
            let total = self
                .txn_counter
                .production
                .as_ref()
                .expect("production counter")
                .clone();
            let failed = self
                .txn_rolledback_counter
                .production
                .as_ref()
                .expect("production rollback counter")
                .clone();
            let durations = self
                .txn_durations
                .production
                .as_ref()
                .expect("production histogram")
                .clone();
            prometheus::register(Box::new(total)).unwrap_or_else(|error| fatal(error.to_string()));
            prometheus::register(Box::new(failed)).unwrap_or_else(|error| fatal(error.to_string()));
            prometheus::register(Box::new(durations))
                .unwrap_or_else(|error| fatal(error.to_string()));
        }
    }

    /// 测试通过这个标记确认 `Init` 没有跳过指标注册。
    pub fn is_registered(&self) -> bool {
        self.registered.load(Ordering::SeqCst)
    }

    /// Render a prometheus-like text exposition for `/metrics`.
    /// 文本只覆盖当前 benchkv 会打印和断言的关键字段，不追求完整 exposition 协议。
    pub fn render(&self) -> String {
        if self.production {
            let encoder = prometheus::TextEncoder::new();
            let mut body = Vec::new();
            encoder
                .encode(&prometheus::gather(), &mut body)
                .unwrap_or_else(|error| fatal(error.to_string()));
            return String::from_utf8(body).expect("prometheus text encoder emits UTF-8");
        }
        let mut out = String::new();
        out.push_str("# HELP tikv_txn_total Counter of txns.\n# TYPE tikv_txn_total counter\n");
        if let Some(value) = self.txn_counter.get_existing(&["txn"]) {
            out.push_str(&format!("tikv_txn_total{{type=\"txn\"}} {value}\n"));
        }
        out.push_str(
            "# HELP tikv_txn_failed_total Counter of rolled back txns.\n# TYPE tikv_txn_failed_total counter\n",
        );
        if let Some(value) = self.txn_rolledback_counter.get_existing(&["txn"]) {
            out.push_str(&format!("tikv_txn_failed_total{{type=\"txn\"}} {value}\n"));
        }
        out.push_str("# HELP tikv_txn_durations_histogram_seconds Txn latency distributions.\n# TYPE tikv_txn_durations_histogram_seconds histogram\n");
        let observations = self.txn_durations.observations(&["txn"]);
        if !observations.is_empty() {
            for bound in &self.txn_durations.buckets {
                let count = observations
                    .iter()
                    .filter(|value| **value <= *bound)
                    .count();
                out.push_str(&format!(
                    "tikv_txn_durations_histogram_seconds_bucket{{type=\"txn\",le=\"{bound}\"}} {count}\n"
                ));
            }
            let count = observations.len();
            let sum: f64 = observations.iter().sum();
            out.push_str(&format!(
                "tikv_txn_durations_histogram_seconds_bucket{{type=\"txn\",le=\"+Inf\"}} {count}\n"
            ));
            out.push_str(&format!(
                "tikv_txn_durations_histogram_seconds_sum{{type=\"txn\"}} {sum}\n"
            ));
            out.push_str(&format!(
                "tikv_txn_durations_histogram_seconds_count{{type=\"txn\"}} {count}\n"
            ));
        }
        out
    }
}

// --- HTTP metrics server / client stubs ---
// 这里同样只保留 `benchkv` 主流程观察得到的行为：
// 注册 `/metrics`、启动监听、读取导出文本，以及关闭响应体。

#[derive(Clone, Debug)]
pub struct HttpServer {
    pub addr: Arc<Mutex<String>>,
    pub started: Arc<AtomicBool>,
    pub routes: Arc<Mutex<HashMap<String, String>>>,
    pub listen_error: Arc<Mutex<Option<Error>>>,
    metrics_handler: Arc<AtomicBool>,
    metrics: Arc<Mutex<Option<Metrics>>>,
    /// When true, ListenAndServe does not spawn a real socket (test/default).
    pub dry_run: bool,
}

impl Default for HttpServer {
    /// 默认启用 `dry_run`，避免 CI 里真的绑定端口，仍可保留“服务器已启动”的语义。
    fn default() -> Self {
        Self {
            addr: Arc::new(Mutex::new(String::new())),
            started: Arc::new(AtomicBool::new(false)),
            routes: Arc::new(Mutex::new(HashMap::new())),
            listen_error: Arc::new(Mutex::new(None)),
            metrics_handler: Arc::new(AtomicBool::new(false)),
            metrics: Arc::new(Mutex::new(None)),
            dry_run: true,
        }
    }
}

impl HttpServer {
    pub fn production() -> Self {
        Self {
            dry_run: false,
            ..Self::default()
        }
    }

    pub fn is_production(&self) -> bool {
        !self.dry_run
    }

    /// 记录路由到正文的映射，用于模拟 `http.Handle` 后由 `/metrics` 返回当前文本。
    /// 这里不做 handler 回调，是因为 benchkv 只需要固定路径返回一段序列化字符串。
    pub fn Handle(&self, path: &str, body: String) {
        self.routes.lock().unwrap().insert(path.to_string(), body);
    }

    /// Register the live Prometheus handler used by Go's `promhttp.Handler()`.
    pub fn HandleMetrics(&self, metrics: &Metrics) {
        self.metrics_handler.store(true, Ordering::SeqCst);
        *self.metrics.lock().unwrap() = Some(metrics.clone());
    }

    /// Go `http.ListenAndServe`. On dry_run, marks started and returns None.
    /// 先写入监听地址和 started 标记，再决定是否返回注入错误，
    /// 这样即使监听失败，测试仍能看到主流程确实尝试过启动服务。
    pub fn ListenAndServe(&self, addr: &str) -> Option<Error> {
        *self.addr.lock().unwrap() = addr.to_string();
        if let Some(err) = self.listen_error.lock().unwrap().clone() {
            return Some(err);
        }
        if self.dry_run {
            self.started.store(true, Ordering::SeqCst);
            return None;
        }
        let bind_addr = addr
            .strip_prefix(':')
            .map(|port| format!("0.0.0.0:{port}"))
            .unwrap_or_else(|| addr.to_owned());
        let server = match tiny_http::Server::http(&bind_addr) {
            Ok(server) => server,
            Err(error) => return Some(Error::new(error.to_string())),
        };
        self.started.store(true, Ordering::SeqCst);
        loop {
            let request = match server.recv() {
                Ok(request) => request,
                Err(error) => return Some(Error::new(error.to_string())),
            };
            let response = if request.url() == "/metrics" {
                let body = self
                    .metrics
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(Metrics::render)
                    .unwrap_or_default();
                let content_type = tiny_http::Header::from_bytes(
                    b"Content-Type".as_slice(),
                    b"text/plain; version=0.0.4; charset=utf-8".as_slice(),
                )
                .expect("valid metrics content type");
                tiny_http::Response::from_string(body).with_header(content_type)
            } else {
                tiny_http::Response::from_string("404 page not found\n").with_status_code(404)
            };
            if let Err(error) = request.respond(response) {
                log_error("http response", &error.to_string());
            }
        }
    }

    /// 测试可通过注入错误覆盖默认成功路径，验证后台监听失败时的日志行为。
    pub fn set_listen_error(&self, err: Option<Error>) {
        *self.listen_error.lock().unwrap() = err;
    }

    /// 只暴露只读状态，避免测试意外篡改启动结果。
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// 返回最近一次监听的地址，帮助确认 Rust 与 Go 使用同一端口。
    pub fn listen_addr(&self) -> String {
        self.addr.lock().unwrap().clone()
    }
}

/// 简化版 HTTP 响应，只保留正文和关闭副作用。
pub enum HttpBody {
    Stub(Vec<u8>),
    Network(Mutex<Option<reqwest::blocking::Response>>),
}

impl fmt::Debug for HttpBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stub(body) => f.debug_tuple("Stub").field(&body.len()).finish(),
            Self::Network(_) => f.write_str("Network(..)"),
        }
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub body: HttpBody,
    pub closed: Arc<AtomicBool>,
    pub close_err: Option<Error>,
}

impl HttpResponse {
    pub fn stub(body: Vec<u8>, close_err: Option<Error>) -> Self {
        Self {
            body: HttpBody::Stub(body),
            closed: Arc::new(AtomicBool::new(false)),
            close_err,
        }
    }

    /// 关闭响应体时只翻转标记并返回预置错误，模拟 `Body.Close` 的可观察效果。
    pub fn Close(&self) -> Option<Error> {
        if let HttpBody::Network(response) = &self.body {
            drop(response.lock().unwrap().take());
        }
        self.closed.store(true, Ordering::SeqCst);
        self.close_err.clone()
    }

    /// 供测试确认主流程是否完成了资源清理。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// Go `io.ReadAll(resp.Body)`：生产读取真实网络流，测试复制内存正文。
pub fn read_all(body: &HttpBody) -> (Vec<u8>, Option<Error>) {
    match body {
        HttpBody::Stub(body) => (body.clone(), None),
        HttpBody::Network(response) => {
            let mut guard = response.lock().unwrap();
            let Some(response) = guard.as_mut() else {
                return (Vec::new(), Some(Error::new("response body is closed")));
            };
            let mut body = Vec::new();
            match response.read_to_end(&mut body) {
                Ok(_) => (body, None),
                Err(error) => (body, Some(Error::new(error.to_string()))),
            }
        }
    }
}

/// Go `http.Get` against the in-process metrics server / metrics render.
/// 为了把测试约束在 benchkv 自己启动的 metrics 端口上，
/// 这里只接受 `localhost:9191` 或 `127.0.0.1:9191`。
pub fn http_get(
    url: &str,
    metrics: &Metrics,
    server: &HttpServer,
) -> (HttpResponse, Option<Error>) {
    if server.is_production() {
        return match reqwest::blocking::get(url) {
            Ok(response) => (
                HttpResponse {
                    body: HttpBody::Network(Mutex::new(Some(response))),
                    closed: Arc::new(AtomicBool::new(false)),
                    close_err: None,
                },
                None,
            ),
            Err(error) => (
                HttpResponse::stub(Vec::new(), None),
                Some(Error::new(error.to_string())),
            ),
        };
    }
    if !url.contains("localhost:9191") && !url.contains("127.0.0.1:9191") {
        return (
            HttpResponse::stub(Vec::new(), None),
            Some(Error::new(format!("unexpected url: {url}"))),
        );
    }
    // Prefer live metrics render (matches /metrics handler).
    // 若服务端已有缓存正文则优先使用，模拟 `http.Handle("/metrics", ...)` 当前挂载结果；
    // 否则退回动态渲染，保证没显式预热路由时也能拿到最新指标。
    let text = if server.metrics_handler.load(Ordering::SeqCst) {
        metrics.render()
    } else if let Some(cached) = server.routes.lock().unwrap().get("/metrics") {
        cached.clone()
    } else {
        metrics.render()
    };
    (HttpResponse::stub(text.into_bytes(), None), None)
}

/// Injectable open/store + HTTP + metrics for binary and parity tests.
/// 把外部依赖组合成一个显式对象，替代 Go 包级全局变量，方便测试按场景替换。
/// `main.rs` 只依赖这个聚合对象暴露的表面，因此内部桩可以在不改主流程签名下演进。
#[derive(Clone)]
pub struct RuntimeDeps {
    pub driver: TiKVDriver,
    pub metrics: Metrics,
    pub http: HttpServer,
    /// Optional pre-opened store; when set, Init uses it instead of Open.
    /// 预置存储让测试可以跳过 Open 逻辑，直接把失败注入或既有状态传入主流程。
    pub store_override: Option<Storage>,
}

impl Default for RuntimeDeps {
    /// 默认依赖连接真实 TiKV、Prometheus 注册表和 HTTP 服务。
    fn default() -> Self {
        Self {
            driver: TiKVDriver::default(),
            metrics: Metrics::production(),
            http: HttpServer::production(),
            store_override: None,
        }
    }
}

impl RuntimeDeps {
    /// 测试专用依赖，隔离真实 PD/TiKV、端口和全局 Prometheus 注册表。
    pub fn for_test() -> Self {
        Self {
            driver: TiKVDriver::stub(),
            metrics: Metrics::default(),
            http: HttpServer::default(),
            store_override: None,
        }
    }
}

impl fmt::Debug for RuntimeDeps {
    /// 避免把内部共享状态整坨打印出来，调试输出只需表明依赖对象存在即可。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeDeps")
    }
}

/// Duration helper matching Go `time.Since(...).Seconds()`.
/// 把 Rust `Duration` 明确转换成秒浮点数，供直方图观测复用同一单位。
pub fn duration_seconds(d: Duration) -> f64 {
    d.as_secs_f64()
}

static TXN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Test helper to reset a global-ish counter if needed.
/// 当前文件暂未消费该序列，但保留重置入口，方便未来桩扩展仍能在测试中复位全局状态。
pub fn reset_txn_seq() {
    TXN_SEQ.store(0, Ordering::SeqCst);
}
