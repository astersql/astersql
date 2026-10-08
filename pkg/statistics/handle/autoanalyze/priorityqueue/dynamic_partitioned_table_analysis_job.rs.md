# `pkg/statistics/handle/autoanalyze/priorityqueue/dynamic_partitioned_table_analysis_job.rs`

## 文件定位

本文件实现自动统计优先队列中的“动态分区裁剪表”分析作业。它位于 `astersql-statistics-handle-autoanalyze-priorityqueue` crate 中，由同目录 `lib.rs` 声明并公开重导出；crate 的 `Cargo.toml` 仅直接依赖 `astersql-statistics-handle-logutil`，本文件自身则通过 `crate::job::*` 使用队列公共的作业、运行时、元数据和日志/冷却判断抽象。

源码链接：[`dynamic_partitioned_table_analysis_job.rs`](dynamic_partitioned_table_analysis_job.rs)。

作业由 `AnalysisJobFactory::CreateDynamicPartitionedTableAnalysisJob` 创建：工厂汇总需要重分析的分区 ID、缺统计的新索引及调度指标，再调用 `NewDynamicPartitionedTableAnalysisJob`，以 `Box<dyn AnalysisJob>` 交给队列。`AnalysisPriorityQueue::Push`/`Pop` 管理这个 trait object；但当前生产 Rust 源码中未检索到 `Pop` 后调用 `ValidateAndPrepare` 或 `Analyze` 的执行器，因此“创建和排队”已有接线，而实际调度执行入口仍未在 Rust 生产链中验证。对应的完整 Go 执行语义在同路径 `.go` 文件中。

## 核心职责

- 用 `DynamicPartitionedTableAnalysisJob` 保存逻辑表 ID、待处理的物理分区/索引 ID、统计版本、重写告警标志和优先级指标。
- 在 `ValidateAndPrepare` 中重新读取当前表元数据，将持久的 ID 映射为执行 SQL 所需的 schema、表、分区和索引名称，并执行最近失败分析的冷却检查。
- 在 `Analyze` 中根据是否存在 `PartitionIndexIDs`，选择整分区分析或新增索引分析；两条路径都按 `AnalysisRuntime::partition_batch_size` 分批生成参数化 `ANALYZE TABLE` 模板并交给 `execute_analyze`。
- 维护成功/失败钩子、队列权重和指标，并通过 `AsJSON`、`Display` 提供观测视图。

重要不变量是：`PartitionIDs`/`PartitionIndexIDs` 是创建阶段的 ID 输入，`PartitionNames`/`PartitionIndexNames` 是校验阶段根据最新元数据派生的名称；调用方应先 `ValidateAndPrepare` 再 `Analyze`，不能把构造后的空名称字段误认为无需分析。

## 主要符号

- `ANALYZE_DYNAMIC_PARTITION` 与 `ANALYZE_DYNAMIC_PARTITION_INDEX`：分别标识整分区和新增索引作业；`GetAnalyzeType` 仅以 `PartitionIndexIDs.is_empty()` 判型。
- `DynamicPartitionedTableAnalysisJob`：核心状态类型。私有字段是 `success_hook`、`failure_hook`、`indicators`；其余公开字段包含 ID 输入、执行选项、权重及校验后名称。
- `NewDynamicPartitionedTableAnalysisJob(...)`：构造器。复制 ID/选项，组装 `Indicators`，把权重置为 `0.0`，名称与钩子置空；它不访问元数据，也不验证输入。
- `GetPartitionSQL(prefix, suffix, count)`：生成 `%n` 标识符占位符列表。`count == 0` 时直接拼接前后缀；非零时占位符以 `", "` 分隔。
- `GetPartitionNames(indexes)`：展平“索引名 → 分区名列表”；基于 `HashMap`，返回顺序没有稳定保证。
- `ValidateAndPrepare`：`AnalysisJob` 的准备阶段，负责表/分区类型检查、ID 到名称的解析及冷却检查。
- `AnalyzePartitions`、`AnalyzePartitionIndexes`：两个私有批处理执行器。后者只处理 `PartitionIndexNames.iter().next()` 的一个索引。
- `Analyze` 与 `finish`：选择执行器、传播运行时错误并统一触发完成钩子。
- `SetWeight`/`GetWeight`、`GetIndicators`/`SetIndicators`、`GetTableID`、`HasNewlyAddedIndex`：队列排序和类型判断所需的 trait 方法。
- `RegisterSuccessHook`/`RegisterFailureHook`：保存 `Arc<dyn Fn... + Send + Sync>` 回调。
- `AsJSON`、`fmt::Display`：分别生成结构化摘要和与 Go 文本格式对齐的人类可读摘要；`as_any` 支持 `job.rs::IsDynamicPartitionedTableAnalysisJob` 下转型判断。

## 执行流程

1. `AnalysisJobFactory::CreateDynamicPartitionedTableAnalysisJob` 先要求全局统计存在且可分析，再计算版本是否匹配、跨分区指标、需重分析的分区 ID 和缺统计索引；两类集合都为空时返回 `None`，否则调用本文件构造器。
2. 调度者应调用 `ValidateAndPrepare`。它通过 `runtime.table_by_id(GlobalTableID)` 获取最新 `TableMetadata`：找不到表时以 `need_retry = false` 调失败钩子并返回 `TABLE_NOT_EXIST`；表没有分区时同样不可重试并返回 `NOT_PARTITIONED_TABLE`。
3. 校验成功取得元数据后，方法写入 `SchemaName`/`GlobalTableName`，建立分区 ID 到名称的映射，再过滤 `PartitionIDs` 得到 `PartitionNames`。它清空旧的 `PartitionIndexNames`，遍历元数据索引，把仍存在的索引 ID 和分区 ID 转为名称；空的索引分区集合不会保留。
4. 若两类名称均为空，返回 `(true, "")`，不调用冷却检查。否则把整分区名称与索引涉及的分区名称合并，调用 `IsValidToAnalyze`；拒绝时以 `need_retry = true` 调失败钩子并原样返回原因。
5. `Analyze` 依据原始 `PartitionIndexIDs` 是否为空选择路径。整分区路径将 `PartitionNames` 按 `max(partition_batch_size, 1)` 分块，生成 `analyze table %n.%n partition ...`；索引路径对 `PartitionIndexNames` 的首个条目分块，并追加 `index %n` 与索引名参数。
6. 每批调用 `runtime.execute_analyze(sql, params, TableStatsVer, NeedVersionRewriteWarn)`。返回 `Ok(false)` 时停止后续批次；返回 `Err` 时也停止并向上返回错误。全部批次成功（或名称列表为空）得到 `Ok(true)`。
7. `Analyze` 对 `Ok(success)` 调 `finish(success)` 后返回 `Ok(())`；对 `Err(error)` 调 `finish(false)` 后返回原错误。因而业务失败 `Ok(false)` 通过失败钩子表达但不是 Rust `Err`。

## 数据与状态

状态分成四组：作业身份（`GlobalTableID`）、待处理集合（`PartitionIDs`、`PartitionIndexIDs`）、调度/执行选项（`indicators`、`TableStatsVer`、`NeedVersionRewriteWarn`、`Weight`），以及延迟派生状态（schema/表/分区/索引名称）。`ValidateAndPrepare` 会覆盖名称字段，因此同一作业重新校验时可反映元数据变化；尤其 `PartitionIndexNames.clear()` 防止已删除索引残留。

所有集合使用 `HashMap`。这意味着 `PartitionNames` 来源于 `PartitionIDs.keys()`，`PartitionIndexNames.iter().next()` 选择哪个索引，以及 `AsJSON().IndexNames` 的顺序都不稳定；`Display` 则通过 `format_go_string_list_map` 对索引键排序，以对齐 Go 的映射展示。索引执行只取首项是统计版本 2 的刻意语义：分析一个索引会刷新该批分区的列和全部索引，避免重复执行；若未来支持不同统计版本，不能默认这一不变量仍成立。

`GetIndicators` 克隆指标，`SetIndicators` 整体替换；钩子也以 `Arc` 克隆后调用，以避免在回调借用 `&mut self` 时仍借用字段。构造器接受 `impl Into<AnalysisDuration>`，使标准 `Duration` 和 Go 兼容的有符号纳秒表示都可进入作业。

## 依赖与调用关系

上游直接证据：

- `analysis_job_factory.rs::CreateDynamicPartitionedTableAnalysisJob` 调用构造器并返回 `Box<dyn AnalysisJob>`。
- `lib.rs` 声明模块并 `pub use dynamic_partitioned_table_analysis_job::*`。
- `queue.rs` 以 `Box<dyn AnalysisJob>` 接收、堆排序、弹出和生成 JSON 快照；当前生产 Rust 搜索没有发现它调用本文件的校验/执行方法。
- `job.rs::IsDynamicPartitionedTableAnalysisJob` 通过 `as_any().is::<DynamicPartitionedTableAnalysisJob>()` 识别类型。

RustCodeGraph 确认的下游边包括：`ValidateAndPrepare → AnalysisRuntime::table_by_id / GetPartitionNames / IsValidToAnalyze`；`Analyze → AnalyzePartitions / AnalyzePartitionIndexes / finish`；两个批执行器都调用 `GetPartitionSQL`、`partition_batch_size` 和 `execute_analyze`。公共类型和行为契约来自 `job.rs` 的 `AnalysisJob`、`AnalysisRuntime`、`TableMetadata`、`Indicators`、钩子别名及格式化辅助函数。

## 错误处理与边界

- 表不存在、表不再分区是不可重试的元数据失效：返回固定原因并以 `false` 调失败钩子。Rust 的 `TableMetadata` 已含 schema 名，没有 Go 中独立 `SchemaByID` 查询，因此没有单独的 `SCHEMA_NOT_EXIST` 分支。
- 已删除/未知的分区或索引 ID 被 `filter_map` 静默丢弃。若全部 ID 都过期，校验仍返回成功，随后 `Analyze` 成为空操作；独立 Rust 测试明确锁定了这一行为。
- 冷却查询失败或冷却期未满由 `IsValidToAnalyze` 转为 `(false, reason)`，并标为需要重试。
- `partition_batch_size()` 为零时被 `.max(1)` 规范成 1，避免 `chunks(0)` panic。
- `execute_analyze` 的 `Err(String)` 被保留并传播；`Ok(false)` 则触发失败钩子但 `Analyze` 返回 `Ok(())`。调用方必须结合钩子/队列状态判断业务成功，不能只检查 `Result`。
- `GetPartitionSQL` 不校验 prefix/suffix，也不转义名称；安全边界在 `%n` 模板与独立参数列表，实际执行器必须按标识符绑定语义处理参数。
- 空名称列表、空索引映射都被视作成功空操作；是否应创建这种作业由上游工厂和校验阶段共同约束。

## 并发与资源生命周期

类型通过 `AnalysisJob: Send + Sync` 进入并发队列；运行时也要求 `AnalysisRuntime: Send + Sync`。作业本身不创建线程、任务、锁、通道或事务，也不持有 session/统计句柄；这些资源全部由 `AnalysisRuntime` 在每次调用期间借用提供。

成功/失败钩子以 `Arc` 保存，可跨线程共享；注册和执行都需要 `&mut self`，所以同一作业的状态变更由外部独占借用串行化。`finish` 在回调前克隆 `Arc`，回调可以安全接收同一作业的可变 trait 引用。批执行是严格串行的，遇到首个失败立即停止，不回滚已经成功的前序批次；重试时应依赖最新元数据与运行时的幂等/统计覆盖语义。

## 与 Go 版本的对应关系

总体流程与 `dynamic_partitioned_table_analysis_job.go` 对齐：相同的构造输入、ID 到名称延迟解析、失败冷却、分批 SQL、统计版本 2 只分析首个索引、钩子以及展示字符串。Go 的 `AutoAnalyzePartitionBatchSize` 对应 Rust 运行时方法，`exec.AutoAnalyze` 对应 `execute_analyze`，`statsutil.CallWithSCtx` 的 session 获取被收敛到 Rust 的运行时边界。

已验证的差异如下：

- Go 在 `ValidateAndPrepare` 中分别查表和 schema；Rust 的 `table_by_id` 一次返回带 schema 名的 `TableMetadata`，所以只能表达“表不存在”或“非分区表”。
- Go `Analyze` 的 session 获取可能返回 error，但底层分析成功与否由布尔值和钩子表达；Rust 允许 `execute_analyze` 直接返回 `Err(String)`，并在失败钩子后传播。
- Go `AsJSON` 输出 `PartitionIDs`、`PartitionIndexIDs`、`HasNewlyAddedIndex` 和数值权重；Rust `AnalysisJobJSON` 输出校验后的名称、字符串化指标/权重，未携带原始 ID。这是当前公共 Rust JSON 类型的差异，不应描述为完全等价序列化。
- Go map 与 Rust `HashMap` 都不承诺迭代顺序；两版都只处理首个索引，因此选择的具体索引不稳定，但在统计版本 2 下预期效果等价。
- Go 测试包含真实数据库/统计集成与查询计划检查；Rust 测试文件前半部分只是注释化迁移记录，当前实际可执行测试仅覆盖 SQL/名称辅助函数、索引类型、缺表钩子和过期 ID。

## 扩展指南

- 增加新的动态分区分析模式时，优先修改 `GetAnalyzeType`、`Analyze` 分派和 `AnalysisJobJSON`，并同步工厂的创建条件；不要只根据派生名称判型，因为现有契约使用原始 `PartitionIndexIDs`。
- 修改批处理策略时，在 `AnalyzePartitions` 与 `AnalyzePartitionIndexes` 保持一致的 `max(1)` 保护、参数顺序和首失败短路；若要并行化，必须先定义钩子只触发一次、部分批次成功后的重试语义及运行时并发能力。
- 支持统计版本 1 或逐索引差异化分析时，应重新审视“仅首个索引”的假设，并消除 `HashMap` 首项不稳定带来的行为差异。
- 改变元数据失效策略时，修改 `ValidateAndPrepare` 并同步独立测试 `dynamic_partitioned_table_analysis_job_test.rs`；当前过期 ID 被忽略且可形成成功空操作，这是已锁定行为。
- 调整 JSON/Display 输出时同时核对 `job.rs::AnalysisJobJSON`、`format_go_string_list_map`、`format_go_duration` 以及 Go `AsJSON`/`String`，明确兼容目标。
- 测试必须继续放在独立的 `dynamic_partitioned_table_analysis_job_test.rs`。新增执行语义至少应覆盖多批次参数、首批失败短路、`Err` 传播、成功/失败钩子恰好一次以及多索引只执行一次；真实 session/statistics 集成仍需与 Go 同路径测试的断言对齐。

## 验证依据

- 目标源码：`pkg/statistics/handle/autoanalyze/priorityqueue/dynamic_partitioned_table_analysis_job.rs`（结构体、构造器、辅助函数、`AnalysisJob`/`Display` 实现）。
- crate 与装配：同目录 `Cargo.toml`、`lib.rs`。
- 直接上游/公共契约：`analysis_job_factory.rs::CreateDynamicPartitionedTableAnalysisJob`、`queue.rs` 的 `Push`/`Pop`/快照接口、`job.rs` 的 `AnalysisJob`、`AnalysisRuntime`、`IsValidToAnalyze`、`IsDynamicPartitionedTableAnalysisJob`。
- RustCodeGraph：索引状态为 11,467 文件；`node --file` 读取目标文件；`query` 定位 `DynamicPartitionedTableAnalysisJob` 和构造器；`callees` 核对 `ValidateAndPrepare`、`Analyze`、`AnalyzePartitions`、`AnalyzePartitionIndexes`、`GetPartitionSQL` 的调用边。图未返回 trait 方法调用者，故用限定 `rg` 补查工厂、队列及生产执行引用。
- Rust 独立测试：`dynamic_partitioned_table_analysis_job_test.rs` 的三个可执行测试覆盖两分区 SQL、零分区模板、名称展平/新增索引判定、缺表不可重试钩子，以及过期分区/索引 ID 的成功空操作；文件中注释化的 Go 集成测试仅作迁移意图证据，不算可执行覆盖。
- Go 对照：`dynamic_partitioned_table_analysis_job.go`；Go 独立测试 `dynamic_partitioned_table_analysis_job_test.go` 覆盖真实分区统计、索引统计、失败冷却与查询计划。
- 本说明是只读分析，不运行 Cargo；最终以任务规定的 11 个固定二级标题结构检查作为交付验证。
