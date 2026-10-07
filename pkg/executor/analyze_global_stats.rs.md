# `pkg/executor/analyze_global_stats.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod analyze_global_stats` 公开该模块，`pkg/executor/Cargo.toml` 则把 `lib.rs` 声明为库入口。它描述动态分区裁剪模式下，在各物理分区完成 ANALYZE 后，将分区统计合并为逻辑表级全局统计的执行侧协调逻辑。对应的 Go 实现是 `pkg/executor/analyze_global_stats.go`。

当前 Rust 接线需要特别区分两层：`pkg/executor/analyze.rs::handleGlobalStats` 已在 ANALYZE 主流程中收集待合并的表、列和索引信息，而本文件另行定义了自己的 `AnalyzeExec`、`globalStatsMap` 和 `globalStatsRuntime`。RustCodeGraph 与全仓引用搜索均未发现生产代码构造这里的 `AnalyzeExec` 或调用这里的 `AnalyzeExec::handleGlobalStats`。因此，本模块目前是可编译公开的合并协调边界，但尚未接到 `analyze.rs` 的实际主链，不能把 Go 端已接线状态等同于本 Rust 文件的运行时状态。

## 核心职责

- `AnalyzeExec::handleGlobalStats` 按逻辑表 ID 对 `globalStatsMap` 去重和分组，为每个列统计对象（`indexID == -1`）或索引统计对象创建合并作业，选择 ANALYZE 选项，并请求运行时合并、持久化统计。
- `AnalyzeExec::newAnalyzeHandleGlobalStatsJob` 将数字键转成用户可读的数据库、表和索引描述，供 ANALYZE 作业状态与诊断日志使用。
- `globalStatsRuntime` 把元数据查询、作业登记、真实统计合并、历史统计记录和日志从协调流程中抽离。此文件本身不加载分区统计，也不实现直方图、TopN、FM Sketch 或 CM Sketch 的合并算法；这些行为由 `merge_partition_stats_to_global_and_persist` 的实现者负责。
- `globalStatsKey`、`globalStatsInfo`、`mergePartitionStatsRequest` 和 `GLOBAL_STATS_COMPONENTS` 定义协调层传递给运行时的数据契约。

## 主要符号

- `GlobalStatsError(String)` 与 `GlobalStatsResult<T>`：模块的轻量错误类型和结果别名。错误只保存文本，实现 `Display` 与 `std::error::Error`。
- `analyzeOptionType`：桶数、TopN 数、样本数和采样率四类选项的键。选项值统一以 `u64` 表达。
- `v2AnalyzeOptions { filledOptions }`：按表保存已填充的 v2 ANALYZE 选项。
- `globalStatsKey { tableID, indexID }`：待合并对象的有序键；`indexID == -1` 表示列统计，非负值表示索引统计。
- `globalStatsInfo { isIndex, histogramIDs, statsVersion }`：来自上游分析结果的合并元信息，作为请求中的 `info` 原样传给运行时。
- `globalStatsMap`：`BTreeMap<globalStatsKey, globalStatsInfo>`，记录一次协调需要处理的对象，并提供稳定的键遍历顺序。
- `statisticsComponent` 与 `GLOBAL_STATS_COMPONENTS`：列出每次请求的四种组件：`Histogram`、`TopN`、`FmSketch`、`CmsSketch`。
- `tableIdentity`：运行时返回的库名、表名及 `indexID -> indexName` 映射。
- `analyzeJob`：合并作业的可展示数据库名、表名和 `jobInfo`。
- `mergePartitionStatsRequest`：一次合并的表 ID、索引 ID、统计元信息、选项和组件集合。
- `globalStatsRuntime`：所有有副作用的同步 trait 边界，要求实现者满足 `Send + Sync`。
- `AnalyzeExec { options, OptionsMap, runtime }`：本模块的协调器。它与 `pkg/executor/analyze.rs::AnalyzeExec` 是不同类型。

本文件没有条件编译项，也没有内嵌测试模块。

## 执行流程

`AnalyzeExec::handleGlobalStats` 的流程如下：

1. 从 `statsMap` 的键提取 `tableID`，放入 `BTreeSet`，使同一逻辑表只进入一次外层循环。
2. 每处理一个表，先把表 ID 放入 `historical_table_ids`；随后扫描完整 `statsMap`，只处理属于当前表的键值对。
3. 调用 `newAnalyzeHandleGlobalStatsJob` 查询表身份并生成作业。如果 `table_identity` 返回 `None`，记录“找不到分区表”的诊断并跳过该对象；同表其他对象仍继续。
4. 调用 `add_new_analyze_job`。登记失败只调用 `log_add_job_error`，不会停止流程；之后仍调用 `start_analyze_job`。
5. 选择合并选项：若 `OptionsMap` 存在且包含当前 `tableID`，使用该表的 `filledOptions`；否则克隆执行器级默认 `options`。覆盖粒度是表，不是单个索引。
6. 构造 `mergePartitionStatsRequest`，其中 `components` 固定包含全部四种统计组件，然后调用 `merge_partition_stats_to_global_and_persist`。
7. 合并失败时记录 `log_merge_error`；无论成功失败，都调用 `finish_global_stats_job`，并把合并错误的引用作为作业结束状态传入。
8. 所有对象处理结束后，按去重的表 ID 各调用一次 `record_historical_stats`。历史记录失败只写 `log_historical_error`。
9. 返回 `Ok(())`。合并失败、作业登记失败和历史记录失败都被定义为作业级诊断，不上升为语句级错误。

`newAnalyzeHandleGlobalStatsJob` 对列键生成 `merge global stats for <db>.<table> columns`；对索引键从 `indexNames` 查名称并生成 `merge global stats for <db>.<table>'s index <index>`。表身份缺失时返回 `None`；索引名称缺失时使用空字符串，仍返回作业。

## 数据与状态

协调输入是按 `(tableID, indexID)` 唯一化的 `globalStatsMap`。`BTreeMap`/`BTreeSet` 使 Rust 版本按数值键稳定遍历；Go 对照使用原生 `map`，不承诺遍历顺序。正确性不应依赖作业先后次序。

`AnalyzeExec::options` 是默认选项，`OptionsMap` 是可选的表级覆盖。读取时只克隆所选映射，不修改执行器或输入。`globalStatsInfo` 中的 `histogramIDs`、`isIndex` 和 `statsVersion` 也只被克隆进请求；本文件不解释或改变这些值。

`historical_table_ids` 保证一个逻辑表在本次调用中最多记录一次历史快照。表 ID 在查找 `tableIdentity` 之前就被加入集合，所以即使该表的所有对象都因元数据缺失而跳过，末尾仍会尝试记录该表的历史统计；这与 Go 实现先填充 `tableIDs` 再创建作业的顺序一致。

## 依赖与调用关系

上游事实分为“设计来源”和“当前接线”两部分：

- Go 主链 `pkg/executor/analyze.go` 在保存分区 ANALYZE 结果后调用 `(*AnalyzeExec).handleGlobalStats`，其实现位于 `pkg/executor/analyze_global_stats.go`。
- Rust 主链 `pkg/executor/analyze.rs` 的 worker 汇总阶段调用该文件自己的自由函数 `analyze.rs::handleGlobalStats` 来填充 `analyze.rs::globalStatsMap`，最终通过 `analyzeRuntime::merge_global_stats` 进入运行时。它没有调用本模块同名方法。
- 本模块仅由 `pkg/executor/lib.rs` 声明公开；RustCodeGraph 对精确符号 `pkg/executor/analyze_global_stats.rs::handleGlobalStats` 未给出外部调用边，全仓对 `globalStatsRuntime`、`mergePartitionStatsRequest` 和模块路径的搜索也只有本文件自身命中。

下游全部通过 `globalStatsRuntime`：`table_identity` 提供元数据；作业相关方法维护生命周期；`merge_partition_stats_to_global_and_persist` 承担实际读取、版本校验、四类组件合并与持久化；`record_historical_stats` 负责历史快照；四个日志方法保留非致命故障证据。文件自身只依赖标准库的有序集合、格式化和 `Arc`，不直接使用 `pkg/executor/Cargo.toml` 中的其他 workspace crate。

## 错误处理与边界

- `handleGlobalStats` 的签名允许返回 `GlobalStatsError`，但当前函数体所有运行时错误都被捕获并记录，最终恒为 `Ok(())`。这保留了 Go `handleGlobalStats` 不因单个全局统计合并失败而令 ANALYZE 语句失败的行为。
- 元数据缺失不是错误返回：`newAnalyzeHandleGlobalStatsJob` 通过 `Option` 表示，调用方记录后跳过。
- 作业登记失败后仍会启动、合并并结束作业。运行时实现必须能容忍“登记未成功但后续生命周期回调仍发生”的序列。
- 合并错误会同时用于日志和 `finish_global_stats_job`；历史记录错误只记录，不重试。
- 空 `statsMap` 不调用运行时并返回成功。多个对象中的某一个失败不会阻止同表或其他表继续处理。
- 索引 ID 缺少名称时作业文本包含空索引名，而不是跳过或报错；若此行为需要收紧，应先与 Go 的 `FindIndexNameByID` 表现和兼容性要求对齐。
- `GLOBAL_STATS_COMPONENTS` 表达请求意图，但组件合并的数学正确性、统计版本兼容、缺分区处理和持久化原子性均不在本文件内保证，必须由运行时实现及其测试证明。

## 并发与资源生命周期

本函数内部没有线程、异步任务、锁、通道或事务，所有对象和表按有序集合顺序同步处理。`Arc<dyn globalStatsRuntime>` 允许协调器共享运行时，`Send + Sync` 允许它跨线程持有，但本文件不会自行并发调用运行时。

每个成功构造的作业遵循“尝试登记 -> 启动 -> 合并 -> 结束”的生命周期；即使合并失败，结束回调仍会执行。元数据缺失的对象不会登记、启动或结束作业。历史统计记录发生在全部合并作业结束之后，且每表一次。

由于 trait 方法均接收共享引用 `&self`，运行时若维护作业表、统计缓存或持久化事务，必须自行提供内部同步和资源清理。尤其 `merge_partition_stats_to_global_and_persist` 的事务边界及失败回滚不可从本文件推断，当前证据只能确认协调器会在调用返回后结束作业并继续处理。

## 与 Go 版本的对应关系

`pkg/executor/analyze_global_stats.go` 是逐项核对的主要来源：两版都先对表 ID 去重，再逐表扫描待合并对象；元数据缺失时跳过；表级 v2 选项覆盖默认选项；每个对象都创建、启动并结束合并作业；最后每表记录一次历史统计；合并和历史失败只记录，函数返回成功。

主要表达差异如下：

- Go 直接持有真实 `*handle.Handle`、Domain、InfoSchema、日志器和上下文；Rust 将这些依赖收敛为 `globalStatsRuntime`，便于隔离实现与测试。
- Go 的 `MergePartitionStats2GlobalStatsByTableID` 隐式决定合并组件；Rust 请求显式携带 `GLOBAL_STATS_COMPONENTS` 的四项集合。
- Go 的 `AddNewAnalyzeJob` 没有错误返回；Rust trait 将登记建模为可失败操作，并新增“记录后继续”的分支。
- Go map 遍历无序；Rust `BTreeMap`/`BTreeSet` 有稳定顺序，但外部可见行为不应依赖顺序。
- Go 已从 `pkg/executor/analyze.go` 接入真实 ANALYZE 流程；本 Rust 文件尚未与 `pkg/executor/analyze.rs` 中的同名类型和映射合并。现有 Rust 集成测试能证明系统其他路径可产生全局统计或展示合并作业，但不能单独证明这些结果由本文件执行。

## 扩展指南

- 接入主链时，优先复用或统一 `pkg/executor/analyze.rs` 现有的 `AnalyzeExec`、`globalStatsKey`、`globalStatsInfo` 和 `globalStatsMap`，避免两个公开数据模型长期漂移；接线点应位于 `analyzeRuntime::merge_global_stats` 当前调用位置附近。
- 新增统计组件时同时更新 `statisticsComponent`、`GLOBAL_STATS_COMPONENTS`、运行时请求处理和 Go 对照行为，并覆盖组件缺失、版本不兼容和持久化失败。
- 修改选项选择规则时重点检查 `OptionsMap` 表级覆盖、默认回退和列/索引共享选项的兼容性；对应行为可参考 `pkg/executor/test/analyzetest/options/analyze_saved_options_test.rs` 及其 Go 对照。
- 修改作业文本或生命周期时同步检查 `pkg/executor/show_stats_test.rs::TestShowAnalyzeStatus` 和 `pkg/executor/show_stats_test.go` 中的精确 `jobInfo` 断言。
- 修改合并结果语义时同步检查 `pkg/executor/test/analyzetest/analyze_test.rs` 中 `analyze_partition_dynamic_mode_builds_global_and_partition_stats`、`analyze_partition_static_to_dynamic_rebuilds_global_stats` 等独立测试及对应 Go 测试。
- 若为本模块补直接单元测试，应新建同目录独立测试文件并由 `lib.rs` 在 `#[cfg(test)]` 下声明，覆盖空映射、同表多对象、表级选项覆盖、元数据缺失、登记/合并/历史错误以及“合并失败仍结束作业”。不要把测试内嵌进本生产文件。
- 接入前应评估：重复合并与历史记录的性能成本、运行时持久化原子性、多个 ANALYZE 并发时的作业状态一致性，以及稳定遍历顺序是否无意形成外部契约。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 `pkg/executor/analyze_global_stats.rs`，文件共 233 行、25 个符号；通过 `files`、`explore`、`query` 和 `node --file` 核对了文件、`handleGlobalStats`、`newAnalyzeHandleGlobalStatsJob` 及其内部运行时调用。
- 生产源码：`pkg/executor/analyze_global_stats.rs`（全部类型、trait、常量和两个方法）、`pkg/executor/analyze.rs`（主链收集函数、worker 调用点和 `analyzeRuntime::merge_global_stats` 边界）、`pkg/executor/lib.rs`（模块公开声明）。
- crate 边界：`pkg/executor/Cargo.toml` 的包名为 `astersql-executor`、库入口为 `lib.rs`；本模块源码自身只引用标准库。
- Go 对照：`pkg/executor/analyze_global_stats.go`（合并协调和作业描述）与 `pkg/executor/analyze.go`（真实调用入口）。
- 测试证据：`pkg/executor/analyze_test.rs::TestAnalyzeIndexExtractTopN` 验证主链映射收集；`pkg/executor/show_stats_test.rs::TestShowAnalyzeStatus` 与 `pkg/executor/show_stats_test.go` 验证列/索引全局合并作业文本；`pkg/executor/test/analyzetest/analyze_test.rs` 及其 Go 对照验证动态分区模式能产生和重建表级全局统计。仓库未发现直接实现 `globalStatsRuntime` 或直接调用本模块 `AnalyzeExec::handleGlobalStats` 的 Rust 测试。
- 人工边界复核：文档将“Go 已接线的目标语义”“Rust 系统现有集成行为”和“本文件当前无生产调用者”分开陈述，没有把 trait 注释中的下游责任当作本文件已完成的实现。
