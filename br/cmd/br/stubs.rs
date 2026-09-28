// Copyright 2026 AsterSQL.
//! Local stand-ins for cobra/glue/PD/TiKV/session/config/CLI boundaries (arm64-safe).
//!
//! BR `cmd` 包的本地桩层：在无法链真实 cobra/PD/TiKV/session 时，提供足以驱动
//! CLI 装配、参数解析与单元测试的最小替身。
//! 本文件是适配边界，不是完整实现：多数方法只记录状态、返回固定值或显式错误，
//! 不得据此认为已接入真实集群能力。
//! 结构按职责分区：Error/berrors → Context → Command(cobra 替身) → 日志/构建/
//! summary/session/config → glue(Tidb/Tikv) → debug 辅助(backuppb/metautil/…)。
//! 与 Go 侧真实依赖一一对应的是符号名与调用点，语义深度刻意缩水以保持 arm64 可测。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;

use astersql_br_pkg_task::stubs::{FlagSet as TaskFlagSet, Glue as TaskGlue, MemGlue};
use astersql_br_pkg_task_operator::stubs::{
    FlagSet as OpFlagSet, Glue as OpGlue, KVStorage, Result as OpResult,
};

/// CLI 桩统一 Result；错误为本地精简 Error。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 精简错误：仅消息串，供 Trace/Annotate 与测试断言。
pub struct Error {
    pub msg: String,
}

impl Error {
    // 基础构造；无堆栈。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
    // 对齐 Go errors.Errorf 命名。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
    // 消息形如 `msg: base`，便于 ErrorEqual。
    pub fn Annotate(base: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", msg.into(), base.into()),
        }
    }
    pub fn Annotatef(base: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::Annotate(base, msg)
    }
    // 桩无堆栈，直接透传。
    pub fn Trace(err: Self) -> Self {
        err
    }
    // 外层消息在前。
    pub fn Wrapf(err: Self, msg: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", msg.into(), err.msg),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

impl From<astersql_br_pkg_task::stubs::Error> for Error {
    fn from(e: astersql_br_pkg_task::stubs::Error) -> Self {
        Self::new(e.msg)
    }
}

impl From<astersql_br_pkg_task_operator::stubs::Error> for Error {
    fn from(e: astersql_br_pkg_task_operator::stubs::Error) -> Self {
        Self::new(e.msg)
    }
}

/// BR 错误码常量与相等判断；仅覆盖 cmd 现用码。
pub mod berrors {
    use super::Error;

    // 目标集群非空。
    pub const ErrRestoreNotFreshCluster: &str = "BR:Restore:ErrRestoreNotFreshCluster";
    // 系统表不兼容。
    pub const ErrRestoreIncompatibleSys: &str = "BR:Restore:ErrRestoreIncompatibleSys";
    // 校验失败。
    pub const ErrBackupChecksumMismatch: &str = "BR:Backup:ErrBackupChecksumMismatch";
    // 通用非法参数。
    pub const ErrInvalidArgument: &str = "invalid argument";

    // 整串或子串匹配。
    pub fn ErrorEqual(err: &Error, code: &str) -> bool {
        err.msg.contains(code) || err.msg == code
    }

    // Annotate 校验错误码。
    pub fn checksum_mismatch(msg: impl Into<String>) -> Error {
        Error::Annotate(ErrBackupChecksumMismatch, msg)
    }
}

// --- size constants (pkg/util/size) ---
// 容量单位，对齐 Go pkg/util/size。
/// 1 MiB。
pub const MB: u64 = 1 << 20;
/// 1 GiB。
pub const GB: u64 = 1 << 30;

// --- context ---
// Context 替身：取消标志 + WithValue，无截止时间树。
#[derive(Clone, Default)]
/// 可取消上下文；values 用 u64 键。
pub struct Context {
    pub cancelled: Arc<AtomicBool>,
    pub values: Arc<Mutex<HashMap<u64, Arc<dyn std::any::Any + Send + Sync>>>>,
    ancestors: Vec<Arc<AtomicBool>>,
    next_key: Arc<AtomicU64>,
}

impl Context {
    // 对齐 context.Background。
    pub fn Background() -> Self {
        Self::default()
    }
    // 继承 values 快照与父取消链，并保留独立的本级取消标志。
    pub fn WithCancel(parent: &Self) -> (Self, Box<dyn Fn() + Send>) {
        let mut ancestors = parent.ancestors.clone();
        ancestors.push(parent.cancelled.clone());
        let ctx = Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            values: parent.values.clone(),
            ancestors,
            next_key: parent.next_key.clone(),
        };
        let flag = ctx.cancelled.clone();
        let cancel = Box::new(move || {
            flag.store(true, Ordering::SeqCst);
        });
        (ctx, cancel)
    }
    // 派生不可变 values 快照，避免子上下文反向污染父上下文。
    pub fn WithValue(parent: &Self, key: u64, val: Arc<dyn std::any::Any + Send + Sync>) -> Self {
        let mut ctx = parent.clone();
        ctx.values = Arc::new(Mutex::new(parent.values.lock().unwrap().clone()));
        ctx.values.lock().unwrap().insert(key, val);
        ctx
    }
    // 按键读取。
    pub fn Value(&self, key: u64) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        self.values.lock().unwrap().get(&key).cloned()
    }
    // 取消是否置位。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
            || self
                .ancestors
                .iter()
                .any(|flag| flag.load(Ordering::SeqCst))
    }
}

// --- cobra ---
// cobra.Command 替身：子命令树、PreRun/RunE、flag 继承与 Execute。
// 非完整解析器；args 已拆好切片。
/// 叶子执行回调。
pub type RunEFn = Arc<dyn Fn(&mut Command, &[String]) -> Result<()> + Send + Sync>;
/// 持久前置回调。
pub type PreRunEFn = Arc<dyn Fn(&mut Command, &[String]) -> Result<()> + Send + Sync>;
/// 自定义 Help。
pub type HelpFn = Arc<dyn Fn(&mut Command, &[String]) + Send + Sync>;
/// 状态 HTTP 注册回调。
pub type StatusServerRegistrar = Arc<dyn Fn(&mut ServeMux) + Send + Sync>;
/// 状态服务准备回调。
pub type StatusServerPreparer =
    Arc<dyn Fn(&mut Command) -> Result<Option<StatusServerRegistrar>> + Send + Sync>;

// Command 单调 id。
static CMD_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
/// cobra.Command 替身；Root 返回 self。
pub struct Command {
    pub id: u64,
    pub Use: String,
    pub Short: String,
    pub Long: String,
    // 默认静默 usage。
    pub SilenceUsage: bool,
    // 桩未使用。
    pub TraverseChildren: bool,
    // 隐藏子命令。
    pub Hidden: bool,
    // 版本占位。
    pub Version: String,
    // 版本模板占位。
    pub version_template: String,
    // 别名。
    pub Aliases: Vec<String>,
    // 本地 flags。
    pub flags: TaskFlagSet,
    // 可继承 flags。
    pub persistent_flags: TaskFlagSet,
    // operator flags。
    pub op_flags: OpFlagSet,
    // advancer duration。
    pub advancer_flags: HashMap<&'static str, Duration>,
    // 子命令。
    pub children: Vec<Command>,
    // 叶子执行体。
    pub RunE: Option<RunEFn>,
    // 持久前置。
    pub PersistentPreRunE: Option<PreRunEFn>,
    // 自定义 Help。
    pub help_fn: Option<HelpFn>,
    // stdout 缓冲。
    pub out: Arc<Mutex<Vec<u8>>>,
    // stderr 缓冲。
    pub err_out: Arc<Mutex<Vec<u8>>>,
    // Execute 参数。
    pub args: Vec<String>,
    // 运行时 Context。
    pub context: Option<Context>,
    // 无位置参数标记。
    pub no_args: bool,
}

// 分配新 id；空 flags。
impl Default for Command {
    fn default() -> Self {
        Self {
            id: CMD_ID.fetch_add(1, Ordering::SeqCst),
            Use: String::new(),
            Short: String::new(),
            Long: String::new(),
            SilenceUsage: false,
            TraverseChildren: false,
            Hidden: false,
            Version: String::new(),
            version_template: String::new(),
            Aliases: Vec::new(),
            flags: TaskFlagSet::new(),
            persistent_flags: TaskFlagSet::new(),
            op_flags: OpFlagSet::new(),
            advancer_flags: HashMap::new(),
            children: Vec::new(),
            RunE: None,
            PersistentPreRunE: None,
            help_fn: None,
            out: Arc::new(Mutex::new(Vec::new())),
            err_out: Arc::new(Mutex::new(Vec::new())),
            args: Vec::new(),
            context: None,
            no_args: false,
        }
    }
}

impl Command {
    // 本地标志。
    pub fn Flags(&mut self) -> &mut TaskFlagSet {
        &mut self.flags
    }
    // 持久标志。
    pub fn PersistentFlags(&mut self) -> &mut TaskFlagSet {
        &mut self.persistent_flags
    }
    // operator 标志。
    pub fn OpFlags(&mut self) -> &mut OpFlagSet {
        &mut self.op_flags
    }
    // advancer 标志。
    pub fn AdvancerFlags(&mut self) -> &mut HashMap<&'static str, Duration> {
        &mut self.advancer_flags
    }
    // 追加子命令。
    pub fn AddCommand(&mut self, cmds: Vec<Command>) {
        self.children.extend(cmds);
    }
    // stdout stand-in; writes go to self.out
    // 写入 self.out。
    pub fn SetOut(&mut self, _out: ()) {
        // stdout stand-in; writes go to self.out
    }
    // 注入参数。
    pub fn SetArgs(&mut self, args: Vec<String>) {
        self.args = args;
    }
    // 版本模板。
    pub fn SetVersionTemplate(&mut self, t: &str) {
        self.version_template = t.to_string();
    }
    // 挂 Context。
    pub fn SetContext(&mut self, ctx: Context) {
        self.context = Some(ctx);
    }
    // 取 Context。
    pub fn Context(&self) -> Option<&Context> {
        self.context.as_ref()
    }
    // 覆盖 Help。
    pub fn SetHelpFunc(&mut self, f: HelpFn) {
        self.help_fn = Some(f);
    }
    // 桩无父链。
    pub fn Root(&mut self) -> &mut Command {
        self
    }
    // 默认 no-op。
    pub fn HelpFunc(&self) -> HelpFn {
        self.help_fn.clone().unwrap_or_else(|| {
            Arc::new(|_c: &mut Command, _a: &[String]| {
                // default no-op help
            })
        })
    }
    // 写 out+换行。
    pub fn Println(&self, s: impl AsRef<str>) {
        let mut out = self.out.lock().unwrap();
        out.extend_from_slice(s.as_ref().as_bytes());
        out.push(b'\n');
    }
    // 写 out。
    pub fn Printf(&self, s: impl AsRef<str>) {
        self.out
            .lock()
            .unwrap()
            .extend_from_slice(s.as_ref().as_bytes());
    }
    // 写 err_out。
    pub fn PrintErr(&self, s: impl AsRef<str>) {
        self.err_out
            .lock()
            .unwrap()
            .extend_from_slice(s.as_ref().as_bytes());
    }
    // 读 stdout。
    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }
    // 读 stderr。
    pub fn err_text(&self) -> String {
        String::from_utf8_lossy(&self.err_out.lock().unwrap()).into_owned()
    }
    // Use 首段或 Aliases。
    pub fn find_child(&mut self, name: &str) -> Option<&mut Command> {
        self.children.iter_mut().find(|c| {
            c.Use.split_whitespace().next() == Some(name) || c.Aliases.iter().any(|a| a == name)
        })
    }
    // 启动递归执行。
    pub fn Execute(&mut self) -> Result<()> {
        execute_command(self, &self.args.clone())
    }
}

/// 递归：PreRun→下钻子命令并继承 persistent→RunE。
fn execute_command(cmd: &mut Command, args: &[String]) -> Result<()> {
    // 每层先 PreRun。
    if let Some(pre) = cmd.PersistentPreRunE.clone() {
        pre(cmd, args)?;
    }
    // 无参数则跑 RunE 或成功。
    if args.is_empty() {
        if let Some(run) = cmd.RunE.clone() {
            return run(cmd, args);
        }
        return Ok(());
    }
    let name = args[0].clone();
    let rest: Vec<String> = args[1..].to_vec();
    // 准备继承。
    let parent_persistent = cmd.persistent_flags.clone_values();
    // Also carry defined defaults for known names.
    // 补齐未 Visit 的默认 flag。
    let mut extras = parent_persistent;
    for kn in KNOWN_FLAG_NAMES {
        if let Some(v) = cmd.persistent_flags.Lookup(kn) {
            if !extras.iter().any(|(k, _)| k == kn) {
                extras.push((kn.to_string(), v.clone()));
            }
        }
    }
    // 命中则递归。
    if let Some(child) = cmd.find_child(&name) {
        for (k, v) in extras {
            child.persistent_flags.set_raw(&k, v);
        }
        return execute_command(child, &rest);
    }
    if let Some(run) = cmd.RunE.clone() {
        return run(cmd, args);
    }
    // 未知命令。
    Err(Error::new(format!("unknown command {name}")))
}

/// Extension helpers on TaskFlagSet for cmd-local cloning into children.
/// 子命令继承用；set_raw 先 Define 再 Set。
pub trait FlagSetExt {
    fn clone_values(&self) -> Vec<(String, astersql_br_pkg_task::stubs::FlagValue)>;
    fn set_raw(&mut self, name: &str, value: astersql_br_pkg_task::stubs::FlagValue);
}

impl FlagSetExt for TaskFlagSet {
    // Visit + 常见名 Lookup。
    fn clone_values(&self) -> Vec<(String, astersql_br_pkg_task::stubs::FlagValue)> {
        let mut out = Vec::new();
        self.Visit(|k, v| out.push((k.to_string(), v.clone())));
        // Visit only changed flags; also copy defined defaults for common names.
        // 默认值补齐策略。
        for name in [
            "log-level",
            "log-file",
            "log-format",
            "redact-log",
            "redact-info-log",
            "status-addr",
            "slow-log-file",
            "storage",
            "pd",
            "ca",
            "cert",
            "key",
            "enable-opentracing",
        ] {
            if let Some(v) = self.Lookup(name) {
                if !out.iter().any(|(k, _)| k == name) {
                    out.push((name.to_string(), v.clone()));
                }
            }
        }
        out
    }
    // 按类型 Define 后 Set。
    fn set_raw(&mut self, name: &str, value: astersql_br_pkg_task::stubs::FlagValue) {
        use astersql_br_pkg_task::stubs::FlagValue;
        match &value {
            FlagValue::String(_) => self.DefineString(name, ""),
            FlagValue::Bool(_) => self.DefineBool(name, false),
            FlagValue::StringSlice(_) | FlagValue::StringArray(_) => {
                self.DefineStringSlice(name, vec![])
            }
            FlagValue::Uint(_) => self.DefineUint64(name, 0),
            FlagValue::Uint32(_) => self.DefineUint32(name, 0),
            FlagValue::Int(_) => self.DefineInt64(name, 0),
            FlagValue::Int32(_) => self.DefineInt32(name, 0),
            FlagValue::Duration(_) => self.DefineDuration(name, Duration::from_secs(0)),
        }
        self.Set(name, value);
    }
}

/// 兼容命名空间；含 NoArgs。
pub mod cobra {
    pub use super::Command;
    // 禁止位置参数。
    pub fn NoArgs(_cmd: &Command, args: &[String]) -> Result<(), super::Error> {
        if args.is_empty() {
            Ok(())
        } else {
            Err(super::Error::new("accepts no args"))
        }
    }
}

// --- logging ---
// 日志桩：Error 入缓冲，Warn/Info 丢弃。
/// zap/log 外观。
pub mod log {
    use super::Error;
    use std::sync::{Mutex, OnceLock};
    static LAST_ERROR: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

    /// Level/Format/File。
    pub struct Config {
        pub Level: String,
        pub Format: String,
        pub File: FileConfig,
    }
    impl Default for Config {
        fn default() -> Self {
            Self {
                Level: "info".into(),
                Format: "text".into(),
                File: FileConfig::default(),
            }
        }
    }
    #[derive(Default)]
    /// 日志文件名。
    pub struct FileConfig {
        pub Filename: String,
    }
    /// 占位 Logger。
    pub struct Logger;
    // 不打开文件。
    pub fn InitLogger(_conf: &Config) -> Result<(Logger, ()), Error> {
        Ok((Logger, ()))
    }
    // 全局替换占位。
    pub fn ReplaceGlobals(_lg: Logger, _p: ()) {}
    // 记入 LAST_ERROR。
    pub fn Error(msg: &str, fields: &[(&str, String)]) {
        let mut line = msg.to_string();
        for (k, v) in fields {
            line.push_str(&format!(" {k}={v}"));
        }
        LAST_ERROR
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .push(line);
    }
    // 丢弃。
    pub fn Warn(msg: &str, _fields: &[(&str, String)]) {
        let _ = msg;
    }
    // 丢弃。
    pub fn Info(msg: &str, _fields: &[(&str, String)]) {
        let _ = msg;
    }
    // 排空错误行。
    pub fn take_errors() -> Vec<String> {
        LAST_ERROR
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .drain(..)
            .collect()
    }
}

/// 字段构造器替身。
pub mod zap {
    // key=error。
    pub fn Error(err: &super::Error) -> (&'static str, String) {
        ("error", err.msg.clone())
    }
    // 数值字段。
    pub fn Uint64(key: &'static str, v: u64) -> (&'static str, String) {
        (key, v.to_string())
    }
}

// --- build ---
// 版本信息桩；LogInfo 可 take。
/// BR 应用名与 Info/LogInfo。
pub mod build {
    use std::sync::Mutex;
    static LAST: Mutex<Vec<String>> = Mutex::new(Vec::new());
    // 显示名。
    pub const BR: &str = "Backup & Restore (BR)";
    // 固定 br-dev。
    pub fn Info() -> String {
        "br-dev".into()
    }
    // 记录 loginfo。
    pub fn LogInfo(app: &str) {
        LAST.lock().unwrap().push(format!("loginfo:{app}"));
    }
    // 排空记录。
    pub fn take_logged() -> Vec<String> {
        std::mem::take(&mut *LAST.lock().unwrap())
    }
}

// --- summary ---
// 摘要单位桩，不聚合真实指标。
/// collector 开关与 unit。
pub mod summary {
    use std::sync::atomic::{AtomicBool, Ordering};
    static COLLECTOR: AtomicBool = AtomicBool::new(false);
    static UNIT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    // 备份单位。
    pub const BackupUnit: &str = "backup";
    // 恢复单位。
    pub const RestoreUnit: &str = "restore";
    // 开关。
    pub fn InitCollector(enabled: bool) {
        COLLECTOR.store(enabled, Ordering::SeqCst);
    }
    // 设单位。
    pub fn SetUnit(unit: &str) {
        *UNIT.lock().unwrap() = unit.to_string();
    }
    // 读单位。
    pub fn unit() -> String {
        UNIT.lock().unwrap().clone()
    }
    // 是否启用。
    pub fn collector_enabled() -> bool {
        COLLECTOR.load(Ordering::SeqCst)
    }
}

// --- session / config / gctuner / redact / memory / metrics ---
// TiDB 全局配置与资源钩子替身。
/// 统计禁用开关。
pub mod session {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DISABLED: AtomicBool = AtomicBool::new(false);
    // CLI PreRun 调用。
    pub fn DisableStats4Test() {
        DISABLED.store(true, Ordering::SeqCst);
    }
    // 查询状态。
    pub fn stats_disabled() -> bool {
        DISABLED.load(Ordering::SeqCst)
    }
}

/// 全局 Config：SkipGrantTable/AdvertiseAddress/CoprCache。
pub mod config {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    // 不可承接 cop 的地址。
    pub const UnavailableIP: &str = "0.0.0.0";
    // 超大事务上限。
    pub const SuperLargeTxnSize: u64 = 1 << 40;
    #[derive(Default)]
    /// 实例开关。
    pub struct InstanceConfig {
        pub TiDBEnableDDL: AtomicBool,
        pub EnableSlowLog: AtomicBool,
    }
    #[derive(Default)]
    /// SkipGrantTable。
    pub struct SecurityConfig {
        pub SkipGrantTable: bool,
    }
    #[derive(Default)]
    /// 缓存容量 MB。
    pub struct CoprCache {
        pub CapacityMB: f64,
    }
    #[derive(Default)]
    /// TiKV 客户端子集。
    pub struct TiKVClient {
        pub CoprCache: CoprCache,
    }
    #[derive(Default)]
    pub struct Config {
        pub Instance: InstanceConfig,
        pub Security: SecurityConfig,
        pub AdvertiseAddress: String,
        pub TiKVClient: TiKVClient,
    }
    static GLOBAL: OnceLock<Mutex<Config>> = OnceLock::new();
    // OnceLock 全局。
    fn global() -> &'static Mutex<Config> {
        GLOBAL.get_or_init(|| Mutex::new(Config::default()))
    }
    // 轻量代理。
    pub fn GetGlobalConfig() -> GlobalRef {
        GlobalRef
    }
    /// GetGlobalConfig 句柄。
    pub struct GlobalRef;
    impl GlobalRef {
        // 实例代理。
        pub fn Instance(&self) -> InstanceProxy {
            InstanceProxy
        }
        // restore 跳过 grant。
        pub fn Security_set_SkipGrantTable(&self, v: bool) {
            global().lock().unwrap().Security.SkipGrantTable = v;
        }
        // 读 SkipGrantTable。
        pub fn skip_grant_table(&self) -> bool {
            global().lock().unwrap().Security.SkipGrantTable
        }
        // 读广播地址。
        pub fn AdvertiseAddress(&self) -> String {
            global().lock().unwrap().AdvertiseAddress.clone()
        }
        // 读缓存。
        pub fn CoprCacheCapacityMB(&self) -> f64 {
            global().lock().unwrap().TiKVClient.CoprCache.CapacityMB
        }
    }
    /// 原子字段代理。
    pub struct InstanceProxy;
    impl InstanceProxy {
        // 写 DDL 开关。
        pub fn TiDBEnableDDL_Store(&self, v: bool) {
            global()
                .lock()
                .unwrap()
                .Instance
                .TiDBEnableDDL
                .store(v, Ordering::SeqCst);
        }
        // 写慢日志。
        pub fn EnableSlowLog_Store(&self, v: bool) {
            global()
                .lock()
                .unwrap()
                .Instance
                .EnableSlowLog
                .store(v, Ordering::SeqCst);
        }
        // 读 DDL。
        pub fn TiDBEnableDDL(&self) -> bool {
            global()
                .lock()
                .unwrap()
                .Instance
                .TiDBEnableDDL
                .load(Ordering::SeqCst)
        }
    }
    // 变更全局。
    pub fn UpdateGlobal(f: impl FnOnce(&mut Config)) {
        f(&mut global().lock().unwrap());
    }
    // 测试复位。
    pub fn reset_for_test() {
        *global().lock().unwrap() = Config::default();
    }
}

/// 内存 tuner 桩；Disable/Enable 翻标志。
pub mod gctuner {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DISABLED: AtomicBool = AtomicBool::new(false);
    /// tuner 类型。
    pub struct MemoryLimitTuner;
    // 进程单例。
    pub static GlobalMemoryLimitTuner: MemoryLimitTuner = MemoryLimitTuner;
    impl MemoryLimitTuner {
        // 禁用调限。
        pub fn DisableAdjustMemoryLimit(&self) {
            DISABLED.store(true, Ordering::SeqCst);
        }
        // 启用调限。
        pub fn EnableAdjustMemoryLimit(&self) {
            DISABLED.store(false, Ordering::SeqCst);
        }
        // 查询。
        pub fn is_disabled(&self) -> bool {
            DISABLED.load(Ordering::SeqCst)
        }
    }
}

/// 脱敏开关。
pub mod redact {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ON: AtomicBool = AtomicBool::new(false);
    // 设置。
    pub fn InitRedact(v: bool) {
        ON.store(v, Ordering::SeqCst);
    }
    // 查询。
    pub fn enabled() -> bool {
        ON.load(Ordering::SeqCst)
    }
}

/// 内存查询桩；可注入。
pub mod memory {
    use super::Result;
    use std::sync::Mutex;
    static HOOK: Mutex<Option<(u64, u64)>> = Mutex::new(None);
    // 空成功。
    pub fn InitMemoryHook() -> Result<()> {
        Ok(())
    }
    // 默认 8GiB。
    pub fn MemTotal() -> Result<u64> {
        Ok(HOOK.lock().unwrap().map(|h| h.0).unwrap_or(8 * super::GB))
    }
    // 默认 1GiB。
    pub fn MemUsed() -> Result<u64> {
        Ok(HOOK.lock().unwrap().map(|h| h.1).unwrap_or(super::GB))
    }
    // 测试注入。
    pub fn set_mem_for_test(total: u64, used: u64) {
        *HOOK.lock().unwrap() = Some((total, used));
    }
}

/// BR 指标注册空实现。
pub mod metricsutil {
    use super::Result;
    /// 忽略参数，恒 Ok。
    pub fn RegisterMetricsForBR(
        _pd: &[String],
        _tls: &astersql_br_pkg_task::common::TLSConfig,
        _keyspace: &str,
    ) -> Result<()> {
        Ok(())
    }
}

/// 事务大小限制原子量。
pub mod kv {
    use std::sync::atomic::{AtomicU64, Ordering};
    // CLI 可改写。
    pub static TxnTotalSizeLimit: AtomicU64 = AtomicU64::new(0);
}

/// 日志工具桩。
pub mod logutil {
    use super::{Error, Result};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ENV_LOGS: AtomicUsize = AtomicUsize::new(0);
    #[derive(Default)]
    /// 慢查询与 File。
    pub struct LogConfig {
        pub SlowQueryFile: String,
        pub File: FileLog,
    }
    #[derive(Default)]
    /// 文件名。
    pub struct FileLog {
        pub Filename: String,
    }
    // 空成功。
    pub fn InitLogger(_cfg: &LogConfig) -> Result<()> {
        Ok(())
    }
    // 计数+1。
    pub fn LogEnvVariables() {
        ENV_LOGS.fetch_add(1, Ordering::SeqCst);
    }
    // 读计数。
    pub fn env_log_count() -> usize {
        ENV_LOGS.load(Ordering::SeqCst)
    }
}

// --- utils helpers used by cmd ---
// BR utils 子集。
/// cmd 工具函数。
pub mod utils {
    use super::{Context, Error, Result, ServeMux, TLS};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    // 临时库名约定。
    pub fn TemporaryDBName(db: &str) -> String {
        format!("__TiDB_BR_Temporary_{db}")
    }
    // 去反引号。
    pub fn UnquoteName(name: &str) -> String {
        name.trim_matches('`').to_string()
    }
    // WithCancel 替身。
    pub fn StartExitSingleListener(gCtx: Context) -> (Context, Box<dyn Fn() + Send>) {
        Context::WithCancel(&gCtx)
    }
    static MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
    static MONITOR_ERR: Mutex<Option<String>> = Mutex::new(None);
    // 可注入失败。
    pub fn RunMemoryMonitor(_ctx: &Context, _dumpDir: &str, _memlimit: u64) -> Result<()> {
        if let Some(e) = MONITOR_ERR.lock().unwrap().clone() {
            return Err(Error::new(e));
        }
        MONITOR_STARTED.store(true, Ordering::SeqCst);
        Ok(())
    }
    // 是否启动。
    pub fn monitor_started() -> bool {
        MONITOR_STARTED.load(Ordering::SeqCst)
    }
    // 注入错误。
    pub fn set_monitor_err(e: Option<&str>) {
        *MONITOR_ERR.lock().unwrap() = e.map(|s| s.to_string());
        MONITOR_STARTED.store(false, Ordering::SeqCst);
    }
    // 路由占位。
    pub fn RegisterDefaultStatusHandlers(_mux: &mut ServeMux) {}
    // 不监听。
    pub fn StartStatusListenerWithHandler(_addr: &str, _tls: &TLS, _mux: ServeMux) -> Result<()> {
        Ok(())
    }
    // pprof 占位。
    pub fn StartDynamicPProfListener(_tls: &TLS) {}
    // JSON 序列化。
    pub fn MarshalBackupMeta(meta: &super::backuppb::BackupMeta) -> Result<Vec<u8>> {
        serde_json::to_vec(meta).map_err(|e| Error::new(e.to_string()))
    }
    // JSON 反序列化。
    pub fn UnmarshalBackupMeta(data: &[u8]) -> Result<super::backuppb::BackupMeta> {
        serde_json::from_slice(data).map_err(|e| Error::new(e.to_string()))
    }
}

#[derive(Default, Clone)]
/// TLS 路径替身。
pub struct TLS {
    pub ca: String,
    pub cert: String,
    pub key: String,
}

/// 构造 TLS 桩。
pub fn NewTLS(ca: &str, cert: &str, key: &str, _host: &str) -> Result<TLS> {
    Ok(TLS {
        ca: ca.to_string(),
        cert: cert.to_string(),
        key: key.to_string(),
    })
}

#[derive(Default)]
/// HTTP mux 替身。
pub struct ServeMux {
    pub handlers: Vec<String>,
}
impl ServeMux {
    // 记录 path。
    pub fn Handle(&mut self, path: &str) {
        self.handlers.push(path.to_string());
    }
}

// --- gluetidb / gluetikv ---
// Glue 边界；OpMemGlue 故意 unavailable。
/// 库名过滤器。
pub type DBFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
/// InfoSchema 过滤包装。
pub struct InfoSchemaFilter {
    pub filter: DBFilter,
}

// 默认放行。
impl Default for InfoSchemaFilter {
    fn default() -> Self {
        Self {
            filter: Arc::new(|_db: &str| true),
        }
    }
}

/// 自定义过滤。
pub fn NewInfoSchemaFilter(f: impl Fn(&str) -> bool + Send + Sync + 'static) -> InfoSchemaFilter {
    InfoSchemaFilter {
        filter: Arc::new(f),
    }
}

/// 系统库/临时库判定。
pub fn FilterLoadSysDBs(db: &str) -> bool {
    matches!(
        db.to_lowercase().as_str(),
        "mysql"
            | "sys"
            | "information_schema"
            | "performance_schema"
            | "metrics_schema"
            | "inspection_schema"
    ) || db.starts_with("__TiDB_BR_Temporary_")
}

/// 系统库+指定库。
pub fn FilterLoadSpecifiedDBAndSysDBs(extra: Vec<String>) -> DBFilter {
    Arc::new(move |db: &str| {
        if FilterLoadSysDBs(db) {
            return true;
        }
        extra.iter().any(|e| e.eq_ignore_ascii_case(db))
    })
}

/// TiDB glue 聚合。
pub struct TidbGlue {
    pub InfoSchemaFilter: InfoSchemaFilter,
    pub task: MemGlue,
    pub op: OpMemGlue,
}

// 委托 New。
impl Default for TidbGlue {
    fn default() -> Self {
        Self::New()
    }
}

impl TidbGlue {
    // version=br-glue。
    pub fn New() -> Self {
        Self {
            InfoSchemaFilter: InfoSchemaFilter::default(),
            task: MemGlue {
                version: "br-glue".into(),
                ..Default::default()
            },
            op: OpMemGlue::default(),
        }
    }
    // task Glue。
    pub fn as_task(&self) -> &dyn TaskGlue {
        &self.task
    }
    // operator Glue。
    pub fn as_op(&self) -> &dyn OpGlue {
        &self.op
    }
}

#[derive(Default)]
/// Domain/Session 固定失败。
pub struct OpMemGlue;
impl OpGlue for OpMemGlue {
    // domain unavailable。
    fn GetDomain(
        &self,
        _store: &dyn KVStorage,
    ) -> OpResult<Arc<dyn astersql_br_pkg_task_operator::stubs::Domain>> {
        Err(astersql_br_pkg_task_operator::stubs::Error::new(
            "domain unavailable in cmd stub",
        ))
    }
    // session unavailable。
    fn CreateSession(
        &self,
        _store: &dyn KVStorage,
    ) -> OpResult<Arc<dyn astersql_br_pkg_task_operator::stubs::Session>> {
        Err(astersql_br_pkg_task_operator::stubs::Error::new(
            "session unavailable in cmd stub",
        ))
    }
}

/// TiKV glue：raw/txn 路径。
pub struct TikvGlue;
// 零大小。
impl Default for TikvGlue {
    fn default() -> Self {
        Self
    }
}
// 进度/Console 空实现。
impl TaskGlue for TikvGlue {
    // tikv-glue。
    fn GetVersion(&self) -> String {
        "tikv-glue".into()
    }
    // MemProgress。
    fn StartProgress(
        &self,
        _cmd: &str,
        total: i64,
        _log_progress: bool,
    ) -> Arc<dyn astersql_br_pkg_task::stubs::Progress> {
        let _ = total;
        Arc::new(astersql_br_pkg_task::stubs::MemProgress::default())
    }
    // 丢弃。
    fn Record(&self, _key: &str, _value: u64) {}
    // 空成功。
    fn ConsoleOutWrite(&self, _msg: &[u8]) -> astersql_br_pkg_task::stubs::Result<()> {
        Ok(())
    }
}

// --- debug helpers: backuppb / metautil / rtree / mockid / restoreutils / stream ---
// debug/测试辅助；非生产 backupmeta 管线。
/// 精简 protobuf 形状。
pub mod backuppb {
    use serde::{Deserialize, Serialize};
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 数据文件元信息。
    pub struct File {
        pub Name: String,
        pub Sha256: Vec<u8>,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Crc64Xor: u64,
        pub TotalKvs: u64,
        pub TotalBytes: u64,
        pub StartVersion: u64,
        pub EndVersion: u64,
    }
    impl File {
        // 文件名。
        pub fn GetName(&self) -> &str {
            &self.Name
        }
        // KV 数。
        pub fn GetTotalKvs(&self) -> u64 {
            self.TotalKvs
        }
        // 字节。
        pub fn GetTotalBytes(&self) -> u64 {
            self.TotalBytes
        }
        // CRC。
        pub fn GetCrc64Xor(&self) -> u64 {
            self.Crc64Xor
        }
        // 起点。
        pub fn GetStartKey(&self) -> &[u8] {
            &self.StartKey
        }
        // 终点。
        pub fn GetEndKey(&self) -> &[u8] {
            &self.EndKey
        }
        // 版本下界。
        pub fn GetStartVersion(&self) -> u64 {
            self.StartVersion
        }
        // 版本上界。
        pub fn GetEndVersion(&self) -> u64 {
            self.EndVersion
        }
    }
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 备份元数据桩。
    pub struct BackupMeta {
        pub StartVersion: u64,
        pub EndVersion: u64,
        pub Version: i32,
        pub FileIndex: Option<MetaFile>,
        pub RawRangeIndex: Option<MetaFile>,
        pub SchemaIndex: Option<MetaFile>,
        pub Schemas: Vec<Schema>,
    }
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 元文件索引。
    pub struct MetaFile {
        pub name: String,
    }
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// Schema 占位。
    pub struct Schema {
        pub name: String,
    }
    #[derive(Clone, Debug, Default)]
    /// 前缀重写。
    pub struct RewriteRule {
        pub OldKeyPrefix: Vec<u8>,
        pub NewKeyPrefix: Vec<u8>,
    }
}

/// metautil 桩；tables.json 约定。
pub mod metautil {
    use super::backuppb::{BackupMeta, File, MetaFile as Idx};
    use super::{Error, Result};
    use astersql_br_pkg_task::stubs::Storage;
    use std::collections::HashMap;
    use std::sync::Arc;

    // 元文件名。
    pub const MetaFile: &str = "backupmeta";
    // JSON 元文件。
    pub const MetaJSONFile: &str = "backupmeta.json";
    // 版本 2。
    pub const MetaV2: i32 = 2;

    #[derive(Clone, Debug, Default)]
    /// 大小写信息串。
    pub struct CIStr {
        pub O: String,
        pub L: String,
    }
    impl CIStr {
        // O/L。
        pub fn new(s: &str) -> Self {
            Self {
                O: s.to_string(),
                L: s.to_lowercase(),
            }
        }
    }
    impl std::fmt::Display for CIStr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.O)
        }
    }

    #[derive(Clone, Debug, Default)]
    /// 索引信息。
    pub struct IndexInfo {
        pub ID: i64,
        pub Name: CIStr,
    }
    #[derive(Clone, Debug, Default)]
    /// 分区定义。
    pub struct PartitionDefinition {
        pub ID: i64,
        pub Name: CIStr,
    }
    #[derive(Clone, Debug, Default)]
    /// 分区容器。
    pub struct PartitionInfo {
        pub Definitions: Vec<PartitionDefinition>,
    }
    #[derive(Clone, Debug, Default)]
    /// 表信息。
    pub struct TableInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Indices: Vec<IndexInfo>,
        pub Partition: Option<PartitionInfo>,
    }
    #[derive(Clone, Debug, Default)]
    /// 库信息。
    pub struct DBInfo {
        pub Name: CIStr,
    }
    #[derive(Clone, Debug, Default)]
    /// 备份表视图。
    pub struct Table {
        pub Info: Option<TableInfo>,
        pub FilesOfPhysicals: HashMap<i64, Vec<File>>,
        pub TotalKvs: u64,
        pub TotalBytes: u64,
        pub Crc64Xor: u64,
    }
    #[derive(Clone, Debug, Default)]
    /// 库+表。
    pub struct Database {
        pub Info: DBInfo,
        pub Tables: Vec<Table>,
    }

    /// Meta+Storage。
    pub struct MetaReader {
        pub meta: BackupMeta,
        pub storage: Arc<dyn Storage>,
    }
    /// 构造；cipher 未用。
    pub fn NewMetaReader(
        meta: BackupMeta,
        s: Arc<dyn Storage>,
        _cipher: &astersql_br_pkg_task::stubs::backuppb::CipherInfo,
    ) -> MetaReader {
        MetaReader { meta, storage: s }
    }
    /// 空 map 桩。
    pub fn LoadBackupTables(
        _reader: &MetaReader,
        _load_stats: bool,
    ) -> Result<HashMap<String, Database>> {
        // Decode JSON sidecar written by tests / debug encode path.
        Ok(HashMap::new())
    }
    /// 读 tables.json sidecar。
    pub fn LoadBackupTablesFromStorage(s: &dyn Storage) -> Result<HashMap<String, Database>> {
        if let Ok(data) = s.ReadFile("tables.json") {
            let v: serde_json::Value =
                serde_json::from_slice(&data).map_err(|e| Error::new(e.to_string()))?;
            let mut dbs = HashMap::new();
            if let Some(arr) = v.as_array() {
                for dbv in arr {
                    let db_name = dbv["name"].as_str().unwrap_or("db").to_string();
                    let mut tables = Vec::new();
                    if let Some(tarr) = dbv["tables"].as_array() {
                        for tv in tarr {
                            let mut files = HashMap::new();
                            let mut file_list = Vec::new();
                            if let Some(farr) = tv["files"].as_array() {
                                for fv in farr {
                                    file_list.push(File {
                                        Name: fv["name"].as_str().unwrap_or("").to_string(),
                                        Sha256: hex::decode(fv["sha256"].as_str().unwrap_or(""))
                                            .unwrap_or_default(),
                                        StartKey: fv["start"]
                                            .as_str()
                                            .unwrap_or("")
                                            .as_bytes()
                                            .to_vec(),
                                        EndKey: fv["end"]
                                            .as_str()
                                            .unwrap_or("")
                                            .as_bytes()
                                            .to_vec(),
                                        Crc64Xor: fv["crc"].as_u64().unwrap_or(0),
                                        TotalKvs: fv["kvs"].as_u64().unwrap_or(0),
                                        TotalBytes: fv["bytes"].as_u64().unwrap_or(0),
                                        ..Default::default()
                                    });
                                }
                            }
                            files.insert(1, file_list);
                            let tname = tv["name"].as_str().unwrap_or("t").to_string();
                            let tid = tv["id"].as_i64().unwrap_or(1);
                            tables.push(Table {
                                Info: Some(TableInfo {
                                    ID: tid,
                                    Name: CIStr::new(&tname),
                                    Indices: tv["indices"]
                                        .as_array()
                                        .map(|ia| {
                                            ia.iter()
                                                .map(|iv| IndexInfo {
                                                    ID: iv["id"].as_i64().unwrap_or(1),
                                                    Name: CIStr::new(
                                                        iv["name"].as_str().unwrap_or("idx"),
                                                    ),
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    Partition: None,
                                }),
                                FilesOfPhysicals: files,
                                TotalKvs: tv["total_kvs"].as_u64().unwrap_or(0),
                                TotalBytes: tv["total_bytes"].as_u64().unwrap_or(0),
                                Crc64Xor: tv["crc"].as_u64().unwrap_or(0),
                            });
                        }
                    }
                    dbs.insert(
                        db_name.clone(),
                        Database {
                            Info: DBInfo {
                                Name: CIStr::new(&db_name),
                            },
                            Tables: tables,
                        },
                    );
                }
            }
            return Ok(dbs);
        }
        Ok(HashMap::new())
    }
    /// 解码占位。
    pub fn DecodeMetaFile(
        _s: &dyn Storage,
        _cipher: &astersql_br_pkg_task::stubs::backuppb::CipherInfo,
        _idx: &Option<Idx>,
    ) -> Result<()> {
        Ok(())
    }
    /// 统计解码占位。
    pub fn DecodeStatsFile(
        _s: &dyn Storage,
        _cipher: &astersql_br_pkg_task::stubs::backuppb::CipherInfo,
        _schemas: &[super::backuppb::Schema],
    ) -> Result<()> {
        Ok(())
    }
    /// 明文+零 IV。
    pub fn Encrypt(
        data: &[u8],
        cipher: &astersql_br_pkg_task::stubs::backuppb::CipherInfo,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let _ = cipher;
        // plaintext path: empty IV + raw content
        // 非真正加密。
        Ok((data.to_vec(), vec![0u8; 16]))
    }
}

/// 简单 RangeTree，重叠检测。
pub mod rtree {
    use super::backuppb::File;
    #[derive(Clone, Debug)]
    /// [Start, End)。
    pub struct KeyRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }
    #[derive(Clone, Debug)]
    /// KeyRange 包装。
    pub struct Range {
        pub KeyRange: KeyRange,
    }
    impl std::fmt::Display for Range {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "[{:?}, {:?})",
                self.KeyRange.StartKey, self.KeyRange.EndKey
            )
        }
    }
    #[derive(Default)]
    /// 线性 ranges。
    pub struct RangeTree {
        ranges: Vec<Range>,
    }
    impl RangeTree {
        // 空树。
        pub fn new() -> Self {
            Self::default()
        }
        // 重叠返回已有。
        pub fn InsertRange(&mut self, r: Range) -> Option<Range> {
            for existing in &self.ranges {
                if ranges_overlap(&existing.KeyRange, &r.KeyRange) {
                    return Some(existing.clone());
                }
            }
            self.ranges.push(r);
            None
        }
    }
    // 半开重叠。
    fn ranges_overlap(a: &KeyRange, b: &KeyRange) -> bool {
        a.StartKey < b.EndKey && b.StartKey < a.EndKey
    }
    // File→Range。
    pub fn file_range(file: &File) -> Range {
        Range {
            KeyRange: KeyRange {
                StartKey: file.StartKey.clone(),
                EndKey: file.EndKey.clone(),
            },
        }
    }
}

/// 单调 ID 分配器。
pub mod mockid {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub struct IDAllocator {
        next: AtomicU64,
    }
    impl IDAllocator {
        pub fn new() -> Self {
            Self {
                next: AtomicU64::new(1),
            }
        }
        // fetch_add。
        pub fn Alloc(&self) -> (u64, Result<(), ()>) {
            Ok_alloc(self.next.fetch_add(1, Ordering::SeqCst))
        }
    }
    // 恒 Ok。
    fn Ok_alloc(id: u64) -> (u64, Result<(), ()>) {
        (id, Ok(()))
    }
    // 从 1 起。
    pub fn NewIDAllocator() -> IDAllocator {
        IDAllocator::new()
    }
}

/// 重写规则工具桩。
pub mod restoreutils {
    use super::backuppb::{File, RewriteRule};
    use super::metautil::TableInfo;
    use super::{Error, Result};

    #[derive(Default)]
    /// 数据面规则。
    pub struct RewriteRules {
        pub Data: Vec<RewriteRule>,
    }
    /// 按表 ID 生成前缀。
    pub fn GetRewriteRules(
        new_table: &TableInfo,
        old_table: &TableInfo,
        _ts: u64,
        _with_indices: bool,
    ) -> RewriteRules {
        RewriteRules {
            Data: vec![RewriteRule {
                OldKeyPrefix: old_table.ID.to_le_bytes().to_vec(),
                NewKeyPrefix: new_table.ID.to_le_bytes().to_vec(),
            }],
        }
    }
    /// 粗校验。
    pub fn ValidateFileRewriteRule(file: &File, rules: &RewriteRules) -> Result<()> {
        if rules.Data.is_empty() && !file.StartKey.is_empty() {
            return Err(Error::new("no rewrite rules for file"));
        }
        let _ = file;
        Ok(())
    }
}

/// 日志按 key 搜索桩。
pub mod stream_search {
    use super::{Error, Result};
    use astersql_br_pkg_task::stubs::Storage;
    use serde::Serialize;
    use std::sync::Arc;

    /// 比较器占位。
    pub struct StartWithComparator;
    pub fn NewStartWithComparator() -> StartWithComparator {
        StartWithComparator
    }
    #[derive(Clone, Serialize)]
    /// 结果条目。
    pub struct KV {
        pub key: String,
        pub value: String,
    }
    /// storage/key/ts。
    pub struct StreamBackupSearch {
        storage: Arc<dyn Storage>,
        key: Vec<u8>,
        start_ts: u64,
        end_ts: u64,
    }
    /// 构造搜索器。
    pub fn NewStreamBackupSearch(
        s: Arc<dyn Storage>,
        _cmp: StartWithComparator,
        key: Vec<u8>,
    ) -> StreamBackupSearch {
        StreamBackupSearch {
            storage: s,
            key,
            start_ts: 0,
            end_ts: 0,
        }
    }
    impl StreamBackupSearch {
        // 下界。
        pub fn SetStartTS(&mut self, ts: u64) {
            self.start_ts = ts;
        }
        // 上界。
        pub fn SetEndTs(&mut self, ts: u64) {
            self.end_ts = ts;
        }
        // key 空则失败。
        pub fn Search(&self) -> Result<Vec<KV>> {
            let _ = (&self.storage, self.start_ts, self.end_ts);
            if self.key.is_empty() {
                return Err(Error::new("key param can't be empty"));
            }
            Ok(vec![KV {
                key: hex::encode(&self.key),
                value: String::new(),
            }])
        }
    }
}

/// 版本检查常量透传。
pub mod conn {
    // task 常量。
    pub const NormalVersionChecker: astersql_br_pkg_task::stubs::VersionCheckerType =
        astersql_br_pkg_task::stubs::NormalVersionChecker;
}

/// SetMemoryLimit 替身。
pub mod debug_runtime {
    use std::sync::atomic::{AtomicI64, Ordering};
    /// Mirrors debug.SetMemoryLimit; returns previous limit.
    /// 交换旧 limit。
    static PREV: AtomicI64 = AtomicI64::new(i64::MAX);
    // 返回旧值。
    pub fn SetMemoryLimit(limit: i64) -> i64 {
        PREV.swap(limit, Ordering::SeqCst)
    }
}

/// 环境/退出替身；Exit 不终止进程。
pub mod os_stub {
    use std::sync::Mutex;
    static ENV: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    static EXIT_CODE: Mutex<Option<i32>> = Mutex::new(None);
    // temp_dir。
    pub fn TempDir() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }
    // 真环境优先。
    pub fn LookupEnv(key: &str) -> Option<String> {
        if let Ok(v) = std::env::var(key) {
            return Some(v);
        }
        ENV.lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
    // 注入/删除。
    pub fn set_env_for_test(key: &str, val: Option<&str>) {
        let mut e = ENV.lock().unwrap();
        e.retain(|(k, _)| k != key);
        if let Some(v) = val {
            e.push((key.to_string(), v.to_string()));
        }
    }
    // 空串回退。
    pub fn Getenv(key: &str) -> String {
        LookupEnv(key).unwrap_or_default()
    }
    // 进程参数。
    pub fn Args() -> Vec<String> {
        std::env::args().collect()
    }
    // 只记录码。
    pub fn Exit(code: i32) {
        *EXIT_CODE.lock().unwrap() = Some(code);
    }
    // 取走退出码。
    pub fn take_exit() -> Option<i32> {
        EXIT_CODE.lock().unwrap().take()
    }
    // stdout 占位。
    pub fn Stdout() {}
}

/// Combined flags view: persistent ∪ local for ParseFromFlags.
/// 合并 persistent∪local，并补齐 KNOWN_FLAG_NAMES。
pub fn effective_task_flags(cmd: &Command) -> TaskFlagSet {
    let mut fs = TaskFlagSet::new();
    for (k, v) in cmd.persistent_flags.clone_values() {
        fs.set_raw(&k, v);
    }
    for (k, v) in cmd.flags.clone_values() {
        fs.set_raw(&k, v);
    }
    // Also copy defined-but-unchanged flags via Lookup on known names from both sets.
    // 两侧 Lookup 补齐。
    for src in [&cmd.persistent_flags, &cmd.flags] {
        for name in KNOWN_FLAG_NAMES {
            if let Some(v) = src.Lookup(name) {
                if fs.Lookup(name).is_none() {
                    fs.set_raw(name, v.clone());
                }
            }
        }
    }
    fs
}

// BR CLI 常用 flag 名清单。
const KNOWN_FLAG_NAMES: &[&str] = &[
    "log-level",
    "log-file",
    "log-format",
    "redact-log",
    "redact-info-log",
    "status-addr",
    "slow-log-file",
    "send-credentials-to-tikv",
    "no-credentials",
    "storage",
    "pd",
    "ca",
    "cert",
    "key",
    "checksum-concurrency",
    "ratelimit",
    "ratelimit-unit",
    "concurrency",
    "checksum",
    "filter",
    "case-sensitive",
    "remove-tiflash",
    "check-requirements",
    "switch-mode-interval",
    "grpc-keepalive-time",
    "grpc-keepalive-timeout",
    "enable-opentracing",
    "skip-check-path",
    "dry-run",
    "db",
    "table",
    "type",
    "crypter.method",
    "crypter.key",
    "crypter.key-file",
    "log.crypter.method",
    "log.crypter.key",
    "log.crypter.key-file",
    "master-key",
    "master-key-crypter-method",
    "metadata-download-batch-size",
    "full-backup-storage",
    "pitr-add-index-sql-storage",
    "task-name",
    "start-ts",
    "end-ts",
    "safepoint-ttl",
    "until",
    "yes",
    "with-sys-table",
    "offset",
    "field",
    "search-key",
    "keyspace-name",
];

// silence unused Once import warning by using in init helper
// Once 辅助，兼消化 import。
/// 包装 call_once。
pub fn once_do(once: &Once, f: impl FnOnce()) {
    once.call_once(f);
}
