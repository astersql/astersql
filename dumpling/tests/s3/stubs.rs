// Copyright 2026 AsterSQL.
//! Local stand-ins for MySQL / context / errgroup / cobra / net boundaries (arm64-safe).
//!
//! Mirrors the external surfaces `dumpling/tests/s3` needs from `database/sql`,
//! `golang.org/x/sync/errgroup`, `cobra`, and `net` without pulling heavy crates.
// S3 测试边界桩模块：轻量替代 database/sql、context、errgroup、cobra、net。
// 仅实现 import.go 所需表面，非完整 Go 运行时；缺失能力勿当作生产可用。

use std::fmt;
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

// --- Error (pingcap/errors.Trace shape) ---

// 简单错误类型，Error() 方法名对齐 Go；Display 供 fmt 输出。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    // 错误消息体，与 Go error 字符串一致。
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    // Go error 接口 Error() string；PascalCase 对齐 Go 导出方法。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display 与 Error() 同文案
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

// Trace 不改变消息，仅保留 Go errors.Trace 调用形状。
/// Go `errors.Trace(err)` — preserve message shape for callers.
// Go errors.Trace：此处为恒等，保留消息形状供 Trace(err) 链式调用。
pub fn Trace(err: Error) -> Error {
    err
}

// --- os.Exit / argv ---

// 记录 os.Exit 退出码的全局槽，供测试 take_exit_code 读取。
static PROCESS_EXIT_CODE: OnceLock<Mutex<Option<i32>>> = OnceLock::new();

fn exit_slot() -> &'static Mutex<Option<i32>> {
    // OnceLock 惰性初始化，避免测试启动期全局 Mutex 开销。
    PROCESS_EXIT_CODE.get_or_init(|| Mutex::new(None))
}

/// Go `os.Exit` — records code and panics so tests can catch it.
// 桩 os.Exit：写入退出码后 panic，测试用 catch_unwind 拦截而非真退出进程。
pub fn os_exit(code: i32) -> ! {
    if let Ok(mut g) = exit_slot().lock() {
        // 记录退出码供 take_exit_code 单次消费。
        *g = Some(code);
    }
    // 永不返回，模拟进程终止；测试侧 catch_unwind。
    panic!("os.Exit({code})");
}

// 读取并清空已记录的退出码，单次消费语义。
pub fn take_exit_code() -> Option<i32> {
    exit_slot().lock().ok().and_then(|mut g| g.take())
}

// 跳过 argv[0]，对应 Go os.Args[1:]。
pub fn args_from_env() -> Vec<String> {
    std::env::args().skip(1).collect()
}

// --- net ---

/// Go `net.JoinHostPort(host, port)`.
// IPv6 字面量 host 加方括号，与 Go net.JoinHostPort 规则一致。
pub fn JoinHostPort(host: &str, port: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        // IPv6 字面量需方括号，避免与端口分隔符混淆。
        format!("[{host}]:{port}")
    } else {
        // IPv4 或已 bracket 的 host 直接拼接。
        format!("{host}:{port}")
    }
}

// --- context ---

// context 取消错误，Display 为 "context canceled"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context canceled")
    }
}

impl std::error::Error for Canceled {}

// 取消函数句柄，call 设置关联 AtomicBool。
#[derive(Clone)]
pub struct CancelFunc {
    // 与 Context.cancelled 共享的原子标志。
    flag: Arc<AtomicBool>,
}

impl CancelFunc {
    // 原子置位，模拟 context.CancelFunc 调用。
    pub fn call(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

// 简化 context：本地 cancelled 或沿 parent 链传播 Done。
#[derive(Clone, Debug)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    // 可选 parent，Done 时向上递归。
    parent: Option<Arc<Context>>,
}

impl Context {
    // 根 context：永不自动取消，对应 context.Background。
    pub fn Background() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            // 根 context 无 parent。
            parent: None,
        }
    }

    // 从 parent 派生可单独取消的 child，返回 cancel 句柄。
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            // 子节点继承 parent 链，Done 可向上传播。
            parent: Some(Arc::new(parent.clone())),
        };
        let cancel = CancelFunc {
            flag: child.cancelled.clone(),
        };
        (child, cancel)
    }

    // 自身或祖先任一 cancelled 则 Done。
    pub fn Done(&self) -> bool {
        if self.cancelled.load(Ordering::SeqCst) {
            // 本节点已 cancel。
            return true;
        }
        match &self.parent {
            // 向上查找 parent 取消状态。
            Some(p) => p.Done(),
            None => false,
        }
    }

    // Done 为真时返回 Canceled，否则 None。
    pub fn Err(&self) -> Option<Canceled> {
        if self.Done() { Some(Canceled) } else { None }
    }
}

// 包级便捷函数，对应 context.Background()。
pub fn Background() -> Context {
    Context::Background()
}

// 值语义 parent，内部仍走 Arc 链。
pub fn WithCancel(parent: Context) -> (Context, CancelFunc) {
    Context::WithCancel(&parent)
}

// --- errgroup ---

/// Go `errgroup.Group` subset used by import.go.
// errgroup 子集：WithContext 派生 ctx，首错 cancel，Wait  join 并再次 cancel。
pub struct Group {
    // 已 spawn 的 worker 句柄
    handles: Mutex<Vec<JoinHandle<()>>>,
    // worker 完成时立即发送结果，使 Wait 按完成顺序选择首错。
    results_tx: mpsc::Sender<Result<()>>,
    results_rx: Mutex<mpsc::Receiver<Result<()>>>,
    // WithContext 派生的可取消 ctx
    ctx: Context,
    /// Cancels derived context on first Go error (errgroup.WithContext semantics).
    // 首任务返回 Err 时调用，对齐 errgroup.WithContext 取消语义。
    cancel_on_err: CancelFunc,
}

impl Group {
    /// Go `errgroup.WithContext(ctx)` — returns (group, derived ctx).
    // 从 parent 派生可取消 child context，返回 (Group, derived Context)。
    pub fn WithContext(parent: Context) -> (Self, Context) {
        let (ctx, cancel) = WithCancel(parent);
        let (results_tx, results_rx) = mpsc::channel();
        let g = Self {
            handles: Mutex::new(Vec::new()),
            results_tx,
            results_rx: Mutex::new(results_rx),
            ctx: ctx.clone(),
            cancel_on_err: cancel,
        };
        (g, ctx)
    }

    // 返回派生 context 的克隆，供 worker 检查取消。
    pub fn Context(&self) -> Context {
        self.ctx.clone()
    }

    /// Go `eg.Go(fn)`.
    // spawn 线程执行 f；失败时 trigger cancel_on_err。
    pub fn Go<F>(&self, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let cancel = self.cancel_on_err.clone();
        let results_tx = self.results_tx.clone();
        let handle = thread::spawn(move || {
            let r = panic::catch_unwind(AssertUnwindSafe(f))
                .unwrap_or_else(|_| Err(Error::new("errgroup worker panicked")));
            if r.is_err() {
                // 首错立即 cancel derived ctx。
                cancel.call();
            }
            let _ = results_tx.send(r);
        });
        // 记录 join 句柄，Wait 时统一回收。
        self.handles.lock().unwrap().push(handle);
    }

    /// Go `eg.Wait()` — joins all tasks; returns first error.
    // join 全部 worker，返回首个 Err；结束时 cancel_on_err（含成功路径）。
    pub fn Wait(&self) -> Result<()> {
        let handles = std::mem::take(&mut *self.handles.lock().unwrap());
        let mut first_err: Option<Error> = None;
        for _ in 0..handles.len() {
            if let Ok(Err(e)) = self.results_rx.lock().unwrap().recv()
                && first_err.is_none()
            {
                // channel 接收顺序即 worker 完成顺序，对齐 errgroup 首个非空错误。
                first_err = Some(e);
            }
        }
        for handle in handles {
            let _ = handle.join();
        }
        // Wait 返回前 cancel derived context，与 Go errgroup 一致。
        self.cancel_on_err.call();
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

// --- database/sql stub ---

// 单次 Exec/ExecContext 调用记录，via_context 区分 API。
#[derive(Clone, Debug)]
pub struct ExecRecord {
    pub sql: String,
    // false=Exec，true=ExecContext
    pub via_context: bool,
}

/// Configurable MySQL boundary stub (records Exec / ExecContext).
// 可配置 RecordingDb：记录 SQL、可注入第 N 次 ExecContext 失败。
#[derive(Clone)]
pub struct RecordingDb {
    // 共享内部状态，Clone 后 exec 记录仍汇总到同一 inner。
    inner: Arc<RecordingDbInner>,
}

struct RecordingDbInner {
    // 按调用顺序记录 SQL
    execs: Mutex<Vec<ExecRecord>>,
    // 下一次 Exec 返回的注入错误，用于覆盖 Go db.Exec 建表失败分支。
    fail_next_exec: Mutex<Option<Error>>,
    /// Fail the Nth ExecContext (1-based). None = never fail.
    // 第 n 次 ExecContext（1-based）返回注入错误；None 表示不失败。
    fail_on_exec_context: Mutex<Option<usize>>,
    // ExecContext 调用计数
    exec_context_count: AtomicUsize,
    /// If set, `sql_open` path is not used; Open itself fails when using OpenErrorDb.
    // sql_open 路径写入的 DSN；OpenErrorDb 场景另说（本桩未实现 Open 失败 DB 类型）。
    open_dsn: Mutex<Option<String>>,
}

impl RecordingDb {
    // 空记录、无注入失败点的新 DB。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RecordingDbInner {
                execs: Mutex::new(Vec::new()),
                fail_next_exec: Mutex::new(None),
                fail_on_exec_context: Mutex::new(None),
                exec_context_count: AtomicUsize::new(0),
                open_dsn: Mutex::new(None),
            }),
        }
    }

    /// Fail on the `n`th ExecContext call (1-based).
    // 设置在第 n 次 ExecContext 注入 "injected exec failure"。
    pub fn fail_on_nth_exec_context(&self, n: usize) {
        *self.inner.fail_on_exec_context.lock().unwrap() = Some(n);
    }

    /// Fail the next synchronous `Exec` call with the supplied message.
    pub fn fail_next_exec(&self, message: impl Into<String>) {
        *self.inner.fail_next_exec.lock().unwrap() = Some(Error::new(message));
    }

    // 克隆当前已记录的 Exec/ExecContext 列表。
    pub fn execs(&self) -> Vec<ExecRecord> {
        self.inner.execs.lock().unwrap().clone()
    }

    // 读取 sql_open 写入的 DSN（若有）。
    pub fn open_dsn(&self) -> Option<String> {
        self.inner.open_dsn.lock().unwrap().clone()
    }

    // 线程安全追加一条 exec 记录。
    fn record(&self, sql: &str, via_context: bool) {
        self.inner.execs.lock().unwrap().push(ExecRecord {
            sql: sql.to_string(),
            via_context,
        });
    }

    /// Go `db.Exec(query)`.
    // 同步 Exec：记录 SQL，并可注入一次错误覆盖 Go 建表失败分支。
    pub fn Exec(&self, query: &str) -> Result<()> {
        self.record(query, false);
        if let Some(err) = self.inner.fail_next_exec.lock().unwrap().take() {
            return Err(err);
        }
        Ok(())
    }

    /// Go `db.ExecContext(ctx, query)`.
    // ExecContext：ctx 已取消则返回 canceled；否则记录并可能注入失败。
    pub fn ExecContext(&self, ctx: &Context, query: &str) -> Result<()> {
        if ctx.Err().is_some() {
            // ctx 已 Done，提前失败。
            return Err(Trace(Error::new("context canceled")));
        }
        let n = self.inner.exec_context_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.record(query, true);
        if let Some(fail_at) = *self.inner.fail_on_exec_context.lock().unwrap() {
            if n == fail_at {
                // 测试注入点：第 n 次 ExecContext 失败。
                return Err(Trace(Error::new("injected exec failure")));
            }
        }
        Ok(())
    }
}

impl Default for RecordingDb {
    fn default() -> Self {
        Self::new()
    }
}

/// Go `sql.Open(driver, dsn)` — stub always succeeds and records DSN.
// sql.Open 桩：恒成功，新建 RecordingDb 并保存 dsn 供断言。
pub fn sql_open(_driver: &str, dsn: &str) -> Result<RecordingDb> {
    let db = RecordingDb::new();
    // 记录 DSN 供 open_dsn() 断言，driver 名未校验。
    *db.inner.open_dsn.lock().unwrap() = Some(dsn.to_string());
    Ok(db)
}

/// Injectable open for tests.
// 可注入 OpenFn，测试 open 失败而不走 default sql_open。
pub type OpenFn = Arc<dyn Fn(&str) -> Result<RecordingDb> + Send + Sync>;

pub fn default_open() -> OpenFn {
    // 与 Go sql.Open("mysql", dsn) 一致
    Arc::new(|dsn: &str| sql_open("mysql", dsn))
}

// --- cobra-like flags ---

// CLI flag 快照，默认值与 Go cobra PersistentFlags 一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flags {
    // -B / --database
    pub database: String,
    // -T / --table
    pub table: String,
    // -P / --port，TiDB 默认 4000
    pub port: isize,
    // -w / --worker，并发令牌数
    pub worker: isize,
}

impl Default for Flags {
    // 与 Go cobra PersistentFlags 默认值对齐。
    fn default() -> Self {
        Self {
            database: "s3".to_string(),
            table: "t".to_string(),
            port: 4000,
            worker: 16,
        }
    }
}

/// Parse argv like cobra flags: `-B/--database`, `-T/--table`, `-P/--port`, `-w/--worker`.
// 简易 cobra flag 解析：支持 -B/-T/-P/-w 与 --key=val；未知 flag 报错。
pub fn parse_flags(args: &[String]) -> Result<Flags> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        let (key, inline) = if let Some(rest) = a.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                // --key=val 内联形式。
                (k, Some(v.to_string()))
            } else {
                // --key 下一参数为值。
                (rest, None)
            }
        } else if let Some(rest) = a.strip_prefix('-') {
            if let Some((k, v)) = rest.split_once('=') {
                // -key=val。
                (k, Some(v.to_string()))
            } else if rest.len() > 1 {
                // pflag accepts an attached value for single-letter shorthands: -P4000.
                let (k, v) = rest.split_at(1);
                (k, Some(v.to_string()))
            } else {
                // -key 短 flag。
                (rest, None)
            }
        } else {
            i += 1;
            // 非 flag 参数跳过。
            continue;
        };

        // 内联值或取下一 argv 元素作为 flag 参数。
        let take_val = |inline: Option<String>, idx: &mut usize| -> Result<String> {
            if let Some(v) = inline {
                return Ok(v);
            }
            *idx += 1;
            args.get(*idx)
                .cloned()
                .ok_or_else(|| Error::new(format!("flag needs an argument: -{key}")))
        };

        match key {
            // 短名 B 与长名 database 均映射 database 字段。
            "B" | "database" => flags.database = take_val(inline, &mut i)?,
            "T" | "table" => flags.table = take_val(inline, &mut i)?,
            "P" | "port" => {
                let v = take_val(inline, &mut i)?;
                // 与 cobra 错误格式接近：invalid value "..." for flag -port。
                flags.port = v
                    .parse()
                    .map_err(|_| Error::new(format!("invalid value \"{v}\" for flag -port")))?;
            }
            "w" | "worker" => {
                let v = take_val(inline, &mut i)?;
                flags.worker = v
                    .parse()
                    .map_err(|_| Error::new(format!("invalid value \"{v}\" for flag -worker")))?;
            }
            // help 忽略，不报错。
            "h" | "help" => {}
            _ => {
                // 未知 flag。
                return Err(Error::new(format!("unknown flag: -{key}")));
            }
        }
        i += 1;
    }
    Ok(flags)
}

/// Print failure line matching Go `fmt.Printf("fail to import data, err: %v", err)`.
// 失败输出格式与 Go fmt.Printf 一致，供 main 路径与 parity 断言。
pub fn print_fail(err: &Error) {
    // 无换行，同 Go %v；flush 确保 catch_unwind 前可见。
    let _ = write!(io::stdout(), "fail to import data, err: {err}");
    let _ = io::stdout().flush();
}
