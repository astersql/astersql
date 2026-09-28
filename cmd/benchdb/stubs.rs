// Copyright 2026 AsterSQL.
//! Local stand-ins for session/store/logging/flag boundaries (arm64-safe).
//!
//! 该文件把 `cmd/benchdb` 直接依赖的外部边界压缩成一组本地桩实现，目标不是
//! 提供完整的 TiDB/TiKV 能力，而是让命令入口在 arm64 等受限环境下仍能保留
//! 与 Go 版相同的参数、错误、SQL 发射与资源清理形状。
//! 模块中的每个类型都刻意贴近 Go 调用面，而不是追求更“Rust 风格”的抽象，
//! 这样 parity test 才能把注意力放在公开语义是否对齐，而非适配层本身的设计。
//! Mirrors the external surfaces `cmd/benchdb` needs from config/ddl/session/store
//! without pulling those crates (which transitively require kv/domain/kvproto/grpcio).

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::{self, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Error type standing in for TiDB / terror errors at the benchdb boundary.
/// 这里只保留一个字符串负载，原因是 benchdb 对外只消费错误文本，
/// 不依赖更细粒度的错误码、堆栈或分类信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 统一入口便于在测试里构造与 Go `errors.New` 类似的轻量错误值。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 保留 Go 风格的 `Error()` 命名，减少迁移时的机械映射成本。
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

pub type Result<T> = std::result::Result<T, Error>;

/// Go `terror.MustNil` — panic (process-fatal) when `err` is present.
/// benchdb 把初始化和执行错误都视为不可恢复事件，因此这里不返回
/// `Result` 继续上传，而是直接走与 Go `log.Fatal` 等价的终止路径。
pub fn must_nil(err: Option<Error>) {
    if let Some(e) = err {
        fatal(e.Error());
    }
}

/// Go `log.Fatal` — process exit semantics via panic (catchable in tests).
/// 生产中的 Go 版本会直接退出进程；Rust 桩改用 `panic!` 只是为了
/// 让测试能够捕获致命错误，同时保持“首个错误立即中断”的外部行为。
pub fn fatal(msg: impl AsRef<str>) -> ! {
    panic!("{}", msg.as_ref());
}

// --- flag (Go `flag` package subset) ---

#[derive(Clone, Debug)]
pub struct Flags {
    /// PD 地址决定 benchdb 连接到哪一个 TiKV 集群。
    pub addr: String,
    /// 表名通过 `%n` 标识符参数注入到 SQL 模板中。
    pub table_name: String,
    /// 批大小控制 insert/update-random 每个事务内的语句数。
    pub batch_size: i64,
    /// blob 大小影响插入载荷体积，用于模拟更真实的行宽。
    pub blob_size: i64,
    /// 日志级别仅作为初始化参数记录下来，桩本身不实现分级输出。
    pub log_level: String,
    /// 作业串沿用 Go 版 `|` 分隔协议，入口按顺序逐段执行。
    pub run_jobs: String,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:2379".to_string(),
            table_name: "benchdb".to_string(),
            batch_size: 100,
            blob_size: 1000,
            log_level: "warn".to_string(),
            run_jobs: default_run_jobs(),
        }
    }
}

/// Default `-run` value matching Go `strings.Join([...], "|")`.
/// 默认流水线同时覆盖建表、写入、更新、查询和 GC 阶段，使基准工具
/// 开箱即可跑出一条接近 Go 原版的完整工作负载。
pub fn default_run_jobs() -> String {
    [
        "create",
        "truncate",
        "insert:0_10000",
        "update-random:0_10000:100000",
        "select:0_10000:10",
        "update-range:5000_5100:1000",
        "select:0_10000:10",
        "gc",
        "select:0_10000:10",
    ]
    .join("|")
}

/// Parse argv like Go `flag.Parse` for the flags benchdb defines.
/// 这里只支持 benchdb 真正声明过的旗标，并故意保留未识别旗标即
/// 致命失败的策略，避免调用方误以为参数被接受但实际上被静默忽略。
pub fn parse_flags(args: &[String]) -> Flags {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Go's flag package stops at the first positional argument. An
        // explicit `--` consumes the terminator and stops parsing as well.
        if a == "-" || a == "--" || !a.starts_with('-') {
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
            i += 1;
            continue;
        };

        // 兼容 `-k=v` 与 `-k v` 两种 Go flag 常见写法。
        let take_val = |inline: Option<String>, idx: &mut usize| -> String {
            if let Some(v) = inline {
                return v;
            }
            *idx += 1;
            args.get(*idx)
                .cloned()
                .unwrap_or_else(|| fatal(format!("flag needs an argument: -{key}")))
        };

        match key {
            "addr" => flags.addr = take_val(inline_val, &mut i),
            "table" => flags.table_name = take_val(inline_val, &mut i),
            "batch" => {
                let v = take_val(inline_val, &mut i);
                flags.batch_size = v
                    .parse()
                    .unwrap_or_else(|_| fatal(format!("invalid value \"{v}\" for flag -batch")));
            }
            "blob" => {
                let v = take_val(inline_val, &mut i);
                flags.blob_size = v
                    .parse()
                    .unwrap_or_else(|_| fatal(format!("invalid value \"{v}\" for flag -blob")));
            }
            "L" => flags.log_level = take_val(inline_val, &mut i),
            "run" => flags.run_jobs = take_val(inline_val, &mut i),
            "h" | "help" => {
                print_defaults();
                // Go's default FlagSet uses ExitOnError and exits successfully
                // after displaying help, so benchmark initialization must not run.
                std::process::exit(0);
            }
            _ => {
                // Go flag.Parse errors on undefined flags; keep same fail-fast shape.
                fatal(format!("flag provided but not defined: -{key}"));
            }
        }
        i += 1;
    }
    flags
}

/// Go `flag.PrintDefaults` for benchdb flags.
/// 帮助文本复刻 Go 输出顺序，方便人工对照，也让测试能锁定默认值。
pub fn print_defaults() {
    let d = Flags::default();
    // flag.CommandLine writes usage and defaults to stderr by default.
    let mut out = io::stderr().lock();
    let _ = writeln!(
        out,
        "  -L string\n    \tlog level (default \"{}\")",
        d.log_level
    );
    let _ = writeln!(
        out,
        "  -addr string\n    \tpd address (default \"{}\")",
        d.addr
    );
    let _ = writeln!(
        out,
        "  -batch int\n    \tnumber of statements in a transaction, used for insert and update-random only (default {})",
        d.batch_size
    );
    let _ = writeln!(
        out,
        "  -blob int\n    \tsize of the blob column in the row (default {})",
        d.blob_size
    );
    let _ = writeln!(
        out,
        "  -run string\n    \tjobs to run (default \"{}\")",
        d.run_jobs
    );
    let _ = writeln!(
        out,
        "  -table string\n    \tname of the table (default \"{}\")",
        d.table_name
    );
}

/// 把真实进程参数读取与解析逻辑分开，便于测试直接注入 argv。
pub fn args_from_env() -> Vec<String> {
    env::args().skip(1).collect()
}

// --- logutil ---

/// benchdb 只依赖默认日志格式常量，而不需要完整 formatter 体系。
pub const DEFAULT_LOG_FORMAT: &str = "text";

/// 文件日志配置在当前桩中只是占位类型，用来保持函数签名兼容。
#[derive(Clone, Debug, Default)]
pub struct FileLogConfig;

/// 对应 Go `EmptyFileLogConfig` 的零值常量。
pub static EMPTY_FILE_LOG_CONFIG: FileLogConfig = FileLogConfig;

/// 仅保留 benchdb 初始化真正会传递的字段。
#[derive(Clone, Debug)]
pub struct LogConfig {
    /// 日志等级由命令行 `-L` 注入。
    pub level: String,
    /// 格式在 benchdb 中固定走文本路径。
    pub format: String,
    /// 文件输出配置被保留为兼容字段。
    pub file: FileLogConfig,
    /// 时间戳开关也按 Go 接口保留，即便桩不会真正渲染日志。
    pub disable_timestamp: bool,
}

/// 构造函数只做字段打包，不进行额外校验或副作用。
pub fn new_log_config(
    level: impl Into<String>,
    format: impl Into<String>,
    _file_name: &str,
    _max_size: &str,
    file: FileLogConfig,
    disable_timestamp: bool,
) -> LogConfig {
    LogConfig {
        level: level.into(),
        format: format.into(),
        file,
        disable_timestamp,
    }
}

/// Go `logutil.InitLogger` — records level; always succeeds for the stub.
/// 初始化被简化为无失败路径，因为 benchdb 测试关注的是调用时机，
/// 不是日志后端本身是否可用。
pub fn init_logger(cfg: LogConfig) -> Option<Error> {
    let _ = cfg;
    None
}

// --- config / store / ddl / session bootstrap ---

/// 当前桩只实现 TiKV 一种 store 类型，因为 benchdb 也只注册这一项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreType {
    TiKV,
}

/// 全局配置只保留 store 类型，足以支撑 `new_bench_db` 的对齐逻辑。
#[derive(Clone, Debug)]
pub struct GlobalConfig {
    pub store: StoreType,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            store: StoreType::TiKV,
        }
    }
}

// 使用线程本地存储模拟 Go 中可变的全局配置入口，避免测试之间互相
// 通过静态全局值污染。
thread_local! {
    static GLOBAL_CONFIG: RefCell<GlobalConfig> = RefCell::new(GlobalConfig::default());
}

/// 返回副本而不是借用，调用方可以像 Go 那样直接读取当前快照。
pub fn get_global_config() -> GlobalConfig {
    GLOBAL_CONFIG.with(|c| c.borrow().clone())
}

/// 仅暴露设置 store 的最小接口，满足 benchdb 初始化副作用验证。
pub fn set_global_store(store: StoreType) {
    GLOBAL_CONFIG.with(|c| c.borrow_mut().store = store);
}

/// 驱动类型本身没有行为，只是让注册接口形状与 Go 版一致。
#[derive(Clone, Debug, Default)]
pub struct TiKVDriver;

/// 注册表用于记录某个 store 是否已声明，便于测试断言初始化过程。
fn registered_stores() -> &'static Mutex<HashMap<String, bool>> {
    static REGISTERED: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    REGISTERED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Go `store.Register`.
/// 这里不做重复注册报错，原因是 benchdb 只关心“注册已发生”这一事实。
pub fn store_register(store_type: StoreType, _driver: &TiKVDriver) -> Option<Error> {
    let key = match store_type {
        StoreType::TiKV => "tikv",
    };
    registered_stores()
        .lock()
        .unwrap()
        .insert(key.to_string(), true);
    None
}

/// Opaque storage handle (Go `tikv.Storage`).
/// 路径和 `disable_gc` 是 benchdb 可观察到的两个关键属性，因此直接
/// 暴露出来供主流程与测试读取。
#[derive(Clone, Debug)]
pub struct Storage {
    /// 原始连接串会被保留，方便验证地址和禁用 GC 标志是否拼接正确。
    pub path: String,
    /// benchdb 依赖该开关模拟 `?disableGC=true` 的运行约束。
    pub disable_gc: bool,
}

/// Go `store.New`.
/// 桩版本只解析 `disableGC=true` 是否出现，不尝试实现真实 store 连接。
pub fn store_new(path: &str) -> (Storage, Option<Error>) {
    let disable_gc = path.contains("disableGC=true");
    (
        Storage {
            path: path.to_string(),
            disable_gc,
        },
        None,
    )
}

/// Go `ddl.StartOwnerManager`.
/// 保留独立调用点是为了让初始化顺序与 Go 一致，即便这里没有副作用。
pub fn start_owner_manager(_store: &Storage) -> Option<Error> {
    None
}

/// Go `session.BootstrapSession`.
/// 作为 bootstrap 边界的空实现存在，便于未来替换成更真实的依赖时
/// 不必改动 benchdb 主流程。
pub fn bootstrap_session(_store: &Storage) -> Option<Error> {
    None
}

// --- SQL session / result set ---

/// Bound SQL argument (Go `...any`).
/// 参数枚举覆盖 benchdb 会发出的几类值，既能保留类型信息，也便于
/// 测试逐项比较 SQL 模板与绑定参数是否和 Go 一致。
#[derive(Clone, Debug, PartialEq)]
pub enum SqlArg {
    /// 对应 SQL `NULL` 占位。
    Null,
    /// benchdb 的 id、exp 等数值最终都会落到该变体。
    Int(i64),
    /// 字符串值用于普通文本参数。
    Str(String),
    /// blob 列写入使用字节数组保留原始载荷。
    Bytes(Vec<u8>),
    /// `%n` 标识符参数单独建模，避免与普通字符串混淆。
    Ident(String),
}

impl From<i32> for SqlArg {
    /// 与 Go `int` 到 `any` 的自动装箱效果对齐。
    fn from(v: i32) -> Self {
        SqlArg::Int(i64::from(v))
    }
}

impl From<i64> for SqlArg {
    /// 保留 64 位整数，避免在记录层丢失范围信息。
    fn from(v: i64) -> Self {
        SqlArg::Int(v)
    }
}

impl From<&str> for SqlArg {
    /// 借用字符串会被复制成拥有所有权的参数值，便于后续存档。
    fn from(v: &str) -> Self {
        SqlArg::Str(v.to_string())
    }
}

impl From<String> for SqlArg {
    /// 拥有所有权的字符串可直接进入记录列表。
    fn from(v: String) -> Self {
        SqlArg::Str(v)
    }
}

impl From<Vec<u8>> for SqlArg {
    /// 字节切片会整体保存，方便验证 blob 长度和内容形状。
    fn from(v: Vec<u8>) -> Self {
        SqlArg::Bytes(v)
    }
}

/// 执行记录是 parity test 观察 benchdb 对外 SQL 副作用的核心载体。
#[derive(Clone, Debug)]
pub struct ExecRecord {
    pub sql: String,
    pub args: Vec<SqlArg>,
}

/// 最小 chunk 只保留行数，因为 benchdb 只关心是否还有数据可读。
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    pub num_rows: usize,
}

impl Chunk {
    /// 保留 Go 风格命名，便于与 `req.NumRows()` 调用一一对应。
    pub fn NumRows(&self) -> usize {
        self.num_rows
    }
}

/// Minimal result-set surface used by `mustExec` drain/close.
/// 接口只暴露创建 chunk、推进读取、关闭和观测关闭状态四件事，
/// 足以模拟 Go `ResultSet` 在 benchdb 中的完整使用方式。
pub trait ResultSet {
    fn NewChunk(&self) -> Chunk;
    fn Next(&mut self, req: &mut Chunk) -> Option<Error>;
    fn Close(&mut self) -> Option<Error>;
    fn closed(&self) -> bool;
}

/// Empty result set that closes cleanly (normal success path).
/// 成功路径下很多语句不会真正返回数据；该实现让主流程仍能走完
/// “读空再关闭”的统一协议。
#[derive(Debug, Default)]
pub struct EmptyResultSet {
    closed: bool,
    next_calls: usize,
}

impl ResultSet for EmptyResultSet {
    /// 每次返回一个空 chunk，表示没有任何行待消费。
    fn NewChunk(&self) -> Chunk {
        Chunk { num_rows: 0 }
    }

    /// 把 `num_rows` 设为 0，驱动调用方立即结束 drain 循环。
    fn Next(&mut self, req: &mut Chunk) -> Option<Error> {
        self.next_calls += 1;
        req.num_rows = 0;
        None
    }

    /// 关闭过程永远成功，用来覆盖最普通的资源释放路径。
    fn Close(&mut self) -> Option<Error> {
        self.closed = true;
        None
    }

    /// 测试可以借此确认 Close 是否被真正调用过。
    fn closed(&self) -> bool {
        self.closed
    }
}

/// Session surface matching Go `sessionapi.Session.ExecuteInternal`.
/// 只保留内部 SQL 执行这一项能力，因为 benchdb 不需要更完整的会话 API。
pub trait Session {
    fn ExecuteInternal(
        &mut self,
        sql: &str,
        args: &[SqlArg],
    ) -> (Option<Box<dyn ResultSet>>, Option<Error>);
}

/// Recording session for production stub + parity tests.
/// 该会话既是运行时默认桩，也是测试用观测器，所有执行过的 SQL 都会
/// 被记录下来供外部断言顺序、模板和参数。
#[derive(Clone, Debug, Default)]
pub struct RecordingSession {
    inner: Arc<Mutex<RecordingSessionInner>>,
}

/// 内部状态集中放到共享互斥体中，便于多个结果集实例回写关闭计数。
#[derive(Debug, Default)]
struct RecordingSessionInner {
    /// 按调用顺序保存执行记录，测试据此验证外部副作用。
    pub execs: Vec<ExecRecord>,
    /// 统计结果集关闭次数，覆盖 defer/Close 对齐场景。
    pub close_count: usize,
    /// 命中指定 SQL 子串时模拟执行失败。
    pub fail_on_sql: Option<String>,
    /// 开启后 Close 会返回错误，用于验证致命关闭路径。
    pub fail_on_close: bool,
    /// When set, Next returns this many non-empty chunks before draining.
    /// 用“还剩多少行”来模拟结果集在若干次 `Next` 后耗尽。
    pub rows_before_empty: usize,
}

impl RecordingSession {
    /// 显式构造函数让调用点读起来更像 Go `CreateSession`。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回副本，避免测试长时间持有锁。
    pub fn execs(&self) -> Vec<ExecRecord> {
        self.inner.lock().unwrap().execs.clone()
    }

    /// 暴露关闭次数，便于验证成功路径是否完成资源回收。
    pub fn close_count(&self) -> usize {
        self.inner.lock().unwrap().close_count
    }

    /// 通过 SQL 子串触发失败，足以覆盖 benchdb 的错误传播合同。
    pub fn set_fail_on_sql(&self, sql_substr: impl Into<String>) {
        self.inner.lock().unwrap().fail_on_sql = Some(sql_substr.into());
    }

    /// 切换结果集关闭是否报错，用来模拟 defer Close 失败。
    pub fn set_fail_on_close(&self, v: bool) {
        self.inner.lock().unwrap().fail_on_close = v;
    }

    /// 设置返回若干个非空 chunk，驱动 `must_exec` 走完整 drain 循环。
    pub fn set_rows_before_empty(&self, n: usize) {
        self.inner.lock().unwrap().rows_before_empty = n;
    }
}

/// 记录型结果集与会话共享状态，以便 Close 时回写计数和错误开关。
struct RecordingResultSet {
    session: Arc<Mutex<RecordingSessionInner>>,
    closed: bool,
    remaining_rows: usize,
}

impl ResultSet for RecordingResultSet {
    /// 与空结果集一样返回一个可复用的 chunk 容器。
    fn NewChunk(&self) -> Chunk {
        Chunk { num_rows: 0 }
    }

    /// 每次把剩余“行数”消费掉一格，直到最终返回空 chunk。
    fn Next(&mut self, req: &mut Chunk) -> Option<Error> {
        if self.remaining_rows > 0 {
            self.remaining_rows -= 1;
            req.num_rows = 1;
        } else {
            req.num_rows = 0;
        }
        None
    }

    /// Close 会回写会话统计，并按配置决定是否模拟失败。
    fn Close(&mut self) -> Option<Error> {
        self.closed = true;
        let mut g = self.session.lock().unwrap();
        g.close_count += 1;
        if g.fail_on_close {
            return Some(Error::new("close failed"));
        }
        None
    }

    /// 让测试能确认结果集对象自身是否已关闭。
    fn closed(&self) -> bool {
        self.closed
    }
}

impl Session for RecordingSession {
    /// 执行时先落盘记录，再根据配置决定失败或返回可排空结果集。
    fn ExecuteInternal(
        &mut self,
        sql: &str,
        args: &[SqlArg],
    ) -> (Option<Box<dyn ResultSet>>, Option<Error>) {
        let mut g = self.inner.lock().unwrap();
        g.execs.push(ExecRecord {
            sql: sql.to_string(),
            args: args.to_vec(),
        });
        if let Some(ref needle) = g.fail_on_sql {
            if sql.contains(needle.as_str()) {
                // 失败消息带上 SQL 文本，便于 benchdb 直接打印诊断现场。
                return (None, Some(Error::new(format!("exec failed: {sql}"))));
            }
        }
        let rows = g.rows_before_empty;
        // After first use, drain stays empty unless tests reset.
        // 模拟一次性消费结果集，避免后续执行继承旧状态。
        g.rows_before_empty = 0;
        drop(g);
        (
            Some(Box::new(RecordingResultSet {
                session: Arc::clone(&self.inner),
                closed: false,
                remaining_rows: rows,
            })),
            None,
        )
    }
}

/// Go `session.CreateSession`.
/// 默认工厂返回记录型会话，使生产桩与测试替身复用同一实现。
pub fn create_session(_store: &Storage) -> (RecordingSession, Option<Error>) {
    (RecordingSession::new(), None)
}

// --- math/rand.Read stand-in ---

/// 全局 RNG 状态只用于提供足够接近 Go `math/rand.Read` 的伪随机字节。
static RNG_STATE: AtomicU64 = AtomicU64::new(0);

/// 首次使用时以当前时间纳秒数为种子，并确保最低位为 1。
fn rng_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    nanos | 1
}

/// Go `math/rand.Read` — fills buf with pseudo-random bytes (#nosec G404).
/// 这里不追求密码学安全，只需维持与 benchmark 相同的“随机载荷”
/// 语义，避免所有插入行都携带完全相同的 blob。
pub fn rand_read(buf: &mut [u8]) {
    let mut state = RNG_STATE.load(Ordering::Relaxed);
    if state == 0 {
        state = rng_seed();
        RNG_STATE.store(state, Ordering::Relaxed);
    }
    for b in buf.iter_mut() {
        // xorshift64*
        // 轻量 xorshift 足够便宜，适合在基准循环里频繁调用。
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = (state & 0xff) as u8;
    }
    RNG_STATE.store(state, Ordering::Relaxed);
}

/// Go `rand.Intn(n)` for n > 0.
/// 范围限制交给调用方保证；实现只需保留 `[0, n)` 的返回合同。
pub fn rand_intn(n: i64) -> i64 {
    assert!(n > 0);
    let mut buf = [0u8; 4];
    rand_read(&mut buf);
    let v = u32::from_le_bytes(buf);
    i64::from(v).rem_euclid(n)
}

/// Shared handle so binary and tests can inject a session factory.
/// 通过可克隆工厂把“如何创建 session”从主流程中抽离出来，测试便可
/// 注入预先配置过失败场景或记录能力的会话。
#[derive(Clone)]
pub struct SessionFactory {
    pub create: Rc<dyn Fn(&Storage) -> (RecordingSession, Option<Error>)>,
}

impl Default for SessionFactory {
    /// 默认工厂直接回落到本文件的 `create_session`。
    fn default() -> Self {
        Self {
            create: Rc::new(|store| create_session(store)),
        }
    }
}

impl fmt::Debug for SessionFactory {
    /// 闭包本身不可打印，因此调试输出只暴露类型名。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionFactory")
    }
}
