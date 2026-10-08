# `pkg/statistics/handle/autoanalyze/priorityqueue/static_partitioned_table_analysis_job.rs`

## 文件定位

该文件属于 `astersql-statistics-handle-autoanalyze-priorityqueue` crate。crate 入口 [`lib.rs`](lib.rs) 将本模块声明为“静态分区模式（逐分区分析）分析作业”并重新导出其公开项；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 表明它移植自 Go 包 `pkg/statistics/handle/autoanalyze/priorityqueue`。

在自动分析链路中，[`analysis_job_factory.rs`](analysis_job_factory.rs) 的 `AnalysisJobFactory::CreateStaticPartitionAnalysisJob` 为一个符合条件的物理分区构造 `StaticPartitionedTableAnalysisJob`。作业随后以 `Box<dyn AnalysisJob>` 进入 [`queue.rs`](queue.rs) 的优先队列，由队列计算权重、安装完成钩子、弹出并记录运行状态。真正取得会话/统计执行能力的是 `AnalysisRuntime` 实现；本文件只维护作业状态并生成、派发 ANALYZE 请求。

## 核心职责

- 用 `GlobalTableID` 标识逻辑分区表、用 `StaticPartitionID` 标识本次唯一目标物理分区，并把后者作为队列键；因此同一逻辑表的不同静态分区拥有独立的排队、运行中去重和失败冷却状态（`GetTableID`、`ValidateAndPrepare`）。
- 在执行前通过 `AnalysisRuntime::table_by_id` 解析 schema、逻辑表、目标分区和索引名称，并只针对目标分区调用 `IsValidToAnalyze` 做失败冷却判断（`ValidateAndPrepare`）。
- 根据 `IndexIDs` 是否为空，在“分析整个分区”和“分析分区中新索引”两条路径间选择；索引路径只执行 `IndexNames` 的第一个索引，因为 Analyze V2 分析一个索引时会同时刷新该分区的列和其他索引（`GetAnalyzeType`、`AnalyzeStaticPartitionIndexes`）。
- 将成功、可重试失败和不可重试的元数据失效反馈给队列，并提供权重、指标、JSON 视图和人类可读展示（`finish` 以及 `AnalysisJob`、`fmt::Display` 实现）。

## 主要符号

- `ANALYZE_STATIC_PARTITION` / `ANALYZE_STATIC_PARTITION_INDEX`：对外报告的作业类型字符串，分别表示整分区分析和分区索引分析。
- `StaticPartitionedTableAnalysisJob`：核心状态对象。`GlobalTableID`、`StaticPartitionID`、`IndexIDs`、`TableStatsVer` 与 `NeedVersionRewriteWarn` 是构造时确定的执行输入；`SchemaName`、`GlobalTableName`、`StaticPartitionName`、`IndexNames` 由校验阶段延迟填充；`indicators` 与 `Weight` 服务于队列排序；两个 hook 供队列回收运行状态。
- `NewStaticPartitionTableAnalysisJob(...) -> StaticPartitionedTableAnalysisJob`：构造器。它保留 ID、统计版本、重写告警标志和三个调度指标，名称为空、权重为零、hook 未注册。
- `GetAnalyzeType`：以 `IndexIDs.is_empty()` 为唯一判据。注意它不以解析后的 `IndexNames` 为判据，所以“请求了已消失索引”仍属于索引作业。
- `GenSQLForAnalyzeStaticPartition` / `GenSQLForAnalyzeStaticPartitionIndex`：生成带 `%n` 标识符占位符的 SQL 模板，并按 schema、表、分区、可选索引的顺序返回参数，避免把标识符直接拼入 SQL。
- `AnalyzeStaticPartition` / `AnalyzeStaticPartitionIndexes`：把模板、参数、统计版本与重写告警标志传给 `AnalysisRuntime::execute_analyze`。索引名为空时后者直接返回 `Ok(true)`，不会退化为整分区 ANALYZE。
- `ValidateAndPrepare`：解析元数据、填充名称、筛选索引名并执行分区级冷却检查；也是不可重试/可重试失败语义的主要分界点。
- `Analyze`：按类型派发执行，保证所有正常返回值和错误返回值都会经过 `finish`；执行器返回 `Ok(false)` 时对调用方仍返回 `Ok(())`，但触发失败 hook。
- `GetTableID`：返回 `StaticPartitionID` 而不是 `GlobalTableID`，直接决定堆键、运行中集合和重试集合的粒度。
- `AsJSON` / `fmt::Display`：前者生成 Rust 的结构化调试视图，后者生成与 Go `String()` 格式对齐的多行文本。

## 执行流程

1. `AnalysisJobFactory::CreateStaticPartitionAnalysisJob` 检查分区统计是否合格，计算变化率、表大小、距上次分析时长及待分析索引；若变化率为零且没有新索引则不创建作业，否则调用 `NewStaticPartitionTableAnalysisJob`。
2. `AnalysisPriorityQueue::Push` 以 `GetTableID()` 返回的物理分区 ID 检查是否已运行，计算 `Weight`，注册成功/失败 hook，再交给最大堆；`Pop` 弹出最高权重作业并把同一分区 ID 放进 `running_jobs`。
3. 执行方应先调用 `ValidateAndPrepare`。该方法按全局表 ID 取 `TableMetadata`，依次拒绝“表不存在”“不是分区表”“目标分区不存在”；这些结构性失败调用 `failure_hook(job, false)`，表示不应重试。
4. 元数据有效时，方法写入 schema、表和分区名称，并用公共辅助 `index_names` 按 `IndexIDs` 从元数据顺序提取索引名。随后只把当前 `StaticPartitionName` 传给 `IsValidToAnalyze`；冷却检查失败调用 `failure_hook(job, true)`，表示队列应记录为必须重试。
5. `Analyze` 根据 `GetAnalyzeType` 派发。没有索引 ID 时生成 `analyze table %n.%n partition %n`；有索引 ID 时只对第一个解析出的索引生成 `analyze table %n.%n partition %n index %n`。两者最终都调用 `AnalysisRuntime::execute_analyze`。
6. `execute_analyze` 的 `Ok(true)` 触发成功 hook；`Ok(false)` 或 `Err(_)` 触发 `failure_hook(job, true)`。只有 `Err` 原样返回给调用方，布尔失败被 hook 消化为 `Ok(())`。

## 数据与状态

作业状态分三组。第一组是稳定身份与执行配置：逻辑表 ID、物理分区 ID、索引 ID 集合、统计版本和版本重写告警标志。第二组是调度数据：`Indicators` 中的变化比例、表大小、上次分析间隔，以及队列计算出的 `Weight`。第三组是准备阶段得到的易变名称：schema、逻辑表、静态分区和索引名；这些名称在构造后为空，不能在成功执行 `ValidateAndPrepare` 前用于 SQL。

`IndexIDs: HashMap<i64, ()>` 表示集合，但 `IndexNames` 的顺序来自 `TableMetadata.indices`，不是哈希迭代顺序；因此“首个索引”由元数据顺序决定。若 ID 集合非空但没有任何 ID 能映射到当前元数据，作业类型仍是索引分析，而执行阶段无 SQL 即成功结束。这一不回退行为由 `stale_index_ids_do_not_fall_back_to_analyzing_the_whole_partition` 固定。

`AsJSON` 使用物理分区 ID 作为 `TableID`，并输出当前名称、指标及格式化为六位小数的权重。它是 Rust 本地的 `AnalysisJobJSON` 形状，不应假定与 Go 版本字段逐项相同。

## 依赖与调用关系

上游直接构造者是 `AnalysisJobFactory::CreateStaticPartitionAnalysisJob`；它提供全局表 ID、目标分区 ID、缺失索引集合和调度指标。队列侧的 [`queue.rs`](queue.rs) 通过 `AnalysisJob` trait 使用 `GetTableID`、`SetWeight`、`RegisterSuccessHook`、`RegisterFailureHook` 和 `AsJSON`，[`heap.rs`](heap.rs) 也使用 `GetTableID` 作为增删查键。

本文件通过 `use crate::job::*` 依赖 [`job.rs`](job.rs) 中的 `AnalysisJob`、`AnalysisRuntime`、`Indicators`、hook 类型、错误常量、`IsValidToAnalyze`、`index_names`、`AsJSONIndicators` 和 Go 风格时长格式化。运行时边界向下提供元数据查询、历史分析耗时查询和 ANALYZE 执行；本 crate 唯一显式外部依赖 `astersql-statistics-handle-logutil` 由公共 `job.rs` 的冷却逻辑用于日志，本文件没有直接调用外部 crate API。

RustCodeGraph 将目标文件识别为包含 27 个符号的已索引 Rust 文件，并显示其直接文件级使用者为 `static_partitioned_table_analysis_job_test.rs`。对 trait impl 方法的 `callers`/`callees` 查询未返回边，因此队列和工厂关系由上述实际源码引用补证，不能据图缺边推断生产路径不存在。

## 错误处理与边界

- `table_by_id(GlobalTableID)` 返回空时返回 `(false, TABLE_NOT_EXIST)`；元数据存在但没有分区时返回 `NOT_PARTITIONED_TABLE`；找不到指定物理分区时返回 `PARTITION_NOT_EXIST`。三者均以 `need_retry = false` 调失败 hook。
- `IsValidToAnalyze` 查询失败历史或平均耗时失败，以及尚处冷却期时，都令校验失败并以 `need_retry = true` 调 hook。它只接收当前分区名，所以一个分区刚失败不会冷却同表其他分区。
- Rust 的 `TableMetadata` 已包含 `schema_name`，本文件没有 Go 版本独立的 `SchemaByID` 查询，因此 Rust 当前没有单独产生 `SCHEMA_NOT_EXIST` 的分支；这是运行时抽象差异，不应在文档中声称该分支已移植。
- `index_names` 只按 ID 筛选，不在本文件再次检查 public/columnar 属性；索引 ID 由上游工厂选择。无法解析任何索引名时，索引作业按成功空操作处理，避免错误地分析整个分区。
- `execute_analyze` 的字符串错误会在触发失败 hook 后原样传播；`Ok(false)` 只通过 hook 表达失败。调用方若只观察 `Result` 而忽略 hook，会看不到这种业务失败。
- `GetAnalyzeType` 的匹配包含 `unreachable!()` 兜底，因为返回值只可能是本文件两个静态常量；新增类型时必须同步派发逻辑。

## 并发与资源生命周期

`AnalysisJob` 和 `AnalysisRuntime` 都要求 `Send + Sync`；hook 是 `Arc<dyn Fn(...) + Send + Sync>`。本对象本身不创建线程、任务、通道、事务或会话，也不持有执行器资源。`Analyze` 仅同步借用运行时，资源获取与 SQL 执行生命周期属于 `AnalysisRuntime::execute_analyze` 的实现。

队列用 `Arc<Mutex<QueueState>>` 管理运行状态，注册的弱引用 hook 在完成时移除 `running_jobs`；可重试失败还会写入 `must_retry_jobs`。由于键是 `StaticPartitionID`，同一分区不会被重复调度，而同表不同分区不会互相占用该键。`finish` 在调用 hook 前克隆 `Arc`，避免同时不可变借用 hook 字段和可变借用整个作业；hook 接收 `&mut dyn AnalysisJob`，可检查或修改作业状态。若队列已经销毁，hook 中的 `Weak` 无法升级，只会安全地跳过状态更新。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 [`static_partitioned_table_analysis_job.go`](static_partitioned_table_analysis_job.go)：结构体、构造参数、两种分析类型、物理分区键、准备流程、首索引优化、SQL 模板、权重/指标访问器、hook 和展示字符串均保留了 Go 命名与主要分支。独立 Rust 测试中的 `analyze_indexes_matches_go_by_executing_only_the_first_index` 明确锁定了 Analyze V2 的首索引语义。

主要边界差异如下：

- Go 从 `sessionctx`/InfoSchema 分别解析表和 schema，并直接调用真实统计执行链；Rust 把元数据和执行抽象到 `AnalysisRuntime`，所以当前文件是可接线的作业状态机而不是完整会话集成。
- Go 的 `Analyze` 通过会话池执行，并在延迟函数中完成 hook；Rust 同步调用 `execute_analyze` 后显式 `finish`。两者都把执行器的布尔失败交给 hook，但 Rust 还允许运行时返回 `Err(String)` 并向上传播。
- Go `AsJSON` 输出索引 ID、浮点权重和 `HasNewlyAddedIndex`；Rust `AnalysisJobJSON` 输出解析后的名称、字符串化指标和权重。因此 JSON 数据模型尚非字段级复刻，调用者应以各自类型定义为准。
- Go 测试使用真实 TestKit 验证统计从 pseudo 变为真实、多个索引均被刷新及分区隔离冷却；Rust 可执行测试使用内存 `AnalysisRuntime` 覆盖 SQL 参数、首索引、过期 ID 空操作和 hook 重试语义。Rust 测试文件顶部保留的 Go 流程注释不是可执行覆盖，真实端到端效果仍由 Go 测试提供证据。

## 扩展指南

- 新增分析类型或改变类型判定时，应同步修改类型常量、`GetAnalyzeType`、`Analyze` 派发、`AsJSON`/`Display`，并扩展 `static_partitioned_table_analysis_job_test.rs`；不要让 `IndexIDs` 与 `IndexNames` 对作业类型产生不一致的隐式回退。
- 改 SQL 形状时，应集中修改两个 `GenSQL...` 方法并保持 `%n` 参数化，补充参数顺序测试；同时对照 Go 同名方法，评估标识符转义和兼容性。
- 改元数据准备或失败分类时，应修改 `ValidateAndPrepare`，明确哪些失败需要重试，并同步 `validate_and_prepare_matches_go_failure_hook_retry_semantics`。若引入新的元数据查询，优先扩展 `AnalysisRuntime`，不要让作业持有具体会话实现。
- 改队列身份粒度时，必须联查 `queue.rs` 的 `running_jobs`/`must_retry_jobs` 和 `heap.rs` 的键操作。把键改为全局表 ID 会使同表分区互相阻塞，是行为和并发度变化。
- 若要补齐 Go 端到端能力，应在独立 Rust 测试文件中增加运行时集成测试，覆盖真实分区统计更新、Analyze V2 多索引刷新和同表分区间失败隔离；不要把测试内嵌到本生产文件。
- 性能上应保留“一次索引 ANALYZE 刷新全部列和索引”的去重原则；兼容性上需关注统计版本与 `NeedVersionRewriteWarn` 的透传，正确性上需维持准备成功后名称与 ID 指向同一份最新元数据。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter ...static_partitioned_table_analysis_job.rs` 确认目标已索引；`node --file ... --offset 1 --limit 500` 展示完整 284 行源码、27 个符号及测试文件级使用关系；`query NewStaticPartitionTableAnalysisJob` 同时定位 Rust 与 Go 构造器。`callers`/`callees` 对相关 trait 方法无输出，调用关系因此使用源码引用补证。
- Rust 生产代码：[`static_partitioned_table_analysis_job.rs`](static_partitioned_table_analysis_job.rs)、[`job.rs`](job.rs)、[`analysis_job_factory.rs`](analysis_job_factory.rs)、[`queue.rs`](queue.rs)、[`heap.rs`](heap.rs) 与 [`lib.rs`](lib.rs)。
- crate 边界：[`Cargo.toml`](Cargo.toml)，确认库入口、直接依赖及 Go 包移植元数据。
- Rust 独立测试：[`static_partitioned_table_analysis_job_test.rs`](static_partitioned_table_analysis_job_test.rs) 的四个可执行测试；另参考 [`job_test.rs`](job_test.rs) 对展示、JSON 和 hook 公共行为的覆盖。
- Go 对照：[`static_partitioned_table_analysis_job.go`](static_partitioned_table_analysis_job.go) 与 [`static_partitioned_table_analysis_job_test.go`](static_partitioned_table_analysis_job_test.go)，核对真实会话执行、统计结果、多索引刷新和分区级冷却语义。
- 本任务仅新增说明文档，不修改运行时代码；按任务约束不运行 Cargo。交付前使用任务给定命令验证恰好存在十一个固定二级章节，并人工检查所有关键结论均可追溯到上述符号或文件。
