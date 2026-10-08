# `pkg/statistics/handle/globalstats/global_stats.rs`

## 文件定位

本文件是 `astersql-statistics-handle-globalstats` crate 中的同步分区统计合并核心。crate 入口 `pkg/statistics/handle/globalstats/lib.rs` 公开 `global_stats` 模块并再导出其 API；相邻的 `global_stats_async.rs` 通过 `AsyncMergePartitionStats::merge` 复用这里的 `merge_partition_stats_to_global`。目标是把同一分区表的多个 `PartitionStats` 合并成按列或索引项排列的 `GlobalStats`，供动态分区裁剪场景使用。

当前 Rust 接线应按实际状态理解：根 `Cargo.toml` 把该 crate 列为工作区 facade，`pkg/statistics/handle/Cargo.toml` 对它的依赖以及本 crate 中大部分 Go 对照依赖均置于永假条件 `target.'cfg(any())'.dependencies`。代码图中非测试的直接调用者是同 crate 的异步包装；尚未发现它替代 Go `statsGlobalImpl`、会话上下文、InfoSchema 与持久化 StatsHandle 的完整生产入口。因此本文件是可独立测试的合并模型，不等同于 Go 生产路径已经全部迁移。

## 核心职责

- 用 `PartitionStatsProvider` 隔离分区统计的取得方式，并在合并开始前及长循环边界检查协作取消。
- 对每个统计项检查“未分析”和“有行但直方图、TopN 均为空”两类缺失；由 `MergeOptions::skip_missing` 决定立即报错还是记录诊断并跳过该分区项。
- 汇总表级 `count`、`modify_count`，并按项合并 FM Sketch、CM Sketch、TopN 与直方图。
- 根据统计版本和并发度选择串行 `merge_partition_top_n` 或并发 `merge_global_top_n_by_concurrency`，再把被 TopN 淘汰的值计数写回直方图精确计数。
- 用 `GlobalStatsWriter` 抽象持久化，`write_global_stats` 只写有直方图的项，继续尝试后续项并返回最后一次写入错误。

该职责刻意比 Go `global_stats.go` 窄：表元数据解析、列/索引 ID 选择、会话变量、警告与日志、真实 KV 读写均不在本文件中。

## 主要符号

- `MAX_PARTITION_MERGE_BATCH_SIZE: usize = 256`：并发 TopN 合并时单批分区数的上限。
- `PartitionStats`：一个分区的快照，包含诊断用 `name`、表级 `count`/`modify_count` 和按项下标排列的 `items`。
- `PartitionItemStats`：一个列或索引项的 `Histogram`、`CmSketch`、`TopN`、`FmSketch` 以及 `analyzed` 标记。
- `GlobalStats`：结果容器。四类组件都按调用方提供的 `item_count` 对齐；缺失项保留为 `None`，诊断放入 `missing_partition_stats`。
- `MergeOptions`：控制 TopN 容量、直方图桶数、统计版本、并发度和缺失统计策略。该类型只保存已经解析后的值，不读取会话变量。
- `PartitionStatsProvider::partitions(table_id)`：上游数据源边界，返回完整分区快照或字符串错误。
- `GlobalStatsWriter::save(table_id, item_index, stats)`：逐项写回边界；实现者负责真实事务、KV 和日志语义。
- `check_cancelled`、`missing_item_error`：内部辅助函数，分别生成稳定的 `query interrupted` 错误和带分区名、项下标的缺失诊断。
- `merge_partition_stats_to_global`：主要公开入口，完成读取、验证、组件合并和结果装配。
- `write_global_stats`：公开写回循环，仅以 `histograms[index].is_some()` 判定该项是否存在。

本文件没有条件编译项、后台任务类型或内部测试模块；测试位于独立的 `global_stats_test.rs` 与 `global_stats_internal_test.rs`，符合生产逻辑与测试分文件的布局。

## 执行流程

`merge_partition_stats_to_global` 的流程如下：

1. 以 Acquire 顺序读取 `cancelled`；若已取消，在访问 provider 前返回 `query interrupted`。随后调用 `provider.partitions(table_id)`。
2. 按 `item_count` 创建四个等长、元素初始为 `None` 的结果向量，并创建相同维度的 `all_items` 暂存区。
3. 遍历每个分区：再次检查取消，先把分区的 `count` 与 `modify_count` 各累加一次，再逐项读取 `partition.items[index]`；短于 `item_count` 的输入等价于该项缺失。
4. 若项不存在或 `analyzed == false`，产生 `partition stats missing`。无跳过策略时立即返回；跳过模式记录诊断并禁止该分区项进入合并。
5. 独立检查列统计是否为空：当分区 `count > 0` 且该项不存在，或直方图桶为空且 TopN 总数为零时，产生 `partition column stats missing`。此检查不与上一步互斥，因此同一未分析、空内容项可产生两条诊断，这与 Go 的独立判断顺序一致。
6. 对每个存在有效输入的项，先合并所有 FM Sketch 和 CM Sketch。FM 的 NDV 最终限制为不超过全表 `output.count`。
7. 克隆各分区直方图和 TopN。`concurrency < 2` 或 `version == 1` 时走串行 TopN 合并；否则计算 `clamp(top_ns.len() / concurrency, 1, 256)` 的批大小并走并发合并。
8. 以首个直方图 ID 创建全局直方图，保持分区来源顺序拼接 buckets，并按键累加 `exact_counts`；TopN 合并淘汰值也回灌到 `exact_counts`。
9. 当 `bucket_count > 0` 且桶数超限时从尾部截断；随后把每个桶的 NDV 清零，只在直方图顶层保存全局 NDV。结果保存 histogram、CMS、TopN，FM 槽明确置为 `None`，因为其信息已经转入直方图 NDV。
10. 所有项完成后返回 `GlobalStats`；没有有效输入的项保持四个结果槽为空。

`write_global_stats` 从下标零开始遍历 `stats.histograms`。空槽不调用 writer；非空槽即使保存失败也继续写后续项，最终返回最后一个错误，没有错误则返回 `Ok(())`。

## 数据与状态

数据布局的不变量是“项下标对齐”：`GlobalStats` 的四个向量长度均为 `item_count`，同一下标代表同一列或索引。`PartitionStats::items` 可以更短或含 `None`，读取时统一转成缺失语义。表级计数在分区外层循环累加，所以不会因一个分区含多个统计项而重复计数；`global_stats_internal_test.rs::global_stats_merge_accumulates_partition_counts_once` 直接验证两个分区得到 `(count, modify_count) == (18, 6)`。

合并过程拥有输入组件的克隆：有效 `PartitionItemStats` 被克隆进 `all_items`，随后直方图和 TopN 又组成各项的工作向量。它不会修改 provider 持有的快照。FM Sketch 只是中间 NDV 汇总状态；最终 `fm_sketches[index]` 保持 `None`。CM Sketch 通过 `CmSketch::merge` 累加，直方图精确计数按编码键求和。

桶处理存在一个模型边界：注释指出 Go 按值边界排序，但当前 Rust `Histogram` 模型不携带边界，因此保留来源顺序，不能改成按行数排序；桶数限制是简单 `truncate`。扩展直方图表示时必须重新评估这一兼容点。

## 依赖与调用关系

上游关系：

- `global_stats_async.rs::AsyncMergePartitionStats::merge` 调用 `merge_partition_stats_to_global`，并缓存结果和缺失诊断。
- `global_stats_test.rs`、`global_stats_internal_test.rs` 直接覆盖同步入口；`global_stats_async_test.rs` 通过异步包装覆盖相同核心。
- `lib.rs` 再导出本文件的公开类型和函数。代码图未显示已接入 Rust SQL 执行主链的非测试调用者，不能据此宣称已替换 Go `MergePartitionStats2GlobalStatsByTableID`。

下游关系：

- provider 与 writer 是依赖反转边界，本文件不知道具体缓存、InfoSchema 或存储实现。
- 统计组件 `Histogram`、`CmSketch`、`TopN`、`FmSketch` 由 crate 根再导入。
- 串行 TopN 路径调用 `merge_partition_top_n`；并发路径调用 `merge_global_top_n_by_concurrency`，具体拆批与工作线程逻辑分别位于相邻的 `topn.rs`、`merge_worker.rs`。
- 并发路径接收同一个 `AtomicBool`，使下游 TopN 合并也能参与协作取消。

Cargo 边界由 `pkg/statistics/handle/globalstats/Cargo.toml` 定义，库入口是 `lib.rs`。当前源文件本身只直接使用标准库原子类型和 crate 内统计结构；Cargo 中 InfoSchema、sessionctx、storage、types 等迁移依赖均在 `cfg(any())` 下，不应当作运行时已启用依赖。

## 错误处理与边界

- provider 错误原样以 `String` 返回；取消统一返回 `query interrupted`。
- 默认 `skip_missing == false` 时，遇到第一条缺失统计即停止；开启时累积诊断并继续处理其他分区和项。
- 对有数据分区，空直方图加空 TopN 被视为列统计缺失；零行分区不会仅因二者为空触发该类错误，但仍可能因未分析触发普通缺失。
- `items.is_empty()` 的统计项跳过合并，避免读取 `items[0]`；结果对应位置保持 `None`。
- NDV 被限制在全局行数以内；直方图 bucket NDV 统一清零。
- `bucket_count == 0` 表示不执行截断，而不是产生零个桶。
- `write_global_stats` 的“继续写、返回最后错误”与 Go `WriteGlobalStatsToStorage` 的循环行为一致；它不提供原子提交或回滚，调用方必须接受部分成功。
- 错误类型被简化为字符串，缺少 Go 中 `ErrPartitionStatsMissing`、`ErrPartitionColumnStatsMissing` 的可分类错误、堆栈、告警和日志语义。这是当前移植边界，不应由调用方通过脆弱字符串匹配扩展。

## 并发与资源生命周期

本文件本身不创建线程、锁、通道、事务或异步任务。`concurrency` 只决定委托给哪个 TopN 合并函数以及并发批大小；真正的并发实现位于相邻模块。同步函数在栈上拥有工作向量，返回或报错时由 Rust 自动释放。

`AtomicBool` 由调用方拥有并以共享引用传入。读取使用 `Ordering::Acquire`，检查点位于 provider 调用前、每个分区开始时、每个统计项合并开始时，并继续传给 TopN 合并函数；因此它是协作式而非抢占式取消，单个 provider 调用或一次组件合并进行中不会被本函数强制中断。

内存峰值与分区数、项数及组件克隆大小相关：当前同步路径会先把所有有效分区项聚集在内存，再逐项合并。Go 源码将其称为可能 OOM 的旧 blocking 算法；Rust 版本同样没有流式释放所有输入的完整机制。异步包装只改变作业状态管理，并未改变核心函数的全量收集性质。

`write_global_stats` 顺序调用 writer，没有事务边界；一旦前项写入成功、后项失败，已完成的外部副作用不会由本文件撤销。

## 与 Go 版本的对应关系

主要对应关系如下：

- Rust `GlobalStats` 对应 Go `GlobalStats` 的 `Hg`、`Cms`、`TopN`、`Fms`、`MissingPartitionStats`、`Count`、`ModifyCount`；Rust 用向量长度代替 Go 的显式 `Num`。
- Rust `merge_partition_stats_to_global` 对应 Go `blockingMergePartitionStats2GlobalStats` 的核心收集与合并阶段：逐分区取得统计、判断缺失、计数一次、合并 FM/CMS/TopN/Histogram，并把全局 NDV 写到直方图。
- Rust `write_global_stats` 对应 Go `WriteGlobalStatsToStorage` 的逐项写入、跳过空直方图、继续尝试并返回最后错误。
- `global_stats_async.rs` 复用 Rust 核心；Go `MergePartitionStats2GlobalStats` 则根据会话变量在完整异步 worker 与 blocking 路径间选择。

未完全对应之处必须保留：Go 从 `TableInfo`/InfoSchema/StatsHandle 解析分区和 hist ID，跳过虚拟生成列，支持外部缓存，生成带表、列或索引名的类型化错误，读取会话默认参数，记录日志和 warning，并把统计版本及历史来源写入 KV。Rust provider 直接返回已解析的扁平快照，诊断只有分区名与项下标；writer 也没有 Go 持久化参数。Go 使用 `MergePartTopNAndHistToGlobal` 的完整直方图边界语义，而 Rust 当前模型明确不携带边界，只保留输入顺序并截断。

测试语义来源包括 Go `global_stats_test.go`/`global_stats_internal_test.go` 中的同步与异步展示、错误注入、健康度、NDV、DDL、问题 #24349、空直方图及路径一致性场景；Rust 独立测试把其中可由当前模型表达的计数、缺失、取消、串并行一致性、TopN/精确计数和最后写错误行为固化下来，但不能证明上述未接线的生产能力已经迁移。

## 扩展指南

- 新增统计组件时，应同时扩展 `PartitionItemStats`、`GlobalStats` 的对齐向量、初始化与逐项合并，并在 `global_stats_internal_test.rs` 增加至少一个多分区聚合用例；不能把测试嵌入生产源文件。
- 改变缺失判断时，优先修改 `missing_item_error` 与 `merge_partition_stats_to_global` 的两个独立判断，并同步 `global_stats_internal_test.rs::global_stats_merge_reports_missing_partition_column_stats`、`global_stats_test.rs::skip_missing_reports_unanalyzed_and_empty_item_like_go` 和 Go 对照测试。注意跳过模式允许同一项产生两条诊断。
- 调整 TopN 并发策略时，修改分支和批大小计算，并验证串行/并发结果一致、version 1 强制串行、取消传播及 256 上限；相关实现和测试还涉及 `topn.rs`、`merge_worker.rs`、`topn_test.rs`。
- 改进直方图桶合并前，必须先让 Rust 模型携带 Go 所需的值边界语义；仅改变排序或截断会改变估算含义。兼容风险集中在 bucket 顺序、TopN 淘汰值回灌和 NDV 上限。
- 接入真实生产路径时，不应继续扩大字符串 trait。应在上层适配 InfoSchema/StatsHandle、类型化错误、会话参数、日志/告警和持久化元数据，并保持本文件作为纯合并核心；同时新增独立集成测试证明 SQL/DDL 主链实际调用。
- 改动 `write_global_stats` 时要明确部分写入策略。若要求原子性，应由 writer 或更高层事务接口提供，不能假定当前循环会回滚。
- 性能评估应关注 `PartitionItemStats`、Histogram 与 TopN 的多次克隆和全分区驻留；优化所有权或流式处理时，需与 Go 的结果顺序和错误时机保持一致。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 Rust/Go 源和测试均已索引。
- RustCodeGraph `files --filter pkg/statistics/handle/globalstats`：确认目标文件、`lib.rs`、同步/异步实现、TopN/worker 与独立测试的模块范围。
- RustCodeGraph `node --file pkg/statistics/handle/globalstats/global_stats.rs`：核对 255 行源码中的常量、5 个公开数据/trait 类型、2 个内部 helper 和 2 个公开函数及全部控制流。
- RustCodeGraph `explore`、`query merge_partition_stats_to_global`、`query write_global_stats`：确认同步入口由 `global_stats_async.rs::merge` 和独立 Rust 测试调用；写回入口由内部测试直接验证。代码图没有给出 Rust SQL 主链的生产调用边。
- 已读 Rust 文件：`pkg/statistics/handle/globalstats/lib.rs`、`global_stats_async.rs`、`global_stats_test.rs`、`global_stats_internal_test.rs`；目标目录没有 `doc.go`。
- 已读 Cargo：`pkg/statistics/handle/globalstats/Cargo.toml`、`pkg/statistics/handle/Cargo.toml` 和根 `Cargo.toml` 的对应依赖段，确认 crate 入口、facade 成员和 `cfg(any())` 迁移边界。
- 已读 Go 对照：`pkg/statistics/handle/globalstats/global_stats.go`、`global_stats_test.go`、`global_stats_internal_test.go`，核对生产入口、blocking 合并、写回循环与主要回归场景。
- 人工事实复核：文档区分了当前 Rust 事实与 Go 完整能力，未把测试覆盖或 facade 注册误写为生产主链接线。
