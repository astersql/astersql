# `br/cmd/br/cmd.rs`

## 文件定位

`cmd.rs` 是 `astersql-br-cmd-br` crate 的 BR 命令公共运行时层，而不是备份、恢复或日志备份的业务实现。crate 由 `br/cmd/br/Cargo.toml` 定义为同时提供 `lib.rs` 和 `bin_main.rs` 的二进制包；`lib.rs` 将本文件公开为 `cmd` 模块，`bin_main.rs` 再经库入口进入 `main.rs`。根入口 `entry::main` 创建 `br` 根命令后调用 `DefineCommonFlags` 和 `SetDefaultContext`，具体子命令随后复用本文件的初始化、日志、内存、状态服务、过滤器、上下文与 tracing 辅助逻辑。

本文件直接对照 `br/cmd/br/cmd.go`。它处于命令树和 `astersql-br-pkg-task` 任务层之间：上游是 `main.rs` 以及 `backup.rs`、`restore.rs`、`stream.rs`、`debug.rs`、`operator.rs`、`abort.rs` 等命令模块；下游主要是 `astersql_br_pkg_task` 的公共 flags/TLS/参数日志能力、`astersql_br_pkg_trace`，以及 `stubs.rs` 暴露的迁移期 CLI、日志、内存、TLS、status mux 和 glue 接口。Cargo 注释明确该 crate 面向 arm64 Darwin 的瘦 BR 依赖布局，部分 TiDB/PD/TiKV/CLI 能力由本地 trait/stub 承接，因此不能把这里的调用形状等同于所有外部服务均已接入真实实现。

## 核心职责

1. 用 `DefineCommonFlags` 为 BR 根命令挂载版本、日志、脱敏、status/TLS 及任务层公共参数，并隐藏兼容参数。
2. 用 `Init`/`init_inner` 只执行一次进程级初始化：配置两套日志、初始化内存 hook、按需设置内存上限和监控、设置脱敏规则，最后启动 status/pprof 服务。
3. 保存跨子命令共享的进程状态：默认 `Context`、是否使用日志文件、唯一 `TidbGlue`，以及按命令 ID 注册的 status server 准备器。
4. 提供 Go 对齐的数据库过滤规则、内存预留公式、日志文件命名和可选 tracing 生命周期包装。
5. 把通用机制留在命令层：实际备份/恢复工作仍由各子命令解析配置后交给 `astersql-br-pkg-task` 等 crate；本文件不读取备份数据，也不调度恢复任务。

## 主要符号

- 进程级状态：`INIT_ONCE: Once` 保证 `Init` 的闭包最多运行一次；`DEFAULT_CONTEXT: Mutex<Option<Context>>` 保存 CLI 默认上下文；`HAS_LOG_FILE: AtomicU64` 发布日志文件状态；`TIDB_GLUE: OnceLock<Mutex<TidbGlue>>` 惰性创建共享 glue；`STATUS_PREPARERS: Mutex<Vec<(u64, StatusServerPreparer)>>` 按 `Command.id` 保存附加 status handler 的准备函数。
- flags 与环境变量：`FlagLogLevel`、`FlagLogFile`、`FlagLogFormat`、`FlagStatusAddr`、`FlagSlowLogFile`、`FlagRedactLog`、`FlagRedactInfoLog` 是公共参数名；`envLogToTermKey` 允许 `BR_LOG_TO_TERM` 强制终端日志；`envBRHeapDumpDir`/`defaultHeapDumpDir` 决定 heap dump 目录。`flagVersionShort` 当前只被保留并显式忽略，因为 stub 的 `TaskFlagSet` 没有 Go `BoolP` 的短参数实现。
- 内存常量：`quarterGiB`、`halfGiB`、`fourGiB` 分别为 256 MiB、512 MiB 和 4 GiB，是最小工作集与预留公式的边界。
- glue/过滤器：`tidbGlue() -> &'static Mutex<TidbGlue>` 返回共享实例；`filterOutSysAndMemKeepAuthAndBind()` 默认先接受全部对象，再排除临时库、系统库及大部分 `mysql` 对象，同时重新纳入权限、角色和 bind 表；`acceptAllTables()` 仅返回 `*.*`；`setTiDBGlueDBFilter(DBFilter)` 替换过滤器并返回一次性恢复闭包。
- status 扩展点：`registerStatusServerPreparer` 对同一命令 ID 执行覆盖式注册；`prepareStatusServer` 在释放注册表锁后调用克隆出的 preparer，并以 `Ok(None)` 表示无扩展 handler；`startStatusServer` 组装 TLS 与默认 mux，再挂载可选 registrar。
- 初始化入口：`DefineCommonFlags(&mut Command)` 定义公共参数；`calculateMemoryLimit(u64) -> u64` 计算 Go 对齐的可用上限；`setupMemoryMonitoring(&Context, total, used)` 设置 runtime 上限并启动监控；`Init(&mut Command)` 是公开的一次性入口，`init_inner` 是实际步骤实现。
- 共享查询/辅助：`HasLogFile` 原子读取日志状态；`SetDefaultContext`/`GetDefaultContext` 设置、克隆默认上下文，未设置时回退 `Context::Background()`；`log_arguments_for` 统一调用任务层 `LogArguments`；`with_tracing` 在开启时包裹闭包并保证正常或错误返回后执行 `TracerFinishSpan`；`reset_init_for_test` 仅清理可变副作用，不能重置 `Once`。

## 执行流程

根流程从 `bin_main.rs::main` 到 `lib.rs::main`，再到 `main.rs::main`。后者创建可取消上下文和根 `Command`，调用 `DefineCommonFlags`、`SetDefaultContext`，注册六组一级命令并执行命令树。RustCodeGraph 显示 `Init` 的直接 Rust 调用者是 `NewBackupCommand`、`NewDebugCommand`、`NewStreamCommand`；这些模块把它放在命令的公共前置阶段，而 restore/abort 等路径通过父命令或共享配置消费本模块状态。

`DefineCommonFlags` 先写入构建版本，并把 `version` 放在当前命令的 local flags；然后向 persistent flags 加入日志、脱敏、status 参数，调用任务 crate 的 `TaskDefineCommonFlags` 补齐 PD/TLS 等通用项，最后加入并隐藏 slow-log 与旧脱敏参数。`parity_test.rs::version_flag_is_local_like_go_cobra_command` 固定了 version 不是 persistent flag 的约束。

首次调用 `Init` 时，`init_inner` 按以下顺序运行：

1. 通过 `effective_task_flags` 读取当前命令的有效 flags；根据 slow-log 参数决定为 TiDB 风格日志创建专用文件，或关闭全局 slow log，然后调用 `logutil::InitLogger`。
2. 读取 BR 日志级别、文件和格式；若存在 `BR_LOG_TO_TERM`，清空文件名。文件名非空时原子设置 `HAS_LOG_FILE`、启用 summary collector，并把日志位置打印到 stderr。
3. 初始化并替换 BR 全局 logger，再调用 `memory::InitMemoryHook`。
4. 只有 `debug_runtime::SetMemoryLimit(-1)` 表明此前没有显式限制时，才读取总内存与已用内存并调用 `setupMemoryMonitoring`。后者计算剩余内存，取 `calculateMemoryLimit` 与 256 MiB 的较大值；上限能放入 `i64` 时写入 runtime，随后从环境变量或默认目录启动内存监控。
5. 合并新旧脱敏开关，调用 `redact::InitRedact`，最后进入 `startStatusServer`。

`startStatusServer` 先执行命令专属 preparer，再从 flags 解析 status 地址和 TLS 三元组，创建包含默认 handler 的 `ServeMux` 并追加 registrar。地址非空时固定监听并返回启动结果；地址为空时启动动态 pprof listener 后返回成功。`operator.rs::newCRRCheckpointCommand` 是扩展路径实例：它注册 preparer，preparer 创建服务、把共享状态放进命令 context，并返回仅负责挂载路由的 registrar。

业务执行阶段，`backup.rs` 等模块读取 `HasLogFile` 决定 `LogProgress`，读取 `GetDefaultContext` 继承取消状态，并用 `with_tracing` 包裹任务调用。逻辑备份还通过 `setTiDBGlueDBFilter` 临时替换全局过滤器，并把返回闭包放入 Drop 守卫以在成功或错误退出时恢复。

## 数据与状态

所有长期状态都是进程级而非单次命令级。`INIT_ONCE` 让 logger、内存 hook、脱敏和监听器初始化无法重复；若首次 `init_inner` 返回错误，`Once` 也已经消费，后续 `Init` 不会重试。这一点与 Go `sync.Once` 包裹具名错误变量的行为形状一致，扩展时不应假定失败可重入。

`HAS_LOG_FILE` 使用 `SeqCst` 读写，使各子命令观察到统一的日志状态。它只在选择非空日志文件时置为 1，正常生产路径不清零。`DEFAULT_CONTEXT` 保存 `Context` 的克隆；测试证明克隆共享取消标志，所以父级取消能被后续获取的上下文观察到。未注入上下文时 Rust 版显式回退后台上下文，而 Go 版变量默认值是 `nil`，正常根入口会在命令执行前设置它。

`TIDB_GLUE` 中的 `InfoSchemaFilter` 是可变全局状态。`setTiDBGlueDBFilter` 在持锁时保存旧值并写入新值，但返回的恢复动作必须由调用方执行；当前 `backup.rs::scopeguard_restore_filter` 将其包装为 Drop，避免普通错误路径泄漏状态。`STATUS_PREPARERS` 使用命令的数值 ID 而非对象地址作为键；再次注册会替换旧 preparer，`cmd_test.rs` 对此有直接断言。取出 preparer 时先克隆 `Arc` 再释放表锁，从而避免用户回调在注册表锁内执行。

过滤规则每次返回新的 `Vec<String>`，调用方修改自己的列表不会污染其他命令。`with_tracing` 的泛型结果不保存全局 span；开启时创建 trace store，执行闭包，然后结束 span并原样返回闭包结果。

## 依赖与调用关系

- crate 边界：`br/cmd/br/Cargo.toml` 直接依赖 `astersql-br-pkg-task`、`astersql-br-pkg-task-operator`、`astersql-br-pkg-trace`、streamhelper 配置及少量通用库；本文件直接使用前述 task 和 trace crate，其余 CLI/runtime 类型来自 `crate::stubs::*`。
- 上游装配：`main.rs::main -> DefineCommonFlags/SetDefaultContext`；RustCodeGraph 查询得到 `NewBackupCommand`、`NewDebugCommand`、`NewStreamCommand -> Init`，并得到 backup/stream/restore/debug/abort/operator 对 `GetDefaultContext`、`HasLogFile`、过滤器或 status 注册接口的调用。
- 日志链：`DefineCommonFlags -> timestampLogFileName`；`Init -> init_inner -> logutil::InitLogger/log::InitLogger/log::ReplaceGlobals`；`log_arguments_for -> astersql_br_pkg_task::LogArguments`。日志文件存在状态继续流向各任务配置的 `LogProgress`。
- 内存链：`init_inner -> memory::MemTotal/MemUsed -> setupMemoryMonitoring -> calculateMemoryLimit -> debug_runtime::SetMemoryLimit/utils::RunMemoryMonitor`。监控启动失败在 `setupMemoryMonitoring` 中作为错误返回，但 `init_inner` 捕获后只记录，不中断初始化。
- status 链：`operator.rs::newCRRCheckpointCommand -> registerStatusServerPreparer`；`init_inner -> startStatusServer -> prepareStatusServer`；随后 `NewTLS`、`RegisterDefaultStatusHandlers` 和固定地址或动态 pprof listener 完成服务装配。
- glue 链：`tidbGlue` 被 backup、restore、debug、operator 等模块共享；RustCodeGraph 明确 `backup.rs::runBackupCommand -> setTiDBGlueDBFilter`，返回的恢复动作由相邻 scopeguard 管理。
- 测试链：`cmd_test.rs` 覆盖时间戳布局和 status preparer 覆盖；`main_test.rs` 复刻 Go 内存表值；`parity_test.rs` 覆盖过滤器、公共 flags、内存边界、默认上下文取消传播、`HasLogFile` API 与 tracing/资源清理契约。测试均由 `lib.rs` 以独立 `*_test.rs` 模块接入，没有内嵌在生产文件中。

## 错误处理与边界

flags 读取、logger 初始化、内存 hook、TLS 解析、TLS 构造、固定 status listener 启动以及 preparer 的错误均通过 `Result` 传播；`Init` 将首次初始化错误包装为 `Error::Trace`。内存监控是刻意的软失败：内存读取或 hook 初始化仍会中断，但 `RunMemoryMonitor` 的失败在 `init_inner` 只写错误日志。`memUsed >= memTotal` 被视为无法可靠估算，记录警告并跳过限制；`calculateMemoryLimit(0)` 返回 0，极小输入在预留量不小于剩余量时返回全部剩余量以避免无符号下溢；最终监控上限仍会至少提升到 256 MiB。计算值达到或超过 `i64::MAX` 时不调用 runtime 限制 API。

`timestampLogFileName` 假定 trace 模块返回的文件名以 `br.trace.` 开头且是 UTF-8；违反约定会触发 `expect` panic。所有标准库 `Mutex` 均用 `unwrap`，锁中毒也会 panic。`setTiDBGlueDBFilter` 不是自恢复 API：调用方若丢弃恢复闭包而不执行，会留下全局过滤器。`with_tracing` 能覆盖闭包返回 `Err` 的普通路径，但闭包 panic 时没有 RAII guard，`TracerFinishSpan` 不会执行。

status 地址为空并不代表完全禁用诊断监听，而是调用 `StartDynamicPProfListener`；固定地址路径则直接返回 listener 的结果。额外 registrar 总是在默认 handlers 之后追加。没有 preparer 是正常情况而非错误。由于 preparer 以命令 ID 关联，复制或重建命令时必须确认 ID 语义，不能用命令名字猜测关联。

`reset_init_for_test` 不会也不能恢复 `INIT_ONCE`，只清除日志标记、summary、脱敏、配置与 preparer；需要再次覆盖初始化细节的测试只能调用直接 helper 或隔离进程。生产扩展不应调用该函数。

## 并发与资源生命周期

`Once`、`OnceLock`、`Mutex` 和原子变量使共享对象可跨命令回调访问。`tidbGlue` 的每次读写都要求持锁；调用下游任务时若长期持有该锁，其他需要 glue 的路径会串行等待，因此新增代码不应在持锁期间反向调用可能再次获取 `tidbGlue()` 的函数。过滤器恢复闭包在执行时也会重新加锁，绝不能在仍持有 glue guard 时调用。

status preparer 注册表的锁只保护查找/替换，不覆盖 preparer 本身的执行。这样既缩短临界区，也避免 preparer 注册路由或访问其他共享状态时形成自锁。`Arc<StatusServerPreparer>`/`Arc<StatusServerRegistrar>` 允许闭包跨阶段共享；CRR checkpoint preparer 还把服务状态放进 `Command` context，运行完成后由其命令逻辑消费一次 cleanup。

内存监控和 status/pprof listener 由公共工具函数启动，当前接口没有在本文件保存 join handle 或 stop handle，生命周期因此随进程或下游实现管理。logger、redact、summary 和 runtime memory limit 同样是全局副作用，不以单条命令结束为界回滚。默认 context 通过克隆共享取消状态，根入口的退出监听器负责触发取消。

`with_tracing` 的资源顺序是 start span、执行业务闭包、finish span；普通成功和 `Err` 都会 finish。逻辑备份的过滤器恢复与 GC tuner 恢复由 `backup.rs` 的 Drop 守卫完成，不属于本文件自动保证的资源语义。

## 与 Go 版本的对应关系

Rust 的常量、过滤规则、flags、内存公式、日志/status 初始化顺序主要逐项对应 `br/cmd/br/cmd.go`。`main_test.go::TestCalculateMemoryLimit` 的六个精确表值被 `main_test.rs::test_calculate_memory_limit` 原样复刻；`parity_test.rs` 还验证 4 GiB 拐点、极小/大内存性质、过滤器内容和 context 取消传播。

存在以下实现层差异，扩展时应明确保留或补齐而不是假设完全等价：

- Go 使用包级 `tidbGlue` 与 `sync.Map`，Rust 使用 `OnceLock<Mutex<TidbGlue>>` 和 `Mutex<Vec<(id, preparer)>>`，从而显式串行化访问并按 `Command.id` 关联。
- Go 的过滤器是包级切片，Rust 函数每次构造新向量；Go 的过滤器类型是 `func(ast.CIStr) bool`，Rust 迁移接口使用 `DBFilter`/`InfoSchemaFilter` stub。
- Go `DefineCommonFlags` 为 version 提供 `-V`，日志级别提供 `-L`；Rust 注释说明当前 `TaskFlagSet` 缺少对应 short-form API，`flagVersionShort` 仅保留，不能声称短参数已实现。
- Go `defaultContext` 未设置时返回零值 `nil`；Rust `GetDefaultContext` 回退 `Context::Background`。正常 `main` 路径都会先设置上下文。
- Go 直接在 `Init` 的 `sync.Once.Do` 闭包内实现全部步骤；Rust 拆出 `init_inner` 以便组织和测试，但仍保持一次性副作用。Rust 另有 `reset_init_for_test`、`log_arguments_for` 和 `with_tracing` 辅助，这些是迁移层为相邻模块集中出来的接口，并非 `cmd.go` 中同名函数。
- Rust 的时间戳文件名复用 `astersql_br_pkg_trace::timestampTraceFileName` 再替换前缀；`cmd_test.rs` 验证 Go 的 `br.log.2006-01-02T15.04.05Z0700` 布局。

## 扩展指南

- 新增所有 BR 子命令共享的参数，应修改 `DefineCommonFlags`，先判断参数应是 local 还是 persistent，并同步任务层 flags（若 SQL/BRIE 也需要）、Go `cmd.go` 以及 `parity_test.rs` 的参数契约。若需要短参数，先补齐真实 flag API，不能只保留常量。
- 新增初始化步骤，应放到 `init_inner` 的依赖顺序中，并明确失败是硬失败还是像内存监控一样降级；同时考虑 `Once` 首次失败后不可重试、全局副作用不可回滚，以及测试间污染。测试应新增到独立的 `cmd_test.rs`、`main_test.rs` 或 `parity_test.rs`，不要写进 `cmd.rs`。
- 新增 status 路由时，普通公共路由可进入 `RegisterDefaultStatusHandlers` 的实现；命令专属路由应调用 `registerStatusServerPreparer` 并返回 registrar。若 preparer 创建资源，应像 `operator.rs` 的 CRR checkpoint 路径一样把状态与 cleanup 的所有权传给运行阶段。
- 修改内存公式时必须同步 `calculateMemoryLimit`、Go 实现及两侧精确表值测试，并复核 0、极小值、4 GiB 除法拐点、超大值和 `u64`/`i64` 转换。性能风险主要是过低上限引发频繁 GC/过早 OOM，或过高上限挤压宿主机。
- 新增共享 glue 操作时要缩小锁范围，避免重入 `tidbGlue()`；临时改写必须返回或创建 RAII 恢复守卫，并为成功、错误及 panic/提前返回路径增加独立测试。兼容风险是全局过滤规则泄漏到后续子命令。
- 修改 tracing 时建议将 finish 资源改为明确的 Drop guard 后再依赖 panic 安全；现状只保证普通返回。修改 listener/monitor 生命周期时，需要为启动失败和退出清理设计可观察的 handle，而不能假设本文件已经持有它们。
- 修改过滤列表时同步检查 backup/restore/abort 的 `DefineFilterFlags` 调用点，并在 `parity_test.rs::contract_normal_command_tree_and_filters` 固定系统库排除与权限/bind 表保留意图。

## 验证依据

- 目标源码：RustCodeGraph `node --file br/cmd/br/cmd.rs` 读取了 432 行全貌；`explore "DefineCommonFlags setupMemoryMonitoring Init ctlInit prepareStatusServer setTiDBGlueDBFilter in br/cmd/br/cmd.rs"` 和针对公共 helper 的 callers 查询确认主要调用边。
- crate/入口：读取 `br/cmd/br/Cargo.toml`、RustCodeGraph 节点 `br/cmd/br/lib.rs`、`main.rs`、`bin_main.rs`，确认库/二进制边界、模块导出和根命令启动顺序。
- Go 对照：读取 `br/cmd/br/cmd.go` 全文及 `br/cmd/br/main_test.go::TestCalculateMemoryLimit`，核对 flags、初始化、内存、status、context 和精确测试值。
- Rust 测试：读取 `br/cmd/br/cmd_test.rs`、`main_test.rs` 的内存测试、`parity_test.rs` 的命令/过滤器/边界/context 契约；读取 `operator.rs` 的 CRR status preparer 和 `backup.rs` 的调用、tracing、过滤器恢复路径。
- RustCodeGraph 的关键结果包括：`Init` 由 backup/debug/stream 命令入口调用；`setTiDBGlueDBFilter` 由逻辑备份使用；`prepareStatusServer` 由 `startStatusServer` 和独立测试调用；`calculateMemoryLimit` 由监控逻辑及两组 Rust 测试调用；`GetDefaultContext`、`HasLogFile`、`with_tracing` 被多个业务命令消费。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证文档存在且恰好包含这 11 个固定二级标题；事实准确性另由上述源码、调用图、Cargo、Go 对照和独立测试人工复核。
