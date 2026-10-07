# `br/pkg/restore/split/split.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-restore-split`。该 crate 的入口是 [`lib.rs`](./lib.rs)，入口以 `pub mod split` 装入本文件并通过 `pub use split::*` 扁平导出其公开项；[`Cargo.toml`](./Cargo.toml) 将 crate 标记为 Go 包 `br/pkg/restore/split` 的 Rust library 移植。它位于 BR 恢复流程的 Region 准备层：上层给出有序拆分键或待扫描键区间，本文件负责组织 Region 拆分/散射、从 PD 分页扫描 Region、校验扫描结果连续且可用，并提供相应退避策略。

本文件不是 PD RPC 的具体实现。外部交互都经过 [`client.rs`](./client.rs) 中的 `SplitClient` trait；策略组合位于 [`splitter.rs`](./splitter.rs)。因此这里既是“基础拆分执行器”，也是多个恢复调用方共用的“可靠 Region 扫描”实现。

## 核心职责

1. `RegionSplitter` 把一组**已经排序**的拆分键分成粗拆和细拆两阶段，调用 `SplitClient` 执行 split/scatter，并以有上限的等待观察 scatter 结果。
2. `PaginateScanRegion`、`ScanRegionsWithRetry` 及内部辅助函数处理 PD 扫描结果可能暂时为空、不连续、缺 Leader 或边界覆盖不足的情况；首次允许 follower handle，遇错后收紧为 Leader 扫描并退避重试。
3. `PaginateScanRegionWithCodecAware` 在逻辑键、PD 扫描键和 codec-aware Region 边界之间转换。
4. `WaitRegionOnlineBackoffer` 与 `BackoffMayNotCountBackoffer` 给扫描和 scatter/split 客户端提供错误分类明确的指数退避。
5. `getSplitKeysOfRegions` 将有序拆分键分配给覆盖它们的 Region，跳过空键、既有 Region 起点及不在 Region 内部的键；`CheckRegionEpoch` 则为客户端刷新 Region 后验证 epoch。

## 主要符号

- `WaitRegionOnlineAttemptTimes`、`SplitRetryTimes`：公开的 `RetryCounter` 静态值，默认分别为 1800 和 150。`RetryCounter::{load,store}` 保留类似原子计数器的测试接口，但实际使用 `thread_local!` 的 `Cell<i32>`，让并行 Rust 测试的临时覆盖互不干扰。
- `SplitRetryInterval`（50 ms）、`SplitMaxRetryInterval`（4 s）、`ScatterWaitUpperInterval`（30 min）、`ScanRegionPaginationLimit`（128）、`DefaultRegionIndexStep`（128）：与重试、scatter 等待、分页和粗拆步长相关的公开默认值。
- `NormalizeRegionIndexStep(u32) -> u32`：配置值为 0 时回退到 `DefaultRegionIndexStep`，避免后续按步长循环无法前进。
- `RegionSplitter { client, regionIndexStep, coarseScatter }`：基础执行器。`NewRegionSplitter` 使用默认步长；`NewRegionSplitterWithRegionIndexStep` 接受配置步长并立即归一化；`SetCoarseScatter` 决定细拆阶段是否 scatter。
- `RegionSplitter::ExecuteSortedKeysOnRegion`：直接把指定 Region 和键转交给 `SplitClient::SplitWaitAndScatter`，返回新 Region。
- `RegionSplitter::ExecuteSortedKeys`：全局有序键入口；空键集直接成功，否则进入 `executeSplitByRanges`。
- `checkRegionConsistency`：完整/限量扫描的严格检查，覆盖空结果、首尾边界、Leader、Store ID 和相邻 Region 连续性。
- `scanRegionsLimitWithRetry`：单页扫描与页内一致性重试，返回 `(batch, mustLeader)`，把“后续是否必须走 Leader”显式传给分页循环。
- `PaginateScanRegionWithCodecAware`、`encodeRegionKeys`、`PaginateScanRegion`：codec 转换、分页收集及完整覆盖校验。
- `checkPartRegionConsistency`、`ScanRegionsWithRetry`：只验证首 Region 覆盖起点与相邻连续性的单批扫描路径，不要求覆盖至 `endKey`，也不检查 Leader。
- `WaitRegionOnlineBackoffer`：只对错误链中属于 `ErrPDBatchScanRegion` 的错误指数退避，其他错误立即放弃。
- `BackoffMayNotCountBackoffer`、`ErrBackoff()`、`ErrBackoffAndDontCount()`：分别表达“退避并计数”和“退避但恢复一次计数”；其他错误使策略立即放弃。
- `getSplitKeysOfRegions`：按 Region ID 返回其内部需要拆分的原始键；编码比较由 `codec::EncodeBytesExt(..., isRawKV)` 完成。
- `CheckRegionEpoch`：两侧均有 Region 元数据且 `RegionEpoch` 相等时才返回 `true`。

## 执行流程

### 有序键拆分

`ExecuteSortedKeys` 的输入契约是有序键。它先在 `executeSplitByRanges` 中每隔 `regionIndexStep` 取一个粗拆键；如果存在粗拆键，先调用 `executeSplitByKeys(..., scatter = true)`，让大 Region 先被切开并散射。随后对全部键细拆；普通模式传 `scatter = true`，`coarseScatter` 模式则传 `false`，只对粗拆结果 scatter。`executeSplitByKeys` 根据该布尔值选择 `SplitKeysAndScatter` 或 `SplitKeys`，若客户端返回需观察的 Region，则最多按 `ScatterWaitUpperInterval` 等待。等待错误不会从 `waitRegionsScattered` 向上传播，符合“拆分完成后即使散射未完全结束，恢复仍可继续”的 Go 行为。

`ExecuteSortedKeysOnRegion` 是另一条更窄的路径：策略层已定位 Region 时，直接执行 `SplitWaitAndScatter`。[`splitter.rs`](./splitter.rs) 的 `PipelineRegionsSplitterImpl::splitRegionByPoints` 优先走这条路径，失败后排序 split points，再回退到全局 `ExecuteSortedKeys`。

### 分页扫描

`PaginateScanRegion` 首先拒绝非空且 `startKey > endKey` 的范围。每次外层尝试都从原始 `startKey` 开始重建 `regions`：

1. 调用 `scanRegionsLimitWithRetry` 取一页。首次允许 follower handle；只要扫描或页内一致性检查失败，后续尝试就不再添加 `WithAllowFollowerHandle()`。
2. 将页面追加到结果；当页长小于 `limit`、最后 Region 的 `EndKey` 为空，或已达到 `endKey` 时停止翻页，否则以下一页的起点设为最后 Region 的 `EndKey`。
3. 完成一轮后强制后续外层尝试只用 Leader。若本轮 Region 数量较上一轮发生变化，调用 `ReduceRetry` 抵消一次重试消耗，把它视为 PD/TiKV 仍在推进的信号。
4. 对整批结果调用 `checkRegionConsistency(..., limitted = false)`。失败且上下文未取消时退避并从头扫描；成功才返回完整结果。

`PaginateScanRegionWithCodecAware` 在上述流程外再包一层键转换。有 `CodecPDClient` 时先用其 codec `DecodeRange` 得到 PD 扫描范围，扫描后用捕获的 `EncodeRegionRange` 原地重编码每个 Region 的首尾键；没有 codec client 时，以 `codec::EncodeBytes` 编码输入范围，返回的 Region 边界保持 PD 编码形态。

`ScanRegionsWithRetry` 适合只取有限批次的调用方。它不分页，使用 `checkPartRegionConsistency`，因此只保证结果非空、首 Region 覆盖起点且相邻连续。`splitter.rs::SplitPoint` 在本地 Region 缓冲耗尽时以 64 为 limit 调用它并继续消费。

## 数据与状态

- 键范围均使用字节序比较。空 `endKey` 表示开放上界；Region 的空 `EndKey` 表示键空间末端。分页推进依赖“下一页起点等于上一页最后 Region 的 `EndKey`”这一不变量。
- `RegionInfo` 的 `Region` 元数据包含 `Id`、`StartKey`、`EndKey`、`RegionEpoch`，`Leader` 包含 `StoreId`。严格一致性检查要求首 Region 起点不晚于请求起点、非限量结果覆盖请求终点、每个 Region 有合法 Leader，并要求相邻边界完全相等。
- `RegionSplitter` 拥有 `Box<dyn SplitClient>`，没有内部锁；`regionIndexStep` 在构造时固定，`coarseScatter` 只能通过 `&mut self` 修改。
- 两类重试次数是线程局部 `Cell`，生产默认值固定，但测试可以通过 `load/store` 在当前测试线程临时缩小。它们不是跨线程共享配置，不能用于协调工作线程。
- `PaginateScanRegion` 的 `lastRegions` 保存上一次完整扫描尝试，`mustLeader` 在第一次失败或第一轮结束后变为 `true`，`RetryState` 保存剩余次数和指数退避区间。
- `getSplitKeysOfRegions` 的输出以 Region ID 为键，而 Go 版本以 `*RegionInfo` 为 map key；Rust 通过 ID 让 [`client.rs`](./client.rs) 遍历扫描结果时定位对应 split keys。函数要求 `sortedKeys` 升序、`sortedRegions` 连续且按起点升序，并预期 Region 覆盖全部键。

## 依赖与调用关系

上游直接证据：

- [`splitter.rs`](./splitter.rs) 为 `RegionSplitter` 实现 `Splitter` trait；`NewPipelineRegionsSplitter` 构造它，策略执行委托 `ExecuteSortedKeysOnRegion`、`ExecuteSortedKeys` 和 `WaitForScatterRegionsTimeout`，`SplitPoint` 使用 `ScanRegionsWithRetry` 补充 Region 缓冲。
- [`client.rs`](./client.rs) 的批量 split 循环调用 `PaginateScanRegion` 扫描覆盖拆分键的 Region，再用 `getSplitKeysOfRegions` 分组；NotLeader 刷新路径调用 `CheckRegionEpoch`；scatter 等待创建 `NewBackoffMayNotCountBackoffer`。
- [`../misc.rs`](../misc.rs) 的 Region scanner 用 `ScanRegionsWithRetry` 获取恢复范围；[`../log_client/import_retry.rs`](../log_client/import_retry.rs) 使用 `PaginateScanRegion` 和 `CheckRegionEpoch` 完成日志恢复重试。

下游依赖：

- `SplitClient` 提供 `SplitWaitAndScatter`、`SplitKeysAndScatter`、`SplitKeys`、`WaitRegionsScattered`、`ScanRegions` 和可选 `GetCodecPDClient`，将本文件与真实 PD/TiKV 客户端及测试替身解耦。
- `crate::stubs` 提供 `Context`、`WithRetry`、`RetryState`、`BackoffStrategy`、codec、日志与脱敏键格式；`astersql-br-pkg-errors` 提供 `ErrInvalidRange`、`ErrPDBatchScanRegion` 及错误身份判断；`astersql-errors` 提供共享错误、包装和根因提取。
- `Cargo.toml` 的直接依赖只有 `astersql-br-pkg-errors`、`astersql-br-pkg-restore-utils`、`astersql-errors` 和 `hex`；本文件直接使用前三者中的错误体系及 `hex`，其余协议替身由 crate 内模块提供。

RustCodeGraph 对 `PaginateScanRegion` 的被调用边确认了 `scanRegionsLimitWithRetry`、`checkRegionConsistency`、`NewWaitRegionOnlineBackoffer`、上下文 `Done`、`ReduceRetry` 与退避方法；精确路径搜索补充了上述跨文件调用点。

## 错误处理与边界

- `PaginateScanRegion` 与 `ScanRegionsWithRetry` 对 `startKey > endKey` 返回带十六进制键上下文的 `ErrInvalidRange`；`endKey` 为空不触发该检查。
- `checkRegionConsistency` 将空批次、首端缺口、尾端缺口、Leader 缺失、Leader Store ID 为 0、相邻 Region 不连续统一包装为 `ErrPDBatchScanRegion`，并在可用时附 Region ID、epoch 和脱敏键。`limitted = true` 时不要求最后 Region 覆盖请求终点，适用于单页检查。
- `checkPartRegionConsistency` 有意弱于严格检查：它不验证尾端覆盖、Leader 或 Store ID。当前实现直接解包首个 `Region` 元数据，因此调用方/`SplitClient` 必须保证非空条目含 Region；这是一项扩展时不可破坏的隐含前置条件。
- `PaginateScanRegion` 在单页扫描失败时用 `ErrPDBatchScanRegion.Wrap` 保留根因并附当前页起点；上下文取消时立即返回当前错误，不再继续退避。
- `WaitRegionOnlineBackoffer` 只重试错误链中可识别为 `ErrPDBatchScanRegion` 的错误。`BackoffMayNotCountBackoffer` 通过解包后的 sentinel 文本区分两种退避指令；普通错误调用 `GiveUp`。
- `WaitForScatterRegionsTimeout` 无论客户端等待返回何种错误，都只返回未完成数量；客户端调用本身失败时回退为输入 Region 总数。此处错误被有意降级，调用者不能把返回 0 以外的值解释为恢复失败。
- `getSplitKeysOfRegions` 对空输入返回空 map，并跳过空键和正好等于 Region 起点的键。如果 Region 未覆盖所有键，它只记录错误日志并返回已分配部分，不返回 `Result`；调用方的前置分页扫描必须承担覆盖保证。
- `limit` 由调用者提供；本文件没有显式拒绝 0 或负值，安全扩展时必须先确认 `SplitClient::ScanRegions` 的约定，不能仅在这里猜测新语义。

## 并发与资源生命周期

- 本文件不创建线程、异步任务或通道。并发边界主要来自可共享的 `SplitClient: Send + Sync` 和线程局部重试覆盖；`RegionSplitter` 方法以共享借用调用客户端，只有配置 `coarseScatter` 需要独占可变借用。
- `WaitForScatterRegionsTimeout` 通过 `Context::WithTimeout` 派生 `ctx2`，派生取消句柄由局部变量持有，作用域结束时释放；等待只持续到传入 timeout 或父上下文结束。
- 扫描循环在每轮重试内新建 Region 向量，不跨失败轮次复用不完整分页结果。`lastRegions` 只用于比较进展和在最终错误路径保留最后一轮结果，不会与别的线程共享。
- `RetryState::ExponentialBackoff` 管理睡眠间隔与剩余次数。`BackoffMayNotCountBackoffer` 在 `ErrBackoffAndDontCount` 路径先执行一次指数退避，再 `ReduceRetry` 抵消计数；它不是无限重试，仍受上下文和其他错误约束。
- codec-aware 扫描把 codec 的编码闭包保存到本次函数调用结束，扫描完成后同步改写返回对象，不保存全局 codec 状态。

## 与 Go 版本的对应关系

直接对照文件是 [`split.go`](./split.go)，测试对照为 [`split_test.go`](./split_test.go)。Rust 保留了 Go 的两阶段粗/细拆分、scatter 错误降级、follower-first/leader-after-error 扫描、整批连续性复核、Region 数量变化抵消重试、两种 sentinel 退避以及 RawKV/TxnKV 拆分键编码分支。

主要语言适配如下：

- Go 的包级可变 `WaitRegionOnlineAttemptTimes`/`SplitRetryTimes` 在 Rust 中变为线程局部 `RetryCounter`。默认值一致，但测试覆盖的可见范围从进程全局缩小为线程局部，以支持 Rust 测试并行执行。
- Go 的 `SplitClient` 接口值在 Rust 中由 `Box<dyn SplitClient>` 拥有；Go 的 `[]*RegionInfo` 对应 Rust 的 `Vec<RegionInfo>`，可选 protobuf/Leader 字段通过 `Option` 表达。
- Go `getSplitKeysOfRegions` 以 Region 指针为 map key；Rust 用稳定的 `Region.Id: u64`，随后由 `client.rs` 按 ID 取键。
- Go 的 failpoint `hint-scan-region-backoff` 在 Rust stubs 中由原子布尔 `HINT_SCAN_REGION_BACKOFF` 表达；启用时同样把退避缩短到 1 微秒。
- Go 通过 `errors.As`/`ErrorEqual` 判断错误身份；Rust 分别使用 `BrIs` 遍历包装链，以及 `Cause` 后的 sentinel 文本比较。扩展错误包装时必须保留这些分类结果。
- Rust 在 `checkRegionConsistency` 的部分路径对缺失 `Region` 返回 `New("nil region")` 或按默认值生成诊断；`checkPartRegionConsistency` 仍继承 Go “Region 必定存在”的假设。文档不将这一点描述为全面的空指针防护。

独立 Rust 测试 [`split_test.rs`](./split_test.rs) 对应 Go 场景，并额外覆盖 Rust 适配：扫描成功/失败/停止重试、scatter 等待、两类 backoffer、上下文取消、拆分键归属、codec-aware 分页、分页边界和断裂结果、严格/部分一致性、粗拆步长、空输入、split/scatter、RawKV、未完全 scatter 以及 epoch helper。

## 扩展指南

- 修改粗拆算法时，从 `NormalizeRegionIndexStep`、`executeSplitByRanges` 和 `coarseScatter` 三处同时检查；必须保持步长可前进、输入有序以及“粗拆先 scatter、细拆按模式决定是否 scatter”的顺序。同步扩展 `split_test.rs::test_region_splitter_rough_split_uses_configured_region_index_step` 及 Go 对应测试。
- 新增扫描错误类型或包装层时，验证 `BrIs(..., ErrPDBatchScanRegion)` 仍能命中，否则 `WaitRegionOnlineBackoffer` 会立即放弃。若新增可重试类别，应同时调整 Go `WaitRegionOnlineBackoffer::NextBackoff`，并在独立测试中覆盖计数与上下文取消。
- 调整分页终止条件时，保持页起点单调前进，继续识别空 Region `EndKey`，并在返回前做完整 `checkRegionConsistency`；重点复用 `test_paginate_scan_region`、`test_paginate_scan_region2` 和 `test_scan_regions_limit_with_retry` 的断裂、空批次、Leader/follower 回退场景。
- 扩展 codec 支持时，修改 `PaginateScanRegionWithCodecAware`/`encodeRegionKeys`，确保输入 decode 和输出 Region 边界 re-encode 成对出现，并同步 `test_paginate_scan_region_with_codec_aware_codec_pd_client`。
- 修改 `getSplitKeysOfRegions` 时同时检查 `client.rs` 按 Region ID 取 map 的逻辑；保持空键和 Region 起点不产生无效 split，保持 RawKV 与 TxnKV 编码差异，并扩展 `test_get_split_key_per_region`。
- 如果要强化缺失 `Region` 元数据或非法 `limit` 的处理，应先对齐 Go 版本和 `SplitClient` 契约，再新增回归测试；不要只让一个语言版本静默接受或拒绝输入。
- Rust 单元测试继续放在独立 [`split_test.rs`](./split_test.rs)，不要嵌回生产文件。涉及 Go/Rust 对齐的行为应同步检查 [`split_test.go`](./split_test.go)，避免只满足一侧测试。

## 验证依据

- 源码全貌：[`split.rs`](./split.rs)；模块装配：[`lib.rs`](./lib.rs)；crate 边界与依赖：[`Cargo.toml`](./Cargo.toml)。本文件无条件编译分支；测试由 `lib.rs` 的 `#[cfg(test)] #[path = "split_test.rs"]` 独立挂载。
- 直接实现依赖：[`client.rs`](./client.rs) 的 `SplitClient` 以及批量 split、epoch 与 scatter 调用；[`region.rs`](./region.rs) 的 `RegionInfo`/`ContainsInterior`；[`stubs.rs`](./stubs.rs) 的 context、retry、codec 和协议类型。
- 上游调用：[`splitter.rs`](./splitter.rs)、[`../misc.rs`](../misc.rs)、[`../log_client/import_retry.rs`](../log_client/import_retry.rs)。
- Go 语义：[`split.go`](./split.go)；Rust 独立测试：[`split_test.rs`](./split_test.rs)；Go 对照测试：[`split_test.go`](./split_test.go)。
- RustCodeGraph：索引状态为 7032 个 Rust 文件，目标 `split.rs` 识别 60 个符号；`query` 同时定位 Rust/Go 的 `RegionSplitter`、`PaginateScanRegion`、`NewWaitRegionOnlineBackoffer`、`checkRegionConsistency`；`callees PaginateScanRegion` 核实扫描、严格校验、backoffer、取消与重试状态边。由于全仓同名 callers 查询超时，跨文件调用者使用上述精确路径搜索补证。
- 结构验收命令：`test -f br/pkg/restore/split/split.rs.md && test "$(rg -c '^## (文件定位|核心职责|主要符号|执行流程|数据与状态|依赖与调用关系|错误处理与边界|并发与资源生命周期|与 Go 版本的对应关系|扩展指南|验证依据)$' br/pkg/restore/split/split.rs.md)" -eq 11`。
