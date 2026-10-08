# `pkg/statistics/handle/autoanalyze/priorityqueue/job.rs`

## 文件定位

本文件是 `astersql-statistics-handle-autoanalyze-priorityqueue` crate 的作业公共契约层。crate 入口 `pkg/statistics/handle/autoanalyze/priorityqueue/lib.rs` 以私有模块 `job` 装入它，再通过 `pub use job::*` 对外重导出。它不负责维护堆或扫描统计信息，而是定义队列、作业工厂、三类具体作业与执行环境之间共享的类型和辅助逻辑。

`pkg/statistics/handle/autoanalyze/priorityqueue/Cargo.toml` 表明该 crate 的直接外部依赖只有 `astersql-statistics-handle-logutil`；本文件另外只使用标准库的 `Any`、`HashMap`、格式化、`Arc` 和时间类型。当前 Rust 文件已经带有 AsterSQL 处理标记，并保留 PingCAP Apache License。

## 核心职责

1. 用 `AnalysisJob` 统一非分区表、静态分区表和动态分区表作业的校验、执行、权重、指标、身份、回调和日志视图接口。
2. 用 `AnalysisRuntime` 隔离作业状态机与真实会话/统计执行环境，使作业只依赖元数据查询、历史耗时查询和 ANALYZE SQL 执行能力。
3. 在 `IsValidToAnalyze` 中实现失败后的再分析冷却策略，避免刚失败或仍处于等待期的对象反复占用队列。
4. 提供跨具体作业复用的数据模型与 Go 兼容格式化，包括指标 JSON、带符号纳秒时长、字符串列表映射和索引名解析。
5. 用 `IsDynamicPartitionedTableAnalysisJob` 做运行时具体类型识别，供需要区分动态分区作业的上层逻辑使用。

这些职责由 `job.rs` 的 `AnalysisJob`、`AnalysisRuntime`、`IsValidToAnalyze`、`AsJSONIndicators`、`format_go_duration`、`format_go_string_list_map` 与 `index_names` 直接体现；实际 SQL 拼装与作业字段保存在三个 `*_analysis_job.rs` 文件中。

## 主要符号

- `DEFAULT_FAILED_ANALYSIS_WAIT_TIME`：没有成功分析平均耗时记录时采用的 30 分钟冷却阈值。`SCHEMA_NOT_EXIST`、`TABLE_NOT_EXIST`、`NOT_PARTITIONED_TABLE`、`PARTITION_NOT_EXIST` 是具体作业校验时共享的稳定原因文本。
- `AnalysisDuration(i64)`：以有符号纳秒保存与 Go `time.Duration` 对齐的值。`from_secs` 和两倍平均耗时计算都保留 `i64` wrapping 语义；`Display` 委托给 Go 风格时长格式化。它与标准库 `Duration` 的比较只允许非负值相等。
- `Indicators`：调度权重输入，包含变更比例 `ChangePercentage`、表规模 `TableSize` 和距上次分析时长 `LastAnalysisDuration`。`IndicatorsJSON` 是三个字段的字符串展示形式。
- `IndexMetadata`、`PartitionMetadata`、`TableMetadata`：`AnalysisRuntime::table_by_id` 返回的精简元数据边界，供具体作业刷新 schema、表、索引和分区名称。
- `AnalysisJobJSON`：跨三类作业的统一诊断视图，包含类型、表身份、分区/索引名、指标和权重；具体作业在各自 `AsJSON` 中构造它。
- `AnalysisRuntime`：`Send + Sync` 的宿主接口。它提供表元数据、最后失败间隔、平均成功耗时、动态分区批量大小（默认 128）和 `execute_analyze`；返回 `Result<_, String>`，让宿主错误沿作业边界传播。
- `SuccessJobHook`、`FailureJobHook`：以 `Arc<dyn Fn... + Send + Sync>` 保存的完成回调。回调取得完整可变 `AnalysisJob`，失败回调还取得 `mustRetry`。
- `AnalysisJob`：要求实现 `Display + Send + Sync`。核心方法是 `ValidateAndPrepare`、`Analyze`、权重和指标 getter/setter、表 ID、钩子注册、`AsJSON` 及供向下转型的 `as_any`；默认 `String` 与 `Display` 输出保持一致。
- `IsValidToAnalyze`：公共冷却门禁。`IsDynamicPartitionedTableAnalysisJob` 通过 `Any::is` 检查动态分区具体类型。
- `format_go_duration`、`format_go_string_list_map`、`AsJSONIndicators`、`index_names`：分别负责 Go 时长文本、稳定排序的 Go map 文本、指标展示和按元数据顺序筛选索引名。

## 执行流程

作业的完整路径是：`AnalysisJobFactory` 创建具体作业并装箱为 `Box<dyn AnalysisJob>`；`AnalysisPriorityQueue` 与 `AnalysisJobHeap` 根据 `GetTableID`、`GetWeight` 等接口保存、更新和弹出作业；执行前调用具体类型的 `ValidateAndPrepare`，由运行时元数据刷新名称并调用本文件的 `IsValidToAnalyze`；通过后再由具体类型的 `Analyze` 拼 SQL、调用 `AnalysisRuntime::execute_analyze`，并触发已注册的成功或失败钩子。

`IsValidToAnalyze` 的分支顺序是一个可观察契约：

1. 先调用 `last_failed_analysis_duration`；失败时记录 Warn 采样日志并立即返回，不能继续查询平均耗时。
2. 再调用 `average_analysis_duration`；失败同样记录 Warn 并返回。
3. 将宿主的非负 `Duration` 转成 `AnalysisDuration`；代码保留对 `-1` 无记录哨兵的过滤结构，但标准库 `Duration` 本身不能构造负值，所以当前宿主通常用 `None` 表示无记录。
4. 最近失败间隔为零时，以 Info 日志拒绝，原因固定为 `last analysis just failed`。
5. 有失败记录但无平均值时，间隔严格小于 30 分钟才拒绝；等于阈值允许执行。
6. 同时有失败和平均值时，失败间隔严格小于平均耗时的两倍才拒绝；等于两倍允许执行。乘法采用 Go 有符号纳秒溢出语义。
7. 其余情况返回 `(true, "")`。

展示流程中，具体作业的 `AsJSON` 调用 `AsJSONIndicators`；百分比先乘 100，再固定两位小数，表规模固定两位小数，时长使用 Go 样式。具体作业的 `Display` 还复用 `format_go_duration`，动态分区索引展示复用 `format_go_string_list_map`。

## 数据与状态

本文件没有全局可变队列状态。常量与元数据值均为只读；`Indicators`、`TableMetadata` 和 JSON 视图由调用者按值拥有。作业内可变状态通过 `&mut dyn AnalysisJob` 暴露给校验、执行及钩子，因而权重、指标、解析后的名称和回调效果都归具体作业对象所有。

`AnalysisDuration` 特意不使用标准库无符号时长作为内部表示：Go 的 `time.Duration` 是有符号 `int64` 纳秒，可能出现负值和 wrapping 溢出。格式化按纳秒、微秒、毫秒、秒、分钟、小时逐级输出，支持 `i64::MIN`；指标浮点格式还保留 `+Inf`、`-Inf`、`NaN` 与负零的 Go 文本语义。

`index_names` 以请求 ID map 作为成员集合，但遍历 `TableMetadata.indices`，所以输出顺序由元数据决定，不由 `HashMap` 决定；不存在的请求 ID 被静默忽略。`format_go_string_list_map` 则显式排序字符串键，以消除 Rust `HashMap` 的非确定迭代顺序，同时保留每个值向量的原顺序。

## 依赖与调用关系

上游直接关系包括：

- `analysis_job_factory.rs` 返回 `Box<dyn AnalysisJob>`，构造三类作业。
- `queue.rs` 的构建、重建、推入、更新和弹出 API 传递 `Box<dyn AnalysisJob>`；`heap.rs` 以表 ID 为键并按作业权重组织它们。
- `calculator.rs::CalculateWeight` 和 `GetSpecialEvent` 只依赖 `&dyn AnalysisJob` 的指标与新索引状态。
- `non_partitioned_table_analysis_job.rs`、`static_partitioned_table_analysis_job.rs`、`dynamic_partitioned_table_analysis_job.rs` 实现 trait；三者的 `ValidateAndPrepare` 都调用 `IsValidToAnalyze`，三者的 `AsJSON` 都调用 `AsJSONIndicators`。非分区与静态分区实现还调用 `index_names`。

下游依赖只有日志设施和标准库。冷却错误使用 `StatsErrVerboseSampleLogger`，策略性跳过使用 `StatsSampleLogger`，日志携带 schema、table、partitions；涉及等待阈值时还携带最近失败和平均耗时。执行、历史查询和元数据查询均反向委托给调用者实现的 `AnalysisRuntime`。

RustCodeGraph 对目标文件识别出 60 个符号；精确图查询对通用名称存在大量同名噪声，因此调用边又以目标目录内直接引用检索核实。crate 之外通过 `lib.rs` 的重导出使用公开类型；`format_go_duration`、`format_go_string_list_map` 和 `index_names` 保持 `pub(crate)`，只服务 crate 内实现与测试。

## 错误处理与边界

`AnalysisRuntime` 使用字符串错误而非专门错误枚举。`IsValidToAnalyze` 会给历史查询错误添加固定上下文前缀，记录原始错误后返回拒绝；它不会吞掉错误，也不会尝试执行 ANALYZE。策略性跳过不是 `Err`，而是 `(false, reason)`，使队列能区分“不适合当前执行”与具体作业 `Analyze` 的执行失败。

冷却比较都是严格小于：30 分钟和平均耗时两倍的相等边界均允许分析。没有最后失败记录时，即使存在平均耗时也允许分析。最近失败为零优先于后续冷却判断。日志字段由闭包每次重新构造，避免跨分支共享可变上下文。

格式化边界包括零值 `0s`、亚微秒、最长正负 `i64` 纳秒、非有限浮点和负零。`AnalysisDuration::from(Duration)` 直接把 `u128` 纳秒转换为 `i64`，因此超过 `i64::MAX` 的宿主值按 Rust cast 截断到低 64 位；这是与 Go 有符号纳秒边界一致的有意兼容取舍。`IsDynamicPartitionedTableAnalysisJob` 只识别当前 crate 的具体动态分区结构，包装器或其他语义等价实现不会被判为动态类型。

## 并发与资源生命周期

`AnalysisJob` 和 `AnalysisRuntime` 都要求 `Send + Sync`，允许队列及后台刷新/执行路径跨线程共享抽象；本文件自身不创建线程、任务、通道、事务或锁。运行时实现负责会话、统计句柄和 SQL 执行资源的真实生命周期。

钩子存于 `Arc`，闭包同样必须 `Send + Sync`。它们接收当前完整作业的可变借用，因此触发期间不能并发修改同一作业；借用结束后资源仍由具体作业持有。`job_test.rs::hooks_receive_complete_jobs_for_all_job_types` 证明三类作业均把完整 JSON 视图传给钩子，成功钩子可以修改权重，失败钩子会收到重试标志。日志器由 logutil crate 管理，本文件只提交结构化日志项。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/autoanalyze/priorityqueue/job.go`。Rust 的 `Indicators`、成功/失败钩子、`AnalysisJob` 方法、四个校验原因、30 分钟默认等待、冷却查询顺序和严格比较均对应 Go 定义。`IsDynamicPartitionedTableAnalysisJob` 在 Go 中用类型断言，在 Rust 中用 `Any` 向下类型检查；`asJSONIndicators` 对应 Rust 的公开 `AsJSONIndicators`。

Rust 为可独立运行的移植边界增加了 Go 文件中没有的 `AnalysisRuntime` 和精简元数据结构：Go 接口直接接收 `sessionctx.Context`、`StatsHandle` 与 `sysproctrack.Tracker`，Rust 把这些能力收敛为可模拟的 trait。Rust 的 `AnalysisJobJSON`/`IndicatorsJSON` 在本 crate 内定义，而 Go 使用 `statistics/handle/types` 中的视图类型。

Rust 还显式实现 Go 格式细节：有符号纳秒 `AnalysisDuration`、时长文本、map 键排序、`+Inf` 与 wrapping 乘法。这些并非业务简化。Go `job_test.go` 直接覆盖六类作业字符串和动态类型判断；Rust `job_test.rs` 除对齐这些用例外，还覆盖冷却错误/边界/日志、非有限浮点、时长和 map、索引顺序、三类钩子及负时长权重行为。

## 扩展指南

- 新增作业类型时，应在独立生产文件中实现完整 `AnalysisJob`，保留 `Display`、`AsJSON`、钩子、校验和执行语义，并更新独立测试文件；不要把实现或测试塞入 `job.rs`。若该类型需要被特殊识别，应评估是否扩展现有类型判断，而不是仅依赖字符串类型名。
- 新增运行时能力应优先加入 `AnalysisRuntime` 的最小方法，并同步所有生产和测试实现。无默认实现的方法会造成所有实现点编译失败；带默认值的方法需确认 Go 配置来源及兼容默认值。
- 修改冷却策略必须同步 `IsValidToAnalyze`、Go `job.go` 对照判断和 `job_test.rs::cooldown_matches_go_branches_and_query_order`，尤其不能无意改变查询短路顺序、严格比较边界、wrapping 算术、原因文本或日志字段。
- 修改指标或 JSON 字段时，应同步 `Indicators`、`IndicatorsJSON`、`AnalysisJobJSON`、三个具体作业的 `AsJSON`、权重计算器和独立测试；还需检查 Go 的 `statistics/handle/types` 视图契约。
- 修改格式化辅助函数时，应保留确定性顺序和 Go 极值语义，重点扩充 `duration_and_map_format_match_go`、`indicators_json_matches_go_nonfinite_and_signed_zero` 与 `signed_duration_format_and_weight_match_go`。
- 修改索引解析时，应保持元数据顺序和缺失 ID 的既有处理，并同步 `index_names_preserves_metadata_order_and_skips_missing_ids` 及具体作业测试。

主要兼容风险是日志/原因文本或 JSON 字符串变化影响运维观测；正确性风险集中在冷却边界和错误短路；性能风险较低，但动态分派、克隆日志字段和格式化发生在队列/日志路径，扩展时不应引入不受控的元数据扫描或锁等待。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter .../job.rs` 确认目标文件已索引且含 60 个符号；`node --file .../job.rs` 阅读了完整 418 行；`query AnalysisJob`、`query Indicators` 定位 trait、结构、三个具体实现的方法和测试引用。精确 `explore` 受仓库大量同名符号干扰，故目录内直接引用由 `rg` 补充核验。
- Rust 源与入口：`pkg/statistics/handle/autoanalyze/priorityqueue/job.rs`、`lib.rs`、`analysis_job_factory.rs`、`queue.rs`、`heap.rs`、`calculator.rs`、三个 `*_analysis_job.rs`。
- crate 边界：`pkg/statistics/handle/autoanalyze/priorityqueue/Cargo.toml`，确认库入口、logutil 依赖和 Go package 移植元数据。
- Go 对照：`pkg/statistics/handle/autoanalyze/priorityqueue/job.go`；Go 测试 `job_test.go`。
- Rust 独立测试：`pkg/statistics/handle/autoanalyze/priorityqueue/job_test.rs`，覆盖字符串、动态类型、冷却、日志、JSON/时长/map、索引顺序、回调与负时长；其他具体作业测试提供 `AnalysisRuntime` 实现和校验/执行路径证据。
- 人工复核结论：本文件存在的原因是稳定自动分析作业的跨类型契约与 Go 兼容公共语义；它经工厂、队列/堆、具体作业和运行时边界参与执行；安全扩展点及应同步的独立测试已在“扩展指南”列明。
