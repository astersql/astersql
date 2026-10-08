# `pkg/statistics/handle/globalstats/global_stats_async.rs`

## 文件定位

本文件属于 workspace crate `astersql-statistics-handle-globalstats`。crate 入口 `pkg/statistics/handle/globalstats/lib.rs` 将它声明为 `global_stats_async` 模块并公开再导出其符号；根 facade `pkg/lib.rs` 又在 `statistics::handle::globalstats` 下公开该 crate。它封装“一次把分区统计合并为全局统计”的任务状态，面向动态分区裁剪场景下的整表基数估计。

当前 Rust 文件是 Go `pkg/statistics/handle/globalstats/global_stats_async.go` 的轻量迁移模型，而不是 Go 双 worker 流水线的逐结构复刻：仓库代码搜索中，`AsyncMergePartitionStats` 的直接构造和调用只出现在 `global_stats_async_test.rs` 与 `global_stats_test.rs`；生产 crate/facade 虽公开了 API，但没有找到 Rust 业务入口对它的直接实例化。因此应把“已接入完整 Rust 应用主链”视为未验证，而不能从公开导出推断已接线。

## 核心职责

`AsyncMergePartitionStats<'a>` 负责四件事：保存分区统计提供者、全局表 ID 和待处理项数；在执行前清除旧结果；把 `MergeOptions` 与取消标志转交核心函数 `merge_partition_stats_to_global`；缓存成功结果及其中的缺失分区诊断列表。

本文件不实现具体的 Histogram、CMSketch、TopN 或 FMSketch 合并。实际算法、缺失统计判断、行数汇总以及 TopN 路径选择都在 `global_stats.rs::merge_partition_stats_to_global`。因此该文件的边界是“任务对象与结果访问”，不是“合并算法”。

## 主要符号

- `pub struct AsyncMergePartitionStats<'a>`：任务状态。生命周期 `'a` 保证 `provider: &'a dyn PartitionStatsProvider` 在任务存活期间有效；其余字段为 `table_id: i64`、`item_count: usize`、`result: Option<GlobalStats>` 和 `missing: Vec<String>`。
- `new(provider, table_id, item_count) -> Self`：建立未执行任务，结果为 `None`、缺失列表为空；它不访问 provider，也不启动后台工作。
- `prepare(&mut self) -> Result<(), String>`：当前为空操作并总是成功。注释明确 `item_count == 0` 合法，仍允许下游汇总分区级 `count`/`modify_count`。
- `merge(&mut self, options, cancelled) -> Result<(), String>`：唯一执行入口。它重置可观察状态、预检取消、调用核心合并，并只在成功后同时发布 `missing` 与 `result`。
- `result(&self) -> Option<&GlobalStats>`：借用式读取最近一次成功结果；构造后、失败后均返回 `None`。
- `missing_partitions(&self) -> &[String]`：读取最近一次成功结果的缺失诊断；构造后或失败后为空切片。

本文件没有模块级常量、trait、条件编译项或私有辅助函数。全部四个方法均公开，字段均为私有。

## 执行流程

1. 调用方以 `PartitionStatsProvider`、全局表 ID 和项数调用 `new`；此时没有 I/O 或计算。
2. 调用 `merge` 后，先将 `result` 置为 `None` 并清空 `missing`。这保证复用同一任务时不会在新失败后暴露旧成功结果。
3. 用 `cancelled.load(Ordering::Acquire)` 做入口取消检查；已取消则立即返回字符串错误 `query interrupted`，且不调用 provider。
4. 调用 `prepare`。当前实现无副作用；保留该阶段是为了对应 Go 的准备阶段和未来扩展。
5. 调用 `global_stats.rs::merge_partition_stats_to_global(provider, table_id, item_count, options, cancelled)`。该函数拉取所有分区，累加行数与修改数，逐项处理缺失统计，合并 FM/CM Sketch，并根据统计版本与并发度选择普通或分批 TopN 合并助手，最后构建 Histogram。
6. 仅在核心函数成功后，克隆 `result.missing_partition_stats` 到任务的 `missing`，再将完整 `GlobalStats` 放入 `result`，返回 `Ok(())`。
7. 调用方通过 `result` 和 `missing_partitions` 借用读取输出。两者不转移所有权，也不修改状态。

## 数据与状态

输入身份由 `provider`、`table_id`、`item_count` 固定在任务中；每次执行的策略由值传递的 `MergeOptions` 指定。`item_count` 决定 `GlobalStats` 中按项对齐的四组向量长度。即使为零，核心函数仍先调用 provider 并汇总分区的 `count` 与 `modify_count`；`global_stats_async_test.rs::async_merge_allows_an_empty_resolved_histogram_list` 对此有直接断言。

任务有清晰状态转换：`new` 后为“未执行”；`merge` 开始即回到“无结果、无缺失诊断”；成功后为 `Some(GlobalStats)` 与其缺失列表副本；任意错误后保持无结果和空缺失列表。`missing` 是结果字段的克隆，避免访问器依赖深入借用 `GlobalStats`，代价是成功时复制字符串向量。

`GlobalStats` 及 `MergeOptions` 的真实定义位于 `global_stats.rs`。缺失诊断并非本文件生成：核心函数在 `skip_missing` 为真时记录，反之立即返回错误。本文件只复制成功结果中的诊断。

## 依赖与调用关系

直接 Rust 依赖均来自同 crate：`GlobalStats`、`MergeOptions`、`PartitionStatsProvider` 和 `merge_partition_stats_to_global`；标准库依赖只有 `AtomicBool` 与 `Ordering`。`pkg/statistics/handle/globalstats/Cargo.toml` 指定 `lib.rs` 为 crate 根，并记录 Go 包映射。其大量尚未启用的迁移依赖位于 `target.'cfg(any())'.dependencies`，而本文件并不直接使用它们。

已验证的上游调用是两个独立测试模块：`global_stats_async_test.rs` 覆盖零项任务，`global_stats_test.rs` 覆盖常规异步封装、缺失统计与同步核心结果一致性。模块由 `lib.rs` 在 `#[cfg(test)]` 下装配。根 workspace 和 facade 会编译、再导出该 crate，但仓库搜索未发现 Rust 生产调用方直接构造该类型。

唯一直接下游调用边是 `merge -> prepare` 与 `merge -> merge_partition_stats_to_global`；后者再调用 provider，并进入 `global_stats.rs`/`topn.rs`/`merge_worker.rs` 的统计合并逻辑。读取方法只访问任务自身字段。

## 错误处理与边界

所有可失败阶段统一返回 `Result<_, String>`。入口取消固定返回 `query interrupted`；`prepare` 当前不会失败；provider、缺失统计或核心合并产生的字符串错误通过 `?` 原样传播。本文件没有日志、错误包装、重试或 panic 恢复。

关键边界包括：`item_count == 0` 仍执行 provider 和分区元信息汇总；预置取消在 prepare/provider 之前退出；核心函数还会在拉取前、逐分区和逐项合并阶段重复检查取消；缺失项在 `skip_missing == false` 时使本次任务失败，在为真时成功返回并记录诊断。任务复用时先清空状态，所以第二次执行失败不会泄露第一次结果。

本文件不校验负数 `table_id`、项数与 provider 返回的 `items` 长度是否一致，也不解释未知表 ID；这些都属于 provider 或核心函数的职责。`result()` 返回 `None` 本身不区分“从未执行”和“最近执行失败”。

## 并发与资源生命周期

虽然类型名含 `Async`，本文件没有 `async fn`、线程、任务、channel、锁或 worker。`merge` 在调用线程上同步完成。`MergeOptions::concurrency` 只传给核心函数：统计版本不是 1 且并发度至少 2 时，核心函数选择名为 `merge_global_top_n_by_concurrency` 的分批助手；但 `topn.rs` 当前实现仍在 `while` 循环中逐批同步调用 `TopNStatsMergeWorker::run_task`，没有创建并行 worker。因此该参数目前影响分支和批大小，不代表本文件或该助手实际并行执行。

取消使用借用的 `AtomicBool`，入口以 Acquire 顺序读取；核心函数也持有同一引用并在多个阶段协作式检查。这里没有拥有取消标志，调用方须保证其在 `merge` 返回前有效。provider 同样是借用而非拥有，生命周期 `'a` 将这一资源关系编码在类型中。

`result` 和 `missing` 由任务拥有并在任务析构时自动释放。成功发布顺序是先克隆缺失列表、后写入 `result`；由于 `&mut self` 排除了同一任务的并发方法调用，不需要内部同步。类型是否可跨线程取决于 trait object 的具体实现；`PartitionStatsProvider` 本身未声明 `Send` 或 `Sync`，本文件不承诺跨线程共享。

## 与 Go 版本的对应关系

Rust `AsyncMergePartitionStats` 对应 Go `AsyncMergePartitionStats2GlobalStats` 的外层职责：构造任务、准备、执行合并、保存结果并提供读取入口。Rust 的 `new`/`merge`/`result` 分别近似对应 Go 的 `NewAsyncMergePartitionStats2GlobalStats`、`MergePartitionStats2GlobalStats`、`Result`。

两者不是结构等价实现。Go `prepare` 会解析空 `histIDs`、查询分区元数据、累计 Count/ModifyCount、建立跳过集合；Rust `prepare` 是空操作，这些输入已被抽象为 `PartitionStatsProvider` 和 `item_count`，实质工作在共享核心函数完成。Go 用会话池、两个 `errgroup`、FMSketch/CMSketch/Histogram+TopN 通道以及 I/O/CPU 互相退出通道形成真正并发流水线；Rust 封装和当前 TopN 分批助手都同步执行，尚未复刻该并发结构。

Go 还区分索引/列、读取系统表、处理 failpoint、恢复 worker panic，并通过调用方 SQLKiller 响应用户中断；Rust 模型只接受原子取消标志和内存 provider，没有会话、存储、panic 恢复及 failpoint。Go 入口 `global_stats.go::MergePartitionStats2GlobalStats` 在 `EnableAsyncMergeGlobalStats` 开启时构造并调用 worker，证明 Go 的生产接线；Rust 当前只有 facade 暴露和测试调用证据。因此文档中的 Rust 行为应理解为可测试的移植模型，不应宣称已复刻 Go 的异步 I/O 架构。

测试语义仍刻意保持关键一致性：零直方图项仍汇总表级计数；取消错误文案稳定；允许跳过缺失项时同步核心与异步封装返回相同诊断；Go 的并发错误/panic 测试在 Rust `global_stats_test.rs` 中被简化为预置取消传播测试，该差异已由测试注释明确说明。

## 扩展指南

若新增任务级参数或准备逻辑，优先修改 `AsyncMergePartitionStats` 字段、`new` 与 `prepare`，并保持 `merge` 在执行前清除旧状态的不变量。若改变统计算法、缺失项规则、版本/并发选择或结果布局，应修改 `global_stats.rs::merge_partition_stats_to_global` 而不是把算法复制进本文件。

若要真正对齐 Go 异步流水线，需要先决定 Rust 存储/会话抽象和 worker 生命周期，再设计 `Send + Sync` 边界、错误聚合、双向退出、panic/取消语义；不能仅把 `merge` 改成 `async fn`。这会跨越当前文件的轻量封装范围，并应同步独立测试，尤其覆盖 I/O 先失败、CPU 先失败、同时失败、通道关闭和取消竞态。

当前文件的直接回归测试应继续放在独立文件 `pkg/statistics/handle/globalstats/global_stats_async_test.rs`，不要内嵌进生产源。跨同步/异步一致性、缺失统计与 TopN 行为可扩展 `global_stats_test.rs`。建议新增任务复用测试：先成功、后取消/失败，验证 `result() == None` 且缺失列表为空；若改变 `prepare`，还应增加 provider 未被错误提前调用以及零项行为测试。

兼容风险主要是公开构造/访问 API、固定取消错误文本和缺失诊断顺序；正确性风险集中在旧结果清理、零项计数和取消传播；性能风险集中在成功时克隆缺失字符串以及把重计算法错误放入本封装。对 Go 对齐的任何增强都需避免把存储读取与 CPU 合并串行化，或引入 worker 无法互相退出的泄漏。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/statistics/handle/globalstats` 确认目标、核心实现、模块入口和独立测试均已索引；`node --file global_stats_async.rs` 列出完整 88 行及 9 个符号，并报告该文件被 7 个索引文件引用。精确方法级 callers/callees 未产生可用输出，因此调用边又由已索引相邻源码和仓库直接引用搜索核对。
- 目标源码：`pkg/statistics/handle/globalstats/global_stats_async.rs`，核对任务字段、四个公开方法、Acquire 取消检查、状态清理及唯一核心调用。
- 核心 Rust 实现：`pkg/statistics/handle/globalstats/global_stats.rs`，核对 `GlobalStats`、`MergeOptions`、`PartitionStatsProvider`、取消检查、缺失处理、行数汇总和 TopN 分支选择；`topn.rs::merge_global_top_n_by_concurrency` 核对所谓 concurrency 路径当前仍为顺序分批循环。
- crate 与 facade：`pkg/statistics/handle/globalstats/Cargo.toml`、`pkg/statistics/handle/globalstats/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`，核对 crate 边界、workspace 成员和公开再导出；目标包下没有 `doc.go`。
- Rust 测试：`pkg/statistics/handle/globalstats/global_stats_async_test.rs::async_merge_allows_an_empty_resolved_histogram_list`；`global_stats_test.rs::test_show_global_stats_with_async_merge_global`、`run_global_stats_failpoint_case`、`empty_histograms_consume_current_default_map`。
- Go 对照：`pkg/statistics/handle/globalstats/global_stats_async.go` 的同名 worker、`prepare`、`ioWorker`、`cpuWorker`、`MergePartitionStats2GlobalStats` 及各 load/deal 方法；`global_stats.go::MergePartitionStats2GlobalStats` 的生产入口；`global_stats_test.go` 的 worker panic/错误 failpoint 测试。
- 人工复核结论：文档区分了本文件的任务封装与下游算法，明确当前 Rust 接线证据上限、Go/Rust 并发差异、失败后的状态不变量，以及新增功能应修改的符号和独立测试位置。
