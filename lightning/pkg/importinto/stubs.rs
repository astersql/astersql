// Copyright 2026 AsterSQL.
//! Local stand-ins for SQL/config/common/log/importsdk/objstore boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//! 中文注释索引开始。
//! 本文件把 `importinto` 依赖的多组 Go 边界折叠进一个可移植的 Rust 桩文件，
//! 让 `checkpoint.rs`、`importer.rs`、`job_submitter.rs` 等逻辑在不引入
//! `kv`、`domain`、`grpcio`、`kvproto` 之类重量依赖的前提下仍能表达同名行为。
//! 这里的核心目标不是实现生产能力，而是把调用方真正观察到的契约固定下来，
//! 例如错误身份、取消语义、配置默认值、检查点持久化形状、日志记录格式和作业状态推进。
//! 因此阅读本文件时，应把每个子模块理解为“为 importinto 提供最小可验证语义的 stand-in”，
//! 而不是理解为对 Go 依赖包的完整移植。
//! 与 Go 对照时，可以把这些模块粗略映射为：
//! `errors` 对应 `pingcap/errors` 的常用形状；
//! `context` 对应 `context.Context` 及其取消辅助；
//! `config`、`common`、`log` 负责承接 importinto 直接访问的配置、通用工具和日志接口；
//! `sql`、`importsdk`、`objstore` 则分别替代最小数据库、后端 SDK 与本地对象存储边界。
//! 这些桩的共同约束是“只覆盖当前 Rust 端真实会用到的分支”。
//! 例如：
//! `sql::DB` 只模拟 checkpoint 相关 SQL 的副作用；
//! `importsdk::MockSDK` 只覆盖建表、生成 SQL、提交流程、按组查询和取消作业；
//! `objstore::LocalStorage` 只提供本地文件读写删除，不抽象远端存储一致性。
//! 这意味着若调用方未来新增了新的 SQL 模式、日志字段或 SDK 交互，
//! 首先要检查这里是否仍然表达了对应的可观察行为，而不是假设桩会自动具备生产实现的全部能力。
//! 本文件还刻意保留了若干 Go 风格命名，如 `Error()`、`Load()`、`RowsAffected()`，
//! 目的是降低 Rust 侧语义对照成本，让相邻 Go 文件和 Rust 文件之间更容易逐符号比对。
//! 命名不够 Rust 风格是已知取舍，但当前任务只补注释，不重塑 API。
//! 对维护者而言，最重要的风险点有三类：
//! 第一类是把“占位能力”误读成“正式支持”，例如 `DumpEngines`/`DumpChunks` 返回空成功；
//! 第二类是忽略这里为了测试可控性而做的弱化实现，例如 `AfterFunc` 用轮询线程近似回调；
//! 第三类是忘记错误身份与字符串内容本身就是契约，导致上层分支判断失真。
//! 所以本次中文注释会优先解释为什么某些实现看起来简化、它保护了什么语义、
//! 以及它与 Go 行为保持一致或刻意缩减的边界在哪里。
//! 只要调用方仍然只依赖这些被描述的契约，本文件就足以支撑 importinto 的 Rust 语义镜像。
//! 维护本文件时还应特别注意以下隐藏约束：
//! `errors::Annotate` 会重建最外层错误，因此调用方若只比较顶层消息，结果会改变；
//! `errors::Cause` 会递归走到底，所以底层类名和 not-found 标记不能被意外丢失。
//! `context::WithTimeout` 通过惰性检查 deadline 生效，
//! 因而测试若依赖“超时后立即触发副作用”，通常要配合显式查询状态或 `AfterFunc`。
//! `context::WithoutCancel` 同时切断取消链与 deadline，
//! 这与 importer 在清理或 grace timeout 场景下的预期一致。
//! `atomic::Int64` 之所以支持序列化，是因为配置对象会被整体复制或序列化到测试快照中。
//! `config::Config::NewConfig` 的默认值是上层许多测试的起点，
//! 改动默认并发或 checkpoint driver 会直接改变导入器初始化分支。
//! `common::ErrCheckpointTableNotFound` 的消息格式、类名和 not-found 身份三者缺一不可，
//! 因为 file/mysql 两种 checkpoint 管理器都可能借此向上层报告“目标表不存在”。
//! `log::Logger` 通过共享 `Arc<Mutex<Vec<String>>>` 保持克隆后仍写入同一缓冲区，
//! 这使带字段的子 logger 仍能被测试统一检查。
//! `sql::DB::ExecContext` 对 SQL 的识别是“模板敏感”的，
//! 所以上层若改写 SQL 文本结构，先坏掉的通常不是业务逻辑，而是这个桩的关键字分支。
//! `sql::Rows::ScanCheckpoint` 固定了列顺序，
//! 任何新增字段都要同步调整读写两端，而不能只改其中一侧。
//! `importsdk::MockSDK` 的队列能力保证了“第一次失败、第二次成功”这类时序测试可重放，
//! 若把它简化成单次固定返回，很多 orchestrator 测试会失去表达力。
//! `importsdk::NewImportSDK` 虽然总是返回 mock，
//! 但仍必须按顺序应用 option，避免 submitter 生成 SQL 时读不到预期配置。
//! `objstore::LocalStorage` 把不存在文件转换成结构化错误，
//! 这直接影响 file checkpoint 初始化时“没有历史文件应视为正常”的控制流。
//! `ast::RedactURL` 先拼接资源参数再脱敏，
//! 其意义在于日志输出必须与真实提交 SQL 足够接近，同时又不能泄露密钥。
//! `units::FromHumanSize` 接受 `KiB` 等写法但仍按 1000 进位，
//! 这是当前调用方约定的一部分，不能未经核对改成 1024。
//! `failpoint` 模块目前只暴露一个覆盖点，
//! 新增测试若需要更多注入位，应优先确认 Go 端是否真有对应 failpoint 名称和语义。
//! `io::Writer`、`os::IsNotExist`、`uuid_util::New` 看似很小，
//! 但它们让上层 API 形状保持接近 Go 版本，从而减少翻译层噪音。
//! 还要注意本文件顶部已有 `AsterSQL` 版权行，
//! 这属于仓库当前状态的一部分，本任务只补注释，不主动扩展或清理版权声明。
//! 总之，评价这里的实现是否正确时，应优先问“调用方能观察到的行为是否仍对齐”，
//! 而不是问“它是否像生产实现一样完整”。
//! 中文注释索引结束。

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Error {
    pub msg: String,
    pub not_found: bool,
    pub cause: Option<Box<Error>>,
    pub class: Option<&'static str>,
}

/// 这个 `Error` 结构只保留 importinto 目前真正依赖的错误要素。
/// `msg` 负责承接最外层展示文本，供日志、比较和错误传播使用。
/// `not_found` 用来模拟 Go 中“资源不存在”这类可分支判断的错误身份。
/// `cause` 允许像 `errors.Annotate` 那样形成包装链，但这里不保存栈信息。
/// `class` 则补位 Go 端基于错误类名做特判的场景，例如 checkpoint not found。
/// 这使得上层既可以按消息判断，也可以按类名判断，避免把语义退化成纯字符串匹配。
impl Error {
    /// 构造最小错误值，不附带栈和类名，适合普通桩路径直接返回。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            not_found: false,
            cause: None,
            class: None,
        }
    }

    /// 近似 Go `GenWithStackByArgs` 的“格式化新错误”行为。
    /// 这里保留原错误的身份位，但不复制 `cause`，因为当前调用方只关心最终文本与类名。
    pub fn GenWithStackByArgs(&self, args: impl fmt::Display) -> Error {
        Error {
            msg: format!("{}: {}", self.msg, args),
            not_found: self.not_found,
            cause: None,
            class: self.class,
        }
    }

    /// 递归展开最外层消息和因果链，保持与 Go `Error()` 类似的展示顺序。
    pub fn Error(&self) -> String {
        match &self.cause {
            Some(c) => format!("{}: {}", self.msg, c.Error()),
            None => self.msg.clone(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.Error())
    }
}

impl std::error::Error for Error {}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.Error() == other.Error()
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// 这一节提供与 `pingcap/errors` 常用入口相似的薄包装。
/// 目标不是复制完整错误库，而是让 importinto 的调用代码保留原先的控制流结构。
/// 因为上层会调用 `errors::Trace`、`Annotate`、`Cause`、`IsNotFound`，
/// 所以这里至少要保证这些函数在成功/失败路径上的返回形状稳定。
/// 未实现的部分主要是栈追踪、错误码系统和复杂的类型匹配。
/// 只要 importinto 继续按“类名 + not_found + 文本”这三个维度判断错误，
/// 当前缩减版就足够承接 Rust 端语义。
pub mod errors {
    use super::Error;
    use std::fmt;

    pub use super::{Error as ErrorType, Result};

    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    pub fn Errorf(msg: impl fmt::Display) -> Error {
        Error::new(msg.to_string())
    }

    pub fn Trace(err: Error) -> Error {
        err
    }

    /// 与 Go `perrors.IsNotFound` 对齐，但实现只覆盖 importinto 真实会触发的判定来源。
    /// 除了显式的 `not_found` 位，还兼容 checkpoint 类名和常见文本片段。
    /// 这样既能覆盖结构化错误，也能覆盖某些只能通过消息落地的桩路径。
    pub fn IsNotFound(err: &Error) -> bool {
        err.not_found
            || err.class == Some("Lightning:Checkpoint:ErrCheckpointTableNotFound")
            || err.Error().to_lowercase().contains("not found")
    }

    pub fn Annotate(err: Error, msg: impl Into<String>) -> Error {
        Error {
            msg: msg.into(),
            not_found: false,
            cause: Some(Box::new(err)),
            class: None,
        }
    }

    pub fn Annotatef(err: Error, msg: impl fmt::Display) -> Error {
        Annotate(err, msg.to_string())
    }

    /// 沿着包装链找到最底层错误，便于对齐 Go `errors.Cause` 的调用习惯。
    pub fn Cause(err: &Error) -> &Error {
        match &err.cause {
            Some(c) => Cause(c),
            None => err,
        }
    }

    /// 比较优先使用错误类名，其次退回到消息文本。
    /// 这是因为某些上层分支把类名视为稳定身份，而其他分支仍旧只持有字符串。
    pub fn ErrorEqual(a: &Error, b: &Error) -> bool {
        (a.class.is_some() && a.class == b.class) || a.msg == b.msg
    }
}

// ---------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------

pub mod context {
    use super::{Error, errors};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// 这个 `Context` 只实现 importinto 所需的取消、原因和 deadline 能力。
    /// 与 Go 真正的层级 `Context` 相比，这里没有值传递，也没有完整父子传播树。
    /// 子上下文创建时只复制父上下文当下的取消状态与截止时间，
    /// 后续父上下文再变化时，除 `AfterFunc` 轮询检查外，不会自动级联到底层结构。
    /// 这样的缩减足以支撑导入任务取消、超时和测试中的 grace-period 行为验证。
    #[derive(Clone, Default)]
    pub struct Context {
        cancelled: Arc<AtomicBool>,
        cause: Arc<Mutex<Option<Error>>>,
        deadline: Option<Instant>,
    }

    impl std::fmt::Debug for Context {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Context")
        }
    }

    /// 返回一个未取消、无截止时间的根上下文。
    pub fn Background() -> Context {
        Context::default()
    }

    pub type CancelFunc = Box<dyn FnOnce() + Send>;

    /// 生成可取消子上下文。
    /// 这里复制父上下文的已知状态，而不是共享完整父子树。
    /// 因此它保证“初始约束一致”，但不承诺生产版那种实时层级传播。
    pub fn WithCancel(parent: Context) -> (Context, CancelFunc) {
        let child = Context {
            cancelled: Arc::new(AtomicBool::new(parent.is_cancelled())),
            cause: Arc::new(Mutex::new(None)),
            deadline: parent.deadline,
        };
        let flag = child.cancelled.clone();
        let cause = child.cause.clone();
        (
            child,
            Box::new(move || {
                flag.store(true, Ordering::SeqCst);
                let _ = cause.lock().map(|mut g| {
                    if g.is_none() {
                        *g = Some(Error::new("context canceled"));
                    }
                });
            }),
        )
    }

    /// 允许调用方显式写入取消原因，供上层区分普通取消与 failover 取消。
    pub fn WithCancelCause(parent: Context) -> (Context, Box<dyn FnOnce(Error) + Send>) {
        let child = Context {
            cancelled: Arc::new(AtomicBool::new(parent.is_cancelled())),
            cause: Arc::new(Mutex::new(None)),
            deadline: parent.deadline,
        };
        let flag = child.cancelled.clone();
        let cause = child.cause.clone();
        (
            child,
            Box::new(move |err: Error| {
                flag.store(true, Ordering::SeqCst);
                let _ = cause.lock().map(|mut g| *g = Some(err));
            }),
        )
    }

    /// 以固定持续时间生成带 deadline 的上下文。
    /// 这里不启动后台定时器，而是在读取状态时惰性判断是否已过期。
    pub fn WithTimeout(parent: Context, timeout: Duration) -> (Context, CancelFunc) {
        let (child, cancel) = WithCancel(parent);
        let deadline = Some(Instant::now() + timeout);
        let mut out = child;
        out.deadline = deadline.or(out.deadline);
        (out, cancel)
    }

    /// 清空取消状态、取消原因与 deadline。
    /// Go `WithoutCancel` 返回的上下文不再暴露父级 deadline、Done、Err 或 Cause。
    pub fn WithoutCancel(_parent: Context) -> Context {
        Context {
            cancelled: Arc::new(AtomicBool::new(false)),
            cause: Arc::new(Mutex::new(None)),
            deadline: None,
        }
    }

    /// 返回上下文取消原因。
    /// 若没有显式原因但上下文已取消，则退回统一的 `"context canceled"` 文本。
    pub fn Cause(ctx: &Context) -> Error {
        if let Ok(g) = ctx.cause.lock() {
            if let Some(err) = g.as_ref() {
                return err.clone();
            }
        }
        if ctx.is_cancelled() {
            return Error::new("context canceled");
        }
        Error::new("")
    }

    impl Context {
        /// 惰性检查当前上下文是否已取消或已超时。
        /// 如果 deadline 已到，会顺带把内部标记置为取消，方便后续快速读取。
        pub fn is_cancelled(&self) -> bool {
            if self.cancelled.load(Ordering::SeqCst) {
                return true;
            }
            if let Some(deadline) = self.deadline {
                if Instant::now() >= deadline {
                    self.cancelled.store(true, Ordering::SeqCst);
                    return true;
                }
            }
            false
        }

        /// 与 Go `Err()` 一样，仅在上下文已取消时返回错误。
        pub fn Err(&self) -> Option<Error> {
            if self.is_cancelled() {
                Some(Cause(self))
            } else {
                None
            }
        }

        /// 直接把取消原因写回当前上下文，供测试或桥接代码主动终止流程。
        pub fn cancel_with(&self, err: Error) {
            self.cancelled.store(true, Ordering::SeqCst);
            if let Ok(mut g) = self.cause.lock() {
                *g = Some(err);
            }
        }
    }

    /// `AfterFunc` 只做“尽力而为”的近似实现。
    /// 若父上下文已取消，则同步执行回调，保持立即触发语义。
    /// 否则启动轻量线程轮询父上下文状态，主要服务测试里的短超时和清理路径。
    /// 这里没有复杂调度器，也不保证像 Go 一样精确按取消瞬间执行。
    /// 但对 importinto 而言，调用方只要求“取消后最终会触发回调或可被显式取消”，
    /// 因而这种实现足以覆盖 grace timeout 相关分支。
    pub fn AfterFunc(parent: Context, f: impl FnOnce() + Send + 'static) -> CancelFunc {
        if parent.is_cancelled() {
            f();
            return Box::new(|| {});
        }
        // Store callback; callers cancel the grace context themselves on Drop.
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = ran.clone();
        std::thread::spawn(move || {
            // Poll lightly; used only for grace-timeout paths in tests with short sleeps.
            for _ in 0..200 {
                if parent.is_cancelled() {
                    if !ran2.swap(true, Ordering::SeqCst) {
                        f();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Box::new(move || {
            ran.store(true, Ordering::SeqCst);
        })
    }

    /// 上下文错误在这里不做额外转换，保持错误对象原样回传给上层。
    pub fn map_ctx_err(err: Error) -> errors::ErrorType {
        err
    }
}

// ---------------------------------------------------------------------------
// atomic
// ---------------------------------------------------------------------------

pub mod atomic {
    use super::{AtomicI64, Ordering};

    /// 这个原子整数包装保留 Go `atomic.Int64` 的最小 API 形状。
    /// importinto 只会读取、写入和序列化最大错误计数，因此无需更复杂的原子操作。
    #[derive(Debug)]
    pub struct Int64(AtomicI64);

    impl Int64 {
        /// 用给定初始值创建原子计数器。
        pub fn new(v: i64) -> Self {
            Self(AtomicI64::new(v))
        }
        /// 采用顺序一致语义读取，优先保证与 Go 直观语义一致而不是极限性能。
        pub fn Load(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        /// 采用顺序一致语义写入，便于测试和配置读取保持稳定。
        pub fn Store(&self, v: i64) {
            self.0.store(v, Ordering::SeqCst);
        }
    }

    impl Clone for Int64 {
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }

    impl Default for Int64 {
        fn default() -> Self {
            Self::new(0)
        }
    }

    impl serde::Serialize for Int64 {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            serializer.serialize_i64(self.Load())
        }
    }

    impl<'de> serde::Deserialize<'de> for Int64 {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            let v = i64::deserialize(deserializer)?;
            Ok(Self::new(v))
        }
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

pub mod config {
    use super::atomic::Int64;
    use std::time::Duration;

    /// 这一组配置结构只覆盖 importinto 创建导入任务时会读取的字段。
    /// 未被 Rust 端用到的 Lightning 全量配置并未搬入这里，避免桩无限膨胀。
    /// 这也解释了为什么很多结构只保留少数字段，看起来不像完整生产配置。
    /// 维护时若新增调用方读取了其他字段，应同步在这里补位，而不是默认读取空值。
    pub const CheckpointDriverFile: &str = "file";
    pub const CheckpointDriverMySQL: &str = "mysql";
    pub const CheckpointRemove: i32 = 0;
    pub const CheckpointRename: i32 = 1;
    pub const CheckpointOrigin: i32 = 2;

    /// `MaxError` 只保留类型错误计数，因为提交导入作业时只消费这一项。
    #[derive(Clone, Debug, Default)]
    pub struct MaxError {
        pub Type: Int64,
    }

    /// 这些字段直接驱动预检查开关、并发度和容错阈值。
    #[derive(Clone, Debug, Default)]
    pub struct Lightning {
        pub CheckRequirements: bool,
        pub TableConcurrency: i32,
        pub MaxError: MaxError,
    }

    /// 这里只保留“是否在导入 SQL 里移除 S3 external-id”这一兼容开关。
    #[derive(Clone, Debug, Default)]
    pub struct TikvImporter {
        pub StripS3ExternalIDForImportSQL: bool,
    }

    /// CSV 配置只保留导入 SQL 构造时真正引用的字段。
    #[derive(Clone, Debug, Default)]
    pub struct CSVConfig {
        pub Header: bool,
        pub Separator: String,
        pub Delimiter: String,
    }

    /// Mydumper 运行期配置主要被 SDK 选项和导入选项构造流程消费。
    #[derive(Clone, Debug, Default)]
    pub struct MydumperRuntime {
        pub SourceDir: String,
        pub StrictFormat: bool,
        pub CSV: CSVConfig,
        pub CharacterSet: String,
        pub DataCharacterSet: String,
        pub Filter: Vec<String>,
        pub FileRouters: Vec<String>,
    }

    /// `DBStore` 目前仅承担 SQL mode 透传。
    #[derive(Clone, Debug, Default)]
    pub struct DBStore {
        pub SQLMode: String,
    }

    /// checkpoint 配置要同时支撑 file 和 mysql 两种管理器分支。
    #[derive(Clone, Debug, Default)]
    pub struct Checkpoint {
        pub Enable: bool,
        pub Driver: String,
        pub DSN: String,
        pub Schema: String,
        pub KeepAfterSuccess: i32,
        pub MySQLParam: Option<super::common::MySQLConnectParam>,
    }

    /// 对齐 Go 配置对象里“带时间长度”的嵌套形状，便于直接取 `Duration`。
    #[derive(Clone, Debug)]
    pub struct DurationCfg {
        pub Duration: Duration,
    }

    impl Default for DurationCfg {
        fn default() -> Self {
            Self {
                Duration: Duration::from_secs(60),
            }
        }
    }

    /// `Cron` 目前只保留日志上报间隔，因为 orchestrator 只会读取这一项。
    #[derive(Clone, Debug, Default)]
    pub struct Cron {
        pub LogProgress: DurationCfg,
    }

    /// `Config` 汇总 importinto 各流程直接触达的子配置。
    /// 它不是 Lightning 全量配置镜像，而是围绕导入、checkpoint、日志节奏与路由裁剪后的最小集合。
    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub App: Lightning,
        pub TikvImporter: TikvImporter,
        pub Checkpoint: Checkpoint,
        pub Mydumper: MydumperRuntime,
        pub TiDB: DBStore,
        pub Cron: Cron,
        pub Routes: Vec<String>,
    }

    impl Config {
        /// 构造与 Go 默认值兼容的最小配置。
        /// 这里显式补上预检查默认开启、表并发默认为 10、checkpoint 默认走 file 驱动。
        pub fn NewConfig() -> Self {
            let mut cfg = Self::default();
            cfg.App.CheckRequirements = true;
            cfg.App.TableConcurrency = 10;
            cfg.Checkpoint.Driver = CheckpointDriverFile.into();
            cfg
        }
    }
}

// ---------------------------------------------------------------------------
// common
// ---------------------------------------------------------------------------

pub mod common {
    use super::{Error, Result, context::Context, log::Logger, sql::DB};
    use std::fmt;

    /// `common` 汇总 importinto 会直接复用的通用函数和错误身份。
    /// 这一层的重点不是“工具函数多”，而是它承接了多处跨模块共享的行为约定。
    /// 例如标识符转义必须与 SQL 拼接规则一致，
    /// `AllTables` 必须与 checkpoint 管理器的“全量操作”哨兵值一致，
    /// “context canceled”“retryable” 的判断也要与上层分支保持一致。
    /// 因为这些契约被多个模块共同依赖，所以它们即便实现简单，也属于高价值注释点。
    pub const AllTables: &str = "all";

    /// 以 MySQL 风格反引号转义标识符，避免 schema/table 名里的反引号破坏 SQL。
    pub fn EscapeIdentifier(identifier: &str) -> String {
        let mut builder = String::with_capacity(identifier.len() + 2);
        builder.push('`');
        for b in identifier.bytes() {
            if b == b'`' {
                builder.push_str("``");
            } else {
                builder.push(b as char);
            }
        }
        builder.push('`');
        builder
    }

    /// 生成 ``db`.`table`` 形状的唯一表名字符串，供 checkpoint 与日志统一使用。
    pub fn UniqueTable(schema: &str, table: &str) -> String {
        format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
    }

    /// 通过消息文本识别上下文取消错误。
    /// 这是因为当前桩路径不保证所有取消都保留专门类型，只能以展示文本兜底。
    pub fn IsContextCanceledError(error: Option<&Error>) -> bool {
        let Some(err) = error else {
            return false;
        };
        let msg = err.Error().to_lowercase();
        msg.contains("context canceled") || msg.contains("context cancelled")
    }

    /// 按消息关键字给出“可重试”近似判断，覆盖导入与取消重试测试依赖的几类错误。
    pub fn IsRetryableError(error: Option<&Error>) -> bool {
        let Some(err) = error else {
            return false;
        };
        let msg = err.Error().to_lowercase();
        msg.contains("timeout")
            || msg.contains("temporary")
            || msg.contains("try again")
            || msg.contains("task not found")
    }

    /// 这是 Go `common.ErrCheckpointTableNotFound` 的最小形状。
    /// 其关键契约不是类型本身，而是生成出的错误必须同时具备 not-found 身份和稳定类名。
    #[derive(Clone, Copy, Debug)]
    pub struct CheckpointNotFound;

    impl CheckpointNotFound {
        pub fn GenWithStackByArgs(self, table: impl fmt::Display) -> Error {
            // 这里保留 Go 同款消息前缀和 NotFound 身份，供上层按类名或文本分支。
            let mut e = Error::new(format!("checkpoint for table {table} not found"));
            e.class = Some("Lightning:Checkpoint:ErrCheckpointTableNotFound");
            e.not_found = true;
            e
        }
    }

    #[allow(non_upper_case_globals)]
    pub static ErrCheckpointTableNotFound: CheckpointNotFound = CheckpointNotFound;

    /// 连接参数不负责真实网络连接；这里返回内存 DB 以承接 checkpoint SQL 语义。
    #[derive(Clone, Debug, Default)]
    pub struct MySQLConnectParam {
        pub Host: String,
        pub Port: i32,
        pub User: String,
        pub Password: String,
        pub SQLMode: String,
        pub MaxAllowedPacket: u64,
        pub AllowFallbackToPlaintext: bool,
    }

    impl MySQLConnectParam {
        /// 用内存数据库 stand-in 替代真实 MySQL 连接。
        /// importinto 调用方只要求拿到一个满足 `ExecContext`/`QueryContext` 的对象。
        pub fn Connect(&self) -> Result<DB> {
            Ok(DB::new_memory())
        }
    }

    /// 这个事务包装只模拟“回调成功则提交、失败则回滚”的控制流。
    /// 没有指数退避或重试循环，但足以保护 DestroyError 这类事务性删除路径。
    #[derive(Clone, Debug)]
    pub struct SQLWithRetry {
        pub DB: DB,
        pub Logger: Logger,
    }

    impl SQLWithRetry {
        pub fn Transact<F>(&self, ctx: Context, _name: &str, f: F) -> Result<()>
        where
            F: FnOnce(Context, &super::sql::Tx) -> Result<()>,
        {
            let tx = self.DB.Begin()?;
            match f(ctx, &tx) {
                Ok(()) => tx.Commit(),
                Err(e) => {
                    let _ = tx.Rollback();
                    Err(e)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// log / zap
// ---------------------------------------------------------------------------

pub mod zap {
    /// `zap` stand-in 只保留键值字段构造能力，供 logger 组合可读输出。
    /// 字段值统一转成字符串，避免为测试桩再引入复杂编码逻辑。
    #[derive(Clone, Debug, Default)]
    pub struct Field {
        pub key: String,
        pub value: String,
    }

    pub fn String(k: &str, v: impl ToString) -> Field {
        Field {
            key: k.into(),
            value: v.to_string(),
        }
    }
    pub fn Int(k: &str, v: i64) -> Field {
        String(k, v)
    }
    pub fn Int64(k: &str, v: i64) -> Field {
        String(k, v)
    }
    pub fn Error(e: &super::Error) -> Field {
        String("error", e.Error())
    }
    pub fn NamedError(k: &str, e: &super::Error) -> Field {
        String(k, e.Error())
    }
}

pub mod log {
    use super::zap::Field;
    use std::sync::{Arc, Mutex};

    /// 这里的 `Logger` 不是完整日志系统，而是一个可检查缓冲区。
    /// 它把不同级别的消息分别积累到内存中，方便测试验证日志是否出现。
    /// `fields` 会在 `With` 时前置保存，使后续 `Info/Warn/Error` 输出都能携带上下文键值。
    /// 这与 Go `log.Logger.With(zap.Field...)` 的调用形状保持一致，但省略了编码、采样和输出后端。
    #[derive(Clone, Debug, Default)]
    pub struct Logger {
        pub inner: Option<()>,
        pub fields: Vec<Field>,
        pub infos: Arc<Mutex<Vec<String>>>,
        pub warns: Arc<Mutex<Vec<String>>>,
        pub errors: Arc<Mutex<Vec<String>>>,
    }

    impl Logger {
        /// 返回附加了一个字段的新 logger，保留原对象的消息缓冲区共享语义。
        pub fn With(mut self, field: Field) -> Self {
            self.fields.push(field);
            self
        }
        /// 把消息、logger 继承字段与本次调用字段拼成稳定文本。
        fn format_line(&self, msg: String, fields: &[Field]) -> String {
            let mut s = msg;
            for f in self.fields.iter().chain(fields) {
                s.push(' ');
                s.push_str(&f.key);
                s.push('=');
                s.push_str(&f.value);
            }
            s
        }
        pub fn Info(&self, msg: impl Into<String>, fields: &[Field]) {
            if let Ok(mut g) = self.infos.lock() {
                g.push(self.format_line(msg.into(), fields));
            }
        }
        pub fn Warn(&self, msg: impl Into<String>, fields: &[Field]) {
            if let Ok(mut g) = self.warns.lock() {
                g.push(self.format_line(msg.into(), fields));
            }
        }
        pub fn Error(&self, msg: impl Into<String>, fields: &[Field]) {
            if let Ok(mut g) = self.errors.lock() {
                g.push(self.format_line(msg.into(), fields));
            }
        }
        pub fn Debug(&self, _msg: impl Into<String>, _fields: &[Field]) {}
        pub fn buffer_string(&self) -> String {
            let infos = self.infos.lock().unwrap().clone();
            let warns = self.warns.lock().unwrap().clone();
            let errors = self.errors.lock().unwrap().clone();
            infos
                .into_iter()
                .chain(warns)
                .chain(errors)
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    pub fn L() -> Logger {
        Logger {
            inner: Some(()),
            infos: Arc::new(Mutex::new(Vec::new())),
            warns: Arc::new(Mutex::new(Vec::new())),
            errors: Arc::new(Mutex::new(Vec::new())),
            ..Default::default()
        }
    }

    /// 对齐 Go `log.MakeTestLogger` 的使用习惯：返回一个能被测试读取缓冲内容的 logger。
    pub fn MakeTestLogger() -> Logger {
        L()
    }
}

// ---------------------------------------------------------------------------
// sql
// ---------------------------------------------------------------------------

pub mod sql {
    use super::{Error, Result, context::Context};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// 这一节提供最小内存 SQL 引擎，只实现 importinto checkpoint 流程会用到的查询形状。
    /// 它不是通用 SQL 解析器，判断逻辑主要依赖小写化后的关键片段匹配。
    /// 这种实现看起来粗糙，但可以稳定覆盖 Go 侧会发出的固定 SQL 模板，
    /// 尤其是建库建表、checkpoint upsert、按状态更新、删除失败 checkpoint 以及查询当前行。
    /// 如果未来 SQL 模板变化，需要同步更新这里的关键字匹配，否则测试可能出现“语义空成功”。
    /// 此外，`DB` 内部显式维护 `exec_log`，让调用方能检查执行过哪些语句和参数。
    #[derive(Clone, Debug)]
    pub enum SqlValue {
        Null,
        Int64(i64),
        String(String),
    }

    impl From<i64> for SqlValue {
        fn from(v: i64) -> Self {
            SqlValue::Int64(v)
        }
    }
    impl From<String> for SqlValue {
        fn from(v: String) -> Self {
            SqlValue::String(v)
        }
    }
    impl From<&str> for SqlValue {
        fn from(v: &str) -> Self {
            SqlValue::String(v.to_string())
        }
    }

    impl SqlValue {
        pub fn as_i64(&self) -> i64 {
            match self {
                SqlValue::Int64(v) => *v,
                SqlValue::String(s) => s.parse().unwrap_or(0),
                SqlValue::Null => 0,
            }
        }
        pub fn as_string(&self) -> String {
            match self {
                SqlValue::Null => String::new(),
                SqlValue::Int64(v) => v.to_string(),
                SqlValue::String(s) => s.clone(),
            }
        }
    }

    /// `NullString` 保持与 Go `sql.NullString` 相似的读取形状。
    #[derive(Clone, Debug)]
    pub struct NullString {
        pub String: String,
        pub Valid: bool,
    }

    /// `DB` 用内存结构模拟 checkpoint 表。
    /// 当前只维护一张按 `table_name` 索引的逻辑表，因为 importinto 只会操作这一组状态。
    #[derive(Clone, Debug)]
    pub struct DB {
        inner: Arc<Mutex<DbInner>>,
    }

    #[derive(Debug, Default)]
    struct DbInner {
        closed: bool,
        exec_log: Vec<(String, Vec<SqlValue>)>,
        // 用 `table_name -> row` 模拟 checkpoint 主键索引，便于表达 upsert/删除/查询行为。
        checkpoints: HashMap<String, CheckpointRow>,
        next_affected: i64,
    }

    /// 单行 checkpoint 记录对应 Go 里的 `TableCheckpoint` 在 SQL 层的落盘投影。
    #[derive(Clone, Debug, Default)]
    struct CheckpointRow {
        table_name: String,
        job_id: i64,
        status: i32,
        message: String,
        group_key: String,
    }

    impl DB {
        /// 创建一个全新的内存数据库实例，默认下一次通用写操作影响 1 行。
        pub fn new_memory() -> Self {
            Self {
                inner: Arc::new(Mutex::new(DbInner {
                    next_affected: 1,
                    ..Default::default()
                })),
            }
        }

        pub fn Close(&self) -> Result<()> {
            self.inner.lock().unwrap().closed = true;
            Ok(())
        }

        pub fn is_closed(&self) -> bool {
            self.inner.lock().unwrap().closed
        }

        pub fn exec_log(&self) -> Vec<(String, Vec<SqlValue>)> {
            self.inner.lock().unwrap().exec_log.clone()
        }

        /// 记录执行日志并针对 importinto 关心的 SQL 片段更新内存状态。
        /// 未识别的语句会返回默认受影响行数，而不是报错，以保持桩的宽容性。
        pub fn ExecContext(
            &self,
            _ctx: &Context,
            query: &str,
            args: &[SqlValue],
        ) -> Result<ExecResult> {
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push((query.to_string(), args.to_vec()));
            let q = query.to_lowercase();

            if q.contains("create database") || q.contains("create table") {
                return Ok(ExecResult { affected: 0 });
            }

            if q.contains("insert into") && q.contains("on duplicate key update") {
                let table_name = args.first().map(|a| a.as_string()).unwrap_or_default();
                let job_id = args.get(1).map(|a| a.as_i64()).unwrap_or(0);
                let status = args.get(2).map(|a| a.as_i64() as i32).unwrap_or(0);
                let message = args.get(3).map(|a| a.as_string()).unwrap_or_default();
                let group_key = args.get(4).map(|a| a.as_string()).unwrap_or_default();
                g.checkpoints.insert(
                    table_name.clone(),
                    CheckpointRow {
                        table_name,
                        job_id,
                        status,
                        message,
                        group_key,
                    },
                );
                return Ok(ExecResult { affected: 1 });
            }

            if q.starts_with("delete from") {
                if q.contains("where table_name") && q.contains("and status") {
                    // 单表删除失败 checkpoint 的路径要求先按表名和状态同时过滤。
                    let table_name = args.first().map(|a| a.as_string()).unwrap_or_default();
                    let status = args.get(1).map(|a| a.as_i64() as i32).unwrap_or(-1);
                    let before = g.checkpoints.len();
                    if let Some(row) = g.checkpoints.get(&table_name) {
                        if row.status == status {
                            g.checkpoints.remove(&table_name);
                        }
                    }
                    let affected = (before - g.checkpoints.len()) as i64;
                    return Ok(ExecResult { affected });
                }
                if q.contains("where status") && !q.contains("table_name") {
                    let status = args.first().map(|a| a.as_i64() as i32).unwrap_or(-1);
                    let before = g.checkpoints.len();
                    g.checkpoints.retain(|_, r| r.status != status);
                    let affected = (before - g.checkpoints.len()) as i64;
                    return Ok(ExecResult { affected });
                }
                if q.contains("where table_name") {
                    let table_name = args.first().map(|a| a.as_string()).unwrap_or_default();
                    let affected = if g.checkpoints.remove(&table_name).is_some() {
                        1
                    } else {
                        0
                    };
                    return Ok(ExecResult { affected });
                }
                // delete all
                let affected = g.checkpoints.len() as i64;
                g.checkpoints.clear();
                return Ok(ExecResult { affected });
            }

            if q.starts_with("update") {
                if q.contains("where status") && !q.contains("table_name") {
                    // 全量 IgnoreError 会把失败状态重置为 pending，并清空 message/job_id。
                    let new_status = args.first().map(|a| a.as_i64() as i32).unwrap_or(0);
                    let old_status = args.get(1).map(|a| a.as_i64() as i32).unwrap_or(-1);
                    let mut affected = 0i64;
                    for row in g.checkpoints.values_mut() {
                        if row.status == old_status {
                            row.status = new_status;
                            row.message.clear();
                            row.job_id = 0;
                            affected += 1;
                        }
                    }
                    return Ok(ExecResult { affected });
                }
                if q.contains("where table_name") {
                    let new_status = args.first().map(|a| a.as_i64() as i32).unwrap_or(0);
                    let table_name = args.get(1).map(|a| a.as_string()).unwrap_or_default();
                    let old_status = args.get(2).map(|a| a.as_i64() as i32).unwrap_or(-1);
                    let mut affected = 0i64;
                    if let Some(row) = g.checkpoints.get_mut(&table_name) {
                        if row.status == old_status {
                            row.status = new_status;
                            row.message.clear();
                            row.job_id = 0;
                            affected = 1;
                        }
                    }
                    return Ok(ExecResult { affected });
                }
            }

            Ok(ExecResult {
                affected: g.next_affected,
            })
        }

        /// 仅支持按表名读取单行 checkpoint，足够承接 `Get` 和“确保存在”分支。
        pub fn QueryRowContext(
            &self,
            _ctx: &Context,
            query: &str,
            args: &[SqlValue],
        ) -> Result<Option<(i64, i32, NullString, NullString)>> {
            let g = self.inner.lock().unwrap();
            let q = query.to_lowercase();
            if q.contains("where table_name") {
                let table_name = args.first().map(|a| a.as_string()).unwrap_or_default();
                if let Some(row) = g.checkpoints.get(&table_name) {
                    return Ok(Some((
                        row.job_id,
                        row.status,
                        NullString {
                            String: row.message.clone(),
                            Valid: !row.message.is_empty(),
                        },
                        NullString {
                            String: row.group_key.clone(),
                            Valid: !row.group_key.is_empty(),
                        },
                    )));
                }
                return Ok(None);
            }
            Ok(None)
        }

        /// 查询多行 checkpoint。
        /// 这里故意先判断 `WHERE table_name AND status`，
        /// 因为 SELECT 列表本身也会包含 `table_name`、`status` 字样，不能仅靠是否出现单词来分支。
        pub fn QueryContext(&self, _ctx: &Context, query: &str, args: &[SqlValue]) -> Result<Rows> {
            let g = self.inner.lock().unwrap();
            let q = query.to_lowercase();
            let mut rows = Vec::new();
            // 小心区分 WHERE 子句与 SELECT 列表里的同名字段，避免把过滤条件误判成“全量查询”。
            if q.contains("where table_name") && q.contains("and status") {
                let table_name = args.first().map(|a| a.as_string()).unwrap_or_default();
                let status = args.get(1).map(|a| a.as_i64() as i32).unwrap_or(-1);
                if let Some(row) = g.checkpoints.get(&table_name) {
                    if row.status == status {
                        rows.push(row.clone());
                    }
                }
            } else if q.contains("where status") {
                let status = args.first().map(|a| a.as_i64() as i32).unwrap_or(-1);
                for row in g.checkpoints.values() {
                    if row.status == status {
                        rows.push(row.clone());
                    }
                }
            } else {
                for row in g.checkpoints.values() {
                    rows.push(row.clone());
                }
            }
            Ok(Rows { rows, idx: 0 })
        }

        /// 事务对象只是 DB 的轻量句柄，提交和回滚本身不引入额外状态。
        pub fn Begin(&self) -> Result<Tx> {
            Ok(Tx { db: self.clone() })
        }
    }

    /// 返回受影响行数，满足调用方对 `RowsAffected()` 的最小依赖。
    #[derive(Clone, Debug)]
    pub struct ExecResult {
        pub affected: i64,
    }

    impl ExecResult {
        pub fn RowsAffected(&self) -> Result<i64> {
            Ok(self.affected)
        }
    }

    /// `Rows` 以预先收集好的向量形式暴露遍历接口，避免实现真正的数据库游标。
    #[derive(Clone, Debug)]
    pub struct Rows {
        rows: Vec<CheckpointRow>,
        idx: usize,
    }

    impl Rows {
        pub fn Next(&mut self) -> bool {
            if self.idx < self.rows.len() {
                self.idx += 1;
                true
            } else {
                false
            }
        }
        /// 以 checkpoint 专用投影读取当前行，顺序与 Go 侧 `rows.Scan(...)` 保持一致。
        pub fn ScanCheckpoint(&self) -> (String, i64, i32, NullString, NullString) {
            let row = &self.rows[self.idx - 1];
            (
                row.table_name.clone(),
                row.job_id,
                row.status,
                NullString {
                    String: row.message.clone(),
                    Valid: !row.message.is_empty(),
                },
                NullString {
                    String: row.group_key.clone(),
                    Valid: !row.group_key.is_empty(),
                },
            )
        }
        pub fn Err(&self) -> Result<()> {
            Ok(())
        }
        pub fn Close(&mut self) {}
        /// 以简单 CSV 形式导出表级 checkpoint，服务 DumpTables 相关断言。
        pub fn write_csv(&self, writer: &mut dyn std::io::Write) -> Result<()> {
            writeln!(writer, "table_name,job_id,status,message,group_key")
                .map_err(|e| Error::new(e.to_string()))?;
            for row in &self.rows {
                writeln!(
                    writer,
                    "{},{},{},{},{}",
                    row.table_name, row.job_id, row.status, row.message, row.group_key
                )
                .map_err(|e| Error::new(e.to_string()))?;
            }
            Ok(())
        }
    }

    #[derive(Clone, Debug)]
    pub struct Tx {
        db: DB,
    }

    impl Tx {
        pub fn QueryContext(&self, ctx: &Context, query: &str, args: &[SqlValue]) -> Result<Rows> {
            self.db.QueryContext(ctx, query, args)
        }
        pub fn ExecContext(
            &self,
            ctx: &Context,
            query: &str,
            args: &[SqlValue],
        ) -> Result<ExecResult> {
            self.db.ExecContext(ctx, query, args)
        }
        pub fn Commit(&self) -> Result<()> {
            Ok(())
        }
        pub fn Rollback(&self) -> Result<()> {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// importsdk
// ---------------------------------------------------------------------------

pub mod importsdk {
    use super::{Error, Result, context::Context, log::Logger};
    use std::sync::{Arc, Mutex};

    /// `importsdk` 是本文件里最关键的跨边界 stand-in 之一。
    /// importinto 的主体流程并不关心 TiDB 后端如何真实解析数据文件，
    /// 它更关心“能否拿到表元数据”“能否生成导入 SQL”“能否提交并观察作业状态”。
    /// 因此这里把 SDK 抽象压缩成一个 trait 和一个高可控 `MockSDK` 实现，
    /// 重点保护提交流程、group key、取消行为和错误注入点，而不模拟真实分布式执行。
    /// 对齐 Go 时要记住：这里的 `JobStatus` 是用户态观察值，不代表 TiDB 内部完整状态机。
    #[derive(Clone, Debug, Default)]
    pub struct DataFileMeta {
        pub Path: String,
        pub Size: i64,
        pub Format: FileFormat,
    }

    /// 文件格式只保留 importinto 会生成导入 SQL 时用到的几个枚举值。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub enum FileFormat {
        #[default]
        Unknown,
        CSV,
        SQL,
    }

    impl FileFormat {
        pub fn String(&self) -> &'static str {
            match self {
                FileFormat::CSV => "csv",
                FileFormat::SQL => "sql",
                FileFormat::Unknown => "",
            }
        }
    }

    /// `TableMeta` 汇总一次表级导入所需的数据库名、表名、文件列表和 schema 相关信息。
    #[derive(Clone, Debug, Default)]
    pub struct TableMeta {
        pub Database: String,
        pub Table: String,
        pub DataFiles: Vec<DataFileMeta>,
        pub TotalSize: i64,
        pub WildcardPath: String,
        pub SchemaFile: String,
    }

    /// `ImportOptions` 对应提交 IMPORT INTO 时会拼进 SQL 的那组选项。
    /// 这里只保留 Rust 侧已经会读取或写入的字段，避免把 Go 全量选项不加甄别搬进来。
    #[derive(Clone, Debug, Default)]
    pub struct ImportOptions {
        pub Format: String,
        pub CSVConfig: Option<super::config::CSVConfig>,
        pub SplitFile: bool,
        pub RecordErrors: i64,
        pub Detached: bool,
        pub CloudStorageURI: String,
        pub GroupKey: String,
        pub SkipRows: i64,
        pub CharacterSet: String,
        pub DisablePrecheck: bool,
        pub ResourceParameters: String,
    }

    /// `JobStatus` 把作业生命周期压缩成一组字符串字段，方便与 Go SDK 结果做近似对照。
    #[derive(Clone, Debug, Default)]
    pub struct JobStatus {
        pub JobID: i64,
        pub GroupKey: String,
        pub Phase: String,
        pub Status: String,
        pub SourceFileSize: String,
        pub ImportedRows: i64,
        pub ResultMessage: String,
        pub Step: String,
        pub TotalSize: String,
        pub Percent: String,
    }

    impl JobStatus {
        /// `finished`、`failed`、`cancelled` 是当前调用方真正关心的终态集合。
        pub fn IsFinished(&self) -> bool {
            self.Status == "finished"
        }
        pub fn IsFailed(&self) -> bool {
            self.Status == "failed"
        }
        pub fn IsCancelled(&self) -> bool {
            self.Status == "cancelled"
        }
        pub fn IsCompleted(&self) -> bool {
            self.IsFinished() || self.IsFailed() || self.IsCancelled()
        }
    }

    /// trait 只声明 importinto 主流程会调用的最小后端接口。
    /// 这样 `Importer` 和 `JobSubmitter` 能围绕稳定契约编程，而不依赖具体实现。
    pub trait SDK: Send + Sync {
        fn CreateSchemasAndTables(&self, ctx: &Context) -> Result<()>;
        fn GetTableMetas(&self, ctx: &Context) -> Result<Vec<TableMeta>>;
        fn GenerateImportSQL(&self, table: &TableMeta, opts: &ImportOptions) -> Result<String>;
        fn SubmitJob(&self, ctx: &Context, sql: &str) -> Result<i64>;
        fn GetJobsByGroup(&self, ctx: &Context, group_key: &str) -> Result<Vec<JobStatus>>;
        fn CancelJob(&self, ctx: &Context, job_id: i64) -> Result<()>;
        fn Close(&self) -> Result<()>;
    }

    /// 这些 option 构造器当前都只保留调用形状。
    /// 原因是 `NewImportSDK` 返回的本地 mock 并不会真实消费全部配置；
    /// 但保留签名可以让 Rust 代码与 Go 创建 SDK 的步骤保持一致。
    pub type SDKOption = Box<dyn FnOnce(&mut MockSDK) + Send>;

    pub fn WithSQLMode(_mode: String) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithFilter(_f: Vec<String>) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithFileRouters(_r: Vec<String>) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithRoutes(_r: Vec<String>) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithCharset(_c: String) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithDataCharacterSet(_c: String) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithCSVConfig(_c: super::config::CSVConfig) -> SDKOption {
        Box::new(|_| {})
    }
    pub fn WithLogger(_l: Logger) -> SDKOption {
        Box::new(|_| {})
    }

    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex as StdMutex;

    /// 生成 SQL 与取消作业都支持注入 hook，便于测试构造精确分支。
    type GenSqlHook =
        Arc<dyn Fn(&TableMeta, &ImportOptions) -> Result<String> + Send + Sync + 'static>;
    type CancelHook = Arc<dyn Fn(i64) -> Result<()> + Send + Sync + 'static>;

    /// `MockSDK` 通过内存状态和结果队列提供强可控行为。
    /// 它能同时覆盖固定返回、按次序弹出结果、延迟成功和记录最后一次输入等测试需求。
    /// 这让 orchestrator、submitter、importer 可以在不连 TiDB 的前提下验证核心控制流。
    #[derive(Clone, Default)]
    pub struct MockSDK {
        pub tables: Vec<TableMeta>,
        pub jobs: Arc<Mutex<Vec<JobStatus>>>,
        pub next_job_id: Arc<Mutex<i64>>,
        pub closed: Arc<Mutex<bool>>,
        pub create_err: Option<Error>,
        pub get_metas_err: Option<Error>,
        pub submit_err: Option<Error>,
        pub cancel_err: Option<Error>,
        pub close_err: Option<Error>,
        pub gen_sql_err: Option<Error>,
        pub fixed_sql: Option<String>,
        pub fixed_job_id: Option<i64>,
        pub cancel_attempts: Arc<Mutex<HashMap<i64, i32>>>,
        pub cancel_retry_times: i32,
        pub get_jobs_queue: Arc<Mutex<VecDeque<Result<Vec<JobStatus>>>>>,
        pub cancel_queue: Arc<Mutex<VecDeque<Result<()>>>>,
        pub last_import_opts: Arc<Mutex<Option<ImportOptions>>>,
        pub last_submit_sql: Arc<Mutex<Option<String>>>,
        pub gen_sql_hook: Option<GenSqlHook>,
        pub cancel_hook: Option<CancelHook>,
    }

    impl MockSDK {
        /// 创建一个默认可用的 SDK mock。
        /// 默认下次作业 ID 从 1 开始，取消失败重试阈值为 2，且所有可观测状态都可被测试读取。
        pub fn new() -> Self {
            Self {
                next_job_id: Arc::new(Mutex::new(1)),
                jobs: Arc::new(Mutex::new(Vec::new())),
                closed: Arc::new(Mutex::new(false)),
                cancel_attempts: Arc::new(Mutex::new(HashMap::new())),
                cancel_retry_times: 2,
                get_jobs_queue: Arc::new(Mutex::new(VecDeque::new())),
                cancel_queue: Arc::new(Mutex::new(VecDeque::new())),
                last_import_opts: Arc::new(Mutex::new(None)),
                last_submit_sql: Arc::new(Mutex::new(None)),
                ..Default::default()
            }
        }

        /// 按顺序压入“查询作业列表”的返回结果，用来模拟轮询过程中的状态变化。
        pub fn push_get_jobs(&self, result: Result<Vec<JobStatus>>) {
            self.get_jobs_queue.lock().unwrap().push_back(result);
        }

        /// 按顺序压入“取消作业”的返回结果，用来模拟第一次失败、后续成功等场景。
        pub fn push_cancel(&self, result: Result<()>) {
            self.cancel_queue.lock().unwrap().push_back(result);
        }
    }

    impl SDK for MockSDK {
        /// 建表建 schema 在 mock 中默认为成功，除非测试显式注入错误。
        fn CreateSchemasAndTables(&self, _ctx: &Context) -> Result<()> {
            if let Some(err) = &self.create_err {
                return Err(err.clone());
            }
            Ok(())
        }
        /// 默认返回预置的表列表，让上层可以验证遍历和提交顺序。
        fn GetTableMetas(&self, _ctx: &Context) -> Result<Vec<TableMeta>> {
            if let Some(err) = &self.get_metas_err {
                return Err(err.clone());
            }
            Ok(self.tables.clone())
        }
        /// 生成 SQL 时会优先记录最后一次导入选项，再按“注入错误 > hook > 固定 SQL > 默认模板”决策。
        fn GenerateImportSQL(&self, table: &TableMeta, opts: &ImportOptions) -> Result<String> {
            *self.last_import_opts.lock().unwrap() = Some(opts.clone());
            if let Some(err) = &self.gen_sql_err {
                return Err(err.clone());
            }
            if let Some(hook) = &self.gen_sql_hook {
                return hook(table, opts);
            }
            if let Some(sql) = &self.fixed_sql {
                return Ok(sql.clone());
            }
            Ok(format!(
                "IMPORT INTO `{}`.`{}` FROM '{}' OPTIONS(group_key='{}', format='{}', resource='{}')",
                table.Database,
                table.Table,
                table.WildcardPath,
                opts.GroupKey,
                opts.Format,
                opts.ResourceParameters
            ))
        }
        /// 提交作业会记录最后一次 SQL，并默认生成一个 running 状态的新作业。
        fn SubmitJob(&self, _ctx: &Context, sql: &str) -> Result<i64> {
            *self.last_submit_sql.lock().unwrap() = Some(sql.to_string());
            if let Some(err) = &self.submit_err {
                return Err(err.clone());
            }
            if let Some(id) = self.fixed_job_id {
                return Ok(id);
            }
            let mut id_g = self.next_job_id.lock().unwrap();
            let id = *id_g;
            *id_g += 1;
            self.jobs.lock().unwrap().push(JobStatus {
                JobID: id,
                Status: "running".into(),
                Phase: "importing".into(),
                Step: "import".into(),
                Percent: "0".into(),
                ..Default::default()
            });
            Ok(id)
        }
        /// 查询作业时优先消费结果队列，否则按 group key 过滤现有内存作业。
        fn GetJobsByGroup(&self, _ctx: &Context, group_key: &str) -> Result<Vec<JobStatus>> {
            if let Some(next) = self.get_jobs_queue.lock().unwrap().pop_front() {
                return next;
            }
            let jobs = self.jobs.lock().unwrap();
            Ok(jobs
                .iter()
                .filter(|j| j.GroupKey.is_empty() || j.GroupKey == group_key)
                .cloned()
                .collect())
        }
        /// 取消作业支持三种测试入口：预置队列、显式 hook、按次数重试后成功。
        /// 只有未完成作业会被转成 `cancelled`，以贴近真实后端的幂等表现。
        fn CancelJob(&self, _ctx: &Context, job_id: i64) -> Result<()> {
            if let Some(next) = self.cancel_queue.lock().unwrap().pop_front() {
                if next.is_ok() {
                    let mut jobs = self.jobs.lock().unwrap();
                    for j in jobs.iter_mut() {
                        if j.JobID == job_id && !j.IsCompleted() {
                            j.Status = "cancelled".into();
                        }
                    }
                }
                return next;
            }
            if let Some(hook) = &self.cancel_hook {
                return hook(job_id);
            }
            if let Some(err) = &self.cancel_err {
                let mut attempts = self.cancel_attempts.lock().unwrap();
                let n = attempts.entry(job_id).or_insert(0);
                *n += 1;
                if *n < self.cancel_retry_times {
                    return Err(err.clone());
                }
            }
            let mut jobs = self.jobs.lock().unwrap();
            for j in jobs.iter_mut() {
                if j.JobID == job_id && !j.IsCompleted() {
                    j.Status = "cancelled".into();
                }
            }
            Ok(())
        }
        /// `Close` 先记录关闭状态，再决定是否返回注入错误，方便测试同时观察副作用与错误。
        fn Close(&self) -> Result<()> {
            *self.closed.lock().unwrap() = true;
            if let Some(err) = &self.close_err {
                return Err(err.clone());
            }
            Ok(())
        }
    }

    // 仅用于压住某些编译配置下的未使用警告，不承载运行期语义。
    #[allow(dead_code)]
    type _Unused = StdMutex<()>;

    /// `NewImportSDK` 始终返回 `MockSDK`，并顺序应用所有 option。
    /// 这保证创建流程的调用形状与 Go 相近，但不会真正连接外部系统。
    pub fn NewImportSDK(
        _ctx: &Context,
        _source_dir: &str,
        _db: &super::sql::DB,
        opts: Vec<SDKOption>,
    ) -> Result<Arc<dyn SDK>> {
        let mut sdk = MockSDK::new();
        for opt in opts {
            opt(&mut sdk);
        }
        Ok(Arc::new(sdk))
    }

    /// SQL 生成器接口单独暴露，是为了复用“生成日志用 SQL”这类与提交 SQL 略有差异的场景。
    pub trait SQLGenerator: Send + Sync {
        fn GenerateImportSQL(&self, table: &TableMeta, opts: &ImportOptions) -> Result<String>;
    }

    /// 默认生成器会刻意把敏感资源参数写成 `/*redacted*/`，供日志场景安全展示。
    pub struct DefaultSQLGenerator;

    impl SQLGenerator for DefaultSQLGenerator {
        fn GenerateImportSQL(&self, table: &TableMeta, opts: &ImportOptions) -> Result<String> {
            Ok(format!(
                "IMPORT INTO `{}`.`{}` FROM '{}' /*redacted*/ OPTIONS(group_key='{}')",
                table.Database, table.Table, table.WildcardPath, opts.GroupKey
            ))
        }
    }

    pub fn NewSQLGenerator() -> Box<dyn SQLGenerator> {
        Box::new(DefaultSQLGenerator)
    }
}

// ---------------------------------------------------------------------------
// objstore / s3like / ast / units / mathutil / failpoint
// ---------------------------------------------------------------------------

pub mod objstore {
    /// `objstore` stand-in 只覆盖 importinto checkpoint 文件路径需要的最小本地存储能力。
    /// 这里既不模拟远端对象存储协议，也不处理并发上传、一致性或权限模型。
    /// 唯一重要的契约是：
    /// 目录会在初始化时创建；
    /// 读不存在文件时能返回带 not-found 身份的错误；
    /// 删除不存在文件时可按配置选择忽略。
    use super::{Error, Result, context::Context};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// 按 scheme 判断是否属于 S3Like 存储，用于 external-id 兼容逻辑。
    pub fn IsS3Like(u: &url::Url) -> bool {
        matches!(
            u.scheme(),
            "s3" | "gs" | "gcs" | "azblob" | "azure" | "oss" | "cos"
        )
    }

    /// 归一化查询参数键名，兼容大小写和下划线/连字符差异。
    pub fn NormalizeQueryParameterKey(key: &str) -> String {
        key.to_ascii_lowercase().replace('_', "-")
    }

    /// 本地存储把所有对象路径映射到一个根目录下。
    /// 互斥锁当前主要表达“这里原本是有并发边界的”，并未实现细粒度锁语义。
    #[derive(Debug)]
    pub struct LocalStorage {
        root: PathBuf,
        pub IgnoreEnoentForDelete: bool,
        _lock: Mutex<()>,
    }

    /// 创建根目录并返回本地存储对象。
    pub fn NewLocalStorage(dir: impl AsRef<Path>) -> Result<LocalStorage> {
        let root = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(|e| Error::new(e.to_string()))?;
        Ok(LocalStorage {
            root,
            IgnoreEnoentForDelete: false,
            _lock: Mutex::new(()),
        })
    }

    impl LocalStorage {
        /// 读取文件；若文件不存在，则返回带 `ENOENT` 类名和 `not_found` 标记的错误。
        pub fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
            let path = self.root.join(name);
            match std::fs::read(&path) {
                Ok(b) => Ok(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let mut err = Error::new(e.to_string());
                    err.not_found = true;
                    err.class = Some("ENOENT");
                    Err(err)
                }
                Err(e) => Err(Error::new(e.to_string())),
            }
        }

        /// 写文件前会确保父目录存在，贴近对象存储“先确保容器可写”的体验。
        pub fn WriteFile(&self, _ctx: &Context, name: &str, content: &[u8]) -> Result<()> {
            let path = self.root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::new(e.to_string()))?;
            }
            std::fs::write(path, content).map_err(|e| Error::new(e.to_string()))
        }

        /// 删除文件时可选择忽略不存在错误，供 checkpoint 清理路径保持幂等。
        pub fn DeleteFile(&self, _ctx: &Context, name: &str) -> Result<()> {
            let path = self.root.join(name);
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound && self.IgnoreEnoentForDelete =>
                {
                    Ok(())
                }
                Err(e) => Err(Error::new(e.to_string())),
            }
        }
    }
}

pub mod s3like {
    /// 这个常量代表导入 SQL 资源参数里 external-id 的规范键名。
    /// submitter 在剥离兼容参数时会把归一化后的 key 与它比较。
    pub const S3ExternalID: &str = "external-id";
}

pub mod ast {
    /// 这里只保留 URL 脱敏逻辑，服务“日志里展示导入 SQL 但不泄露凭据”的场景。
    /// 与 Go parser/ast 真正能力相比，这里只是一个面向 URL 查询参数的专用工具。
    pub fn RedactURL(s: &str) -> String {
        // 尽力遮蔽 access-key / secret-access-key / password 一类查询参数值。
        if let Ok(mut u) = url::Url::parse(s) {
            let mut pairs: Vec<(String, String)> = u
                .query_pairs()
                .map(|(k, v)| {
                    let key = k.to_string();
                    let lk = key.to_ascii_lowercase();
                    if lk.contains("secret") || lk.contains("access-key") || lk == "password" {
                        (key, "xxxxxx".into())
                    } else {
                        (key, v.to_string())
                    }
                })
                .collect();
            if !pairs.is_empty() {
                u.set_query(None);
                {
                    let mut qp = u.query_pairs_mut();
                    qp.clear();
                    for (k, v) in pairs.drain(..) {
                        qp.append_pair(&k, &v);
                    }
                }
                return u.to_string();
            }
        }
        s.to_string()
    }
}

pub mod units {
    use super::{Error, Result};

    /// 解析人类可读大小字符串，供配置和阈值相关测试复用。
    /// 这里统一按十进制 1000 进位处理，和当前调用方预期保持一致。
    /// 虽然对 `KiB`/`MiB`/`GiB` 名称也接受，但并不切换为 1024 进位。
    /// 这是一种“名称宽容、结果稳定”的折中实现，避免测试在单位输入上过于脆弱。
    pub fn FromHumanSize(s: &str) -> Result<i64> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::new("empty size"));
        }
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let (num_str, unit) = s.split_at(i);
        let num: f64 = num_str
            .parse()
            .map_err(|_| Error::new(format!("invalid size: {s}")))?;
        let mult = match unit.trim().to_ascii_uppercase().as_str() {
            "" | "B" => 1.0,
            "K" | "KB" | "KIB" => 1000.0,
            "M" | "MB" | "MIB" => 1000.0 * 1000.0,
            "G" | "GB" | "GIB" => 1000.0 * 1000.0 * 1000.0,
            "T" | "TB" | "TIB" => 1000.0 * 1000.0 * 1000.0 * 1000.0,
            other => return Err(Error::new(format!("unknown unit: {other}"))),
        };
        Ok((num * mult) as i64)
    }
}

pub mod mathutil {
    /// `Clamp` 提供与 Go 通用工具相同的边界夹取语义。
    /// 在这里保留它，是为了让上层逻辑无需为简单数值限制再引入额外依赖。
    pub fn Clamp<T: PartialOrd>(v: T, min: T, max: T) -> T {
        if v < min {
            min
        } else if v > max {
            max
        } else {
            v
        }
    }
}

pub mod failpoint {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::Duration;

    /// 这里不实现通用 failpoint 框架，只保留 importinto 测试真正会碰到的一个覆盖点。
    /// 当前唯一受支持的能力是设置“提交后优雅取消超时”的毫秒级覆盖值。
    /// 这样既能保留与 Go 测试近似的开关注入方式，又避免把完整 failpoint 机制搬进 Rust。
    static SUBMIT_GRACE_TIMEOUT_MS: AtomicI64 = AtomicI64::new(0);

    /// 开启与 Go `setSubmitGraceTimeout` 对应的测试覆盖值，例如 `50ms`。
    pub fn Enable_setSubmitGraceTimeout(timeout: Duration) {
        SUBMIT_GRACE_TIMEOUT_MS.store(timeout.as_millis() as i64, Ordering::SeqCst);
    }

    /// 关闭覆盖值，让调用方退回默认超时配置。
    pub fn Disable_setSubmitGraceTimeout() {
        SUBMIT_GRACE_TIMEOUT_MS.store(0, Ordering::SeqCst);
    }

    /// 若存在覆盖值则返回对应持续时间，否则返回 `None` 表示沿用默认逻辑。
    pub fn submit_grace_timeout_override() -> Option<Duration> {
        let ms = SUBMIT_GRACE_TIMEOUT_MS.load(Ordering::SeqCst);
        if ms > 0 {
            Some(Duration::from_millis(ms as u64))
        } else {
            None
        }
    }

    /// 宏本身不执行任何注入逻辑，只保留调用点形状，避免条件编译和测试代码改写过多。
    #[macro_export]
    macro_rules! failpoint_Inject {
        ($name:literal, $body:expr) => {{
            let _ = $name;
            let _ = $body;
        }};
        ($name:literal, $val:ident, $body:expr) => {{
            let _ = $name;
            let _ = stringify!($val);
            let _ = $body;
        }};
    }
}

pub mod io {
    /// `Writer` 只是 `std::io::Write` 的别名 trait，用来贴近 Go `io.Writer` 的参数风格。
    pub trait Writer: std::io::Write {}
    impl<T: std::io::Write> Writer for T {}
}

pub mod os {
    /// 以 `not_found` 标记或 `ENOENT` 类名判断“文件不存在”，对齐 checkpoint 文件路径的错误分支。
    pub fn IsNotExist(err: &super::Error) -> bool {
        err.not_found || err.class == Some("ENOENT")
    }
}

pub mod uuid_util {
    /// 返回一个新的 UUID 字符串，供导入任务生成 group key。
    /// 保留单独模块是为了让调用方继续沿用 Go 侧的工具入口，而非直接依赖具体库。
    pub fn New() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}
