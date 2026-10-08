# `pkg/statistics/handle/autoanalyze/priorityqueue/non_partitioned_table_analysis_job.rs`

## 文件定位

本文件实现非分区表的自动 `ANALYZE` 作业，属于 `astersql-statistics-handle-autoanalyze-priorityqueue` crate。crate 根在 [`lib.rs`](lib.rs) 中将此模块声明为 `non_partitioned_table_analysis_job` 并公开重导出；[`Cargo.toml`](Cargo.toml) 声明了对应的 Go package 为 `pkg/statistics/handle/autoanalyze/priorityqueue`。文件自身仅依赖 [`job.rs`](job.rs) 提供的作业接口、运行时边界、指标和钩子类型，以及标准库的 `HashMap`/`Any`/`fmt`。

上游创建入口是 [`AnalysisJobFactory::CreateNonPartitionedTableAnalysisJob`](analysis_job_factory.rs)：工厂在统计合格且变化率或缺失索引需要分析时，调用本文件的 `NewNonPartitionedTableAnalysisJob`，并以 `Box<dyn AnalysisJob>` 返回。[`AnalysisPriorityQueue::push_locked`](queue.rs) 为作业计算权重、注册钩子并放入堆。当前 Rust 非测试代码中未检索到 `AnalysisJob::ValidateAndPrepare` 或 `AnalysisJob::Analyze` 的调用者，因此文件已实现作业状态机和队列接入，但 Rust 生产执行 worker 的接线仍未在当前索引中证实。

## 核心职责

- 用 `NonPartitionedTableAnalysisJob` 保存物理表 ID、待分析索引 ID、统计版本、优先级指标、计算后权重以及延迟解析的 schema/表/索引名。
- 通过 `GetAnalyzeType` 以 `IndexIDs` 是否为空决定整表分析（`ANALYZE_TABLE`）还是新增索引分析（`ANALYZE_INDEX`）。
- 在 `ValidateAndPrepare` 中把稳定 ID 解析成 SQL 标识符名称，并通过 `IsValidToAnalyze` 应用最近失败分析的冷却规则。
- 生成使用 `%n` 标识符占位的 `ANALYZE TABLE` SQL 和有序参数，通过 `AnalysisRuntime::execute_analyze` 执行，避免把 schema、表名或索引名直接拼接到 SQL。
- 在结束时通知队列钩子，并向日志/调试层提供 `AsJSON` 和 `Display`。

## 主要符号

- `ANALYZE_TABLE` / `ANALYZE_INDEX`：作业类型字符串，同时出现在分支选择和 `AnalysisJobJSON.Type` 中。
- `NonPartitionedTableAnalysisJob`：核心状态结构。`success_hook`/`failure_hook` 为队列生命周期回调；`TableID` 是稳定元数据键；`IndexIDs` 既决定作业类型，也用于准备阶段筛选名称；`indicators`/`Weight` 服务优先级计算；`SchemaName`/`TableName`/`IndexNames` 是执行前的延迟初始化结果。
- `NewNonPartitionedTableAnalysisJob(...) -> NonPartitionedTableAnalysisJob`：填充 ID、统计版本和 `Indicators`，将权重置零、名称置空且钩子置为 `None`。
- `GetAnalyzeType`：`IndexIDs.is_empty()` 时返回整表类型，否则返回索引类型。`HasNewlyAddedIndex` 复用同一不变量向优先级计算器暴露特殊事件。
- `GenSQLForAnalyzeTable` / `GenSQLForAnalyzeIndex`：分别返回 `analyze table %n.%n` 与 `analyze table %n.%n index %n`，参数顺序固定为 schema、table、可选 index。
- `AnalyzeTable` / `AnalyzeIndexes`：将 SQL、参数、`TableStatsVer` 和 `NeedVersionRewriteWarn` 交给运行时。索引路径只执行 `IndexNames.first()`，空列表直接返回 `Ok(true)`。
- `finish`：成功布尔值为真时调用成功钩子；否则调用失败钩子且传 `must_retry = true`。
- `impl AnalysisJob`：实现准备、执行、权重/指标访问、钩子注册、JSON 投影和 `Any` 向下转型。
- `impl fmt::Display`：以 Go 风格固定精度和 `format_go_duration` 输出人可读摘要。

## 执行流程

1. `AnalysisJobFactory::CreateNonPartitionedTableAnalysisJob` 先检查统计是否合格，计算变化率、待分析索引、表大小和距上次分析时长，再构造本作业。变化率为零且没有待分析索引时不创建。
2. `AnalysisPriorityQueue::push_locked` 通过 `PriorityCalculator` 设置 `Weight`，然后注册管理 `running_jobs` 和 `must_retry_jobs` 的钩子，最后把 trait object 加入堆。
3. 执行者应先调用 `ValidateAndPrepare`。它以 `TableID` 请求 `runtime.table_by_id`：表不存在时以 `must_retry = false` 通知失败钩子并返回 `TABLE_NOT_EXIST`；表存在时用 `index_names` 按元数据顺序解析所需索引名，同时写入 schema 和表名。
4. 准备阶段以空分区参数调用 `IsValidToAnalyze`。冷却检查不通过时，以 `must_retry = true` 通知失败钩子，并原样返回布尔值与原因。
5. `Analyze` 按 `GetAnalyzeType` 分流。整表路径执行一条整表 SQL；索引路径仅取解析后的第一个索引，因为统计版本 2 分析一个索引会一并刷新列和其他索引。
6. `execute_analyze` 返回 `Ok(true)` 时调用成功钩子；`Ok(false)` 时调用可重试失败钩子，但 `Analyze` 本身仍返回 `Ok(())`；`Err(error)` 时也先调用可重试失败钩子，再传播原错误。

## 数据与状态

`TableID` 和 `IndexIDs` 是创建时的调度身份。`IndexIDs` 是 `HashMap<i64, ()>` 集合表示；其非空性在作业整个生命周期中决定“新增索引”和“索引分析”语义。`index_names` 会按 `TableMetadata.indices` 的顺序而非 `HashMap` 迭代顺序生成 `IndexNames`，所以“第一个索引”由元数据顺序决定。

`SchemaName`、`TableName` 和 `IndexNames` 初始为空，只有成功找到表后才由 `ValidateAndPrepare` 更新。因此在未准备的对象上直接调用 `Analyze`会使用空名称；这个顺序约束由调用者承担，类型系统没有将“已准备”建模为独立状态。若 `IndexIDs` 非空但都已从元数据消失，作业仍属于索引分析，但 `AnalyzeIndexes` 不执行 SQL 而按成功结束；它不会回退成整表分析。

`indicators` 可由 trait 整体读写，`Weight` 由队列入堆前计算。`AsJSON` 输出解析后的名称、字符串化指标和六位小数权重；与 Go 版本不同，当前 Rust `AnalysisJobJSON` 没有 `IndexIDs` 和 `HasNewlyAddedIndex` 字段，因而 Rust `AsJSON` 展示的是名称视图而非 Go 的 ID/布尔视图。

## 依赖与调用关系

- 上游创建：`AnalysisJobFactory::CreateNonPartitionedTableAnalysisJob -> NewNonPartitionedTableAnalysisJob`。工厂传入请求的 analyze version，并在现有统计版本不匹配时将 `NeedVersionRewriteWarn` 设为真。
- 调度接入：`AnalysisPriorityQueue::Push -> push_locked -> PriorityCalculator::CalculateWeight / RegisterSuccessHook / RegisterFailureHook / Heap::AddOrUpdate`。成功钩子从 `running_jobs` 移除表 ID；失败钩子也先移除，然后在 `must_retry` 为真时加入 `must_retry_jobs`。
- 元数据和冷却下游：`ValidateAndPrepare -> AnalysisRuntime::table_by_id -> index_names -> IsValidToAnalyze -> last_failed_analysis_duration / average_analysis_duration`。非分区表总是向历史查询传空分区列表。
- SQL 执行下游：`Analyze -> AnalyzeTable | AnalyzeIndexes -> GenSQLForAnalyze* -> AnalysisRuntime::execute_analyze`。运行时是 `Send + Sync` trait 边界，本文件不直接依赖 session、统计 handle 或 SQL executor 的具体类型。
- 观测下游：`AsJSON -> AsJSONIndicators`，`Display -> format_go_duration`。
- 测试调用：[`non_partitioned_table_analysis_job_test.rs`](non_partitioned_table_analysis_job_test.rs) 直接验证 SQL、首索引执行、过期 ID 和失败钩子语义；[`job_test.rs`](job_test.rs) 还通过 trait object 验证通用钩子/JSON/格式行为，队列测试则验证重试集合联动。

RustCodeGraph 的精确 `query` 能定位本文件的结构、构造器、`ValidateAndPrepare`、`AnalyzeIndexes` 等符号，但对这些符号的 `callers`/`callees` 未返回边；上述边因此由已索引的 `node --file` 源码与全库符号搜索交叉核对，未将空图输出解释成“无调用”。

## 错误处理与边界

- `table_by_id` 仅用 `Option` 表示结果；`None` 会返回 `(false, TABLE_NOT_EXIST)`，并以不重试通知队列。与 Go 实现分别查表和 schema 不同，Rust `TableMetadata` 直接包含 `schema_name`，因此本文件没有可独立触发的 `SCHEMA_NOT_EXIST` 分支。
- `IsValidToAnalyze` 内部查历史失败或平均耗时的错误会被转成 `(false, reason)`；本文件将所有这类拒绝视为可重试失败。立即失败、无平均耗时时未满 30 分钟、或未达平均耗时两倍都可拒绝执行；具体算法在 `job.rs::IsValidToAnalyze`。
- 索引 ID 未解析到名称不是错误。非空 `IndexIDs` 仍使 `GetAnalyzeType` 返回索引类型，而空 `IndexNames` 使执行成为成功 no-op，这避免因过期索引 ID 意外分析整表。
- `Analyze` 将运行时的 `Err(String)` 原样传播，但布尔失败 `Ok(false)` 只通过钩子表示，对调用者返回 `Ok(())`。执行者不应只用 `Result` 判断作业是否成功。
- `finish` 克隆 `Arc` 后再传入 `&mut self`，避免在持有字段不可变借用时可变借用整个作业。钩子为空时安静跳过。
- `match self.GetAnalyzeType()` 的默认分支为 `unreachable!()`；当前安全性依赖类型值只由该方法中的两个常量产生。

## 并发与资源生命周期

`AnalysisJob` 与 `AnalysisRuntime` 都要求 `Send + Sync`，钩子也是 `Arc<dyn Fn + Send + Sync>`，因此作业可跨线程交给调度/执行层。本结构本身不含锁、通道、任务句柄、会话或事务；其可变状态通过 `&mut self` 串行更新。`AnalysisRuntime` 拥有具体会话/执行资源，本文件只在方法调用期间借用 `&dyn AnalysisRuntime`，不保存借用也不负责关闭资源。

队列钩子使用 `Weak<Mutex<QueueState>>`，因而作业不会通过回调强持有队列并形成循环；队列已销毁时 `upgrade` 失败，回调什么也不做。`finish` 只调用一类钩子，但没有内部“已完成”标志；若外部重复调用 `Analyze`，钩子也会重复执行。

## 与 Go 版本的对应关系

直接对照文件是 [`non_partitioned_table_analysis_job.go`](non_partitioned_table_analysis_job.go)。结构字段、整表/索引分流、`%n` 模板参数顺序、只分析第一个索引的 version-2 语义、指标/权重访问和展示字符串都有直接对应。Rust `AnalysisRuntime` 把 Go 中的 `sessionctx.Context`、`StatsHandle`、`sysproctrack.Tracker` 和 `exec.AutoAnalyze` 抽象为可测试边界，而 `AnalyzeDuration` 保留 Go `time.Duration` 的有符号纳秒和格式语义。

需要注意的差异有：

- Go `Analyze` 用 `defer` 保证在 `CallWithSCtx` 返回后触发钩子；Rust 在 `execute_analyze` 的 `Result<bool, String>` 后显式调用 `finish`。布尔失败与错误失败均会进入可重试钩子。
- Go `ValidateAndPrepare` 先分别确认表和 schema，可返回 `tableNotExist` 或 `schemaNotExist`；Rust 运行时一次返回已含 schema 名的 `TableMetadata`，只保留表不存在分支。
- Go `AsJSON` 输出 `IndexIDs`、数值 `Weight` 和 `HasNewlyAddedIndex`，不输出准备后名称；Rust 的共享 JSON 类型输出 `SchemaName`/`TableName`/`IndexNames`、字符串权重和字符串指标。这是现有观测契约差异，不应在本文档中宣称完全等价。
- Go 的生产调用链可直接得到 session 并执行；当前 Rust 仓库搜索只证实了工厂、入堆和测试中的直接执行，未证实生产 worker 调用准备/执行方法。

Go 回归测试在 [`non_partitioned_table_analysis_job_test.go`](non_partitioned_table_analysis_job_test.go) 中覆盖真实 mock store 的行数更新、多索引 version-2 联合刷新、`mysql.analyze_jobs` 仅一条记录以及失败冷却时间线。Rust 独立测试使用内存 `Runtime` 验证作业层契约，没有复刻 Go 的完整 session/统计存储集成环境。

## 扩展指南

- 增加新的作业分流时，同步修改 `GetAnalyzeType`、`Analyze` 和 `AsJSON`，并评估 `HasNewlyAddedIndex` 与优先级计算器的语义；不要只添加常量，否则 `unreachable!()` 和可观测输出会脱节。
- 改变 SQL 时优先保留 `%n` 标识符参数化，严格校验 `GenSQLForAnalyzeTable`/`GenSQLForAnalyzeIndex` 的参数顺序、`TableStatsVer` 和 `NeedVersionRewriteWarn` 传递；不应将元数据名称字符串拼接到 SQL。
- 改变索引选择时必须保留或明确变更两个边界：version 2 只发一条 SQL，以及过期 ID 不得回退为整表分析。元数据索引顺序是当前首索引选择依据。
- 改变准备失败分类时，同时检查 `AnalysisPriorityQueue::hooks` 的 `running_jobs`/`must_retry_jobs` 转移；永久消失的表应传 `false`，短期冷却或执行失败应传 `true`。
- 如果补齐 Rust 生产 worker，必须在 Pop 后严格按 `ValidateAndPrepare -> Analyze` 顺序调用，并对 `Ok(false)` 由钩子而非 `Result` 传递的成败契约做显式验证。这属于后续代码任务，不是本纯文档任务的修改范围。
- 所有作业逻辑测试应保持在独立的 [`non_partitioned_table_analysis_job_test.rs`](non_partitioned_table_analysis_job_test.rs)，不内嵌到源文件。工厂参数派生改动还应同步 [`analysis_job_factory_test.rs`](analysis_job_factory_test.rs)，通用 JSON/钩子/格式改动应同步 [`job_test.rs`](job_test.rs)，队列状态转移改动应同步 [`queue_test.rs`](queue_test.rs)。与 Go 语义对齐时同时对照 Go 源文件和 Go 回归测试，不以缩减集成逻辑的 Rust 测试替代 Go 行为。

## 验证依据

- RustCodeGraph 索引状态：本工作区索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/autoanalyze/priorityqueue` 确认了本 crate 的 Rust/Go 对照文件集。
- RustCodeGraph 符号查询：`query NonPartitionedTableAnalysisJob --json` 定位了 Rust/Go 结构、Rust/Go 构造器及 Rust 工厂入口；`query ValidateAndPrepare --json`、`query AnalyzeIndexes --json` 定位了 trait 和具体实现。`callers`/`callees` 对这些 Rust 符号未返回可用边，因此调用关系另用已索引文件节点和 `rg` 引用搜索验证。
- 主实现证据：[`non_partitioned_table_analysis_job.rs`](non_partitioned_table_analysis_job.rs) 的 258 行全文，以及 [`job.rs`](job.rs) 中 `AnalysisRuntime`、`AnalysisJob`、`IsValidToAnalyze`、`index_names`、`AsJSONIndicators` 的契约。
- crate 与模块证据：[`Cargo.toml`](Cargo.toml) 的 package/lib/path dependency/Go package metadata，以及 [`lib.rs`](lib.rs) 的模块声明、重导出和独立测试挂载。该目录及 `pkg/statistics` 下未找到 `doc.go`，因此无更近的 package contract 可读。
- 上游/队列证据：[`analysis_job_factory.rs`](analysis_job_factory.rs) 的 `CreateNonPartitionedTableAnalysisJob`，以及 [`queue.rs`](queue.rs) 的 `hooks`、`push_locked`、`Push`。全库非测试 Rust 搜索未找到 trait 执行入口的生产调用。
- Go 对照证据：[`non_partitioned_table_analysis_job.go`](non_partitioned_table_analysis_job.go) 全文与 [`non_partitioned_table_analysis_job_test.go`](non_partitioned_table_analysis_job_test.go) 的 SQL、真实统计刷新、多索引和失败冷却测试。
- Rust 测试证据：[`non_partitioned_table_analysis_job_test.rs`](non_partitioned_table_analysis_job_test.rs) 的 `non_partitioned_job_generates_identifier_parameterized_sql`、`analyze_indexes_matches_go_by_executing_only_the_first_index`、`stale_index_ids_do_not_fall_back_to_analyzing_the_whole_table`、`validate_and_prepare_matches_go_failure_hook_retry_semantics`。本任务为纯文档分析，按计划未运行 Cargo。
