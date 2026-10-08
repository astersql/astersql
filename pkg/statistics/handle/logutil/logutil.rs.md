# `pkg/statistics/handle/logutil/logutil.rs`

## 文件定位

本文件实现 statistics 子系统的专用日志构造层，位于独立 crate `astersql-statistics-handle-logutil` 中。crate 入口 [`lib.rs`](lib.rs) 以 `stats_logutil` 模块加载本文件，并把其中公开函数重新导出；底层 `Logger`、`LogField`、后台 logger 与详细错误 logger 来自路径依赖 `astersql-util-logutil`（见 [`Cargo.toml`](Cargo.toml)）。因此它不负责格式化、落盘或日志级别过滤，而是在通用日志设施上统一附加 statistics 分类并提供固定采样策略。

当前 Rust 调用链已经接入自动分析相关路径：`pkg/statistics/handle/autoanalyze/exec/exec.rs` 使用 `StatsLogger` 记录版本改写、执行失败、时间窗口解析失败和超窗终止；`pkg/statistics/handle/autoanalyze/priorityqueue/job.rs::IsValidToAnalyze` 使用两个采样入口记录历史查询失败和冷却跳过。RustCodeGraph 还显示同目录迁移测试和 `job_test.rs` 是直接调用者。

## 核心职责

- 用私有辅助函数 `with_stats_category` 给通用 logger 固定追加 `category=stats`，使 statistics 日志可被检索、过滤和归类。
- `StatsLogger` 与 `StatsErrVerboseLogger` 分别选择普通后台日志族和详细错误日志族，同时保持同一分类字段约定。
- `StatsSampleLogger` 和 `StatsErrVerboseSampleLogger` 分别建立 5 分钟、10 分钟的进程级共享采样器；两者阈值均为 1，并附加空字符串形式的 `sampled` 标记。
- 通过返回可克隆的 `Logger` 值，让调用方继续追加 SQL、表名、分区、错误等上下文字段，而采样实例内部的 sink 与计数器仍保持共享。

本文件不决定具体日志消息、级别或业务分支，也不初始化全局日志配置；这些职责分别属于上游调用点和 `pkg/util/logutil/log.rs`。

## 主要符号

- `STATS_CATEGORY: &str = "stats"`：私有常量，是本文件所有非采样和采样 logger 的分类值。
- `with_stats_category(logger: Logger) -> Logger`：私有纯装饰函数，通过 `Logger::with_fields` 追加 `LogField::String(LogFieldCategory, STATS_CATEGORY)`。它被两个基础公开构造函数复用，避免分类键或值漂移。
- `StatsLogger() -> Logger`：公开入口，从 `background_logger()` 派生 statistics logger。RustCodeGraph 的直接生产调用者包括 `execOptionForAnalyzeVersion`、`AutoAnalyze`、`CheckAutoAnalyzeWindow` 和 `KillAutoAnalyzeOutsideWindow`。
- `StatsErrVerboseLogger() -> Logger`：公开入口，从 `err_verbose_logger()` 派生 statistics logger；当前直接生产调用者是 `StatsErrVerboseSampleLogger`，同目录测试还直接检查其字段。
- `StatsSampleLogger() -> Logger`：公开采样入口。函数内静态 `OnceLock<Logger>` 首次调用时构造 `StatsLogger`，追加 `sampled=""`，再调用 `sample(Duration::from_secs(300), 1)`；之后返回共享实例的 clone。
- `StatsErrVerboseSampleLogger() -> Logger`：详细错误版本的公开采样入口，构造过程相同，但基于 `StatsErrVerboseLogger`，窗口为 600 秒。

文件没有公开类型、trait、宏或条件编译项；`#![allow(non_snake_case)]` 是为了保留 Go API 风格的函数名。

## 执行流程

普通日志路径如下：

1. 上游调用 `StatsLogger` 或 `StatsErrVerboseLogger`。
2. 函数取得对应的通用全局 logger clone。
3. `with_stats_category` 在 clone 的固定字段列表中追加 `category=stats` 并返回。
4. 上游可继续调用 `with_fields` 增加业务字段，最终通过 `Logger::log/info/warn/error` 写入共享 sink。

采样日志路径如下：

1. 上游调用一个采样函数；函数访问各自独立的函数内 `OnceLock<Logger>`。
2. 首次调用执行初始化闭包：选择普通或详细错误 statistics logger，追加 `sampled=""`，创建窗口为 300 秒或 600 秒、阈值为 1 的采样器。
3. 后续调用不再创建采样器，只 clone 已缓存的 `Logger`。根据 `pkg/util/logutil/log.rs::Logger` 的实现，clone 会共享 `Arc<Mutex<Sampler>>` 和 sink。
4. 写日志时，底层采样器以 `(LogLevel, message)` 为计数键；同一窗口内同级别、同消息只允许第一条，不同级别或消息独立计数，窗口到期后重置该键的计数。
5. 通过采样的记录同时包含固定的 `category`、`sampled` 字段和调用方追加的业务字段。

`IsValidToAnalyze` 展示了实际使用方式：读取最近失败时长或平均耗时失败会写 `StatsErrVerboseSampleLogger` 的 Warn；冷却条件命中则写 `StatsSampleLogger` 的 Info。这把高频轮询路径的重复消息限制在共享窗口内。

## 数据与状态

本文件自身没有可变业务对象。持久状态仅存在于两个互不共享的 `OnceLock<Logger>`：普通采样 logger 与详细错误采样 logger 各自只初始化一次，并分别维护 5 分钟和 10 分钟的采样计数。

`Logger` 的 clone 语义由 `pkg/util/logutil/log.rs` 定义：固定字段 `Vec<LogField>` 随值克隆，因此调用方追加字段不会反向修改缓存模板；sink、日志级别和可选 sampler 使用 `Arc` 共享。采样器内部是 `HashMap<(LogLevel, String), (Instant, usize)>`，所以状态粒度是日志级别与完整消息文本，而不是调用点或所带字段。由此可见，相同级别和消息即使业务字段不同，也会竞争同一个采样额度；扩展调用点时必须考虑这一点。

`category` 与 `sampled` 都是固定字符串字段。`sampled` 的值为空字符串，它是标记字段而不是采样计数或布尔值。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 确定：直接路径依赖为 `astersql-util-logutil`；清单还声明 `chrono`，但本文件没有使用它。入口 [`lib.rs`](lib.rs) 将依赖 crate 的 `log` 模块再导出，然后以 `pub use stats_logutil::*` 暴露本文件四个公开函数。

下游依赖关系为：

- `StatsLogger -> background_logger -> with_stats_category`；
- `StatsErrVerboseLogger -> err_verbose_logger -> with_stats_category`；
- `StatsSampleLogger -> StatsLogger -> Logger::with_fields -> Logger::sample`；
- `StatsErrVerboseSampleLogger -> StatsErrVerboseLogger -> Logger::with_fields -> Logger::sample`。

RustCodeGraph 记录的生产上游包括 `pkg/statistics/handle/autoanalyze/exec/exec.rs` 与 `pkg/statistics/handle/autoanalyze/priorityqueue/job.rs`。前者消费 `StatsLogger`，后者的 `IsValidToAnalyze` 同时消费两个采样入口。`pkg/statistics/handle/autoanalyze/exec/Cargo.toml`、`priorityqueue/Cargo.toml` 以及父 crate `pkg/statistics/handle/Cargo.toml` 均通过路径依赖接入本 crate。

Go 版本的调用面远大于当前 Rust 图中已接线范围，例如 statistics 的 refresher、DDL、storage、syncload、cache 和 globalstats 都调用同路径 Go API；这说明 Rust API 的设计是迁移兼容边界，但不能据此声称这些 Go 子系统已经全部接入 Rust 实现。

## 错误处理与边界

四个公开函数均不返回 `Result`，本文件也没有显式错误分支：构造 logger 和追加字段是内存操作，`OnceLock::get_or_init` 保证初始化闭包成功返回后才发布实例。

实际写入、级别过滤和锁行为在底层 `Logger` 中发生。`pkg/util/logutil/log.rs` 显示锁中毒会通过 `expect` 触发 panic，文件打开或写入失败则被忽略；这些是通用 logger 的边界，不是本文件单独处理的错误。采样入口也不会报告日志被抑制：超过阈值时底层 `Logger::log` 直接返回。

采样只比较级别和消息，不比较附加字段。因此，不应让不同业务事件长期复用完全相同的级别与消息却仅靠字段区分，否则它们会互相抑制。反之，动态拼接高度离散的消息会生成更多计数键并削弱限流效果；稳定消息加结构化字段是更安全的调用方式。

## 并发与资源生命周期

两个 `OnceLock` 都具有进程级静态生命周期，并发首次调用只会发布一个完整的 logger。每次函数返回的 clone 不拥有独立采样窗口：底层 `Arc<Mutex<Sampler>>` 使所有 clone 串行更新同一计数表，这正是跨调用点限流的基础。sink 使用独立的 `Arc<Mutex<Sink>>`，日志级别使用 `Arc<RwLock<LogLevel>>`，所以 clone 也保持相同输出目标和动态级别状态。

本文件不创建线程、异步任务、通道、事务、文件句柄或显式清理 guard。采样计数表随静态 logger 存活到进程结束；底层实现只在再次遇到某个键且窗口过期时重置该键，没有在本文件中设置容量上限或后台清理任务。Go 的工厂注释写明最多支持 4096 类相同级别/消息日志，但当前 Rust `Sampler` 源码未体现对应硬上限，不能把该限制当作 Rust 已实现能力。

## 与 Go 版本的对应关系

直接对照文件是 [`logutil.go`](logutil.go)。API 名称、返回目的和固定参数逐项对应：

- Go `StatsLogger` 使用 `BgLogger().With(category=stats)`；Rust 使用 `background_logger()` 加同名同值字段。
- Go `StatsErrVerboseLogger` 使用 `ErrVerboseLogger()`；Rust 使用 `err_verbose_logger()`，都用于保留更详细的错误信息。
- Go `sampleLoggerFactory` 使用 5 分钟、首条阈值 1；Rust `StatsSampleLogger` 使用 300 秒和 1。
- Go `sampleErrVerboseLoggerFactory` 使用 10 分钟、首条阈值 1；Rust详细错误采样入口使用 600 秒和 1。
- Go 工厂通过闭包内 `sync.Once` 复用单个 zap logger；Rust通过函数内 `OnceLock<Logger>` 复用单个模板，并借助 `Arc` 保持共享采样状态。

两侧都附加 `sampled=""` 标记并按级别、消息采样。需要保留的已知差异是：Go 使用 zap sampler，且其工厂注释声明最多 4096 类日志；Rust 使用自有 `HashMap` 采样器，当前源码没有对应容量限制。此外 Rust 返回拥有所有权的可克隆值，Go 返回 `*zap.Logger` 指针，但两者都让调用方链式追加字段而不改写全局模板。

同目录没有 Go `*_test.go`；Go 采样工厂的行为由 `pkg/util/logutil/log_test.go::TestSampleLoggerFactory` 验证。Rust 的同目录独立测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 验证四个入口的分类字段、采样字段、共享实例效果和“窗口内只保留第一条”。

## 扩展指南

- 新增 statistics logger 变体时，优先复用 `with_stats_category`，不要在多个函数中手写分类键值；如果是采样变体，应明确基础日志族、窗口、阈值和是否需要 `sampled` 字段。
- 修改采样窗口、阈值、计数键语义或详细错误基础 logger 时，必须先与 [`logutil.go`](logutil.go) 及 `pkg/util/logutil/log.go` 的工厂语义核对，避免破坏 Go/Rust 对齐。
- 新增或修改本 crate 行为时，应在同目录独立测试文件 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 中补充回归；不要把测试内嵌到 `logutil.rs`。若变更影响冷却分支的真实消费方式，还应同步 `pkg/statistics/handle/autoanalyze/priorityqueue/job_test.rs`。
- 为新的高频调用点选择消息时，应保持消息稳定、把维度放入结构化字段，并检查是否会与现有相同级别/消息共享采样额度。
- 若要对齐 Go 的 4096 类日志容量边界，修改位置在通用 `pkg/util/logutil/log.rs::Sampler`，不是本文件；这属于跨 crate 行为变化，需要通用 logger 的独立测试，不能只改本文件绕过。
- 若只是接入新的 statistics 子系统调用点，消费本 crate 的公开函数并在调用方 Cargo manifest 增加路径依赖即可；不要复制 logger 构造逻辑。

兼容性风险主要是字段名/值、采样窗口与基础日志族变化会改变日志检索和告警可见性；性能风险主要是动态消息扩大采样键集合，以及大量并发写入竞争共享 sampler mutex。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标目录的 `logutil.rs` 被识别为 6 个符号，并被 4 个文件使用。
- RustCodeGraph `node` 证据：`StatsLogger` 的生产调用者为 `execOptionForAnalyzeVersion`、`AutoAnalyze`、`CheckAutoAnalyzeWindow`、`KillAutoAnalyzeOutsideWindow`；`StatsSampleLogger` 与 `StatsErrVerboseSampleLogger` 的生产调用者包含 `IsValidToAnalyze`；两个基础入口都调用 `with_stats_category`，两个采样入口分别调用对应基础入口。
- 已读实现与装配：[`logutil.rs`](logutil.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、`pkg/util/logutil/log.rs`。
- 已读 Go 对照与基础测试：[`logutil.go`](logutil.go)、`pkg/util/logutil/log.go`、`pkg/util/logutil/log_test.go::TestSampleLoggerFactory`。
- 已读 Rust 独立测试和真实调用测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、`pkg/statistics/handle/autoanalyze/priorityqueue/job_test.rs::cooldown_matches_go_branches_and_query_order`，并核对 `pkg/statistics/handle/autoanalyze/exec/exec.rs` 与 `priorityqueue/job.rs::IsValidToAnalyze`。
- 目标目录不存在 `doc.go`，因此没有可补读的 package contract；本说明以实际 Rust/Go 源码、Cargo 清单、调用边和独立测试为准。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行固定 11 章节结构检查及文档适用的仓库 Ready 检查。
