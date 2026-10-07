# `br/pkg/restore/log_client/log_split_strategy.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` library crate；crate 入口 `br/pkg/restore/log_client/lib.rs` 以 `pub mod log_split_strategy` 挂载并通过 `pub use log_split_strategy::*` 扁平导出。它位于日志恢复（PiTR）的 DML 文件扫描与 Region 预切分之间：把 `LogDataFileInfo` 过滤、按表累积成 `SplitHelper` 区间，并在文件数达到批次阈值时建议上层执行拆分。

当前 Rust 接线与 Go 不完全相同。Rust 的直接生产调用者是 `LogClient::PreSplitRegions`（`client.rs:579`），该调用者会执行过滤、累积和阈值判断，但阈值命中后只调用 `BaseSplitStrategy::ResetAccumulations`，并未把 `GetAccumulations` 交给 `PipelineRegionsSplitter::ExecuteRegions`。Go 的 `WrapLogFilesIterWithSplitHelper` 则把策略交给 `PipelineRestorerWrapper.WithSplit`，会实际执行 split/scatter。因此，本文件提供了拆分决策所需状态，不能据此宣称 Rust 预切分链已经完整执行 Region 拆分。

## 核心职责

- `SplitFileThresholdDefault` 把默认参与拆分统计的最小文件长度固定为 1 MiB；`Length <= threshold` 的文件不会累积，避免大量小文件使区间树和 split/scatter 调用膨胀。
- `NewLogSplitStrategy` 从 rewrite 规则提取下游表 ID，并在启用 checkpoint 时仅载入这些表的已完成文件偏移，避免已删除或本次不恢复的表污染跳过集合。
- `ShouldSkip` 在累积前排除 meta 文件、没有 rewrite 规则的表文件和 checkpoint 已完成文件；checkpoint 命中时仍回调进度，保证恢复统计包含重启前已完成的工作。
- `Accumulate` 按上游 `TableId` 懒建 `SplitHelper`，合并文件键范围、字节数和 entry 数，为后续按容量/键数选择分裂点提供输入。
- `ShouldSplit` 以已累积文件数大于 4096 为批次边界；它只返回建议，不执行拆分。
- `maybeUpdateMemUsage` 最多每 30 秒遍历一次所有 helper 节点，并更新 `KV_SPLIT_HELPER_MEM_USAGE` 指标。

## 主要符号

- `pub const SplitFileThresholdDefault: u64 = 1024 * 1024`：默认文件大小门槛。判断使用 `<=`，所以恰好 1 MiB 也被排除。
- `pub struct LogSplitStrategy`：策略状态容器。
  - `base: BaseSplitStrategy` 保存 `AccumulateCount`、按上游表 ID 分组的 `TableSplitter` 和 `Rules`。
  - `checkpointSkipMap: LogFilesSkipMap` 保存 meta group、group offset、merged-file offset 三级 checkpoint 命中关系。
  - `checkpointFileProgressFn: Box<dyn FnMut(u64, u64) + Send>` 在跳过已完成文件时报告 entry 数与字节数；`Send` 允许策略随流水线跨线程转移，但不表示本文件自行并发调用它。
  - `splitFileThreshold: u64` 是构造时注入的阈值。
  - `lastMemUsageUpdate: Instant` 是内存指标的节流时钟，私有以维持刷新不变量。
- `NewLogSplitStrategy(...) -> Result<LogSplitStrategy>`：公开构造函数。参数包括上下文、checkpoint 开关与 manager、rewrite 规则、进度回调和文件阈值。
- `LogSplitStrategy::Accumulate(&mut self, &LogDataFileInfo)`：把一个足够大的数据文件合并到表级 helper。
- `LogSplitStrategy::ShouldSplit(&self) -> bool`：当 `AccumulateCount > 4096` 时返回真；4096 本身不触发。
- `LogSplitStrategy::ShouldSkip(&mut self, &LogDataFileInfo) -> bool`：执行 meta、rewrite 范围和 checkpoint 三层过滤。之所以需要 `&mut self`，是 checkpoint 命中时会调用 `FnMut` 进度回调。
- `LogSplitStrategy::maybeUpdateMemUsage(&mut self)`：私有指标刷新函数，不改变拆分语义。

与 Go 的编译期接口断言不同，Rust 文件没有为 `LogSplitStrategy` 实现 `splitter::SplitStrategy<LogDataFileInfo>`；上层当前直接调用其固有方法，并直接访问 `base` 完成重置。

## 执行流程

1. 上层准备旧表 ID 到 `RewriteRules` 的映射并调用 `NewLogSplitStrategy`。
2. 构造函数遍历规则，把每条规则的 `NewTableID` 放入 `downstreamIdset`。
3. 若 `useCheckpoint` 为真，构造函数要求 manager 存在，调用 `LoadCheckpointData`；每条 checkpoint 数据只为命中 `downstreamIdset` 的 table ID 把 `(groupKey, Goff, foff)` 插入 skip map。加载完成后用返回时间调用 `summary::AdjustStartTimeToEarlierTime`。
4. 构造函数创建 `BaseSplitStrategy`，保存回调和阈值，并把 `lastMemUsageUpdate` 初始化为当前时间前 60 秒，使第一次有效累积可立即刷新内存指标。
5. `LogClient::PreSplitRegions` 从 `LogFileManager::LoadDMLFiles` 获取迭代器。每个文件先经过 `ShouldSkip`：meta 文件直接跳过；无规则文件记录 info 日志后跳过；checkpoint 命中文件报告进度后跳过。
6. 未跳过文件进入 `Accumulate`。长度不超过阈值时立即返回；否则计数加一，按 `TableId` 获取或创建 helper，并把 `[StartKey, EndKey)` 及 `Length`、`NumberOfEntries` 合并进去。
7. 有效累积结束时调用 `maybeUpdateMemUsage`；距离上次刷新至少 30 秒才遍历各 helper 的 `Valued::MemSize` 并写 gauge。
8. 上层读取 `ShouldSplit`。当前 Rust `PreSplitRegions` 在大于 4096 个有效文件后清空 helper 与计数，尚未消费累积结果执行实际 Region split；Go wrapper 会在相应位置执行拆分后再重置。

## 数据与状态

策略以两套表 ID 视角工作：`BaseSplitStrategy::Rules` 和 `TableSplitter` 使用文件中的上游 `TableId`；checkpoint 的 `Foffs` 使用 rewrite 后的下游 table ID，因此构造函数用 `RewriteRules::NewTableID` 过滤 checkpoint。混淆两者会导致应恢复文件被误跳过，或 checkpoint 无法命中。

每个 `Valued` 由文件 `StartKey`、`EndKey` 组成 `Span`，并携带 `Value { Size: Length, Number: NumberOfEntries }`。`SplitHelper::Merge` 负责合并重叠/相邻区间的权重；本文件不复制文件内容，也不持有文件对象。`AccumulateCount` 统计的是超过大小阈值且被上层判定为不跳过后传入的文件次数，不是 helper 节点数、entry 数或表数。

checkpoint skip map 在构造后只读查询。进度回调仅在 checkpoint 命中时执行；meta、无规则和小文件分支都不会调用它。指标时间戳使用单调 `Instant`，不受系统墙钟回拨影响。首次更新时间被人为回拨 60 秒，而后采用 30 秒最小间隔。

## 依赖与调用关系

上游关系经 RustCodeGraph 核验：

- `client.rs::PreSplitRegions` 调用 `NewLogSplitStrategy`，随后依次调用 `ShouldSkip`、`Accumulate`、`ShouldSplit`，并在阈值命中时调用 `strategy.base.ResetAccumulations`。
- `log_split_strategy_test.rs` 直接构造策略，覆盖阈值、checkpoint 过滤与缺少 manager 的错误。
- `parity_test.rs::go_rust_public_contract_matches` 对照 Go 契约，验证 checkpoint offset、大小阈值和累积计数。
- `client_test.rs::test_log_split_strategy` 验证 `PreSplitRegions` 入口能遍历安装好的文件管理器并完成策略循环。

下游依赖包括：

- `astersql_br_pkg_restore_split::splitter::{BaseSplitStrategy, NewBaseSplitStrategy}`：规则、计数、表级 helper 及导出/重置能力。
- `astersql_br_pkg_restore_split::sum_sorted::{NewSplitHelper, Span, Value, Valued}`：有权区间的合并与内存大小统计。
- `RewriteRules`：连接上游表 ID、下游表 ID 和键重写语义。
- `LogFilesSkipMap` 与 checkpoint stubs：恢复已完成文件的索引。
- `summary`、`metrics`、`log`：恢复起始时间、内存 gauge 和无规则文件日志。

`Cargo.toml` 将该目录定义为独立 library crate，直接依赖本地 `restore-split`、`restore-utils`、`checkpoint`、`stream`、`utils-iter` 等 crate；无 feature 条件控制本文件。`lib.rs` 对测试文件使用 `#[cfg(test)]` 与 `#[path = "log_split_strategy_test.rs"]` 分离挂载，生产逻辑与测试没有混在同一 Rust 文件。

## 错误处理与边界

- `useCheckpoint == true` 且 manager 为 `None` 时，构造立即返回 `Error("checkpoint enabled but manager is None")`；独立 Rust 测试对此有回归断言。Go 版本依赖接口调用在 nil 情况下失败/崩溃，Rust 在边界处给出显式错误，属于更明确的防御性差异。
- `LoadCheckpointData` 的错误通过 `?` 原样传播，策略不会以部分 skip map 继续运行；`AdjustStartTimeToEarlierTime` 只在加载成功后执行。
- checkpoint 回调内部当前不会自行产生错误，单条记录的过滤与插入完成后返回 `Ok(())`。
- 文件长度边界是严格“大于”阈值；恰好等于默认 1 MiB 不累积。拆分计数边界同样严格：4096 不触发，4097 触发。
- `ShouldSkip` 必须先于 `Accumulate` 由上层调用；`Accumulate` 本身不校验 meta、rewrite rule 或 checkpoint。若调用顺序被绕过，无规则表也可能进入 `TableSplitter`，随后 `BaseSplitStrategy::GetAccumulations` 会把这种状态视为不可达错误。
- 键范围合法性、区间合并及整数累计语义委托给 `SplitHelper`；本文件没有验证 `StartKey < EndKey`。文件计数使用 `i32`，极端超过其范围的输入风险没有在本文件处理。
- `checkpointFileProgressFn` 没有 `Result` 返回值；回调无法用正常错误通道中止恢复，panic 也未在此捕获。

## 并发与资源生命周期

`LogSplitStrategy` 由上层以 `&mut self` 顺序驱动，不创建线程、异步任务、通道、锁、文件句柄或网络连接。`checkpointFileProgressFn` 被要求 `Send`，便于整个策略转移到流水线执行环境；它不要求 `Sync`，也没有并发调用保证。测试若需从闭包外观察进度，应像 `log_split_strategy_test.rs` 一样使用 `Arc<AtomicU64>`，而不是依赖非同步共享可变状态。

构造期间对 checkpoint manager 的借用只持续到 `LoadCheckpointData` 返回；策略不会保存 manager 引用。规则和 skip map 由策略拥有，文件键在 `Accumulate` 时克隆进 helper，因此源 `LogDataFileInfo` 可在调用后释放。指标刷新遍历成本与累计 helper 节点数相关，30 秒节流避免每个文件都做全量遍历；重置 `base` 会释放当前表级 helper 内容，但不会清空 checkpoint map、回调、规则或指标时间戳。

## 与 Go 版本的对应关系

`log_split_strategy.rs` 逐项对应同目录 `log_split_strategy.go`：常量值、下游 ID checkpoint 过滤、`Valued` 字段、4096 阈值、skip 顺序、checkpoint 进度回调及 30 秒指标节流均保持一致。Rust 用 `HashMap<i64, RewriteRules>` 持有规则值，Go 使用 `map[int64]*RewriteRules`；Rust 的 `Box<dyn FnMut + Send>` 对应 Go 函数值。

可观察差异如下：

- Rust 在启用 checkpoint 但缺少 manager 时返回显式 `Result::Err`；Go 构造函数没有同等的 nil guard。
- Rust 把 `lastMemUsageUpdate` 初始化为 60 秒前，确保第一次有效累积刷新指标；Go 零值 `time.Time` 同样会使首次调用通过 30 秒检查，效果一致。
- Go 用 `var _ split.SplitStrategy[*LogDataFileInfo] = &LogSplitStrategy{}` 强制接口匹配；Rust 没有实现对应 trait，而是使用同名固有方法。
- Go `ShouldSkip` 的无规则日志包含 table ID 字段，Rust stub 日志目前只有固定文本，诊断信息较少。
- 最关键的接线差异在上层：Go `WrapLogFilesIterWithSplitHelper` 会通过 pipeline wrapper 执行 Region split；Rust `PreSplitRegions` 当前只在阈值处重置累积。因此策略算法虽已移植，完整副作用尚未对齐。

Go 的 `client_test.go::TestLogSplitStrategy` 还验证实际 mock PD region 边界变化和 wrapper 过滤顺序；Rust 独立测试目前主要验证本文件状态转换，没有等价证明实际 split/scatter 的集成结果。

## 扩展指南

- 调整文件大小或文件数阈值时，应同步 `SplitFileThresholdDefault`、`ShouldSplit`、同目录 Go 文件及 `log_split_strategy_test.rs` 的 `threshold`/4096/4097 边界断言；还要评估 helper 内存、split 数量与 scatter 压力。
- 扩充 checkpoint 语义时，优先修改 `NewLogSplitStrategy` 的下游 ID 过滤和 `ShouldSkip` 的命中处理，并同步 `log_file_map.rs`。必须保留“已完成文件仍计入进度”契约，以及 dropped downstream table 不得误命中的测试。
- 接通真实 Rust Region 拆分时，应在上层消费 `base.GetAccumulations()` 并调用 `PipelineRegionsSplitter::ExecuteRegions`，只在执行成功后重置；需要新增独立集成测试验证 split 调用、错误传播、尾批刷新以及不会重复 split，而不是在本生产文件内嵌测试。
- 若要让通用 pipeline 接受本策略，应实现与所有权模型匹配的 `SplitStrategy<LogDataFileInfo>`（或引用类型变体），并处理 `ShouldSkip` 需要可变回调而现有 trait 签名为 `&self` 的冲突；不要仅为满足 trait 删除 checkpoint 进度副作用。
- 若增加并发累积，需要重新设计 `TableSplitter`、`FnMut` 回调和指标时间戳的同步边界；简单包裹全局锁可能把区间合并变成热点。
- 若改善错误诊断，可让 Rust 无规则日志带 `TableId`，但应保持跳过行为不变并同步 Go/Rust parity 期望。

相关测试应继续放在独立文件 `br/pkg/restore/log_client/log_split_strategy_test.rs`；跨模块公开契约可补充到 `parity_test.rs`，真实拆分链的行为则应在 client/pipeline 对应的独立测试中覆盖。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标文件可解析。
- RustCodeGraph `node --file br/pkg/restore/log_client/log_split_strategy.rs`：核对全部 161 行、所有字段、函数、分支和 `Accumulate -> maybeUpdateMemUsage` 内部调用。
- RustCodeGraph `explore "br/pkg/restore/log_client/log_split_strategy.rs LogSplitStrategy LogSplitHelper iterator traverse"`：确认目标文件被 `client.rs`、`client_test.rs`、`log_split_strategy_test.rs`、`parity_test.rs` 使用，并确认 `PreSplitRegions -> NewLogSplitStrategy` 调用边。
- RustCodeGraph `node --file br/pkg/restore/log_client/client.rs --offset 540 --limit 120`：核对 Rust 上层 `PreSplitRegions` 的加载、过滤、累积、阈值判断与仅重置的当前接线。
- RustCodeGraph `node --file br/pkg/restore/split/splitter.rs --offset 1 --limit 250`：核对 `SplitStrategy` trait、`BaseSplitStrategy` 状态、`GetAccumulations` 和 `ResetAccumulations`。
- RustCodeGraph `node` 读取 `log_split_strategy_test.rs` 及 `parity_test.rs:350-439`：核对 1 MiB、4096/4097、meta/无规则/checkpoint、dropped table、进度回调和缺少 manager 的测试证据。
- 读取 `br/pkg/restore/log_client/Cargo.toml` 与 `lib.rs`：核对 crate 边界、直接依赖、公开再导出和独立测试挂载；该目录不存在 `doc.go`，无额外包契约文件可读。
- 读取 `br/pkg/restore/log_client/log_split_strategy.go`、`client.go` 与 `client_test.go::TestLogSplitStrategy`：核对 Go 算法、pipeline 接线和 mock PD 集成语义。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核所有“当前已支持”陈述均有上述源码或测试证据。
