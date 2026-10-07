# `br/pkg/restore/split/splitter.rs`

## 文件定位

`splitter.rs` 属于 `astersql-br-pkg-restore-split` library crate。该 crate 的入口是 [`lib.rs`](./lib.rs)，后者把本文件声明为 `pub mod splitter` 并扁平再导出其公开项。其 Cargo 清单 [`Cargo.toml`](./Cargo.toml) 说明这是 Go 包 `br/pkg/restore/split` 的 Rust 移植；本文件直接依赖同 crate 的 `client`、`region`、`split`、`sum_sorted` 与 `stubs`，并依赖 `astersql-br-pkg-restore-utils` 的键改写规则和 `astersql-errors` 的共享错误。

它位于日志/SST 恢复的“统计待恢复键范围 -> 按 PD Region 切片 -> 选择 split key -> 调用 RegionSplitter -> 等待 scatter”链路中，算法下游是 [`split.rs`](./split.rs) 的 `RegionSplitter`、`ScanRegionsWithRetry`，上游策略实例包括 `br/pkg/restore/log_client/log_split_strategy.rs` 与 `br/pkg/restore/log_client/compacted_file_strategy.rs` 对 `BaseSplitStrategy` 的复用。

需要区分“API 已实现”和“Rust 生产链已接线”：当前 Rust 全仓搜索中，`NewPipelineRegionsSplitter` 只被本目录独立测试 `splitter_test.rs` 构造；`SplitPoint` 除文件内部外由 `split_test.rs` 与 `parity_test.rs` 调用。`br/pkg/restore/restorer.rs` 使用的是 `br/pkg/restore/stubs.rs` 中另一套本地 `PipelineRegionsSplitter`/`SplitStrategy` 接口，而 Rust `log_client/client.rs::PreSplitRegions` 当前累计后直接 `ResetAccumulations`，没有调用本文件的 `ExecuteRegions`。因此本文件是可调用的完整拆分库实现，但尚不能据此声称 Rust 主恢复流程已与 Go 完整接线。

## 核心职责

本文件承担四层职责：

1. 用 `Splitter` 抽象单 Region 拆分、全局有序键拆分和 scatter 等待，并让 `RegionSplitter` 与流水线实现共享该契约。
2. 用 `BaseSplitStrategy` 按旧 table ID 保存 `SplitHelper` 和 `RewriteRules`，将累积结果转换成按重写后表前缀排序的 `SplitHelperIterator`。
3. 用 `SplitPoint` 把重写后的 `Valued` 区间与 PD 扫描得到的 Region 求交，把跨 Region 的 Size/Number 均摊后交给回调。
4. 用 `PipelineRegionsSplitterImpl::splitRegionByPoints` 按 size/key 阈值选取 split key，优先针对已扫描 Region 拆分，失败时退化为全局有序键拆分，并收集新区供 scatter 等待。

本文件不负责产生文件/SST 的 `Valued` 数据，不实现 PD RPC，也不决定检查点跳过规则；这些分别由日志策略、`SplitClient`/`RegionSplitter` 和上层恢复流水线负责。

## 主要符号

- `trait Splitter: Send + Sync`：基础执行边界。`ExecuteSortedKeysOnRegion` 在指定 Region 内拆分，`ExecuteSortedKeys` 处理可能跨 Region 的全局键，`WaitForScatterRegionsTimeout` 返回超时后仍未完成的 Region 数。`RegionSplitter` 和 `PipelineRegionsSplitterImpl` 都通过委托实现它。
- `trait SplitStrategy<T>`：定义 `Accumulate`、`ShouldSplit`、`ShouldSkip`、`GetAccumulations`、`ResetAccumulations` 五阶段策略契约。本文件只定义接口；具体日志/SST 策略当前以固有方法复用 `BaseSplitStrategy`，未在此实现该 trait。
- `BaseSplitStrategy` / `NewBaseSplitStrategy`：保存 `AccumulateCount`、旧 table ID 到 `SplitHelper` 的 `TableSplitter`、旧 table ID 到 `RewriteRules` 的 `Rules`。构造时只安装规则，分表累积器由上游懒创建。
- `BaseSplitStrategy::GetAccumulations`：为每个已累积表查找改写规则，通过 `GetRewriteTableID` 得到新表 ID，以编码后的新表前缀构造 `RewriteSplitter`，按 `RewriteKey` 排序并返回迭代器。
- `BaseSplitStrategy::ResetAccumulations`：清空所有分表区间并把计数归零；规则保留，供下一批复用。
- `RewriteSplitter` / `NewRewriteSpliter`：绑定重写后排序键、新 table ID、该表规则与区间树。`Spliter` 的单 `t` 拼写为保留 Go API 的既有命名。
- `SplitHelperIterator::Traverse`：按新表前缀顺序遍历；每表的逻辑上界是 `EncodeTablePrefix(tableID + 1)` 的编码值。回调返回 `false` 时 Rust 实现同时终止当前表和后续表遍历。
- `trait PipelineRegionsSplitter: Splitter`：在基础拆分能力之上增加 `ExecuteRegions`。
- `PipelineRegionsSplitterImpl` / `NewPipelineRegionsSplitter`：组合 `RegionSplitter`、size/key 阈值和 `Mutex<Vec<RegionInfo>>` scatter 缓冲。
- `SplitPoint`：本文件的纯调度核心。它不自行选择阈值点，而是把每个 Region 的重叠区间、初始均摊 Size/Number 传给 `splitF`。
- `PipelineRegionsSplitterImpl::splitRegionByPoints`：阈值选择与实际拆分入口；该方法是 `pub(crate)`，独立测试可直接覆盖。
- `type SplitFunc<'a>`：描述拆分回调签名，但当前 `SplitPoint` 使用等价的泛型约束，别名没有参与运行路径。
- `_unused`：仅用类型标注保留 `Arc` 引用，受 `dead_code` 允许；没有运行时语义。

## 执行流程

累积结果导出流程如下：

1. 上游策略按旧 table ID 向 `BaseSplitStrategy::TableSplitter` 合并 `Valued`。
2. `GetAccumulations` 要求每个累积表在 `Rules` 中有规则；求出新 table ID，跳过无法得到有效新 ID（值为 0）的表。
3. 每个表被包装成 `RewriteSplitter`，以编码后的新表前缀排序，避免 `HashMap` 迭代顺序影响跨表执行顺序。
4. `SplitHelperIterator::Traverse` 逐表遍历区间，并把“下一 table ID 前缀”作为本表扫描上界传给 `SplitPoint`。

`SplitPoint` 的 Region 切片流程如下：

1. 跳过 `Number == 0` 或 `Size == 0` 的区间；用 `GetRewriteEncodedKeys` 把备份侧起止键映射到恢复侧编码键。
2. 复用最多 64 个 Region 的本地扫描窗口；窗口耗尽时从当前区间起点或上一批最后 Region 的终点调用 `ScanRegionsWithRetry`。
3. 对跨越多个 Region 的一个 `Valued` 统计 `regionOverCount`，以整数除法计算每个重叠 Region 的 `endLength`/`endNumber`。切换 Region 时，先补齐上一 Region 的尾段，再调用 `splitF`。
4. 完全落在单个 Region 的区间原样进入 `regionValueds`；跨 Region 的最后一段则作为下一 Region 的 `initialLength`/`initialNumber`，用于延续阈值累计。
5. 遍历完成后，对最后一个仍有 `regionValueds` 的 Region 执行扫尾回调；重写、扫描或回调错误经 `SharedError` 返回。

`ExecuteRegions` 与实际拆分流程如下：

1. 清空 `regions_buf`，调用 `SplitPoint`，回调到 `splitRegionByPoints`。
2. `splitRegionByPoints` 从 Region 起点和跨 Region 初值开始累加。只有当前起点相对 `lastKey` 前进，且“加入当前区间后”严格超过 size 或 key 阈值时，才把当前起点作为 split key，并清零段累计。
3. split key 优先通过 `codec::DecodeBytes` 转回 raw key；为对齐 Go 忽略解码错误的行为，失败时加入空字节数组，而不是返回错误。
4. 有 split point 时先调用 `ExecuteSortedKeysOnRegion`。成功且上下文未取消则把新区加入 scatter 缓冲；失败则排序 split points，再调用 `ExecuteSortedKeys` 走跨 Region 的降级路径。
5. 所有回调同步结束后，复制缓冲并最多等待 60 秒 scatter；返回的未完成数量被忽略，`ExecuteRegions` 仍返回成功。

## 数据与状态

- 键有两种表示：`SplitHelper`/`SplitPoint` 比较的是 memcomparable 编码键，`ExecuteSortedKeysOnRegion` 接收的是解码后的 raw split key。改写在比较与扫描前完成。
- `Value { Size, Number }` 是估算权重，不是实际 KV 内容。跨 Region 分摊使用整数除法，会丢弃余数；Go `SplitPoint` 使用相同算法。
- `BaseSplitStrategy::TableSplitter` 以旧/有效 table ID 聚合区间，`Rules` 决定恢复侧 table ID。`GetAccumulations` 生成的是克隆快照，不会消费原累积状态。
- `RewriteKey` 目前等于新表前缀的编码值，只用于确定跨表顺序；Go 源码也保留了未来直接按新 table ID 排序的 TODO。
- `regions` 与 `regionIndex` 是 `SplitPoint` 单次调用内的滑动窗口；`regionInfo`、`regionValueds`、`initialLength`、`initialNumber` 描述尚待回调的当前 Region。
- `regions_buf` 跨 `splitRegionByPoints` 回调共享，但每次 `ExecuteRegions` 开始都会清空，结束前复制为 scatter 等待输入。
- 阈值判断使用 `>` 而非 `>=`：累计值恰等于阈值不会在该点拆分；还要求起点不同于 `lastKey`，避免连续范围解码到同一用户键时产生重复点。

## 依赖与调用关系

上游直接证据：

- `br/pkg/restore/log_client/log_split_strategy.rs` 和 `compacted_file_strategy.rs` 导入 `BaseSplitStrategy`/`NewBaseSplitStrategy`，分别累计日志文件与压缩 SST 的范围。
- `br/pkg/restore/split/split_test.rs::{test_split_point,test_split_point2}`、`parity_test.rs::go_rust_public_contract_matches` 直接调用 `SplitPoint`。
- `br/pkg/restore/split/splitter_test.rs::malformed_encoded_split_key_matches_go_ignored_decode_error` 构造 `NewPipelineRegionsSplitter` 并直接调用 `splitRegionByPoints`。

下游直接证据：

- `GetRewriteTableID`、`GetRewriteEncodedKeys` 和 `RewriteRules` 来自 `astersql-br-pkg-restore-utils`，决定表 ID 和键空间映射。
- `SplitHelper::Traverse`、`Valued`、`Span`、`Value` 来自 `sum_sorted.rs`，提供合并后的有值区间。
- `ScanRegionsWithRetry`、`NewRegionSplitter`、`RegionSplitter::{ExecuteSortedKeysOnRegion,ExecuteSortedKeys,WaitForScatterRegionsTimeout}` 来自 `split.rs`，承担 PD Region 发现、拆分与 scatter。
- `SplitClient` 是访问 Region/PD 操作的抽象；构造器取得 `Box<dyn SplitClient>` 后把所有权交给内部 `RegionSplitter`。
- `codec` 和 `tablecodec` 分别完成 memcomparable key 编解码与 table prefix 编码。

Cargo 边界方面，本 crate 的唯一生产反向依赖清单是 `br/pkg/restore/log_client/Cargo.toml`；它当前使用策略与区间类型，但 Rust 源码没有构造流水线 splitter。Go 对应链路则在 `br/pkg/restore/log_client/client.go::{WrapCompactedFilesIterWithSplitHelper,WrapLogFilesIterWithSplitHelper,PreSplitRegions}` 中构造并执行 `NewPipelineRegionsSplitter`。

## 错误处理与边界

- `GetAccumulations` 找不到累积表的规则时调用 `log::Fatal`，这是“累积表必有规则”的编程不变量，不是可恢复输入错误；新 table ID 为 0 时只告警并跳过该表。
- `SplitPoint` 将重写键缺失显式转换为 `New("rewrite keys missing")`；键改写错误、Region 扫描错误和回调错误都会停止 Rust 的整个迭代并返回。
- `ScanRegionsWithRetry` 成功但返回空列表时，后续 `regions[regionIndex]` 会越界 panic。本文件依赖下游扫描契约保证覆盖请求范围；若要支持空结果，必须在这里增加明确错误并配独立回归测试。
- 多处读取 `RegionInfo.Region` 时对 `None` 使用空 key；但最终扫尾对 `regionInfo` 使用 `unwrap`。正常路径依赖扫描结果含有效 Region。伪造或不完整 `RegionInfo` 可能导致错误比较或 panic，不应视为受支持输入。
- `splitRegionByPoints` 的 encoded key 解码失败不会报错，而是传递空 split key；这是由 `splitter_test.rs` 锁定的 Go 兼容行为，修改为严格报错会改变兼容契约。
- 单 Region 拆分失败会告警并回退 `ExecuteSortedKeys`；回退失败才向上传播。成功拆分后的 scatter 超时/未完成数在 `ExecuteRegions` 中被有意忽略。
- `Mutex::lock().unwrap()` 在锁中毒时 panic。当前临界区只执行 clear/clone/extend，没有外部回调，降低了持锁 panic 风险，但接口没有恢复 poisoned lock。
- Size 使用 `u64`、Number 使用 `i64`，阈值相加没有显式饱和处理；调用方应保证估算值不会溢出。

## 并发与资源生命周期

`Splitter` 要求 `Send + Sync`，`PipelineRegionsSplitterImpl` 用 `Mutex<Vec<RegionInfo>>` 使 scatter 缓冲可通过共享引用修改。`ExecuteRegions` 当前是同步流程：Region 扫描、阈值计算和拆分回调都在调用线程完成，没有创建线程、任务、channel 或 worker pool。缓冲生命周期限定为一次 `ExecuteRegions`：入口清空，拆分成功时追加，末尾克隆用于等待。

闭包为了同时借用 `region_splitter.client` 和修改 `regions_buf`，把 `self` 转成裸指针再在回调中 `unsafe` 解引用。安全性依赖 `SplitPoint` 同步执行回调且回调不逸出；当前泛型 `FnMut` 没有 `'static` 要求，函数结束前完成所有调用，因此 `self` 在期间仍存活。若未来把 `SplitPoint` 或回调改成异步/并发执行，必须先移除这项裸指针假设并重构借用边界。

这与 Go 版本存在重要实现差异：Go 的 `PipelineRegionsSplitterImpl` 使用 128 worker 的 `WorkerPool`、`errgroup.Group`、容量 1024 的 `regionsCh` 和单独 scatter 收集 goroutine；Rust 当前串行执行，没有 Go 的并行吞吐、错误组取消和 channel 背压语义。Rust 的同一实例也不适合并发调用 `ExecuteRegions`：每次入口会清空共享缓冲，两个调用可能互相覆盖 scatter 集合，尽管 Mutex 能防止数据竞争。当前 API 未用额外状态禁止这种逻辑竞态。

## 与 Go 版本的对应关系

直接对照文件是 [`splitter.go`](./splitter.go)。以下逻辑保持一致：

- `Splitter`、`SplitStrategy`、`BaseSplitStrategy`、`RewriteSplitter`、`SplitHelperIterator`、`PipelineRegionsSplitter` 的职责与主要字段对应。
- 按重写后 table prefix 排序；以 `tableID + 1` 前缀作为表扫描上界。
- `SplitPoint` 以 64 个 Region 为一批扫描，重写键后求交，对跨 Region 的 Size/Number 做整数均摊。
- split point 的阈值为“加入当前区间后严格大于 size 或 keys 阈值”，且跳过与上一 key 相同的点。
- encoded key 解码错误被忽略；单 Region 拆分失败后排序并走全局拆分；scatter 最长等待一分钟。

已验证的差异与迁移状态：

- Go `ExecuteRegions` 异步提交拆分并等待 errgroup，再关闭 channel、等待 scatter goroutine；Rust 串行拆分，仅以 Mutex Vec 聚合结果，忽略 scatter 返回值。
- Go 的 `SplitHelperIterator::Traverse` 没有在表级显式检查回调停止状态；Rust 保存 `cont` 并在 `false` 时停止后续表，更直接地阻止错误后继续遍历。
- Go 结构嵌入 `*RegionSplitter` 并带 worker pool/error group/channel；Rust 显式字段 `region_splitter`，没有这些并发资源。
- Go 生产链在 `log_client/client.go` 三个入口实际构造/执行流水线；Rust 目前只有策略侧 crate 依赖与测试调用，生产恢复 wrapper 仍使用本地 stubs，属于未完成接线。
- Rust 通过克隆 `RewriteRules`/`SplitHelper` 生成迭代器，Go 使用指针；Rust 快照与原累积状态隔离，但有额外复制成本。

Go 测试 `split_test.go::{TestSplitPoint,TestSplitPoint2}` 与 Rust `split_test.rs::{test_split_point,test_split_point2}` 使用相同的旧表 50 -> 新表 100 改写、Region 布局及跨 256+ Region 场景，验证首尾片段、均摊值和回调 Region。Rust 独有 `splitter_test.rs::malformed_encoded_split_key_matches_go_ignored_decode_error` 固化 Go 忽略解码错误的细节。

## 扩展指南

- 新增一种文件/SST 累积策略时，应在独立源文件中组合 `BaseSplitStrategy`，实现与 `SplitStrategy<T>` 等价的五阶段语义，并把测试放到独立 `*_test.rs`；不要把测试嵌回 `splitter.rs`。
- 改变 table rewrite 或遍历顺序时，重点修改 `GetAccumulations`/`SplitHelperIterator::Traverse`，同步覆盖多表排序、缺失规则、table ID 0 和回调提前停止。
- 改变 Region 求交或权重分摊时，重点修改 `SplitPoint`，同步 Rust `split_test.rs` 与 Go `split_test.go` 的两组对应测试；尤其保留 64 个扫描窗口耗尽、跨大量 Region、末尾扫尾和错误中断场景。
- 改变 split key 阈值或编解码行为时，重点修改 `splitRegionByPoints`，增加独立测试覆盖 `>` 边界、重复起点、size/key 任一超限、解码失败、单 Region 失败后的全局回退，以及取消后不写 scatter 缓冲。
- 若要完成 Rust 生产接线，应统一 `br/pkg/restore/stubs.rs` 与本 crate 的 trait/type 边界，让 `restorer.rs::PipelineRestorerWrapper` 使用真实 `SplitHelperIterator`，并在 `log_client/client.rs` 的 wrapper/pre-split 入口构造真实 `NewPipelineRegionsSplitter`。这属于跨文件功能工作，不应在本说明任务中假定已经完成。
- 若恢复 Go 的并发模型，需要明确最大并发、首错取消、channel 关闭顺序和 scatter 收集生命周期；同时移除 `ExecuteRegions` 中依赖同步回调的裸指针，并验证同一 splitter 是否允许并发调用。
- 性能风险集中在 `GetAccumulations` 克隆整棵 `SplitHelper`、每次扫描最多 64 Region、scatter 缓冲整体克隆，以及当前串行拆分。优化前应以等价行为测试保护键顺序、错误传播和取消语义。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标 `br/pkg/restore/split/splitter.rs` 已索引，报告 57 个符号。通过 `node --file ... --offset 1/500` 阅读完整 558 行源码，并以 `query SplitPoint`、`query PipelineRegionsSplitterImpl`、`query NewPipelineRegionsSplitter` 和精确 `explore` 核对符号与调用者。
- 目标实现：[`splitter.rs`](./splitter.rs)；crate 声明与模块装配：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)。
- Go 对照：[`splitter.go`](./splitter.go)；Go 上游接线：`br/pkg/restore/restorer.go::PipelineRestorerWrapper::WithSplit`、`br/pkg/restore/log_client/client.go::{WrapCompactedFilesIterWithSplitHelper,WrapLogFilesIterWithSplitHelper,PreSplitRegions}`。
- Rust 上游与迁移状态：`br/pkg/restore/log_client/{log_split_strategy.rs,compacted_file_strategy.rs,client.rs}`、`br/pkg/restore/{restorer.rs,stubs.rs}` 及 `br/pkg/restore/log_client/Cargo.toml`。
- 独立 Rust 测试：[`splitter_test.rs`](./splitter_test.rs)、[`split_test.rs`](./split_test.rs)、[`parity_test.rs`](./parity_test.rs)；Go 对照测试：[`split_test.go`](./split_test.go) 的 `TestSplitPoint`、`TestSplitPoint2`。
- 全仓文本核验：`NewPipelineRegionsSplitter(` 的 Rust 命中仅为定义和 `splitter_test.rs`；`ExecuteRegions(` 的 Rust 生产命中位于本文件及 `restorer.rs` 的本地 stub trait 调用，从而支持“真实 splitter 尚未接入 Rust 生产恢复链”的结论。

本任务为纯文档分析，按计划未运行 Cargo 或代码测试。交付前使用任务规定的结构命令确认文件存在且恰有 11 个固定二级标题，并人工复核所有“已支持”陈述都有上述源码、调用边、Cargo、Go 或测试证据。
