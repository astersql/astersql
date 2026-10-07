# `pkg/domain/plan_replayer.rs` 逻辑说明

## 文件定位

`pkg/domain/plan_replayer.rs` 属于 `astersql-domain` crate，并由 `pkg/domain/lib.rs` 以 `pub mod plan_replayer` 公开。它描述 Plan Replayer 的领域侧控制面：dump 文件过期清理、capture 任务收集、有界投递、并发去重，以及将真实 dump 行为隔离在后端 trait 之后。Plan Replayer dump 的具体内容处理另在相邻模块 `pkg/domain/plan_replayer_dump.rs`；本文件本身不解析或生成完整 dump 包。

`pkg/domain/Cargo.toml` 指定 crate 入口为 `pkg/domain/lib.rs`。当前可编译实现直接使用的跨 crate 依赖只有 `astersql-kv`（为收集查询打内部请求标记）和 `astersql-sessionctx-vardef`（读取普通 dump 保留期），其余主体依赖 Rust 标准库。文件中没有 feature 或条件编译项。

需要特别区分文件的两层内容：第 15 至 601 行是一段注释化的 Go 机械映射，保存了尚未移植的完整 worker、状态表 SQL 和 dump 文件生成流程，不能视为 Rust API 或运行时代码；真正参与编译的实现从 `PlanReplayerTaskKey`（第 609 行）开始。RustCodeGraph 对实际符号未找到生产调用者；目前除 `pkg/domain/lib.rs` 的模块导出外，直接调用均位于 `pkg/domain/plan_replayer_test.rs`、`pkg/domain/plan_replayer_handle_test.rs` 和 `pkg/domain/plan_replayer_slow_log_test.rs`。因此该文件当前是可测试的领域组件集合，尚未接入 Rust `Domain` 主生命周期。

## 核心职责

本文件的可执行实现承担四组职责：

1. `parse_dump_time` 从 dump 文件 basename 的最后一个下划线与最后一个点之间解析有符号纳秒时间戳，供 GC 判断创建时间。
2. `DumpFileGcChecker` 串行扫描注册目录，对普通 dump 与 `replayer` + `capture` 文件应用不同保留期，并以 best-effort 方式删除文件和对应状态。
3. `PlanReplayerTaskCollector` 维护已注册且尚未处理的 `(sql_digest, plan_digest)` 集合；`collect_plan_replayer_tasks` 通过 `PlanReplayerTaskSource` 执行带内部请求标记的查询，并在全部查询成功后一次性替换内存快照。
4. `PlanReplayerHandle`、`PlanReplayerDumpTaskStatus` 和 `handle_dump_task` 组成最小投递/处理协议：查询路径非阻塞投递，worker 侧按 key 抢占，持续 capture 成功后写入 finished 集合，真实 dump 委托给 `PlanReplayerDumper`。

它不负责 Rust 生产环境中的 worker 线程创建、channel 接收循环、真实外部存储适配、`mysql.plan_replayer_status` 的插入 SQL、dump 文件创建、指标或日志。上述能力只出现在前半部注释映射和 Go 对照 `pkg/domain/plan_replayer.go` 中，或应由 trait 实现及外围生命周期代码提供。

## 主要符号

- `PlanReplayerTaskKey`：公开、可排序/哈希的任务键，由 `sql_digest` 与 `plan_digest` 共同标识任务。`BTreeSet` 依赖其 `Ord`，也使测试和返回顺序稳定。
- `PlanReplayerDumpTask`：公开任务载荷，保存统计信息、事务时间戳、binding、编码计划、会话变量、SQL 语句、历史统计时间戳、debug trace、文件元数据及 capture 标志。与 Go 的强类型对象相比，若干字段被简化为 `String` 或键值列表。
- `PlanReplayerStatusRecord`：公开 dump 结果记录；`failed_reason` 为空代表成功，但本文件不负责持久化该记录。
- `parse_dump_time(&str) -> Result<SystemTime, String>`：公开纯函数，支持 epoch 前的负纳秒时间。
- `DumpFileStore`：公开 GC 边界 trait，抽象 `list`、`delete` 与 `delete_status`。
- `DumpFileGcChecker`：公开 GC 协调器；`new` 注册目录，`gc_with_current_retention` 组合当前系统变量与固定七天 capture 保留期，`gc` 执行可注入时间/租约的核心算法。
- `PlanReplayerTaskSource`：公开收集边界 trait；`registered_tasks` 接收固定收集 SQL，`is_unhandled` 判断单个 key 是否仍需处理。两者都必须使用传入的内部请求上下文。
- `COLLECT_PLAN_REPLAYER_TASK_SQL`：公开固定 SQL，查询 `mysql.plan_replayer_task` 的两个 digest 字段。
- `PlanReplayerTaskCollector`：公开内存集合；`collect_plan_replayer_tasks` 负责查询，`collect` 负责纯内存过滤/替换，`tasks` 返回副本，`remove` 删除已投递 key。
- `PlanReplayerDumpTaskStatus`：公开运行态集合；`occupy`/`release` 管理 running，`was_finished`/`set_finished`/`clean_finished` 管理持续 capture 的 finished 状态，两个长度方法供观测和测试。
- `PlanReplayerHandle`：公开收集/投递门面；`new` 返回句柄与独立 receiver，`collector` 返回共享收集器，`send_task` 尝试非阻塞发送，`close` 丢弃 sender 以使 receiver 最终断开。
- `PlanReplayerDumper`：公开 dump 后端 trait，接收可变任务并返回状态记录或字符串错误。
- `handle_dump_task`：公开单任务处理函数，组合 finished 检查、running 抢占、dumper 调用、释放与成功标记。

这些实际定义均无 `cfg` 分支；测试模块只在 `pkg/domain/lib.rs` 中以 `#[cfg(test)]` 装配。

## 执行流程

### 文件 GC

1. 外围调用 `DumpFileGcChecker::gc_with_current_retention` 时，普通文件保留期来自 `astersql_sessionctx_vardef::GetPlanReplayerFileRetentionTime()`，capture 保留期固定为七天；也可直接调用 `gc` 注入两类租约。
2. `gc` 获取实例级 `Mutex<()>`，保证同一 checker 的多个 GC pass 不并发执行。
3. 对每个 `paths` 项调用 `DumpFileStore::list`。列举失败只跳过该目录，不影响后续目录。
4. 对每个路径调用 `parse_dump_time`；不可解析文件被忽略。basename 同时用于类型判断和 status token。
5. basename 同时包含 `replayer` 与 `capture` 时使用 capture cutoff，其他所有文件使用 default cutoff。创建时间小于或等于 cutoff 即过期。
6. 文件删除失败只跳过当前文件。删除成功且 basename 包含 `replayer` 时，再 best-effort 调用 `delete_status`；状态删除失败不撤销文件删除，也不令整个 GC 失败。
7. 返回值只包含实际删除成功的文件路径。

### 任务收集与投递

1. `collect_plan_replayer_tasks` 创建 `astersql_kv::Context::todo()`，再通过 `WithInternalSourceType(..., InternalTxnStatsForegroundPriority)` 标记为内部查询。
2. `registered_tasks` 接收同一 context 和 `COLLECT_PLAN_REPLAYER_TASK_SQL`；随后每个 key 都用该 context 调用 `is_unhandled`。
3. 任一查询报错时立即返回，旧 `tasks` 快照保持不变。全部成功后调用 `collect`，用新结果原子替换集合；`BTreeSet` 同时完成去重。
4. `PlanReplayerHandle::new(capacity)` 创建 `sync_channel(capacity.max(1))`，所以传入零容量不会得到 rendezvous channel，而会被提升为容量一。
5. `send_task` 在 sender 锁内执行 `try_send`。channel 满或已关闭时返回 `false`；成功时，普通任务从 collector 删除，持续 capture 任务保留以便后续匹配。

### 单任务处理

1. `handle_dump_task` 对持续 capture 先查 finished；已完成则直接返回 `false`。所有任务随后尝试 `occupy`，同 key 已在运行时也返回 `false`。
2. 抢占成功后调用 `PlanReplayerDumper::dump`。
3. 无论 dump 成功还是返回 `Err`，函数都会在同步返回路径上调用 `release`，避免错误后残留 running key。
4. 仅当 dump 成功且任务是持续 capture 时调用 `set_finished`；最后以 `result.is_ok()` 表示处理成功。

## 数据与状态

`PlanReplayerTaskCollector.tasks`、`PlanReplayerDumpTaskStatus.running` 与 `finished` 都是 `RwLock<BTreeSet<PlanReplayerTaskKey>>`。collector 是待处理任务的快照；running 是跨 worker 的瞬时互斥集合；finished 是持续 capture 的进程内成功缓存。三者互不替代：普通任务投递成功即从 collector 移除，running 在每次处理结束时释放，而 finished 只为持续 capture 保留。

`PlanReplayerHandle.collector` 使用 `Arc`，让收集端与查询投递端共享同一集合；`sender` 使用 `Mutex<Option<SyncSender<_>>>`，其中 `Some` 表示开放、`None` 表示已关闭。receiver 的所有权交给调用方，本文件不保存 worker 列表或 join handle。

`PlanReplayerDumpTask` 的 `Default` 会把所有数字置零、字符串/列表置空且标志置 `false`。其中 `historical_stats_ts == 0` 的“使用最新统计”含义来自字段注释；实际解释由 dumper 后端负责。`PlanReplayerStatusRecord` 只作为返回数据，不在 `handle_dump_task` 中落库。

GC 没有持久化自身进度。它以文件名纳秒字段作为时间依据，不读取文件元数据；截止时间使用 `checked_sub`，当 `now - lease` 下溢时退化为 `UNIX_EPOCH`。因此 epoch 前的文件名虽然可以解析，但在常规非负 `now` 下会满足过期条件。

## 依赖与调用关系

可执行 Rust 调用关系如下：

- `pkg/domain/lib.rs` → `pub mod plan_replayer`：crate 级公开入口。
- `DumpFileGcChecker::gc_with_current_retention` → `GetPlanReplayerFileRetentionTime` → `DumpFileGcChecker::gc`。
- `DumpFileGcChecker::gc` → `DumpFileStore::{list, delete, delete_status}`，并内部调用 `parse_dump_time`。
- `PlanReplayerTaskCollector::collect_plan_replayer_tasks` → `astersql_kv::{Context::todo, WithInternalSourceType}` → `PlanReplayerTaskSource::{registered_tasks, is_unhandled}` → `collect`。
- `PlanReplayerHandle::send_task` → `mpsc::SyncSender::try_send`；成功的普通任务再调用 `PlanReplayerTaskCollector::remove`。
- `handle_dump_task` → `PlanReplayerDumpTaskStatus::{was_finished, occupy, release, set_finished}` → `PlanReplayerDumper::dump`。

RustCodeGraph 对 `collect_plan_replayer_tasks`、`handle_dump_task` 等关键入口没有发现生产调用边；仓库 `rg` 也只定位到三个 Rust 测试文件。与之相对，Go 生产链明确为：`pkg/domain/domain.go` 的 `SetupPlanReplayerHandle` 创建 collector、channel、共享 status 与 workers，`StartPlanReplayerHandle` 周期调用 `CollectPlanReplayerTask` 并启动 worker，`DumpFileGcCheckerLoop` 周期调用 `GCDumpFiles`；查询执行路径再通过 Go 的 `SendTask` 投递。不能把这条 Go 调用链描述成当前 Rust 已接线事实。

## 错误处理与边界

- `parse_dump_time` 对非 UTF-8 basename、缺少 `_`、缺少或位置错误的 `.`、非 `i64` 数字和 `SystemTime` 溢出均返回带原文件名的字符串错误。它只认最后一个 `_` 和最后一个 `.`，允许路径输入和负时间戳。
- GC 将目录列举失败、单文件解析失败、文件删除失败和 status 删除失败都视为局部故障；公共 `Result` 目前只有 GC 互斥锁 poisoned 时返回 `Err`。这种 best-effort 行为与 Go 的记录日志后继续一致，但 Rust 当前没有日志接口，故故障会被静默跳过。
- 创建时间恰好等于 cutoff 时会删除（`created <= cutoff`），与 Go 的 `!createTime.After(cutoff)` 对齐。
- `collect_plan_replayer_tasks` 在任一 source 调用失败时不执行最终替换，防止半份快照覆盖旧状态。`collect` 自身会过滤 handled 并借助集合去重。
- 所有 `RwLock`/sender 锁在 poisoned 时通过 `expect` panic，而非返回可恢复错误。
- `send_task` 把 `Full` 与 `Disconnected` 都折叠为 `false`，且为调用 `try_send` 克隆整个 `PlanReplayerDumpTask`；大载荷会有复制成本。关闭后再次发送也只是 `false`。
- `handle_dump_task` 只对 dumper 的普通 `Result` 错误保证释放 running key；若 dumper panic，当前函数没有 RAII guard，`release` 不会执行，key 会滞留。Go worker 的 `handleTask` 有 recover 边界，而 Rust 当前没有等价实现。
- `handle_dump_task` 对“已 finished”与“正在运行”都返回 `false`，调用方无法仅凭布尔值区分跳过、竞争失败与 dump 失败。
- trait 本身不验证 status 记录，也不持久化返回值；真实后端必须补足文件关闭、状态写入、失败清理和可观测性。

## 并发与资源生命周期

GC 锁覆盖所有目录和文件的完整扫描，避免同一 checker 内两次 pass 交错，但也意味着慢存储操作会阻塞下一次 GC。`DumpFileStore` 未要求 `Send`/`Sync`，并发策略由调用方和具体实现决定。

collector/status 以细粒度 `RwLock` 保护集合。`occupy` 的“检查并插入”由一次写锁操作完成，保证同 key 只有一个同步调用进入 dumper；不同 key 可越过状态检查，但本函数本身不创建线程。`finished` 与 `running` 使用独立锁，不存在嵌套持锁顺序。

`PlanReplayerHandle` 的有界同步 channel 为查询路径提供背压上限，但 `try_send` 明确选择丢弃而非等待。`close` 只移除该句柄持有的 sender；当所有任务克隆和其他 sender 都释放后，receiver 才观察到断开。当前类型没有 `Drop`、worker 启停、join 或 drain 逻辑，receiver 生命周期由 `new` 的调用方管理。

任务进入 channel 时按值克隆，发送者后续修改原任务不会影响接收端。处理函数借用接收端任务为 `&mut`，允许 dumper 回填 `file_name`、URL 等字段；这些修改不会自动回传给发送者。成功返回的 `PlanReplayerStatusRecord` 在函数结束后立即被丢弃，除非 dumper 自身完成持久化或外围以后扩展消费逻辑。

## 与 Go 版本的对应关系

Rust 的 `parse_dump_time` 对应 Go `parseTime`；`DumpFileGcChecker::{gc_with_current_retention,gc}` 对应 `dumpFileGcChecker.GCDumpFiles/gcDumpFilesByPath`；`PlanReplayerTaskCollector` 对应 `planReplayerTaskCollectorHandle`；`PlanReplayerHandle::send_task` 对应 `planReplayerHandle.SendTask`；`PlanReplayerDumpTaskStatus` 和 `handle_dump_task` 对应 `planReplayerDumpTaskStatus` 与 worker 的 `handleTask/HandleTask` 去重骨架。

已对齐的关键语义包括：普通/capture 双租约、无法解析或删除时继续、删除 replayer 文件后 best-effort 清 status、仅成功投递的普通任务从 collector 移除、channel 满不阻塞查询、running key 防并发、持续 capture 成功后进入 finished，以及收集 SQL使用内部请求标记。

当前 Rust 与 Go 仍有实质差异：

- Go `Domain` 会构造并启动 collector ticker、多个 worker 和 GC ticker；Rust `pkg/domain/domain.rs` 没有对本文件类型的接线，RustCodeGraph 也没有生产调用者。
- Go worker 在处理前查询 `mysql.plan_replayer_status`，生成外部存储文件，调用 `DumpPlanReplayerInfo` 并写 status；Rust 将真实动作压缩到 `PlanReplayerDumper` trait，且不消费其返回记录。
- Go GC 删除 replayer 文件后清空共享 finished 缓存；Rust `DumpFileGcChecker` 不持有 `PlanReplayerDumpTaskStatus`，因此 `delete_status` 后不会自动 `clean_finished`。
- Go 记录指标、warning/debug 日志并在 worker 边界 recover；Rust 当前没有相应指标/日志/recover。
- Go `PlanReplayerDumpTask` 保存 `Binding`、AST、`SessionVars`、writer 和任意统计/debug 对象；Rust 以字符串/列表简化，不能直接承载完整生产 dump 语义。
- Go channel 容量和 worker 数由 Domain 常量/配置决定；Rust `new` 只返回 receiver，不创建 worker，且把容量零提升为一。

因此该 Rust 文件是 Go 行为的局部、可测试移植，而不是完整替代。文件前半段注释不能用于弥补这些运行时缺口。

## 扩展指南

若要扩展 GC 文件分类或保留策略，应优先修改 `DumpFileGcChecker::{gc_with_current_retention,gc}`，保持“单目录/单文件故障不阻断其他对象”和 cutoff 包含边界，并同步 `pkg/domain/plan_replayer_test.rs`、`pkg/domain/plan_replayer_handle_test.rs`。若新增文件名格式，必须同步 `parse_dump_time` 的 basename/最后分隔符契约及 Go `TestDumpGCFileParseTime` 覆盖的格式。

若要接入真实任务表，应实现 `PlanReplayerTaskSource`，确保两个方法都原样使用传入的 `astersql_kv::Context`，并继续用参数化/受控 SQL 边界处理 digest；收集错误不得覆盖旧快照。内部查询的慢日志与 statement summary 行为应同步 `pkg/domain/plan_replayer_slow_log_test.rs`。

若要扩展投递结果，建议把 `send_task` 和 `handle_dump_task` 的布尔值替换或包裹为可区分 `Full`、`Closed`、`AlreadyFinished`、`AlreadyRunning` 与后端错误的结果类型，同时审视兼容调用方。大任务载荷场景应避免当前整任务 clone，或通过 `Arc`/所有权转移降低性能成本。

若要达到 Go 生产能力，最可能修改的入口是 `PlanReplayerHandle`（保存/管理 worker 生命周期）、`PlanReplayerDumper`（真实文件生成与状态落库）、`handle_dump_task`（panic-safe release 与结果持久化）、`DumpFileGcChecker`（与 finished 状态联动）以及 Rust `pkg/domain/domain.rs`（初始化、ticker、关闭顺序）。这属于跨文件接线，不能只靠本文件前半段注释实现。需要同步新增独立 Rust 测试，不能把测试嵌入生产文件。

兼容性风险集中在 Go SQL 表语义、持续 capture 的 `plan_digest == "*"` 行为、status token 与文件 basename 的一致性，以及内部查询标记。正确性风险包括 dumper panic 后 running 泄漏、GC 删除文件但未清 finished、关闭时未 drain。性能风险包括全程持有 GC mutex、逐 key 串行 `is_unhandled`、任务深克隆及单个 sender mutex。

## 验证依据

事实核对使用了以下路径和符号：

- Rust 实现：`pkg/domain/plan_replayer.rs` 的 `PlanReplayerTaskKey`、`PlanReplayerDumpTask`、`parse_dump_time`、`DumpFileGcChecker`、`PlanReplayerTaskCollector`、`PlanReplayerDumpTaskStatus`、`PlanReplayerHandle`、`PlanReplayerDumper`、`handle_dump_task`。
- crate 边界：`pkg/domain/Cargo.toml` 的 `[lib] path = "lib.rs"`、`astersql-kv` 与 `astersql-sessionctx-vardef` 依赖；`pkg/domain/lib.rs` 的 `pub mod plan_replayer` 和三个独立测试模块声明。`pkg/domain/doc.go` 不存在。
- Rust 测试：`pkg/domain/plan_replayer_test.rs` 验证正/负纳秒、非法名、双租约和 GC 容错；`pkg/domain/plan_replayer_handle_test.rs` 验证任务过滤/去重、普通与持续 capture 投递、finished/running 生命周期、失败释放、GC status 清理和原 SQL 保真；`pkg/domain/plan_replayer_slow_log_test.rs` 验证内部请求类型、slow log 标记与 statement summary 过滤。
- Go 对照：`pkg/domain/plan_replayer.go`；`pkg/domain/domain.go` 的 `SetupPlanReplayerHandle`、`StartPlanReplayerHandle`、`SetupDumpFileGCChecker`、`DumpFileGcCheckerLoop`；`pkg/domain/plan_replayer_test.go`、`pkg/domain/plan_replayer_handle_test.go`、`pkg/domain/plan_replayer_slow_log_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/domain/plan_replayer.rs --offset 1 --limit 500` 与 `--offset 500 --limit 600` 读取完整目标；`query collect_plan_replayer_tasks`、`query PlanReplayerHandle`、`query send_task`、`query handle_dump_task`、`query gc_with_current_retention` 核对主要符号。图查询没有给出实际 Rust 生产调用边，仓库直接引用检索也仅发现上述测试。

本任务是纯文档分析，按任务约束未运行 Cargo。结构验证应确认本文件存在且恰好包含任务规定的十一个二级章节；人工复核还应确认所有“当前支持”陈述只描述第 602 行以后的可编译 Rust 实现，没有把注释化机械映射或 Go 生命周期误写为 Rust 现状。
