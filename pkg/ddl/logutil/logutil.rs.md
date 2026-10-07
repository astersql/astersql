# `pkg/ddl/logutil/logutil.rs`

## 文件定位

本文件是 Rust crate `astersql-ddl-logutil` 的核心实现文件，由同目录 `lib.rs` 以 `#[path = "logutil.rs"] pub mod ddl_logutil` 装入并在 crate 根重新导出。它位于 DDL 子系统的可观测性边界：为调用方从全局后台日志器派生带固定 `category` 的日志器，但不创建或执行 DDL job，不读写 DDL 系统表，也不参与 schema state、schema version、回填或回滚流程。

`pkg/ddl/logutil/Cargo.toml` 将 crate 的 Go 对照包声明为 `pkg/ddl/logutil`，直接 Rust 依赖为 `astersql-util-logutil` 和 `chrono`；本文件只使用前者以及标准库的 `OnceLock`、`Duration`，未直接使用 `chrono`。根工作区以 `facade_ddl_logutil` 登记该 crate，`pkg/ddl`、`pkg/ddl/jobsubmit`、`pkg/ddl/ingest`、`pkg/ddl/schemaver`、`pkg/ddl/serverstate`、`pkg/ddl/testutil` 和若干 DDL 测试 crate 在各自 Cargo manifest 中声明依赖。不过，仓库内 Rust 源码对四个公开函数的直接调用目前只出现在 `pkg/ddl/logutil/migration_aster_unit_test.rs`；“manifest 已接入”不等于“生产调用已迁移”。

## 核心职责

- `DDLLogger`、`DDLUpgradingLogger`、`DDLIngestLogger` 从 `BgLogger()` 派生日志器，并分别附加 `category=ddl`、`category=ddl-upgrading`、`category=ddl-ingest`。类别是检索和分流语义，不改变日志级别或 DDL 执行行为。
- `SampleLogger` 提供带 `category=ddl` 的进程内共享采样器，对同一“日志级别 + 消息文本”在 60 秒窗口内只放行前 3 条，防止轮询和重试路径大量重复输出。
- 私有帮助函数 `logger_with_category` 统一普通类别日志器的构造，避免三个公开入口在字段键和值的处理上漂移。
- 文件通过 `#![allow(non_snake_case)]` 保留 Go 风格公开 API 名称，降低 Go 到 Rust 移植时的命名差异。

## 主要符号

- `const DDL_CATEGORY: &str = "ddl"`：普通 DDL 与采样日志器使用的类别值。
- `const DDL_UPGRADING_CATEGORY: &str = "ddl-upgrading"`：集群升级期间暂停、恢复 DDL 等日志的类别值。
- `const DDL_INGEST_CATEGORY: &str = "ddl-ingest"`：fast DDL、ingest 和索引回填相关日志的类别值。
- `fn logger_with_category(category: &str) -> Logger`：调用 `BgLogger().with_fields(...)`，以 `LogFieldCategory` 为键附加一个字符串字段；它是文件内唯一私有函数。
- `pub fn DDLLogger() -> Logger`、`pub fn DDLUpgradingLogger() -> Logger`、`pub fn DDLIngestLogger() -> Logger`：每次调用都从当前全局后台日志器取得一个共享 sink 的克隆，再附加对应固定字段。
- `pub fn SampleLogger() -> Logger`：函数内静态 `OnceLock<Logger>` 惰性保存采样日志器；初始化时调用 `sample_logger_factory(BgLogger(), Duration::from_secs(60), 3, fields)`，随后对保存的 `Logger` 做浅克隆并返回。

本文件没有类型、trait、`impl`、feature gate 或平台条件编译。公开 API 返回的是自有 `Logger` 值，不返回引用，也不暴露内部 `OnceLock`。

## 执行流程

普通类别入口的流程是：调用者选择语义入口 → `logger_with_category` 读取 `BgLogger()` → `Logger::with_fields` 克隆句柄并追加固定 `category` → 调用者再以 `info`、`warn`、`error` 或通用 `log` 写入。底层 `Logger` 的克隆共享 sink 和级别锁，因此字段属于派生句柄，输出目标和级别配置仍来自全局后台日志器。

`SampleLogger` 首次调用时执行 `OnceLock::get_or_init`：取得当时的 `BgLogger()`，附加 `category=ddl` 和底层工厂加入的空 `sampled` 字段，再建立 60 秒、首 3 条的采样状态。之后每次调用只克隆同一个采样日志器。底层 `Logger::log` 以 `(LogLevel, message)` 为计数键；窗口到期时重置计数，未到期且计数已达 3 时直接丢弃该条日志。不同级别或不同消息分别计数。

DDL 完整应用中的位置是“业务路径的输出端”：Go 代码在 `pkg/ddl/job_scheduler.go` 和 `pkg/ddl/jobsubmit/submit.go` 用升级日志器记录暂停/恢复，在 `pkg/ddl/ddl.go`、`pkg/ddl/backfilling.go` 与 `pkg/ddl/ingest/*` 用 ingest 日志器记录 fast DDL 环境和回填，在调度/版本探测的重复告警处用采样日志器。当前 Rust 图和文本搜索未发现对应的生产调用边，因此这些是 Go 对照所证明的目标使用场景，而不是 Rust 已运行的主链。

## 数据与状态

该文件自身只定义三个静态字符串常量和 `SampleLogger` 的函数内 `OnceLock`。普通三个工厂不保存本地状态；返回的 `Logger` 含固定字段，并通过 `Arc` 与全局日志器共享输出 sink 和级别锁。

采样状态位于底层 `Logger` 的 `Arc<Mutex<Sampler>>`：`Sampler` 持有窗口长度、放行上限和按 `(level, message)` 索引的计数表。`sample_logger_factory` 返回的克隆共享这一个 sampler，所以反复调用 `SampleLogger()` 不会绕过配额。采样表属于进程内易失状态，不落盘、不跨进程协调，也不影响 DDL 的持久化状态。

需要注意初始化时序：普通工厂每次重新读取当前 `BgLogger()`；`SampleLogger` 只在首次调用时捕获后台日志器。若首次使用后再用 `InitLogger`/`ReplaceLogger` 替换全局日志器，已缓存的采样日志器仍持有首次捕获的 sink 和级别锁。当前独立测试在调用 `SampleLogger` 前初始化日志器，与这一约束相符。

## 依赖与调用关系

下游调用边如下：`DDLLogger`、`DDLUpgradingLogger`、`DDLIngestLogger` → 私有 `logger_with_category` → `astersql_util_logutil::log::BgLogger` 与 `Logger::with_fields`；`SampleLogger` → `sample_logger_factory` → `Logger::with_fields`、`Logger::sample`。字段类型来自 `LogField`，字段键来自 `LogFieldCategory`，采样窗口来自 `std::time::Duration`。

上游方面，RustCodeGraph 将本文件标为被 `pkg/ddl/logutil/migration_aster_unit_test.rs` 使用；精确 Rust 搜索也只找到该测试对四个公开函数的调用。多个 DDL crate 的 Cargo manifest 声明了 `astersql-ddl-logutil`，但尚无生产 `.rs` 直接调用这些符号。Go 对照的上游则很多：普通 `DDLLogger` 遍及 executor、job worker、schema、table、partition 等路径；升级入口集中在 job scheduler/job submit；ingest 入口集中在 fast DDL/backfill；采样入口位于调度失败和版本探测告警。

`lib.rs` 还重导出 general、slow-query 和底层 log API，并构造 `util::logutil::log` 兼容命名空间；这些是 crate 装配，不是本文件的行为。此文件不直接依赖网络、存储、事务、DDL owner 或异步运行时。

## 错误处理与边界

四个公开函数均不返回 `Result`，构造阶段没有可恢复错误分支。普通日志器构造只克隆内存句柄和追加字段。并发原语若中毒，错误行为来自底层 `Logger` 对锁的 `expect`，会 panic；文件本身不捕获或转换该错误。

采样只保证限制重复输出，不保证消息最终写入。低于日志级别阈值的条目和超过采样上限的条目会被底层直接忽略；文件 sink 打开或写入失败也由底层实现静默放弃。因此不得把这些日志当作 DDL 成功、失败、审计或恢复的唯一事实来源。

边界条件包括：配额按“级别 + 完整消息文本”而非附加字段区分；同一消息携带不同 job ID 字段仍共享配额；`first=3` 与 60 秒窗口是硬编码语义；类别键必须继续使用 `LogFieldCategory`，否则日志过滤兼容性会破坏。该工具不决定某条业务消息应该属于哪个类别，选择错误入口仍会产生合法但误分类的日志。

## 并发与资源生命周期

`OnceLock` 保证采样日志器在并发首次访问时只初始化一次；返回的 `Logger` 克隆共享 `Arc<Mutex<Sampler>>`，因此不同线程看到同一配额。采样计数更新和 sink 写入分别受互斥锁保护，日志级别受读写锁保护。锁只在单次计数或写入范围内持有，本文件没有显式线程、task、channel、异步等待或锁嵌套。

普通日志器是轻量派生句柄，离开作用域后释放自身字段向量和 `Arc` 引用；共享 sink 由其他句柄继续持有。缓存的采样日志器具有进程生命周期，计数表会为遇到的不同“级别 + 消息”保留条目；Go 底层注释明确其实现最多支持 4096 类消息，而当前 Rust `HashMap` 实现未见对应容量限制或淘汰逻辑，这是长时间使用动态消息文本时需要关注的内存差异。

## 与 Go 版本的对应关系

同路径 `logutil.go` 定义相同四个公开函数和三个 category 值。三类普通入口语义直接对应：Go 返回 `*zap.Logger` 并调用 `BgLogger().With(zap.String(...))`，Rust 返回可克隆 `Logger` 并调用 `with_fields`。两者都共享底层日志输出，而不是为每次调用创建独立文件或缓冲区。

Go 在包初始化时创建 `sampleLoggerFactory = SampleLoggerFactory(time.Minute, 3, category)`；工厂内部通过 `sync.Once` 首次构造并始终返回同一 `*zap.Logger`。Rust 将一次初始化折叠到 `SampleLogger` 的 `OnceLock`，并返回共享内部状态的 clone。两边均附加 `sampled` 字段，按相同级别和消息在一分钟内保留前三条。Rust 独立迁移测试还验证不同消息不互相占用额度。

迁移差异必须如实保留：Go 生产调用已广泛接线，Rust 当前只有测试调用；Go zap sampler 文档注释给出最多 4096 类日志，Rust实现使用无显式上限的 `HashMap`；Rust 缓存首次取得的后台日志器，在运行时替换全局日志器后的行为需要单独验证。不能仅凭 API 和 Cargo 依赖一致宣称 DDL 主链迁移完成。

## 扩展指南

新增日志类别时，应在本文件增加语义明确的常量和公开工厂，复用 `logger_with_category`，并在独立测试文件 `pkg/ddl/logutil/migration_aster_unit_test.rs` 增加字段断言；不要把 Rust 测试内嵌到生产源文件。还应同步核对 Go `pkg/ddl/logutil/logutil.go`，除非有明确且记录在案的 Rust 特有需求。

调整采样窗口、上限或计数键属于可观测性兼容变更，应同时修改 `SampleLogger`、DDL 迁移测试以及底层 `pkg/util/logutil/log_test.rs` 的相关预期，并评估告警丢失、日志量和动态消息导致的计数表增长。若要支持全局 logger 热替换，应优先明确 `SampleLogger` 是随替换重建还是保持旧 sink，再修改生命周期设计；简单地移除 `OnceLock` 会让每次调用获得独立 sampler，从而破坏限流。

把 Rust 生产模块接到这些入口时，应按语义选择：通用 DDL 使用 `DDLLogger`，升级暂停/恢复使用 `DDLUpgradingLogger`，ingest/backfill 使用 `DDLIngestLogger`，高频重复告警才使用 `SampleLogger`。扩展不应把日志工具耦合进 job 状态机或用日志替代错误传播。兼容风险主要是 category 值和采样语义，性能风险主要是高频克隆、锁竞争及无界消息键；正确性风险是误分类或在必须保留的事件上错误采样。

## 验证依据

- RustCodeGraph `status`：索引包含目标目录的 `lib.rs`、`logutil.rs`、`logutil.go`、`migration_aster_unit_test.rs`；目标文件共有 6 个索引符号，并显示测试文件为直接使用者。
- RustCodeGraph `node --file`：阅读 `pkg/ddl/logutil/logutil.rs` 全部 84 行、Go 对照 `pkg/ddl/logutil/logutil.go` 全部 49 行、独立测试 `pkg/ddl/logutil/migration_aster_unit_test.rs` 全部 86 行，以及底层 `pkg/util/logutil/log.rs` 的 `Logger`、采样和全局日志器实现。
- RustCodeGraph `query` 及 `callers`/`callees`：精确查询四个公开入口和 `logger_with_category`；图的 callers/callees 子命令未为这些轻量工厂返回边，因此又以 Rust 文本引用搜索核验，未发现生产 `.rs` 调用，只有迁移测试调用。
- 配置与装配：阅读 `pkg/ddl/logutil/Cargo.toml`、`pkg/ddl/logutil/lib.rs`、根 `Cargo.toml` 和各 DDL crate manifest 的依赖声明；阅读 `pkg/ddl/doc.go`，确认本工具只是 DDL 可观测性辅助，不承担在线 DDL 不变量。
- 语义与测试：`pkg/ddl/logutil/migration_aster_unit_test.rs` 验证三类 category 和采样上限/消息隔离；`pkg/util/logutil/log_test.rs::TestSampleLoggerFactory` 与 Go `pkg/util/logutil/log_test.go::TestSampleLoggerFactory` 验证一分钟窗口内 100 次同消息只保留 3 次。按照任务约束，本次纯文档分析未运行 Cargo。
