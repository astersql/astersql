// Copyright 2026 AsterSQL.
//! Local arm64-safe stubs for flag / PD / TiKV / config glue used by tidb-lightning-ctl.
//! Prefer types from slim `astersql-lightning-pkg-server`; only mock network and CLI boundaries.
//! 中文补充：本文件是 `tidb-lightning-ctl` 的本地桩层，专门替代在 arm64 开发机上不易直接复用的 CLI、PD HTTP 与 TiKV 网络依赖。
//! 中文补充：目标不是完整重写 Lightning，而是保留控制命令真正关心的边界协议，让 `main.rs` 和 parity test 能按 Go 的调用顺序运行。
//! 中文补充：因此这里的实现分成三类：
//! 中文补充：一类是参数解析与全局配置装载，只覆盖 ctl 实际读取的字段。
//! 中文补充：一类是 PD/TiKV 交互边界，用可观测 mock 代替真实网络。
//! 中文补充：一类是错误、退出码和 TLS 适配，保证用户可见行为与 Go 版本尽量一致。
//! 中文补充：文件中的“stub”只表示依赖被缩减，不表示语义可以随意简化。
//! 中文补充：凡是 parity test 会观察到的报错文本、资源释放时机、节点遍历规则，都必须保持可比对。
//! 中文补充：注释也会明确指出哪些能力只是占位，例如返回空 `tls::Config` 或把 PD client 投影成 server client。
//! 中文补充：这样做是为了防止后续维护者误把这些接口当成生产实现继续叠功能。
//! 中文补充：因此后续若要扩展功能，应优先补齐真实依赖，再决定是否删除这些兼容性桩。

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use astersql_lightning_pkg_server as server;
/// 中文补充：这些重导出把 ctl 需要的 error/config/context/TLS 入口统一收口到一个模块里，
/// 中文补充：避免 `main.rs` 同时依赖多个 crate 的细节路径。
pub use server::Error;
pub use server::Result;
pub use server::common;
pub use server::config;
pub use server::context;
pub use server::errors;
pub use server::import_sstpb;
/// Go `metapb.StoreState` (Up=0 < Offline=1 < Tombstone=2).
/// Server stub uses Offline=0 sentinel; ctl keeps the real Go values.
/// 中文补充：这里显式保留 Go 的枚举值，原因是 `ForAllStores` 通过数值比较决定哪些 store 会被遍历。
/// 中文补充：如果沿用 server stub 的哨兵值，`Offline` 节点会被错误过滤，导致 ctl 行为与 Go 偏离。
pub mod metapb {
    pub const StoreState_Up: i32 = 0;
    pub const StoreState_Offline: i32 = 1;
    pub const StoreState_Tombstone: i32 = 2;
}
/// 中文补充：PD HTTP 与 TLS 相关类型继续从 server stub 重导出，
/// 中文补充：这样 ctl 主流程只需要依赖当前模块，不必知道底层 crate 的拆分方式。
pub use server::pdhttp;
pub use server::tls;
/// 中文补充：checkpoint 控制器与切换模式逻辑仍复用 server 侧真实 stub，
/// 中文补充：本文件不复制这些业务能力，只负责把外围依赖桥接齐全。
pub use server::{CheckpointControl, NewCheckpointControl, SwitchMode};

pub use astersql_lightning_pkg_importer::FullLevelCompact;

/// Go `common.ErrCheckpointTableNotFound` identity.
/// 中文补充：这里单独建一个零大小标记类型，而不是直接复用字符串常量，
/// 中文补充：是为了保留 Go 里“特定错误类别可被 Equal 识别”的调用方式。
#[derive(Clone, Copy, Debug)]
pub struct CheckpointNotFound;

impl CheckpointNotFound {
    /// 中文补充：生成的错误文本既要让用户看到目标表名，也要保留 class，
    /// 中文补充：这样 `formatFatalError` 才能把它识别为“可提供恢复指引”的特殊报错。
    pub fn GenWithStackByArgs(self, table: impl fmt::Display) -> Error {
        let mut e = Error::new(format!("checkpoint for table {table} not found"));
        e.class = Some("Lightning:Checkpoint:ErrCheckpointTableNotFound");
        e
    }

    /// Mirrors RFC-code identity check used by `formatFatalError`.
    /// 中文补充：按 class 精确匹配，避免把文案相似的普通错误误判成该错误类。
    pub fn Equal(self, err: &Error) -> bool {
        err.class == Some("Lightning:Checkpoint:ErrCheckpointTableNotFound")
    }
}

#[allow(non_upper_case_globals)]
pub static ErrCheckpointTableNotFound: CheckpointNotFound = CheckpointNotFound;

/// Stack-aware error matching pingcap/errors.New for ErrorStack tests.
/// 中文补充：测试只需要“错误正文 + 可辨识调用点”这两个观察面，
/// 中文补充：因此这里不实现完整回溯，只记录 `track_caller` 给出的文件与行号。
#[derive(Clone, Debug)]
pub struct StackError {
    pub inner: Error,
    pub stack: String,
}

impl StackError {
    /// 中文补充：调用位置写入 `stack` 后，`ErrorStack` 输出就能稳定包含测试文件名，
    /// 中文补充：从而验证“普通错误要带栈，而 checkpoint 特判不要带栈”的差异。
    #[track_caller]
    pub fn new(msg: impl Into<String>) -> Self {
        let loc = std::panic::Location::caller();
        Self {
            inner: errors::New(msg),
            stack: format!("{}:{}", loc.file(), loc.line()),
        }
    }
    /// 中文补充：保留与 Go `error.Error()` 等价的访问器，避免调用方直接触碰内部字段布局。
    pub fn Error(&self) -> String {
        self.inner.Error()
    }
}

/// Mirrors `errors.ErrorStack`.
/// 中文补充：这里故意只拼两行，保证输出稳定且足够被测试断言。
pub fn ErrorStack(err: &StackError) -> String {
    format!("{}\n{}", err.Error(), err.stack)
}

// ---- FlagSet (Go `flag.FlagSet`) ----
// 中文补充：这一节实现的是 ctl 够用的 `flag.FlagSet` 子集。
// 中文补充：重点不是支持全部 flag 语法，而是复现 Go 版本会触发的默认值、缺参报错和 `Usage` 回退逻辑。

#[derive(Clone, Debug)]
enum FlagVal {
    // 中文补充：只保留 ctl 用到的三种标量类型，减少 stub 面积。
    Bool(bool),
    String(String),
    Int(i64),
}

#[derive(Clone, Debug)]
struct Flag {
    // 中文补充：usage 与默认值都会被帮助输出和测试观察到，因此不能只存当前值。
    usage: String,
    value: FlagVal,
    def: String,
}

/// Mutable handle returned by `FlagSet::Lookup` (Go `*flag.Flag`).
/// 中文补充：Go 代码会在注册 flag 之后回头修改 `-d` 的默认值，
/// 中文补充：因此这里需要一个“共享单元上的可变引用”语义，而不是按值拷贝。
#[derive(Clone, Debug)]
pub struct FlagRef {
    // 中文补充：名字字段主要用于调试输出与后续扩展，目前即便未直接读取也保留 Go 对应语义。
    name: String,
    cell: Arc<Mutex<Flag>>,
}

impl FlagRef {
    /// 中文补充：该方法只承担测试与初始化阶段的内部改值职责，
    /// 中文补充：容错地忽略非法整数解析，以贴近 Go `Set` 在本项目里的宽松使用方式。
    pub fn set_value(&self, v: &str) {
        let mut g = self.cell.lock().unwrap();
        match &mut g.value {
            FlagVal::String(s) => *s = v.to_string(),
            FlagVal::Bool(b) => *b = v == "true" || v == "1",
            FlagVal::Int(i) => {
                if let Ok(n) = v.parse() {
                    *i = n;
                }
            }
        }
    }
    /// 中文补充：`DefValue` 只影响帮助文本与默认值展示，不应顺带覆盖当前实际值。
    pub fn set_def_value(&self, v: &str) {
        self.cell.lock().unwrap().def = v.to_string();
    }
}

#[derive(Clone, Default)]
/// 中文补充：`FlagSet` 既保存注册表，也缓存解析后剩余的位置参数，
/// 中文补充：这样可以保持与 Go 类似的“遇到非 flag 后停止解析”行为。
pub struct FlagSet {
    flags: HashMap<String, Arc<Mutex<Flag>>>,
    pub args: Vec<String>,
    usage: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl fmt::Debug for FlagSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlagSet")
            .field("flags", &self.flags.keys().collect::<Vec<_>>())
            .field("args", &self.args)
            .finish()
    }
}

impl FlagSet {
    /// 中文补充：默认构造不预注册任何参数，调用方按 Go 主流程逐个添加。
    pub fn new() -> Self {
        Self::default()
    }

    /// 中文补充：允许 `main` 注入自定义 usage，以便测试验证“无动作时会触发帮助输出”。
    pub fn set_usage<F>(&mut self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.usage = Some(Arc::new(f));
    }

    /// 中文补充：优先调用外部自定义 usage；没有时才回退到打印默认 flag 列表。
    pub fn Usage(&self) {
        if let Some(u) = &self.usage {
            u();
        } else {
            self.PrintDefaults();
        }
    }

    /// 中文补充：按名字排序输出，避免哈希表遍历顺序让测试或人工比对结果不稳定。
    pub fn PrintDefaults(&self) {
        let mut names: Vec<_> = self.flags.keys().cloned().collect();
        names.sort();
        for name in names {
            let fl = self.flags[&name].lock().unwrap();
            eprintln!("  -{name}\t{}", fl.usage);
        }
    }

    /// 中文补充：返回的是共享句柄，让注册完成后仍能原地调整默认值与当前值。
    pub fn Lookup(&self, name: &str) -> Option<FlagRef> {
        self.flags.get(name).map(|cell| FlagRef {
            name: name.to_string(),
            cell: cell.clone(),
        })
    }

    /// 中文补充：内部统一注册入口，保证三种 flag 类型都共享同一份存储模型。
    fn define(&mut self, name: &str, usage: &str, value: FlagVal, def: &str) {
        self.flags.insert(
            name.to_string(),
            Arc::new(Mutex::new(Flag {
                usage: usage.to_string(),
                value,
                def: def.to_string(),
            })),
        );
    }

    /// 中文补充：布尔 flag 遵循 Go 习惯，出现但不显式赋值时会被视为 true。
    pub fn Bool(&mut self, name: &str, value: bool, usage: &str) {
        self.define(name, usage, FlagVal::Bool(value), &value.to_string());
    }

    /// 中文补充：字符串 flag 保留原默认文本，帮助输出时可以直接复用。
    pub fn String(&mut self, name: &str, value: &str, usage: &str) {
        self.define(name, usage, FlagVal::String(value.to_string()), value);
    }

    /// 中文补充：整数统一用 `i64` 存储，避免解析阶段再引入更多宽度差异。
    pub fn Int(&mut self, name: &str, value: i64, usage: &str) {
        self.define(name, usage, FlagVal::Int(value), &value.to_string());
    }

    /// 中文补充：getter 假定调用方知道 flag 类型；类型不符时回退到零值，保持 stub 简单。
    pub fn get_bool(&self, name: &str) -> bool {
        match &self.flags.get(name).unwrap().lock().unwrap().value {
            FlagVal::Bool(b) => *b,
            _ => false,
        }
    }

    /// 中文补充：字符串零值返回空串，方便后续用 `is_empty()` 判断是否显式覆盖。
    pub fn get_string(&self, name: &str) -> String {
        match &self.flags.get(name).unwrap().lock().unwrap().value {
            FlagVal::String(s) => s.clone(),
            _ => String::new(),
        }
    }

    /// 中文补充：整数零值与 Go 默认值语义相容，便于区分“未设置”和“显式非零覆盖”。
    pub fn get_int(&self, name: &str) -> i64 {
        match &self.flags.get(name).unwrap().lock().unwrap().value {
            FlagVal::Int(i) => *i,
            _ => 0,
        }
    }

    /// 中文补充：解析规则故意保持最小集合：
    /// 中文补充：支持 `-k=v`、`-k v`、布尔裸 flag、`--` 终止与位置参数截断。
    /// 中文补充：遇到未知 flag 或缺参时立即返回错误，和 Go 的用户体验一致。
    pub fn Parse(&mut self, args: &[String]) -> Result<()> {
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--" {
                // 中文补充：`--` 之后的内容全部作为位置参数保留，不再尝试按 flag 解释。
                self.args.extend(args[i + 1..].iter().cloned());
                break;
            }
            if a == "-" || a.starts_with("---") {
                return Err(Error::new(format!("bad flag syntax: {a}")));
            }
            if !a.starts_with('-') {
                // 中文补充：一旦遇到首个非 flag 参数，剩余参数按 Go 行为整体视为位置参数。
                self.args.extend(args[i..].iter().cloned());
                break;
            }
            let raw = a.trim_start_matches('-');
            let (name, inline) = if let Some((n, v)) = raw.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (raw.to_string(), None)
            };
            if (name == "h" || name == "help") && !self.flags.contains_key(&name) {
                self.Usage();
                return Err(Error::new("flag: help requested"));
            }
            let Some(cell) = self.flags.get(&name).cloned() else {
                return Err(Error::new(format!(
                    "flag provided but not defined: -{name}"
                )));
            };
            let mut fl = cell.lock().unwrap();
            match &mut fl.value {
                FlagVal::Bool(b) => {
                    // 中文补充：布尔 flag 裸出现即真；显式值使用 Go `strconv.ParseBool` 的全部拼写。
                    *b = match inline.as_deref() {
                        Some("1" | "t" | "T" | "TRUE" | "true" | "True") => true,
                        Some("0" | "f" | "F" | "FALSE" | "false" | "False") => false,
                        Some(value) => {
                            return Err(Error::new(format!(
                                "invalid value {value:?} for flag -{name}"
                            )));
                        }
                        None => true,
                    };
                }
                FlagVal::String(s) => {
                    // 中文补充：字符串既支持内联赋值，也支持吃掉下一个 argv。
                    *s = if let Some(v) = inline {
                        v
                    } else {
                        i += 1;
                        if i >= args.len() {
                            return Err(Error::new(format!("flag needs an argument: -{name}")));
                        }
                        args[i].clone()
                    };
                }
                FlagVal::Int(n) => {
                    // 中文补充：整数与字符串相同地支持两种取值方式，但会在这里立刻校验格式。
                    let v = if let Some(s) = inline {
                        s
                    } else {
                        i += 1;
                        if i >= args.len() {
                            return Err(Error::new(format!("flag needs an argument: -{name}")));
                        }
                        args[i].clone()
                    };
                    *n = v
                        .parse()
                        .map_err(|e| Error::new(format!("invalid value for -{name}: {e}")))?;
                }
            }
            i += 1;
        }
        Ok(())
    }
}

// ---- Global config load (subset of Go LoadGlobalConfig) ----
// 中文补充：这一节只实现 ctl 运行链路会读到的全局配置字段。
// 中文补充：目标是把命令行与极小子集配置文件合并成 `GlobalConfig`，再映射到 server stub 的 `config::Config`。

#[derive(Clone, Debug, Default)]
/// 中文补充：TiDB 相关字段全部保留为公开成员，便于后续映射到真正运行配置。
pub struct GlobalTiDB {
    pub Host: String,
    pub Port: i32,
    pub User: String,
    pub Psw: String,
    pub StatusPort: i32,
    pub PdAddr: String,
    pub LogLevel: String,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：ctl 只需要读到数据源目录，因此 `Mydumper` 目前只保留一个字段。
pub struct GlobalMydumper {
    pub SourceDir: String,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：Importer 只保留 backend 与排序目录，刚好覆盖 compact/checkpoint 等路径会观察到的配置。
pub struct GlobalImporter {
    pub Backend: String,
    pub SortedKVDir: String,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：checkpoint 总开关必须保留，因为 ctl 既可能操作 checkpoint，也可能明确关闭它。
pub struct GlobalCheckpoint {
    pub Enable: bool,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：这里承载的是 Lightning 应用层公共 flag，例如状态地址、server-mode 和日志配置。
pub struct GlobalLightning {
    pub StatusAddr: String,
    pub ServerMode: bool,
    pub CheckRequirements: bool,
    pub File: String,
    pub Level: String,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：`GlobalConfig` 是命令行装载阶段的中间表示，
/// 中文补充：它与真正执行期 `config::Config` 分离，便于先完成用户输入归一化，再做运行期校验。
pub struct GlobalConfig {
    pub App: GlobalLightning,
    pub Checkpoint: GlobalCheckpoint,
    pub TiDB: GlobalTiDB,
    pub Mydumper: GlobalMydumper,
    pub TikvImporter: GlobalImporter,
    pub Security: config::Security,
}

/// 中文补充：默认值尽量贴近 Go `config.NewGlobalConfig` 的 ctl 可见部分。
/// 中文补充：尤其是 `CheckRequirements=true`、checkpoint 默认开启、TiDB 默认主机与状态端口。
/// 中文补充：未在这里初始化的字段表示 ctl 当前不会依赖，不等于上游不存在该配置。
pub fn NewGlobalConfig() -> GlobalConfig {
    GlobalConfig {
        App: GlobalLightning {
            CheckRequirements: true,
            ..Default::default()
        },
        Checkpoint: GlobalCheckpoint { Enable: true },
        TiDB: GlobalTiDB {
            Host: "127.0.0.1".into(),
            User: "root".into(),
            StatusPort: 10080,
            LogLevel: "error".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 中文补充：默认退出实现保留真实 `process::exit`，测试可通过覆盖钩子接管。
fn default_exit(code: i32) {
    std::process::exit(code);
}

static EXIT_FN: Mutex<fn(i32)> = Mutex::new(default_exit);
static LAST_CLIENT_CLOSED: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
static LAST_CLIENT_ENDPOINTS: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();

/// Override process exit (Go `exit` / `Must`).
/// 中文补充：把退出函数做成全局可替换钩子，是为了在测试中观察退出码而不真的结束进程。
pub fn set_exit_fn(f: fn(i32)) {
    *EXIT_FN.lock().unwrap() = f;
}

/// 中文补充：每个测试结束后都应恢复默认退出行为，避免污染其他测试。
pub fn reset_exit_fn() {
    *EXIT_FN.lock().unwrap() = default_exit;
}

/// 中文补充：统一通过这一层触发退出，调用方无需知道当前是生产环境还是测试钩子。
pub fn call_exit(code: i32) {
    EXIT_FN.lock().unwrap()(code);
}

/// 中文补充：记录最近一次 PD client 的关闭标记，供测试证明 `defer cli.Close` 等价语义。
fn last_client_closed() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    LAST_CLIENT_CLOSED.get_or_init(|| Mutex::new(None))
}

/// 中文补充：在每轮测试前清掉上一次观测结果，避免误把旧状态当成本次资源释放证据。
pub fn clear_last_client_closed() {
    *last_client_closed().lock().unwrap() = None;
}

/// 中文补充：取出后即清空，确保一次测试只能消费一次关闭结果。
pub fn take_last_client_closed() -> Option<bool> {
    last_client_closed()
        .lock()
        .unwrap()
        .take()
        .map(|closed| closed.load(Ordering::SeqCst))
}

/// Return the endpoint vector passed to the most recently constructed PD client.
pub fn take_last_client_endpoints() -> Option<Vec<String>> {
    LAST_CLIENT_ENDPOINTS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .take()
}

/// Mirrors `config.Must`.
/// 中文补充：保持 Go 的策略：
/// 中文补充：帮助请求退出 0，其他装载错误打印后退出 2，但函数本身仍返回一个默认配置给调用方占位。
pub fn Must(cfg: Result<GlobalConfig>) -> GlobalConfig {
    match cfg {
        Ok(c) => c,
        Err(err) => {
            if err.Error() == "flag: help requested" {
                call_exit(0);
            } else {
                println!("{err}");
                call_exit(2);
            }
            NewGlobalConfig()
        }
    }
}

/// Ctl-specific flags registered via `extraFlags` (mirrors Go main).
/// 中文补充：这些字段是 `main.rs` 动作分发阶段真正消费的那一组控制 flag，
/// 中文补充：与 `GlobalConfig` 分离后，能更清晰地区分“运行配置”和“本次命令动作”。
#[derive(Clone, Debug, Default)]
pub struct CtlActionFlags {
    pub compact: bool,
    pub fetch_mode: bool,
    pub mode: String,
    pub cp_remove: String,
    pub cp_err_ignore: String,
    pub cp_err_destroy: String,
    pub cp_dump: String,
    pub local_storing_tables: bool,
    pub usage_invoked: Arc<AtomicBool>,
}

/// Load global config and ctl action flags (Go `LoadGlobalConfig` + extraFlags).
/// 中文补充：该函数是整个 stub 中最接近 Go 主流程的入口：
/// 中文补充：先注册基础全局参数，再叠加 ctl 专属 flag，最后把结果拆成配置、动作和 `FlagSet` 三部分返回。
pub fn LoadGlobalConfigWithCtl(args: &[String]) -> Result<(GlobalConfig, CtlActionFlags, FlagSet)> {
    let mut cfg = NewGlobalConfig();
    let mut fs = FlagSet::new();
    let usage_invoked = Arc::new(AtomicBool::new(false));
    let usage_flag = usage_invoked.clone();
    fs.set_usage(move || {
        // 中文补充：记录 usage 是否被调用，便于测试验证“无动作时显示帮助”这一行为。
        usage_flag.store(true, Ordering::SeqCst);
        eprintln!("tidb-lightning-ctl usage");
    });

    // 中文补充：这一组是共享的全局 Lightning flag，ctl 与主程序保持同名输入。
    fs.String("config", "", "tidb-lightning configuration file");
    fs.String("c", "", "(deprecated alias of -config)");
    fs.Bool("V", false, "print version of lightning");
    fs.String("L", "", "log level");
    fs.String("log-file", "", "log file path");
    fs.String("tidb-host", "", "TiDB server host");
    fs.Int("tidb-port", 0, "TiDB server port");
    fs.String("tidb-user", "", "TiDB user name");
    fs.String("tidb-password", "", "TiDB password");
    fs.Int("tidb-status", 0, "TiDB status port");
    fs.String("pd-urls", "", "PD endpoint address");
    fs.String("d", "", "Directory of the dump to import");
    fs.String("backend", "", "delivery backend");
    fs.String("sorted-kv-dir", "", "path for KV pairs");
    fs.Bool("enable-checkpoint", true, "whether to enable checkpoints");
    fs.String("status-addr", "", "Lightning server address");
    fs.Bool("server-mode", false, "start in server mode");
    fs.String("ca", "", "CA certificate path");
    fs.String("cert", "", "certificate path");
    fs.String("key", "", "private key path");

    // change the default of `-d` from empty to 'noop://' (Go ctl extraFlags).
    // 中文补充：ctl 自身不读取源目录内容，但全局配置装载会校验 `-d` 是否指向有效存储。
    // 中文补充：因此这里改成安全且无副作用的 `noop://`，避免仅执行控制命令也被无关校验拦住。
    if let Some(d_flag) = fs.Lookup("d") {
        d_flag.set_value("noop://");
        d_flag.set_def_value("noop://");
    }

    // 中文补充：下面是 ctl 自有动作 flag，决定后续 `dispatch` 会走哪条分支。
    fs.Bool(
        "compact",
        false,
        "do manual compaction on the target cluster",
    );
    fs.String(
        "switch-mode",
        "",
        "switch tikv into import mode or normal mode, values can be ['import', 'normal']",
    );
    fs.Bool(
        "fetch-mode",
        false,
        "obtain the current mode of every tikv in the cluster",
    );
    fs.String(
        "checkpoint-remove",
        "",
        "remove the checkpoint associated with the given table (value can be 'all' or '`db`.`table`')",
    );
    fs.String(
        "checkpoint-error-ignore",
        "",
        "ignore errors encoutered previously on the given table (value can be 'all' or '`db`.`table`'); may corrupt this table if used incorrectly",
    );
    fs.String(
        "checkpoint-error-destroy",
        "",
        "deletes imported data with table which has an error before (value can be 'all' or '`db`.`table`')",
    );
    fs.String(
        "checkpoint-dump",
        "",
        "dump the checkpoint information as three CSV files in the given folder",
    );
    fs.Bool(
        "check-local-storage",
        false,
        "show tables that are missing local intermediate files (value can be 'all' or '`db`.`table`')",
    );

    fs.Parse(args)?;

    // 中文补充：`-V` 在 ctl 里等价于打印版本并走帮助退出路径。
    if fs.get_bool("V") {
        println!("tidb-lightning-ctl");
        return Err(Error::new("flag: help requested"));
    }

    // 中文补充：兼容 Go 的 `-config` / `-c` 共享变量语义，命令行中最后出现的值生效。
    let mut config_file = String::new();
    let mut arg_index = 0;
    while arg_index < args.len() {
        let arg = &args[arg_index];
        if arg == "--" || !arg.starts_with('-') {
            break;
        }
        let raw = arg.trim_start_matches('-');
        if let Some((name, value)) = raw.split_once('=') {
            if name == "c" || name == "config" {
                config_file = value.to_string();
            }
        } else if raw == "c" || raw == "config" {
            arg_index += 1;
            config_file = args[arg_index].clone();
        }
        arg_index += 1;
    }
    if !config_file.is_empty() {
        // 中文补充：这里不尝试完整 TOML/YAML 解析，只抽取 ctl 当前会读到的键，降低 stub 复杂度。
        let data = std::fs::read(&config_file)
            .map_err(|e| Error::new(format!("cannot read config file {config_file}: {e}")))?;
        let s = String::from_utf8_lossy(&data);
        for line in s.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("pd-addr") {
                // 中文补充：支持最简单的 `key = "value"` 形式，够覆盖测试与常见配置示例。
                let rest = rest.trim().trim_start_matches('=').trim().trim_matches('"');
                cfg.TiDB.PdAddr = rest.to_string();
            }
            if let Some(rest) = line.strip_prefix("backend") {
                let rest = rest.trim().trim_start_matches('=').trim().trim_matches('"');
                cfg.TikvImporter.Backend = rest.to_string();
            }
        }
    }

    // 中文补充：命令行非空值覆盖默认配置或文件配置，顺序与 Go 一致。
    let log_level = fs.get_string("L");
    if !log_level.is_empty() {
        cfg.App.Level = log_level;
    }
    let log_file = fs.get_string("log-file");
    if !log_file.is_empty() {
        cfg.App.File = log_file;
    }
    let tidb_host = fs.get_string("tidb-host");
    if !tidb_host.is_empty() {
        cfg.TiDB.Host = tidb_host;
    }
    let tidb_port = fs.get_int("tidb-port") as i32;
    if tidb_port != 0 {
        cfg.TiDB.Port = tidb_port;
    }
    let tidb_status = fs.get_int("tidb-status") as i32;
    if tidb_status != 0 {
        cfg.TiDB.StatusPort = tidb_status;
    }
    let tidb_user = fs.get_string("tidb-user");
    if !tidb_user.is_empty() {
        cfg.TiDB.User = tidb_user;
    }
    let tidb_psw = fs.get_string("tidb-password");
    if !tidb_psw.is_empty() {
        cfg.TiDB.Psw = tidb_psw;
    }
    let pd_addr = fs.get_string("pd-urls");
    if !pd_addr.is_empty() {
        cfg.TiDB.PdAddr = pd_addr;
    }
    let data_src = fs.get_string("d");
    if !data_src.is_empty() {
        cfg.Mydumper.SourceDir = data_src;
    }
    if fs.get_bool("server-mode") {
        cfg.App.ServerMode = true;
    }
    let status_addr = fs.get_string("status-addr");
    if !status_addr.is_empty() {
        cfg.App.StatusAddr = status_addr;
    }
    let backend = fs.get_string("backend");
    if !backend.is_empty() {
        cfg.TikvImporter.Backend = backend;
    }
    let sorted_kv = fs.get_string("sorted-kv-dir");
    if !sorted_kv.is_empty() {
        cfg.TikvImporter.SortedKVDir = sorted_kv;
    }
    // enable-checkpoint defaults true; presence without =false keeps true.
    // Go uses Bool pointer default true; `-enable-checkpoint=false` clears it.
    // Our Bool parse without value sets true; use inline false when provided.
    // 中文补充：这里特地保留布尔默认值与显式 false 的差异，否则 checkpoint 控制命令会错误地以为总是开启。
    if !fs.get_bool("enable-checkpoint") {
        cfg.Checkpoint.Enable = false;
    }
    let ca = fs.get_string("ca");
    if !ca.is_empty() {
        cfg.Security.CAPath = ca;
    }
    let cert = fs.get_string("cert");
    if !cert.is_empty() {
        cfg.Security.CertPath = cert;
    }
    let key = fs.get_string("key");
    if !key.is_empty() {
        cfg.Security.KeyPath = key;
    }

    if cfg.App.StatusAddr.is_empty() && cfg.App.ServerMode {
        // 中文补充：server-mode 必须有监听地址，这个前置校验和 Go 一样在真正连外部系统前执行。
        return Err(Error::new(
            "If server-mode is enabled, the status-addr must be a valid listen address",
        ));
    }

    // 中文补充：动作 flag 保持原始文本，不在装载阶段提前做业务合法性校验，交给后续 dispatch/子模块处理。
    let actions = CtlActionFlags {
        compact: fs.get_bool("compact"),
        fetch_mode: fs.get_bool("fetch-mode"),
        mode: fs.get_string("switch-mode"),
        cp_remove: fs.get_string("checkpoint-remove"),
        cp_err_ignore: fs.get_string("checkpoint-error-ignore"),
        cp_err_destroy: fs.get_string("checkpoint-error-destroy"),
        cp_dump: fs.get_string("checkpoint-dump"),
        local_storing_tables: fs.get_bool("check-local-storage"),
        usage_invoked,
    };
    Ok((cfg, actions, fs))
}

/// Apply GlobalConfig onto server `config::Config` (Go `LoadFromGlobal` fields ctl needs).
/// 中文补充：这里只同步 ctl 访问到的字段，故意不宣称已经完整覆盖所有 Lightning 配置项。
pub fn LoadFromGlobal(cfg: &mut config::Config, g: &GlobalConfig) -> Result<()> {
    cfg.Security = g.Security.clone();
    cfg.App.StatusAddr = g.App.StatusAddr.clone();
    cfg.App.Config.File = g.App.File.clone();
    // log::Config level field may be named differently across stubs; File is enough for ctl.
    // 中文补充：日志级别字段在不同精简 stub 里可能名字不一致，因此这里只复制 ctl 真实会观测的文件路径。
    cfg.TiDB.Host = g.TiDB.Host.clone();
    cfg.TiDB.Port = g.TiDB.Port;
    cfg.TiDB.User = g.TiDB.User.clone();
    cfg.TiDB.Psw = g.TiDB.Psw.clone();
    cfg.TiDB.PdAddr = g.TiDB.PdAddr.clone();
    cfg.TiDB.Security = g.Security.clone();
    cfg.Mydumper.SourceDir = g.Mydumper.SourceDir.clone();
    if !g.TikvImporter.Backend.is_empty() {
        cfg.TikvImporter.Backend = g.TikvImporter.Backend.clone();
    }
    if !g.TikvImporter.SortedKVDir.is_empty() {
        cfg.TikvImporter.SortedKVDir = g.TikvImporter.SortedKVDir.clone();
    }
    cfg.Checkpoint.Enable = g.Checkpoint.Enable;
    Ok(())
}

/// Go `Config.ToTLS`.
/// 中文补充：ctl 最终总是拿 TiDB 主机拼出 `host:10080` 作为 TLS server name 来源，
/// 中文补充：因此这里即使先构造了一次裁剪端口的字符串，也会被后一行固定成状态端口 10080。
/// 中文补充：保留这个看似冗余的步骤，是为了让代码形态继续贴近演化中的上游 stub，而不擅自重写行为。
pub fn ToTLS(cfg: &config::Config) -> Result<common::TLS> {
    let host_port = format!("{}:{}", cfg.TiDB.Host, cfg.TiDB.Port.max(1).min(65535));
    let host_port = format!("{}:{}", cfg.TiDB.Host, 10080);
    let _ = host_port;
    common::NewTLS(
        &cfg.Security.CAPath,
        &cfg.Security.CertPath,
        &cfg.Security.KeyPath,
        &format!("{}:{}", cfg.TiDB.Host, 10080),
        &cfg.Security.CABytes,
        &cfg.Security.CertBytes,
        &cfg.Security.KeyBytes,
    )
}

pub trait TlsExt {
    /// 中文补充：这里提供与 Go `tls.TLSConfig()` 相似的调用面，方便主流程统一取 client option。
    fn TLSConfig(&self) -> tls::Config;
}
impl TlsExt for common::TLS {
    fn TLSConfig(&self) -> tls::Config {
        // 中文补充：stub 不建立真实 TLS 会话，因此返回空配置即可；调用方只关心接口存在与否。
        tls::Config::default()
    }
}

// ---- PD HTTP client ----
// 中文补充：这一节把 PD client 缩减成“可关闭、可注入 store 列表、可被测试观察关闭状态”的最小实现。

#[derive(Clone, Debug, Default)]
/// 中文补充：`MetaStore` 只保留 ctl 会使用到的地址和状态位。
pub struct MetaStore {
    pub Address: String,
    pub State: i64,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：包一层 `StoreInfo` 是为了保持字段访问路径与 Go/PD HTTP 返回结构一致。
pub struct StoreInfo {
    pub Store: MetaStore,
}

#[derive(Clone, Debug, Default)]
/// 中文补充：列表层同样沿用上游命名，方便 `ForAllStores` 直接照搬遍历逻辑。
pub struct StoresInfo {
    pub Stores: Vec<StoreInfo>,
}

#[derive(Clone, Debug)]
/// 中文补充：`PdClient` 不发真实 HTTP 请求，只负责保存端点、生命周期与伪造的 store 快照。
pub struct PdClient {
    pub name: String,
    pub endpoints: Vec<String>,
    closed: Arc<AtomicBool>,
    stores: Arc<Mutex<StoresInfo>>,
}

impl PdClient {
    /// 中文补充：关闭只翻转原子标记；真实资源释放由 stub 简化掉，但测试仍能观测到时机。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    /// 中文补充：辅助测试和防御性分支判断，避免对外暴露内部原子实现细节。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    /// 中文补充：导出共享关闭标记，让外部可以在 client 被消费后继续验证关闭结果。
    pub fn closed_flag(&self) -> Arc<AtomicBool> {
        self.closed.clone()
    }
    /// 中文补充：如果 client 已关闭则拒绝再取 store，模拟真实客户端生命周期约束。
    pub fn GetStores(&self, _ctx: &context::Context) -> Result<StoresInfo> {
        if self.is_closed() {
            return Err(Error::new("pd client closed"));
        }
        Ok(self.stores.lock().unwrap().clone())
    }
    /// 中文补充：测试可通过该方法预置集群拓扑，再让 `ForAllStores` 与各类控制动作消费。
    pub fn set_stores(&self, stores: StoresInfo) {
        *self.stores.lock().unwrap() = stores;
    }
    /// 中文补充：server 侧当前只需要一个类型占位，因此这里直接投影成空 `pdhttp::Client`。
    pub fn as_server_client(&self) -> pdhttp::Client {
        pdhttp::Client
    }
}

impl Drop for PdClient {
    fn drop(&mut self) {
        // 中文补充：即便调用方忘记显式关闭，析构时仍会把关闭标记置位，尽量靠近 Go `defer Close` 的安全性。
        self.Close();
    }
}

#[derive(Clone, Debug)]
pub enum ClientOption {
    // 中文补充：目前只需要表达“带 TLS 参数创建 client”这一种选项。
    Tls,
}

/// 中文补充：保留工厂函数形态，让调用方看起来仍像在配置真正的 PD client。
pub fn WithTLSConfig(_cfg: tls::Config) -> ClientOption {
    ClientOption::Tls
}

/// 中文补充：构造 client 时同步登记其关闭标记，便于测试在主流程结束后检查是否已释放。
pub fn NewClient(name: &str, endpoints: Vec<String>, _opts: Vec<ClientOption>) -> PdClient {
    // 中文补充：端点列表按调用方传入顺序保存，方便测试断言 split 后的地址顺序未被打乱。
    *LAST_CLIENT_ENDPOINTS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(endpoints.clone());
    let client = PdClient {
        name: name.to_string(),
        endpoints,
        closed: Arc::new(AtomicBool::new(false)),
        stores: Arc::new(Mutex::new(StoresInfo::default())),
    };
    *last_client_closed().lock().unwrap() = Some(client.closed_flag());
    client
}

// ---- TiKV helpers (network mocked) ----
// 中文补充：这一节模拟 ctl 会触达的 TiKV 边界。
// 中文补充：压缩操作只记录调用参数，模式查询则从预置 map 返回结果，都不会发真实网络请求。

static COMPACT_CALLS: OnceLock<Mutex<Vec<(String, i32, String)>>> = OnceLock::new();
static FETCH_MODE_RESULTS: OnceLock<Mutex<HashMap<String, std::result::Result<String, String>>>> =
    OnceLock::new();

/// 中文补充：惰性初始化全局调用记录，保证测试之间共享而又可重置。
fn compact_calls() -> &'static Mutex<Vec<(String, i32, String)>> {
    COMPACT_CALLS.get_or_init(|| Mutex::new(Vec::new()))
}
/// 中文补充：模式查询结果按地址索引，便于模拟不同节点成功或失败的混合场景。
fn fetch_mode_map() -> &'static Mutex<HashMap<String, std::result::Result<String, String>>> {
    FETCH_MODE_RESULTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 中文补充：每轮测试前清空 TiKV 侧 mock 状态，避免跨用例残留。
pub fn reset_tikv_mocks() {
    compact_calls().lock().unwrap().clear();
    fetch_mode_map().lock().unwrap().clear();
}

/// 中文补充：按节点地址预设 `FetchMode` 的返回值，可模拟正常模式、导入模式或错误。
pub fn mock_fetch_mode(addr: &str, result: std::result::Result<String, String>) {
    fetch_mode_map()
        .lock()
        .unwrap()
        .insert(addr.to_string(), result);
}

/// 中文补充：读取并清空 compact 调用轨迹，便于断言遍历顺序与参数。
pub fn take_compact_calls() -> Vec<(String, i32, String)> {
    std::mem::take(&mut *compact_calls().lock().unwrap())
}

/// Go `tikv.ForAllStores` — maxState inclusive (Up < Offline < Tombstone).
/// 中文补充：这里复刻的是“遍历所有状态不高于上限的 store，并保留首个错误”的控制流。
/// 中文补充：即便后续节点继续执行，最终也只返回第一次失败，和 Go 版本一致。
pub fn ForAllStores<F>(
    ctx: &context::Context,
    cli: &PdClient,
    max_state: i32,
    action: F,
) -> Result<()>
where
    F: Fn(&context::Context, &MetaStore) -> Result<()> + Sync,
{
    let stores = cli.GetStores(ctx)?;
    let (child_ctx, cancel) = context::WithCancel(ctx.clone());
    let first_err = Arc::new(Mutex::new(None));
    std::thread::scope(|scope| {
        for info in stores.Stores {
            // 中文补充：状态比较是包含上界的，因此传 `Offline` 时会同时覆盖 `Up` 与 `Offline`。
            if info.Store.State <= max_state as i64 {
                let child_ctx = child_ctx.clone();
                let first_err = Arc::clone(&first_err);
                let cancel = Arc::clone(&cancel);
                let action = &action;
                scope.spawn(move || {
                    if let Err(err) = action(&child_ctx, &info.Store) {
                        let mut first = first_err.lock().unwrap();
                        if first.is_none() {
                            *first = Some(err);
                            cancel();
                        }
                    }
                });
            }
        }
    });
    // `errgroup.WithContext` also cancels the derived context after `Wait`,
    // including the all-success path.
    cancel();
    let result = first_err.lock().unwrap().take();
    match result {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Go `tikv.Compact` — records call at the network boundary.
/// 中文补充：真实 compact 逻辑在这里被有意裁掉，只保留“请求会发向哪个 TiKV、带什么 level 与资源组”。
pub fn Compact(
    _ctx: &context::Context,
    _tls: &common::TLS,
    tikv_addr: &str,
    level: i32,
    resource_group_name: &str,
) -> Result<()> {
    compact_calls().lock().unwrap().push((
        tikv_addr.to_string(),
        level,
        resource_group_name.to_string(),
    ));
    Ok(())
}

/// Go `tikv.FetchModeFromMetrics` pure algorithm.
/// 中文补充：这个函数保留了唯一的纯文本解析算法，因为它不依赖网络且容易被 parity test 精确比较。
/// 中文补充：约定是：
/// 中文补充：找不到指标则报错，
/// 中文补充：值为 `0` 表示 import，
/// 中文补充：其他非空值一律视为 normal。
pub fn FetchModeFromMetrics(metrics: &str) -> Result<String> {
    let marker =
        "tikv_config_rocksdb{cf=\"default\",name=\"hard_pending_compaction_bytes_limit\"} ";
    let mut found: Option<&str> = None;
    for line in metrics.split('\n') {
        let mut from = 0;
        while let Some(relative) = line[from..].find(marker) {
            let idx = from + relative;
            let has_word_boundary = idx == 0
                || !line.as_bytes()[idx - 1].is_ascii_alphanumeric()
                    && line.as_bytes()[idx - 1] != b'_';
            let value = &line[idx + marker.len()..];
            if has_word_boundary && !value.is_empty() {
                // Go's `([^\n]+)` capture is the complete remainder of the
                // line. In particular, `0 ` is normal mode rather than `0`.
                found = Some(value);
                break;
            }
            from = idx + 1;
        }
        if found.is_some() {
            break;
        }
    }
    match found {
        // 中文补充：缺少指标通常意味着目标 TiKV 未暴露该状态，而不是简单地处于 normal 模式。
        None => Err(Error::new("import mode status is not exposed")),
        // 中文补充：这里遵循 Go 的业务约定，`0` 代表 hard pending compaction 限制被关闭，即 import mode。
        Some("0") => Ok("import".into()),
        // 中文补充：其余值不区分具体阈值大小，只要暴露且非零就统一解释成 normal。
        Some(_) => Ok("normal".into()),
    }
}

/// Go `tikv.FetchMode` — mock map / no real gRPC.
/// 中文补充：真实实现会去抓 TiKV metrics 或 gRPC；stub 版只从预置 map 返回，缺省则报“未提供 mock”。
pub fn FetchMode(_ctx: &context::Context, _tls: &common::TLS, tikv_addr: &str) -> Result<String> {
    if let Some(r) = fetch_mode_map().lock().unwrap().get(tikv_addr).cloned() {
        // 中文补充：这里直接复用预置结果，既允许成功，也允许把字符串错误包装成 `Error`。
        return r.map_err(Error::new);
    }
    Err(Error::new(format!(
        "fetch mode unavailable for {tikv_addr}"
    )))
}
