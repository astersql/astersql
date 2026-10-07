# `br/pkg/restore/log_client/compacted_file_strategy.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` library crate；crate 边界由 [`Cargo.toml`](Cargo.toml) 声明，根模块 [`lib.rs`](lib.rs) 通过 `#[path = "compacted_file_strategy.rs"]` 挂载并扁平再导出其公开符号。它对应 Go 包 `br/pkg/restore/log_client` 中的 [`compacted_file_strategy.go`](compacted_file_strategy.go)，处理 PiTR/日志还原里已压缩 SST 集合的 region 预切分统计和 checkpoint 过滤。

当前 Rust 文件是“策略核心已移植，生产 pipeline 接线尚未完成”的状态：[`client.rs`](client.rs) 导入了 `NewCompactedFileSplitStrategy`，但没有构造或调用它；`PreSplitRegions` 实际构造的是 `NewLogSplitStrategy`。目标类型也尚未实现 `astersql_br_pkg_restore_split::splitter::SplitStrategy<T>`，而 Go `LogClient.WrapCompactedFilesIterWithSplitHelper` 已将同名策略交给 `PipelineRestorerWrapper.WithSplit`。因此不应将 Go 的生产调用链当作 Rust 已接线的事实。

## 核心职责

1. 以“有效表 ID”为键，为每张逻辑目标表维护一个 `SplitHelper`，将 SST 键区间及估算后的 KV 数/字节数合并进区间统计。
2. 对 compacted SST 的 MVCC 放大效应使用 `impactFactor = 16` 稀释数量和大小，避免把压缩产物的逻辑条目数直接当成切分压力。
3. 按 rewrite rule 判断一组 SST 是否属于本次还原，并从集合中原地剔除 checkpoint 已完成的文件。
4. 在跳过 checkpoint 文件时回调其 KV 数和大小，使已恢复数据仍进入外层进度统计。
5. 以严格大于 `4096 / 16` 的已累计文件数作为触发信号，但本文件不执行 region split，也不负责清空累计状态。

## 主要符号

- `const impactFactor: i64 = 16`：Go 对齐的稀释因子，同时参与 `Accumulate` 的 value 换算和 `ShouldSplit` 的阈值计算。
- `pub struct CompactedFileSplitStrategy`：持有 `base: BaseSplitStrategy`、`checkpointSets: HashSet<String>` 和 `checkpointFileProgressFn: Box<dyn FnMut(u64, u64) + Send>`。`base` 包含 rewrite rules、按表的 splitter 以及累计文件数。
- `pub fn NewCompactedFileSplitStrategy(...) -> CompactedFileSplitStrategy`：将 rules 交给 `NewBaseSplitStrategy`，并取得 checkpoint 集合与可变进度回调的所有权。函数本身不验证 rules 和 callback。
- `struct SstIdentity`：文件内部的临时识别结果。`EffectiveID` 选择 splitter 分组，`RewriteBoundary` 是传给 `GetRewriteRawKeys` 的可选改写边界。
- `fn inspect(&self, ssts: &dyn SSTs) -> SstIdentity`：若 `as_rewritten()` 存在且 `RewrittenTo() != TableID()`，以改写目标为有效 ID，并用 `GetRewriteRuleOfTable` 构造单表边界；否则使用原 `TableID()` 且不带边界。
- `pub fn Accumulate(&mut self, ssts: &dyn SSTs)`：懒创建按有效 ID 索引的 `SplitHelper`，遍历 `GetSSTs()` 并累计每个文件。
- `pub fn ShouldSplit(&self) -> bool`：当 `base.AccumulateCount > 256` 时返回 `true`；等于 256 时仍为 `false`。
- `pub fn ShouldSkip(&mut self, ssts: &mut dyn SSTs) -> bool`：无目标 rule 时跳过整组；有 rule 时过滤 checkpoint 文件，必要时通过 `SetSSTs` 改写输入对象。
- `pub fn TableSplitter(&self) -> &HashMap<i64, SplitHelper>`：只读暴露已累计的按表区间统计，供外层生成 split keys。
- `fn hasRule<T>(ssts: &dyn SSTs, rules: &HashMap<i64, T>) -> bool`：对 `RewrittenSSTs` 只查 `RewrittenTo()`，对普通 SST 查 `TableID()`，避免改写后的逻辑表误命中另一张物理 ID 相同的表。

## 执行流程

1. 调用方先用 `NewCompactedFileSplitStrategy` 注入改写规则、已完成文件名和进度回调。在 Go 主链中，这一步由 `WrapCompactedFilesIterWithSplitHelper` 完成；Rust 尚无对等 wrapper 调用。
2. 每组 SST 在累计前应经过 `ShouldSkip`：
   - `hasRule` 为假时，记录警告并返回 `true`，不调用 checkpoint 进度回调。
   - 命中 checkpoint 的文件不进入输出列表，而是将 `(TotalKvs, Size_)` 传给 `checkpointFileProgressFn`。
   - 全部命中时返回 `true`，但不用空列表覆盖原 `SSTs`；部分命中时用 `SetSSTs` 仅保留未完成文件并返回 `false`；零命中直接返回 `false`。
3. `Accumulate` 先通过 `inspect` 得到有效表 ID 和可选改写边界，然后懒创建该表的 `SplitHelper`。
4. 对每个 SST 调用 `GetRewriteRawKeys`。成功时取改写后的起止键；`Option::None` 会被 `unwrap_or_default()` 转成空 key，而 `SplitHelper::Merge` 会忽略空起止键的输入。
5. 每个文件都先使 `AccumulateCount += 1`。`TotalKvs == 0` 或 `Size_ == 0` 的文件不 merge，但仍占一个阈值计数。
6. 非空文件的 KV 数和大小分别整数除以 16；只要原值非零，换算结果最小提升为 1。随后将 `Span { StartKey, EndKey }` 和 `Value { Size, Number }` 交给 `SplitHelper::Merge`。
7. 外层可调用 `ShouldSplit`检查严格阈值，并通过 `TableSplitter` 读取累计结果。触发后如何生成 split keys、执行 region split 及 reset 属于外层职责。

## 数据与状态

- `base.Rules` 是表 ID 到 `RewriteRules` 的只读逻辑输入。`hasRule` 的查找 ID 与 `inspect` 的 `EffectiveID` 都优先采用 `RewrittenTo()`，但 `inspect` 仅在目标 ID 不同时构造 rewrite boundary。
- `base.TableSplitter` 是 `HashMap<i64, SplitHelper>`，每张有效表的 helper 内部用起始 key 索引区间。重叠区间由 `SplitHelper::Merge` 合并，本文件只构造 `Valued`。
- `base.AccumulateCount` 统计看到的 SST 文件数，不是稀释后的 KV 数，也不会因空文件而减少。注释中的“等效文件”不应误解为存储了另一个稀释计数器。
- `checkpointSets` 只以 `File.Name` 判断已恢复状态；它在构造后不在本文件内更新。
- `checkpointFileProgressFn` 是 `FnMut`，因此调用会改变策略内部状态，这也是 Rust `ShouldSkip` 需要 `&mut self` 的原因之一。
- `ShouldSkip` 还可通过 `SSTs::SetSSTs` 改变输入集合。调用方必须在其之后使用同一个对象进行 `Accumulate`/导入，才能避免重复处理 checkpoint 文件。

## 依赖与调用关系

- 上游 Rust 可见性：[`lib.rs`](lib.rs) 将模块及公开符号再导出。RustCodeGraph 显示 `NewCompactedFileSplitStrategy` 的 Rust 调用者为 [`compacted_file_strategy_test.rs`](compacted_file_strategy_test.rs)、[`client_test.rs`](client_test.rs) 和 [`parity_test.rs`](parity_test.rs) 中的测试；未找到生产构造调用。
- 上游通用协议：`astersql-br-pkg-restore-split` 的 `SplitStrategy<T>` 要求 `Accumulate`、`ShouldSplit`、`ShouldSkip`、`GetAccumulations` 和 `ResetAccumulations`。本类型只提供部分同名 inherent methods，签名也不完全匹配，所以目前不能直接交给 Rust 通用 splitter pipeline。
- 下游 split 依赖：`NewBaseSplitStrategy`、`NewSplitHelper`、`SplitHelper::Merge`、`Span`、`Value` 和 `Valued` 来自 `astersql-br-pkg-restore-split`。
- 下游 rewrite 依赖：`GetRewriteRuleOfTable`、`GetRewriteRawKeys` 和 `RewriteRules` 来自 `astersql-br-pkg-restore-utils`。`GetRewriteRawKeys` 会验证起止 key 的表 ID 一致性及规则命中情况。
- 数据抽象：[`ssts.rs`](ssts.rs) 的 `SSTs` 提供 `TableID`/`GetSSTs`/`SetSSTs`/`as_rewritten`，`RewrittenSSTs` 提供 `RewrittenTo`。这使策略同时能处理 `CompactedSSTs` 和带逻辑改写身份的 `CopiedSST`。
- crate 声明：[`Cargo.toml`](Cargo.toml) 将 `../split` 映射为 `astersql-br-pkg-restore-split`，将 `../utils` 映射为 `astersql-br-pkg-restore-utils`，两者都是 workspace path dependency；本文件未直接使用 feature gate 或条件编译。

## 错误处理与边界

- `GetRewriteRawKeys` 返回错误时，`Accumulate` 通过 `log::Panic` 中止，将“SST 与已选 rewrite boundary 不匹配”视为不可达的内部不变式破坏，而不是可恢复的 `Result` 错误。这与 Go 的 `log.Panic` 分支对齐。
- `GetRewriteRawKeys` 成功但返回 `None` 边界时，Rust 将其变为空 key；下游 `SplitHelper::Merge` 会丢弃起始或结束 key 为空的区间。这条路径不会返回错误，但 `AccumulateCount` 已增加。
- `TotalKvs == 0 || Size_ == 0` 采用“不 merge，但计文件数”的语义。这会让大量空/不完整元数据的文件仍可触发 `ShouldSplit`。
- 小于 16 但非零的 KV 数或大小分别下限为 1，保证小 subcompaction 不会在整数除法中完全消失。
- split 阈值使用严格 `>`：256 不切，257 才切。修改比较符会改变 Go 兼容行为。
- 无 rule 分支在 checkpoint 过滤之前返回，所以即使文件名命中 checkpoint，也不会调用进度回调。
- 全部 checkpoint 命中时不清空输入 `SSTs`；正确性依赖调用方看到 `true` 后立即跳过该集合。
- 构造器允许空 rules 和空 checkpoint set，但 callback 在 Rust 类型上必须是有效 `Box<dyn FnMut...>`；这与 Go 测试可传 `nil` callback 的动态风险不同。

## 并发与资源生命周期

本文件不启动线程、async task、channel、事务或外部 I/O。所有累计和过滤都在调用线程中同步完成。

`CompactedFileSplitStrategy` 需要 `&mut self` 才能累计或过滤，所以普通使用下不能被多线程同时改写。进度回调额外要求 `Send`，允许其捕获可跨线程转移的状态，但这不等于整个策略提供并发调用保证；如需共享统计，由 callback 的创建者选择 `Arc<Mutex<_>>`、原子类型或其他同步方式。

`HashMap`、`HashSet`、`SplitHelper` 和 boxed callback 的所有权都随策略对象；对象 drop 时自动释放，没有显式 `Close`。本文件也没有 reset API：若未来实现 pipeline trait，必须明确转发 `BaseSplitStrategy::ResetAccumulations` 后 checkpoint 集合和回调是否保留。

## 与 Go 版本的对应关系

核心数据流与 [`compacted_file_strategy.go`](compacted_file_strategy.go) 对应：`impactFactor` 同为 16，`inspect` 对 rewritten SST 选择目标表 ID，`Accumulate` 按表懒创建 helper、对每文件增加计数、跳过零 KV/零 size 的 merge、将非零小值下限为 1，`ShouldSplit` 使用严格大于 256，`ShouldSkip` 则保留无 rule/全 checkpoint/部分 checkpoint 三类分支的返回值和副作用。

需注意的 Rust 表达差异：

- Go 通过编译期断言 `var _ split.SplitStrategy[SSTs] = &CompactedFileSplitStrategy{}` 证明实现完整协议；Rust 类型目前没有 `impl SplitStrategy<...>`，也缺少 `GetAccumulations`/`ResetAccumulations` 协议方法。
- Go `ShouldSkip(ssts SSTs)` 通过 interface 内部指针改写 SST 列表；Rust 显式要求 `&mut dyn SSTs` 和 `&mut self`，用类型系统表达这两类副作用。
- Go 记录结构化的 table/rule/file 字段；Rust `stubs::log` 当前只传递简化文本，可观测性较弱。
- Go 主链在 `client.go::WrapCompactedFilesIterWithSplitHelper` 构造 pipeline splitter 并调用 `WithSplit`；Rust [`client.rs`](client.rs) 尚没有该 wrapper 的对等实现。
- Go 的 rules/checkpoint map 和 callback 可以是 `nil`，其中 nil callback 如被 checkpoint 分支调用会 panic；Rust 构造器使用实体 `HashMap`/`HashSet` 和必填 boxed callback，把“回调存在”变成构造期不变式。

Go 的 `client_test.go` 还验证了 `PipelineRestorerWrapper.WithSplit` 下的 region 切分和 checkpoint 统计整体效果；Rust 当前的独立测试主要验证策略内部语义，不等价于该生产 pipeline 端到端覆盖。

## 扩展指南

- 调整 compacted SST 的权重时，优先修改 `impactFactor`、`Accumulate` 中 `calculateCount`/`calculateSize` 和 `ShouldSplit`，并同步核对 Go 文件。这三处共享同一常量，只改一处的语义会导致切分频率与区间权重失配。
- 增加 SST 类型或改变 rewrite 身份时，同时审查 `inspect`、`hasRule`、[`ssts.rs`](ssts.rs) 的 `SSTs::as_rewritten`/`RewrittenSSTs::RewrittenTo`。关键不变式是“rule 查找 ID、splitter 分组 ID 和键改写边界指向同一逻辑目标”。
- 改 checkpoint 行为时修改 `ShouldSkip`，并保留无 rule 先行、每个命中文件恰好回调一次、部分命中仅保留待处理文件的语义。
- 补齐 Rust 生产接线时，不应只是在 `client.rs` 构造本类型；还需设计与 Rust `SplitStrategy<T>` 一致的所有权/可变性签名，实现累计导出与 reset，再对齐 Go `WrapCompactedFilesIterWithSplitHelper` 的 pipeline 生命周期。不要通过删减 checkpoint 副作用来迁就现有 trait。
- 性能风险主要在 `GetSSTs()` 返回 `Vec<File>` 可能产生的克隆、大 checkpoint set 的查找、每表区间合并以及过早/过晚 split。优化前必须保持 Go 的输出分组和阈值行为。
- Rust 测试逻辑保持在独立文件中。策略分支应更新 [`compacted_file_strategy_test.rs`](compacted_file_strategy_test.rs)；跨模块公开契约可同步 [`parity_test.rs`](parity_test.rs)；客户端接线和 pipeline 生命周期应更新 [`client_test.rs`](client_test.rs)，并参考 Go `client_test.go` 的 `TestCollectSSTFileSets`、`TestCompactedSplitStrategyWithCheckpoint` 及 wrapper split cases。

## 验证依据

- 目标源码：[`compacted_file_strategy.rs`](compacted_file_strategy.rs) 全文，包括 `impactFactor`、`CompactedFileSplitStrategy`、`NewCompactedFileSplitStrategy`、`SstIdentity`、`inspect`、`Accumulate`、`ShouldSplit`、`ShouldSkip`、`TableSplitter` 和 `hasRule`。
- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 确认目标、Go 对照与测试在索引中；`explore` 和 `node` 确认了上述主要符号、Rust 测试调用者、`client.rs::PreSplitRegions` 的普通日志策略调用链，以及 `SSTs`、`RewrittenSSTs`、`BaseSplitStrategy`、`SplitStrategy`、`SplitHelper::Merge`、`GetRewriteRawKeys` 的签名/边界。
- crate 与模块：[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)，确认 library 边界、path dependencies、公开再导出和独立测试挂载。目标目录及上级 `br/pkg/restore` 未找到 `doc.go`。
- Rust 测试：[`compacted_file_strategy_test.rs`](compacted_file_strategy_test.rs) 覆盖严格 split 阈值、每文件计数、空文件、小值下限、无 rule/全 checkpoint/部分 checkpoint 和 rewritten 目标表分组；[`client_test.rs`](client_test.rs) 的 `test_compacted_split_strategy` 验证部分跳过后继续累计；[`parity_test.rs`](parity_test.rs) 提供跨模块 Go/Rust 公开契约覆盖。
- Go 对照：[`compacted_file_strategy.go`](compacted_file_strategy.go) 确认策略语义；[`client.go`](client.go) 的 `WrapCompactedFilesIterWithSplitHelper` 确认生产 pipeline 位置；[`client_test.go`](client_test.go) 确认 checkpoint 统计、文件集合筛选与 pipeline region split 的期望。
- 本任务是纯文档分析，按计划不运行 Cargo。验收仅执行任务指定的结构命令，确认文档存在且恰有十一个固定二级章节。
