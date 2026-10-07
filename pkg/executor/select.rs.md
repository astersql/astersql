# `pkg/executor/select.rs`

## 文件定位

`select.rs` 属于 `astersql-executor` crate 的 SELECT 执行层。crate 根在 `pkg/executor/Cargo.toml` 中把库入口指定为 `lib.rs`，而 `pkg/executor/lib.rs:192` 以 `pub mod select` 暴露本模块。文件不是 SQL 解析或物理计划构造入口；它把 SELECT 阶段常见的拉取式执行算法、悲观锁处理以及语句上下文辅助操作表达为可由具体会话/存储适配器实现的 Rust 泛型代码。

当前生产接线必须与“源码已存在”区分：仓库内可确认的 Rust 生产调用是 `pkg/executor/physical_plan_runtime.rs:45,91` 调用 `ExecuteLimitValues`，把已经解码的物理计划行应用 LIMIT/OFFSET。`SelectLockExec`、`SelectionExec`、`TableDualExec`、`TableScanExec`、`MaxOneRowExec` 和语句上下文包装函数虽有完整控制流，但没有在本文件中绑定真实 session/chunk/table/TiKV 类型；除测试外也未检索到这些泛型执行器的 Rust 构造点。因此它们目前是可适配的算法边界，而不是已证明接入全部 Rust SQL 主链的具体执行器。

直接 Go 对照为 `pkg/executor/select.go`。Rust 模块保留 Go 风格的类型和方法名以方便逐项迁移，但以 `SelectRuntime`/`LimitRuntime` 将 Go 中的 `sessionctx`、`chunk`、`table`、事务和诊断副作用抽象出去。

## 核心职责

- 管理三类全局资源追踪器：`GLOBAL_MEMORY`、`GLOBAL_DISK`、`GLOBAL_ANALYZE_MEMORY`，以及通过 `globalPanicOnExceed::Action` 将不同资源超限映射为固定 panic 文案（`select.rs:31-141`）。`init` 只做一次性 `OnceLock` 初始化；`Start`/`Stop` 只维护本文件的后端启动标志。
- 通过 `SelectLockExec` 在子执行器 EOF 前累计行键，在 EOF 时过滤临时表键、LOCK TABLES 白名单外键和 untouched-index 键，检查执行期限，再一次性发起悲观锁请求（`select.rs:327-448`）。
- 通过 `LimitExec` 实现 `LIMIT offset, count`：跨 chunk 跳过 OFFSET，控制对子节点的请求容量，截断越过终点的批次，并管理可选 `AdaptiveLimitController` 的 Reset/Stop 生命周期（`select.rs:546-662`）。
- 通过 `ExecuteLimitValues` 为已解码的 `Vec<T>` 提供真实可调用的 LIMIT 适配层；它构造单子节点运行时并严格执行 Open/Next/Close（`select.rs:664-811`）。
- 提供固定空行源 `TableDualExec`、WHERE/HAVING 过滤器 `SelectionExec`、一次性物化信息表的 `TableScanExec`，以及保证标量子查询至多一行的 `MaxOneRowExec`（`select.rs:813-1042`）。
- 将 StatementContext 重置、UPDATE/DELETE 专用重置、TopSQL 快照选项、弱一致读判定暴露为运行时转发函数，并提供锁键稳定去重与物理表列映射别名（`select.rs:1044-1096`）。

## 主要符号

- `ResourceKind` / `ResourceTracker`：资源类别与原子计数/限额容器。`limit < 0` 表示无限制；公开读取使用 Acquire，初始化写入使用 Release。
- `init`、`Start`、`Stop`：初始化全局 tracker 和切换 `BACKEND_STARTED`。`OnceLock::set` 的结果被忽略，重复 `init` 不替换已存在的 tracker，但仍把桶大小重置为 1024。
- `globalPanicOnExceed`：用 `Mutex<()>` 串行化超限动作；`Action` 永不返回，`GetPriority` 返回 `i64::MAX`。
- `dataSourceExecutor`：仅要求返回底层物理表的极小 trait；本文件没有它的实现或调用点。
- `SelectRuntime`：本文件的主要生产边界，关联 `Context`、`Chunk`、`Row`、`Key`、`Table`、四种语句/快照类型和 `Error`，再以必选方法承载 chunk、子执行器、行锁、过滤、信息表扫描及语句上下文动作。`allow_shared_lock_upgrade` 和 `lock_table_filter_enabled` 默认关闭，其余外部动作没有成功占位默认值。
- `SelectLockExec<R>`：持有运行时和跨批次 `keys`；`Open` 清空键并打开唯一子节点，`Next` 在 EOF 时统一加锁。
- `LockContext<K>` / `LockMode`：记录键、等待时长、共享/排他模式及共享锁升级等 TiKV 锁参数。`newLockCtx` 负责从运行时解析等待时长并填充它。
- `LimitRuntime`：从完整 `SelectRuntime` 中切出的 LIMIT 最小能力集；`impl<R: SelectRuntime> LimitRuntime for R` 使完整运行时自动可用于 LIMIT。
- `LimitExec<R>`：状态为 `[begin, end)`、`cursor`、`meet_first_batch` 和可选控制器。公开的 `Open` 转发到 `open`，`Close` 停止控制器后关闭子节点。
- `LimitValueRuntime<T>` / `LimitValueChunk<T>` / `ExecuteLimitValues`：本文件私有的内存行源及其公开入口，也是已确认的 Rust 生产调用路径。
- `TableDualExec<R>`：输出 `num_dual_rows` 个全 NULL 行；每批量受输出 chunk 容量限制。
- `selectionExecutorContext<C>` / `newSelectionExecutorContext`：只保存调用者传入的 session；与 Go 版本包含 tracker、表达式上下文和向量化开关的结构不同。
- `SelectionExec<R>`：维护输入 chunk、选择位图和非批模式游标；批模式整批输出，非批模式填满请求或读到 EOF。
- `TableScanExec<R>`：缓存 `table_scan_all` 返回的全部虚表 chunk，然后逐批交换到调用方。
- `MaxOneRowExec<R>`：标量子查询基数保护器；只评估一次，零行合成 NULL 行，一行时额外探测下一批，多行返回运行时提供的错误。
- `StatementContextRuntime` / `ResetContextOfStmt`：把纯语句重置能力从完整执行能力中分离，使只参与规划的 session 不必实现无关 chunk/锁操作。
- `deduplicateLockKeys`：用 `HashSet` 原地保留第一次出现的键，因此既去重又保持首次出现顺序。
- `PhysicalTableColumnMap<C>`：`HashMap<i64, C>` 的语义别名，本文件不直接消费它。

## 执行流程

`SelectLockExec` 的生命周期为：

1. `Open` 清空上次执行留下的 `keys` 并打开 child 0。
2. 每次 `Next` 先清空输出并从 child 拉一批。未启用 select lock 时直接返回数据。
3. 非空批次只从 chunk 提取键并追加到跨批缓冲，同时把行返回上层；此时不请求锁。
4. 读到空批表示 EOF。无键则结束；否则依次调用 `filterTemporaryTableKeys`、`filterLockTableKeys`，再删除 untouched-index 键。
5. `checkMaxExecutionTimeExceeded` 在建锁上下文前检查一次；`newLockCtx` 解析等待配置和锁模式；`doLockKeys` 在真正调用 `pessimistic_lock` 前再次检查期限，避免准备锁期间越过截止时间。
6. 失败且属于死锁时记录诊断；成功时记录实际提交的键数。注意本流程本身不调用 `deduplicateLockKeys`，具体适配器或上游若需要去重必须显式接入。

`LimitExec` 的生命周期为：

1. `open` 重置自适应控制器、游标和 `meet_first_batch`，再打开唯一子节点。
2. 若 `cursor >= end`，`Next` 立即停止控制器并返回 EOF。
3. OFFSET 尚未跨过时，按 `adjustRequiredRows` 建立临时输入 chunk。整批仍落在 OFFSET 内就丢弃并继续；首个跨过 OFFSET 的批次用选择位图复制 `[skip, skip+take)`。
4. 进入有效窗口后直接拉取到请求 chunk；若本批越过 `end` 则截断。EOF 或到达终点都会停止控制器。
5. `Close` 再次安全地 Stop，并关闭 child 0。`ExecuteLimitValues` 即使 Next 失败也调用 Close，并按“执行错误优先，否则关闭错误”的方式返回。

其他执行器：`TableDualExec::Next` 按容量逐批补 NULL；`SelectionExec::open` 分配输入缓冲，`Next` 根据 `selection_batched` 选择整批过滤或 `unBatchedNext` 的游标续跑；`TableScanExec` 首次 Next 才物化虚表全部 chunk；`MaxOneRowExec::Next` 首次调用完成基数验证，此后总是返回空批。

## 数据与状态

- 全局状态由三个 `OnceLock<Arc<ResourceTracker>>`、`BACKEND_STARTED: AtomicBool` 和 `CheckTableFastBucketSize: AtomicI64` 构成。`ResourceTracker` 的 `consumed`/`limit` 在本文件仅初始化和读取，没有消费、回收或修改限额的 API；不能据此声称资源追踪已完整接入。
- `SelectLockExec.keys` 跨多个非空 chunk 存活，直到 EOF 才消费。一次 Open 会清空它，避免跨语句泄漏；但本类型没有 `Close`，关闭语义完全依赖具体接线或所有权释放。
- `LimitExec.cursor` 统计已从逻辑输入窗口推进的位置；所有加法大量使用 `saturating_add/sub`，避免 OFFSET/COUNT 或批量计算溢出。构造者仍应保证 `end` 表示预期的 `offset + count`；`ExecuteLimitValues` 在 usize 域饱和后再转 u64。
- `SelectionExec.input` 只在 Open 后存在，Close 时置空。未 Open 直接 Next 会因 `expect("SelectionExec opened")` panic，这是生命周期前置条件而不是可恢复错误。
- `TableScanExec.virtual_table_chunks` 持有一次扫描的全部结果，`virtual_table_chunk_index` 指向下一批。`Open` 清缓存并把索引归零；大信息表可能产生与总结果规模成比例的内存占用。
- `MaxOneRowExec.evaluated` 保证一次执行最多产生一批；Open 将其复位。
- `LockContext` 的键 Vec 会被 `SelectLockExec` 克隆一次后交给运行时；大结果集在 EOF 加锁阶段存在额外峰值内存。

## 依赖与调用关系

上游与装配证据：

- `pkg/executor/lib.rs:192` 声明公开 `select` 模块，测试则由同文件 `mod select_test` 独立装配，符合源文件与测试分离约束。
- `pkg/executor/physical_plan_runtime.rs:45,91` 是已确认的 Rust 生产调用边：`execute_limit_rows` 调用 `ExecuteLimitValues`，并把字符串错误包装为 `PhysicalRuntimeError`。
- `pkg/executor/select_test.rs` 直接构造 `SelectLockExec`、调用 `ExecuteLimitValues`、`newLockCtx`、`filterLockTableKeys` 和 `deduplicateLockKeys`；这些是测试调用边，不应当作生产接线证据。
- RustCodeGraph 对目标文件给出的文件级关系为 “used by 1 file: `pkg/executor/distsql.rs`”；但精确符号检索未找到该文件直接引用本模块符号。更可靠的文本证据显示本模块反向依赖 `astersql_executor_internal_exec::adaptive_limit_controller::AdaptiveLimitController`，而 `pkg/executor/distsql.rs` 管理与 Limit 共享的控制器语义。因此该文件级边只能作为模块关联线索，不能写成直接函数调用。

下游依赖：

- 标准库：`HashMap`/`HashSet`、`Arc`/`Mutex`/`OnceLock`、原子类型以及 `Duration`/`Instant`。
- crate 依赖：`AdaptiveLimitController` 来自 `astersql-executor-internal-exec`；`pkg/executor/Cargo.toml` 将该内部 crate 声明为路径依赖。目标文件没有直接引入具体 session、kv、table 或 chunk crate，因为这些能力全部经运行时 trait 注入。
- 运行时调用方向为执行器算法 -> `SelectRuntime`/`LimitRuntime` 实现。具体实现负责子执行器 Open/Next/Close、向量化表达式求值、TiKV 悲观锁、死锁记录、表扫描和语句状态重置。

Go 生产上游更完整：`pkg/executor/builder.go` 会构造 `SelectLockExec`、`LimitExec`、`SelectionExec`、`TableDualExec`、`TableScanExec`、`MaxOneRowExec`；这只能证明 Go 主链位置，不能替代 Rust 接线证据。

## 错误处理与边界

- 绝大多数外部失败以 `R::Error` 原样 `?` 传播，包括子节点打开/读取/关闭、锁等待转换、键提取、悲观锁、过滤表达式、虚表扫描和语句重置。
- `globalPanicOnExceed::Action` 有意 panic；互斥锁中毒也以 `expect` panic。资源类别 `Unknown` 使用独立兜底文案。
- `SelectLockExec` 在 EOF 前不会尝试锁；因此子节点中途报错时已累计键不会提交。死锁只额外记录诊断，不吞掉原错误。
- 期限在 `newLockCtx` 前和 `pessimistic_lock` 前各检查一次，边界为 `Instant::now() >= deadline`。等待锁期间如何响应超时由具体运行时负责。
- `newLockCtx` 固定 `return_values`、`check_existence`、`lock_only_if_exists` 为 false；若具体 TiKV 路径需要这些能力，应扩展 trait/构造参数并增加对照测试，而非在适配器中静默猜测。
- `ExecuteLimitValues` 明确拒绝 `max_chunk_size == 0`，防止零容量导致无法推进；私有运行时还检查 child 必须为 0 且已经 Open。
- `LimitExec` 允许 `begin >= end`，表现为空结果；计数与偏移使用饱和算术。其通用构造字段公开，因此调用者仍需避免把语义错误的 end 传入。
- `TableDualExec` 的 Rust 版本允许任意 `num_dual_rows`，而 Go 注释限定为 0 或 1；当前 Rust 算法会按多批次输出该数量，属于需由构造者约束的语义差异。
- `SelectionExec` 的两个 `expect` 和 `MaxOneRowExec` 的生命周期都假设 Open-before-Next；未按协议调用可能 panic 或产生错误状态。
- Rust `ResetContextOfStmt` 只是运行时转发，不包含 Go 版本的 panic 恢复、tracker 重绑、SQL mode/statement flags、prepared statement、TopSQL 等具体重置逻辑；能力是否完整取决于实现者。

## 并发与资源生命周期

- `ResourceTracker` 使用原子字段，可被 `Arc` 跨线程共享；Acquire/Release 保证读取初始化后的值。`OnceLock` 保障 tracker 只被设置一次。`BACKEND_STARTED` 仅是原子标志，本文件未启动线程、任务或后台循环。
- `globalPanicOnExceed` 的 mutex 只把超限动作串行化；锁 guard 会随 panic 展开而释放，但随后 mutex 可能中毒，下一次 `expect` 再次 panic。
- 各执行器自身没有声明 `Send`/`Sync` 约束，也没有内部并发。它们是有状态的拉取式对象，要求调用者以独占 `&mut self` 顺序执行 Open/Next/Close。
- child 生命周期方面，`LimitExec` 和 `SelectionExec` 有明确关闭路径；`ExecuteLimitValues` 在正常和错误路径都尝试 Close。`SelectLockExec`、`TableDualExec`、`TableScanExec`、`MaxOneRowExec` 没有在本文件定义 Close，是否需要关闭子节点取决于外层执行器协议。
- `AdaptiveLimitController` 由 `LimitExec` 持有共享 `Arc`。Open 调用 Reset；达到 end、子节点 EOF、首批即完成以及 Close 都调用 Stop。Stop 必须可重复调用，这一要求由控制器契约承担。
- `TableScanExec` 一次性缓存所有虚表 chunk；`SelectionExec` 持有一个子 chunk 和选择位图；`SelectLockExec` 持有所有键。这三类资源都随一次 Open 周期增长并在重开/关闭或对象释放时回收。

## 与 Go 版本的对应关系

直接对照 `pkg/executor/select.go` 可确认相同的高层职责和不同的迁移深度：

- `SelectLockExec`：两版都跨非空批累计键并在 EOF 一次加锁，也都过滤临时表和 LOCK TABLES 范围并传播共享锁升级开关。Go 版还从行中构造逻辑/物理表键、处理外连接物理表 ID 为 0、更新表 delta、设置事务 ForUpdate、合并锁统计、资源组 tag、deadlock history 和 assertion；Rust 将这些行为压入 `SelectRuntime`，且本文件没有真实适配器证据。
- `LimitExec`：两版都跨 chunk 跳 OFFSET、控制 required rows、截断终批，并让拥有者管理 adaptive controller。Go 版还处理列裁剪/列交换、缓存 chunk、trace 和慢 Close 日志；Rust 的通用实现只处理行选择与容量，`ExecuteLimitValues` 是面向已解码行的较窄生产用途。
- `TableDualExec`：Go 版约束 0/1 行，并对零列 schema 使用 virtual row；Rust 统一调用 `append_null_row`，允许多行，具体零列行为由运行时决定。
- `SelectionExec`：两版都有向量化/非批路径。Go 版用表达式可向量化性决定 batched，维护 memory tracker，并在非批路径逐行求值以保证 SETVAR/GETVAR 顺序；Rust 由运行时报告 `selection_batched` 并执行过滤，未在本文件内表达内存记账和该 SQL 副作用原因。
- `TableScanExec`：两版都懒加载信息表全部记录再逐 chunk 输出。Go 版负责列转换、`IterRecords` 和 chunk list；Rust 将全部物化过程交给 `table_scan_all`。
- `MaxOneRowExec`：0 行补 NULL、首批多行报错、恰好一行再探测下一批的语义一致。
- StatementContext：Go `ResetContextOfStmt` 是近两百行的实际 session 状态重置流程；Rust 同名函数只委托 trait。`ResetUpdateStmtCtx`、`ResetDeleteStmtCtx`、TopSQL 和弱一致读同样只是转发，因此不能宣称已逐字段移植。
- 全局 tracker：Rust 定义了原子容器与初始化标志，但没有在目标文件中实现 Go 的全局 tracker 挂接、消费链或后台监控逻辑，属于接口/状态骨架。

## 扩展指南

- 接入真实 Rust SQL 主链时，应优先新增或完善独立的运行时适配文件，实现 `SelectRuntime`/`LimitRuntime`，而不是把 session、chunk、TiKV 细节重新硬编码进算法。接线点需从物理计划 builder 构造对应执行器，并证明 Open/Next/Close 全生命周期。
- 扩展悲观锁时，重点修改 `SelectRuntime` 锁能力、`newLockCtx` 和 `SelectLockExec::Next`。必须同步验证 NOWAIT/WAIT N、共享锁升级、临时表、LOCK TABLES、untouched index、物理分区键、死锁诊断、deadline 和大键集内存峰值。若启用去重，应明确在何处调用 `deduplicateLockKeys` 并保持首次顺序。
- 修改 LIMIT 时，保持 `[begin,end)` 不变量以及 controller 的 Reset/Stop 配对；同步 `pkg/executor/select_test.rs` 的跨 chunk、精确边界、输入不足、count=0 用例，并检查 `pkg/executor/physical_plan_runtime_test.rs` 中 root Limit 不再拉 child 的行为。
- 修改过滤器时，应分别覆盖批处理和非批处理、跨 chunk 游标、空输入、输出容量以及表达式错误。若复刻 Go 的 SETVAR/GETVAR 顺序语义，应在运行时选择非批路径并用独立测试证明。
- 修改虚表扫描时，要评估一次性物化的内存风险；若改成流式，必须保持 Open 重置、输出 chunk 所有权交换和错误传播契约。
- 修改 `MaxOneRowExec` 时，必须保留“恰好一行仍需再拉一次”的探测，否则跨 chunk 的第二行会漏报。
- 扩展 StatementContext 时，应在具体 runtime 中逐项对齐 `pkg/executor/select.go:941` 起的 session 状态逻辑，而不是扩大本文件 trait 默认实现；同步 Rust 独立测试，并参考 `pkg/executor/select_test.go` 的 TiKV short-circuit 与 INSERT/IMPORT flags 用例。
- Rust 单元测试继续放在 `pkg/executor/select_test.rs`，由 `lib.rs` 装配，不能内嵌回 `select.rs`。本文件已经保留 PingCAP 许可证并带有 `// Copyright 2026 AsterSQL.`，后续不得删除。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件、4,415 个 Go 文件；`node --file pkg/executor/select.rs --offset 1 --limit 500` 与 `--offset 500 --limit 650` 完整读取 1,096 行源码；`query` 定位了 Rust/Go 两版 `SelectLockExec`、`LimitExec`、`SelectionExec`、`ResetContextOfStmt` 和 Rust `ExecuteLimitValues`。通用名 callers/callees 查询未返回可用符号边，故调用点由后续精确检索核验。
- Rust 源与装配：`pkg/executor/select.rs`、`pkg/executor/lib.rs`、`pkg/executor/physical_plan_runtime.rs`、`pkg/executor/Cargo.toml`。
- Rust 独立测试：`pkg/executor/select_test.rs` 验证共享锁升级开关传播、稳定锁键去重、LIMIT 跨 chunk/边界/输入不足/count=0、锁键跨批缓冲至 EOF，以及 LOCK TABLES 过滤开关。
- Go 对照：`pkg/executor/select.go`；Rust 对应类型在 Go 中的生产构造点由 `pkg/executor/builder.go` 精确检索确认。Go 语句重置测试为 `pkg/executor/select_test.go`，LIMIT/Selection required rows 还可见 `pkg/executor/executor_required_rows_test.go`。
- 人工边界复核：区分了 Rust 已接入的 `ExecuteLimitValues` 与仅有泛型算法/测试证据的其他符号；没有把 Go builder 接线、session 副作用或 TiKV 细节误写为 Rust 当前事实。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行固定十一章节结构检查，并检查变更范围不包含 Rust、Go、Cargo 或只读 `plan.md`。
