# `pkg/statistics/constants.rs`

## 文件定位

本文说明的真实源文件为 [`pkg/statistics/constants.rs`](constants.rs)。它属于 `astersql-statistics` crate（见 `pkg/statistics/Cargo.toml`），保存统计模块与 Go `pkg/statistics/constants.go` 对齐的两个公开、编译期默认值。`pkg/statistics/lib.rs` 以私有模块 `mod constants` 装入文件，再通过 `pub use constants::*` 将常量提升为 crate 根 API，因此其他 crate 使用 `astersql_statistics::DefaultHistogramBuckets`，crate 内部也可通过 `crate::DefaultHistogramBuckets` 或根级通配导入访问。

它不是统计构建算法的实现文件：没有函数、类型、trait、`impl` 或条件编译项，也不持有直方图或 TopN 数据。实际构建逻辑位于 `builder.rs`、`runtime_stats_builder.rs` 等文件。

## 核心职责

- `DefaultTopNValue` 固定为 `100`，提供与 Go 统计包一致的 TopN 容量基线。
- `DefaultHistogramBuckets` 固定为 `256`，提供与 Go 统计包一致的直方图桶数基线。
- 两个常量以 `usize` 表示，适合直接传入 Rust 集合容量、切片长度以及统计构建接口；需要其他整数宽度时由调用点显式转换，例如 `merge_global_test.rs` 中转换为 `u32`。

需要区分固定基线与运行时活动配置：SQL `ANALYZE` 的常规默认选项由 `pkg/planner/core/planbuilder.rs::AnalyzeOptionDefault` 每次读取 `vardef::AnalyzeDefaultNumBuckets` 和 `vardef::AnalyzeDefaultNumTopN`；后两者是 `pkg/sessionctx/vardef/tidb_vars.rs` 中的原子值，可被系统变量更新。本文件常量不会随系统变量变化。

## 主要符号

- `pub const DefaultTopNValue: usize = 100`：统计包公开的固定 TopN 默认容量。`pkg/statistics/main_test.rs::package_constants_match_statistics_contract` 直接锁定值为 `100`；`builder_test.rs` 和 `merge_global_test.rs` 将它作为构建/合并场景输入。
- `pub const DefaultHistogramBuckets: usize = 256`：统计包公开的固定直方图默认桶数。`RuntimeStatsBuilder::build_histogram` 与 `BuildRuntimeTableStatsWithBuilder` 将它传给可指定桶数的下层入口；`main_test.rs` 锁定值为 `256`。

两者都是公开不可变常量，没有初始化函数、延迟计算或可见性更窄的辅助符号。

## 执行流程

1. 编译 `astersql-statistics` 时，`pkg/statistics/lib.rs` 声明 `mod constants`，编译器载入本文件的两个 `usize` 常量。
2. `pub use constants::*` 将其重导出到 crate 根；常量值通常在编译期直接内联，不产生运行时初始化流程。
3. 运行时统计构建路径需要默认桶数时，`RuntimeStatsBuilder::build_histogram` 调用 `build_histogram_with_buckets(..., crate::DefaultHistogramBuckets)`。
4. handle 层的 `BuildRuntimeTableStatsWithBuilder` 同样把 `astersql_statistics::DefaultHistogramBuckets` 传给 `BuildRuntimeTableStatsSelectionWithBuilder`，后者再构建列和索引统计。
5. `DefaultTopNValue` 当前在生产 Rust 源码中没有直接调用点；它作为公开兼容基线，并在独立测试中用于统计构建和全局合并边界。生产路径的 TopN 数量由调用者传入或从活动 `vardef` 配置读取。

## 数据与状态

本文件只包含两个进程内编译期标量，不分配堆内存、不引用外部资源，也不保存每表、每列或每会话状态。其值在一个已编译二进制中不可变。

`100` 表示最多请求的 TopN 条目数基线，不保证最终一定生成 100 项：实际数量受 NDV、样本分布与裁剪规则影响，`builder_test.rs::benchmark_build_hist_and_top_n_with_low_ndv_scenario` 即验证低 NDV 时结果少于该上限。`256` 是请求的桶数上限/默认输入，构建结果也可因数据量和聚合规则少于 256 个桶。

活动 ANALYZE 默认值存放在另一 crate 的 `AtomicU64Value`（`AnalyzeDefaultNumBuckets`、`AnalyzeDefaultNumTopN`），与本文件的不可变 `usize` 常量是两套用途不同但初始数值相同的状态。

## 依赖与调用关系

本文件自身没有 `use`、函数调用或第三方依赖。crate 边界由 `pkg/statistics/Cargo.toml` 定义为 `astersql-statistics`，库入口为 `lib.rs`；Cargo 中列出的 codec、types、stmtctx、vardef 等依赖供统计 crate 的其他实现使用，不是声明这两个常量所必需。

已核实的直接 Rust 使用关系如下：

- `pkg/statistics/lib.rs`：声明并公开重导出常量模块。
- `pkg/statistics/runtime_stats_builder.rs::RuntimeStatsBuilder::build_histogram` → `DefaultHistogramBuckets` → `build_histogram_with_buckets`。
- `pkg/statistics/handle/runtime_stats.rs::BuildRuntimeTableStatsWithBuilder` → `astersql_statistics::DefaultHistogramBuckets` → `BuildRuntimeTableStatsSelectionWithBuilder`。
- `pkg/statistics/main_test.rs`、`builder_test.rs`、`merge_global_test.rs`：验证数值契约及在构建、合并边界中的用途。

RustCodeGraph 的文件节点报告本文件“used by 0 files”，且无法按两个常量名返回独立节点；因此上述引用关系用全仓精确 `rg` 补齐。该图限制意味着不能把“0 files”解释为真实无调用者。

## 错误处理与边界

常量声明本身不会失败，也没有 `Result`、panic 或错误转换。错误与输入校验属于消费者：例如 `planbuilder.rs::handleAnalyzeOptions` 校验显式 `TOPN`/`BUCKETS` 上限，`BUCKETS` 还必须为正；这些约束不由本文件编码。

边界上，`usize` 宽度依赖目标平台。当前值很小，不存在转换溢出；若未来显著增大数值，所有转成 `u32`、`u64` 或其他类型的调用点都应重新审查。修改常量也不会自动修改 `vardef::DefTiDBAnalyzeDefaultNumBuckets` 或 `DefTiDBAnalyzeDefaultNumTopN`，从而可能造成固定基线与动态默认初值分叉。

## 并发与资源生命周期

两个 `const` 可被任意线程无锁读取，没有构造、销毁、所有权转移或资源释放过程，也不参与任务、通道、事务和锁的生命周期。

并发可变性只存在于文件外的 `vardef::AnalyzeDefaultNum*` 原子配置。调用 `AnalyzeOptionDefault` 时读取到的是当时的活动值；读取本文件常量始终得到编译期的 `100`/`256`。扩展时不得用本文件常量替代需要观察运行时系统变量更新的路径。

## 与 Go 版本的对应关系

`pkg/statistics/constants.go` 是直接对照文件，声明同名常量：`DefaultTopNValue = 100`、`DefaultHistogramBuckets = 256`。Rust 版本保持名称和数值一致，并额外显式指定 `usize` 类型；Go 常量本身是无类型整数，使用时按上下文转换。

Go 测试 `pkg/statistics/merge_global_cases_test.go` 使用 `statistics.DefaultTopNValue` 验证全局 TopN 合并与活动默认变化时的裁剪语义；Rust 的 `pkg/statistics/merge_global_test.rs::merge_singleton_filter_tracks_changed_active_default` 覆盖对应区分：活动默认被改成 150 时，传入固定常量 100 仍保留 100 个单例。Rust `main_test.rs` 还直接断言两个常量值。

两端都另有可配置的 ANALYZE 默认值；Rust 侧证据是 `tidb_vars.rs` 的 `DefTiDBAnalyzeDefaultNumBuckets = 256`、`DefTiDBAnalyzeDefaultNumTopN = 100` 及对应原子值。因此，同名统计常量与系统变量初值应保持数值兼容，但语义上不能视为同一存储位置。

## 扩展指南

- 调整任一固定值时，应同步检查 Go `pkg/statistics/constants.go`、Rust `pkg/sessionctx/vardef/tidb_vars.rs` 的 `DefTiDBAnalyzeDefaultNum*`，并确认产品意图是修改固定兼容基线、运行时系统变量初值，还是两者都修改。
- 同步更新独立测试 `pkg/statistics/main_test.rs` 的精确值断言，并检查 `builder_test.rs`、`merge_global_test.rs` 以及 Go `merge_global_cases_test.go` 的边界预期；Rust 测试不得内嵌到本文件。
- 新增统计默认常量时，在本文件声明并由既有 `lib.rs` 通配重导出即可；若该值可由系统变量更改，生产调用路径应读取 `vardef` 活动值，而不是把固定常量当作动态配置。
- 若消费者要求 `u32`/`u64`，优先在明确边界处进行经审查的转换。容量增加会提升 TopN 或直方图的内存、CPU 和持久化体积，应评估构建、合并、编码及查询估算性能。
- 保持 Go/Rust 命名与数值契约一致，并保留源文件现有 PingCAP Apache License 与 AsterSQL 处理标记。

## 验证依据

- 源码与装配：`pkg/statistics/constants.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/Cargo.toml`。
- 直接生产调用：`pkg/statistics/runtime_stats_builder.rs::RuntimeStatsBuilder::build_histogram`、`pkg/statistics/handle/runtime_stats.rs::BuildRuntimeTableStatsWithBuilder`。
- 动态默认边界：`pkg/planner/core/planbuilder.rs::AnalyzeOptionDefault` 与 `handleAnalyzeOptions`；`pkg/sessionctx/vardef/tidb_vars.rs` 的 `DefTiDBAnalyzeDefaultNum*`、`AnalyzeDefaultNum*`。
- Rust 独立测试：`pkg/statistics/main_test.rs::package_constants_match_statistics_contract`、`pkg/statistics/builder_test.rs::benchmark_build_hist_and_top_n_with_low_ndv_scenario`、`sampled_top_n_does_not_consume_every_sampled_value`、`pkg/statistics/merge_global_test.rs::merge_singleton_filter_tracks_changed_active_default`。
- Go 对照与测试：`pkg/statistics/constants.go`、`pkg/statistics/merge_global_cases_test.go`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件且包含 `pkg/statistics/constants.rs`；文件节点可读取完整 29 行源码，但常量名精确查询无节点、文件使用关系报告为 0，故再以全仓精确文本搜索验证实际引用。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前执行固定十一章节结构检查并人工复核符号、调用边、Go 对照、错误边界与并发生命周期描述。
