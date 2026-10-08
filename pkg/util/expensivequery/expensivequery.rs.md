# `pkg/util/expensivequery/expensivequery.rs`

## 文件定位

本文件实现昂贵查询与长事务的进程内巡检器，属于独立 crate `astersql-util-expensivequery`；crate 入口 `pkg/util/expensivequery/lib.rs` 公开 `expensivequery` 模块并重新导出其全部公共项。根 workspace 通过 `facade_util_expensivequery` 纳入该 crate，`pkg/lib.rs` 又把它暴露为 `util::expensivequery`。直接的非测试依赖只有 `astersql-statistics-handle-util`，用于读取自动 ANALYZE 进程集合（`Cargo.toml`、`GLOBAL_AUTO_ANALYZE_PROCESS_LIST`）。

该实现负责“观察和处置已由会话层登记的进程”，不负责执行 SQL、建立会话或采集真实日志字段。`SessionManager`、`Histogram`、`EventLogger` 和 `RunawayChecker` 都是本文件定义的适配边界，实际系统必须注入实现后才会产生指标、日志或 kill 行为。

当前迁移状态需要特别区分：workspace facade 和 `pkg/domain/Cargo.toml` 已声明该 crate，但仓库内 Rust 生产代码没有找到 `new_expensive_query_handle`、`Handle::set_session_manager` 或 `Handle::run` 的直接调用。`cmd/tidb-server/main.rs` 中的 PascalCase 调用由同目标的 `stubs.rs` 提供另一套占位 `Handle`，不是本文件的类型。因此本实现目前可独立使用和测试，但尚无证据表明已接入 Rust 服务器主链；Go 主链则已在 `pkg/domain/domain.go` 和 `cmd/tidb-server/main.go` 完整接线。

## 核心职责

- `Handle::run` 每 100 ms 获取一次会话进程快照，分别检查长事务和非空 SQL 查询。
- `Handle::inspect_transaction` 对超过全局事务阈值的事务分类记录 internal/general 直方图，并以 600 秒为间隔限流记录 `expensive_txn`。
- `Handle::inspect_query` 记录超过查询阈值的 `expensive_query`，并依次执行会话 `max_execution_time`、自动 ANALYZE 最大时长和 runaway 规则的 kill 判定。
- `Handle::log_on_query_exceed_mem_quota` 提供异步巡检之外的主动入口，在连接内存超额时按连接 ID 获取进程并记录事件。
- 三个原子全局阈值与日志级别允许运行期更新；会话、指标和日志通过 trait 解耦。

文件不会去重多个 kill 原因：同一轮中若 max execution time、auto analyze 和 runaway 同时成立，`SessionManager::kill` 可以被连续调用。这与 `inspect_query` 中三个顺序且互不排斥的 `if` 分支一致，注入方应保证重复 kill 可安全处理。

## 主要符号

- `EXPENSIVE_QUERY_TIME_THRESHOLD: AtomicU64`：查询阈值，单位秒，默认 60；`run` 在启动时读取，并在每轮扫描末尾刷新。
- `EXPENSIVE_TXN_TIME_THRESHOLD: AtomicU64`：事务阈值，单位秒，默认 60；刷新时机与查询阈值相同。
- `MAX_AUTO_ANALYZE_TIME: AtomicU64`：自动 ANALYZE 最长运行秒数，0 禁用此限制；`inspect_query` 在每次命中自动分析进程时即时读取。
- `LogLevel`、`set_log_level`、`warn_enabled`：使用 `AtomicU8` 实现的进程级 warn 门控。枚举按 `Debug < Info < Warn < Error` 排序，当前级别不高于 Warn 才输出 warn 事件。
- `RunawayChecker`：返回 `(原因, 是否 kill)` 的线程安全策略接口。
- `ProcessInfo`：巡检所需的会话快照，包含连接 ID、SQL、查询/事务起始时刻、内部 SQL 标记、超时、资源组与 runaway 检查器。两个私有 `Mutex<Option<Instant>>` 分别保存查询和事务的最近日志时刻。
- `ProcessInfo::new`：以当前 `Instant` 初始化查询与事务创建时刻，其余治理字段使用未启用值；调用方需要在真实快照中补齐事务、超时、资源组等状态。
- `SessionManager`：列举快照、按 ID 查询和 kill 的上游抽象。kill 参数顺序为 `(connection_id, query, connection, runaway)`。
- `Histogram`、`EventLogger`：指标与事件输出抽象；`NoopHistogram`、`NoopLogger` 是默认无副作用实现。
- `Handle`：持有退出接收端、可后置绑定的会话管理器，以及两个直方图和一个日志器。
- `Handle::new` / `new_expensive_query_handle`：分别是实际构造器和 Go 命名对应的公共便利入口。
- `Handle::with_observers`：按值消费并返回 `Handle`，在启动前替换默认观察器。
- `Handle::set_session_manager`：通过写锁安装管理器并返回 `&Self`，便于链式调用。
- `Handle::run`、`inspect_transaction`、`inspect_query`：后台循环及两类检查的核心实现。
- `log_on_query_exceed_mem_quota`、`log_expensive_query`：主动内存超额入口和统一 warn 转发层。

## 执行流程

1. 调用方创建 `mpsc::channel`，把接收端交给 `Handle::new` 或 `new_expensive_query_handle`；若需要真实可观测性，再通过 `with_observers` 注入两个直方图和日志器，并在 `run` 前调用 `set_session_manager`。
2. `run` 首先克隆会话管理器；未设置时立即以 `expect` panic。它缓存当前查询/事务阈值，并进入退出接收循环。
3. 每次 `recv_timeout(100 ms)` 超时触发巡检；收到任意退出消息或发现发送端断开都会返回。每隔大于 15 秒的一轮把 `need_metrics` 设为真。
4. 对 `show_process_list` 的每个快照，总是先执行 `inspect_transaction`。只有 `current_txn_start_ts != 0` 且事务耗时达到阈值时，才按 `in_restricted_sql` 选择直方图；指标仅在 `need_metrics` 轮记录。warn 开启且距上次事务日志大于 600 秒时记录 `expensive_txn`。
5. 若 `ProcessInfo::info` 为空，查询检查整体跳过，但事务检查仍已完成。否则 `inspect_query` 计算查询耗时：达到阈值且距上次查询日志大于 60 秒时记录 `expensive_query`。
6. 查询检查继续执行三类治理。超过非零 `max_execution_time_millis` 时调用 `kill(id, true, true, false)`；属于 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST` 且超过非零全局上限时调用 `kill(id, true, false, false)`；runaway checker 要求 kill 时调用 `kill(id, true, false, true)`。
7. 一轮扫描结束后重新读取查询和事务阈值，使运行期更新最迟在下一轮生效。
8. 内存配额入口不经过 SQL 非空判断：warn 被禁用时直接返回；管理器尚未安装时输出 bootstrap info；找不到连接时静默返回；找到快照后无论 SQL 文本是否为空都记录 `memory exceeds quota`。该最后一点由 Rust 回归测试 `log_on_query_exceed_mem_quota_keeps_empty_sql_process` 明确覆盖。

## 数据与状态

全局状态由四个原子量组成：三个时长阈值和 `LOG_LEVEL`。阈值采用 Acquire 读取；日志级别以 Release 写入、Acquire 读取。查询/事务阈值在 `run` 内缓存一轮，自动 ANALYZE 阈值则按进程即时读取，因此它们的变更可见时机略有差异。

`Handle` 的退出接收端被 `Mutex` 包装，因为 `mpsc::Receiver` 不是 `Sync`，而 `Handle` 需要被共享引用调用。会话管理器存储为 `RwLock<Option<Arc<dyn SessionManager>>>`，允许 bootstrap 阶段为空并在之后绑定。`run` 启动时只克隆一次 `Arc`；之后再次替换锁内管理器不会改变已经运行循环所用的实例，而 `log_on_query_exceed_mem_quota` 每次都会重新读取当前实例。

日志限流状态属于每个 `ProcessInfo`，而不属于 `Handle`。因此限流是否跨扫描生效，取决于 `SessionManager::show_process_list` 是否为同一活动进程保留/共享同一个 `ProcessInfo`（通常是 `Arc`）；若每轮重建并清空两个时间字段，限流状态也会丢失。`resource_group_name` 当前由本文件保存但不传给 `EventLogger::warn`；runaway 的详细原因通过 `detail` 参数传出。

## 依赖与调用关系

上游公共入口是 `new_expensive_query_handle`、`Handle::set_session_manager`、`Handle::run` 和 `Handle::log_on_query_exceed_mem_quota`。RustCodeGraph 对目标文件识别出 33 个符号；精确查询定位了 `new_expensive_query_handle`、`inspect_transaction` 和 `inspect_query`，但 callers 查询及仓库 `rg` 均未发现目标 snake_case API 的 Rust 生产调用。已确认的装配关系是根 `Cargo.toml` facade、`pkg/lib.rs` 再导出和 `pkg/domain/Cargo.toml` 依赖声明。

`run` 的直接下游依赖为 `mpsc::Receiver::recv_timeout`、`SessionManager::show_process_list`、两个检查函数和全局原子阈值。`inspect_transaction` 下游为 `Histogram::observe` 与 `EventLogger::warn`。`inspect_query` 除日志器和 `SessionManager::kill` 外，还调用 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST.contains`；该集合由 `pkg/statistics/handle/util/auto_analyze_proc_id_generator.rs` 中的 `AutoAnalyzeTracker::track/untrack` 维护。

Go 应用主链提供预期接线证据：`pkg/domain/domain.go` 用 `NewExpensiveQueryHandle(do.exit)` 创建并由 `ExpensiveQueryHandle()` 暴露，`cmd/tidb-server/main.go` 再绑定 server session manager 并以 goroutine 运行。当前 Rust `cmd/tidb-server/main.rs` 的相似调用来自 `cmd/tidb-server/stubs.rs::domain::Domain::ExpensiveQueryHandle`，只能作为待替换的启动占位，不能作为本文件调用者证据。

## 错误处理与边界

本 API 没有 `Result` 返回值。退出通道收到消息或断开被视为正常终止；查询不到连接、阈值未达到、日志级别过高等均为静默跳过。自动 ANALYZE 上限为 0、会话 max execution time 为 0 时分别表示不限制。

`run` 在会话管理器未设置时明确 panic；所有 `Mutex`/`RwLock` 都使用 `unwrap`，锁中毒同样会 panic。注入的 trait 方法也没有错误通道，其 panic 会向调用线程传播。默认 `NoopHistogram` 和 `NoopLogger` 会使检查与 kill 判定之外的可观测事件全部丢弃，所以生产接线若未注入观察器，不能把“循环运行”视为“日志和指标可用”。

阈值为 0 时，任何非负耗时都满足昂贵条件；初始日志时间为 `None`，所以首轮可以立即记录。日志门控仅包围昂贵事件和内存入口：max execution、auto analyze、runaway 三类处置前的 `logger.warn` 不受 `warn_enabled` 限制。空 SQL 会跳过全部 `inspect_query` 逻辑，包括三类 kill，而内存超额入口仍可记录空 SQL 进程。

## 并发与资源生命周期

所有可注入 trait 均要求 `Send + Sync`，共享对象通过 `Arc` 持有。全局阈值和日志级别无锁更新；会话管理器用读写锁保护替换；每个进程的两类日志时间分别用互斥锁保护，避免并发入口重复更新同一限流时间。

`run` 本身不创建线程，调用方必须决定在专用线程还是当前线程执行；它是阻塞循环。由于每次等待都在 `exit` 的 mutex guard 持有期间完成，同一个 `Handle` 不适合并发启动多个 `run`：多个循环会争用单一接收端并串行等待。资源释放由通道控制，发送一个消息只保证一个接收循环退出，而发送端全部销毁会让所有后续接收观察到断开；正常单循环用法下，两者都能停止后台任务。

扫描使用 `Vec<Arc<ProcessInfo>>` 快照。每轮的 `now` 对全部进程一致，保证该轮耗时判断基准一致；但 `show_process_list` 和各 trait 调用均同步执行，慢实现会直接拉长巡检周期。循环没有固定 ticker 对象，实际周期间隔是“100 ms 等待 + 上一轮扫描耗时”。直方图采样也因此是“至少约 15 秒”，不是精确定时。

## 与 Go 版本的对应关系

Rust `Handle`、`new_expensive_query_handle`、`set_session_manager`、`run`、`log_on_query_exceed_mem_quota` 和 `log_expensive_query` 分别对应 Go 的 `Handle`、`NewExpensiveQueryHandle`、`SetSessionManager`、`Run`、`LogOnQueryExceedMemQuota` 和 `logExpensiveQuery`。100 ms 巡检、15 秒指标采样、60 秒查询日志限流、10 分钟事务日志限流、三类 kill 参数以及每轮刷新查询/事务阈值均保持一致。

结构差异主要来自移植边界：Go 直接使用 `sessmgr.ProcessInfo`、全局 metrics、真实 logger、`vardef` 阈值和 `util.GenLogFields`；Rust 在本 crate 内定义精简 `ProcessInfo` 与四个 trait，并以 Noop 观察器为默认值。Rust 的 `log_expensive_query` 总是把事件交给 `EventLogger`，没有复刻 Go `GenLogFields(...) == nil` 时不输出的过滤，也没有组装完整 zap 字段。Rust 把日志时间放在私有 mutex 中，Go 则直接修改 `sessmgr.ProcessInfo` 字段。

测试覆盖也不等价。Go 同目录 `expensivequery_test.go` 只有 `TestMain` 和 goleak 白名单，没有业务断言；Rust `expensivequery_test.rs` 除公共测试初始化外，新增了空 SQL 内存超额仍记录的回归测试。巡检周期、阈值热更新、日志限流、指标分类、所有 kill 分支、退出通道以及重复 kill 目前在同目录 Rust 测试中均未见覆盖。

## 扩展指南

新增处置规则应优先放在 `inspect_query` 或 `inspect_transaction`，并明确它是否应受空 SQL 提前跳过、warn 日志级别以及现有限流锁影响；若会触发 kill，还要规定四个布尔参数和与已有三个分支同时命中时的幂等语义。新增运行期配置要说明是每轮缓存还是每进程读取，并保持单位清晰。

接入真实系统时，应在 domain 的 Rust 实现中持有本文件 `Handle`，服务器装配阶段注入真实 `SessionManager`、`Histogram` 和 `EventLogger`，再由受控后台线程运行；不要把 `cmd/tidb-server/stubs.rs::Handle` 当成可替换的同一类型。还需要安排退出发送端与线程 join，证明关闭过程中不会遗留巡检线程。

修改 `ProcessInfo`、kill 规则或 Go 对齐行为时，同步更新独立测试 `pkg/util/expensivequery/expensivequery_test.rs`，不要把测试嵌入生产源文件。建议补齐可控时钟或可配置 tick 的测试接缝，以稳定覆盖 60/600/15 秒边界；同时用记录型 manager 验证 kill 次数及参数。兼容风险集中在公开 trait/字段签名，性能风险集中在每 100 ms 全量扫描和锁竞争，正确性风险集中在空 SQL 提前返回、重复 kill 以及快照重建导致限流失效。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter pkg/util/expensivequery` 返回目标 Rust/Go 实现、两份测试和 `lib.rs`；`node --file ... --offset 1 --limit 500` 读取了目标文件全部 348 行；`query` 精确定位 `new_expensive_query_handle`、`inspect_transaction`、`inspect_query`。`explore`/部分 callers、callees 调用未返回内容，故调用接线另用精确仓库搜索复核。
- 生产实现：`pkg/util/expensivequery/expensivequery.rs`；crate 与再导出：`pkg/util/expensivequery/Cargo.toml`、`pkg/util/expensivequery/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/domain/Cargo.toml`。
- 下游自动分析状态：`pkg/statistics/handle/util/auto_analyze_proc_id_generator.rs` 的 `GlobalAutoAnalyzeProcessList`、`AutoAnalyzeTracker::track/untrack`。
- Go 对照和真实接线：`pkg/util/expensivequery/expensivequery.go`、`pkg/domain/domain.go`、`cmd/tidb-server/main.go`；Rust 启动占位边界：`cmd/tidb-server/main.rs`、`cmd/tidb-server/stubs.rs`。
- 独立测试：`pkg/util/expensivequery/expensivequery_test.rs`、`pkg/util/expensivequery/expensivequery_test.go`。Rust 测试证明主动内存超额入口保留空 SQL 事件；其余未覆盖项已在上文明确列出，没有以预期设计代替当前事实。
- 本任务只新增说明文档，未运行 Cargo。交付结构检查要求文档存在且恰有上述 11 个固定二级标题；另以 diff 自审确认没有修改 Rust、Go、Cargo 或总计划。
