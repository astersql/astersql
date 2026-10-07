# `pkg/ingestor/ingestctrl/disk_quota.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；其 crate 根 `pkg/ingestor/ingestctrl/lib.rs` 通过 `pub mod disk_quota` 暴露该模块。它位于本地 ingest Engine 控制面与上层导入流程之间：下游通过 `DiskUsage` 获取所有 Engine 的瞬时占用快照，上游据 `CheckDiskQuota` 的结果决定哪些非导入中 Engine 是缓解配额压力的候选。当前生产接线见 `pkg/session/runtime/import_sst.rs::RuntimeBackend::disk_quota_pressure`，该方法把本地 `Backend` 和 quota 直接传入 `CheckDiskQuota`。

`pkg/ingestor/ingestctrl/Cargo.toml` 将 crate 的库入口指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/ingestor/ingestctrl"` 标记 Go 对照包。本文件自身只依赖 crate 根定义的 `EngineFileSize` 和 `EngineId`，没有条件编译项、外部 I/O 或独立 Cargo feature。

## 核心职责

1. 用 `DiskUsage` 抽象“取得当前全部 Engine 占用快照”，使配额算法不依赖具体 EngineManager，也便于独立测试。
2. 用 `CheckDiskQuota` 对快照排序并累计磁盘、内存占用，以 `totalDiskSize + totalMemSize > quota` 作为严格超限条件。
3. 将超限后的非 importing Engine ID 放入 `largeEngines`，把超限后的 importing Engine 只计入 `inProgressLargeEngines`，同时始终返回所有 Engine 的磁盘和内存总量。

该函数只做判定和候选选择，不关闭、刷盘或导入 Engine，也不返回“需要释放多少字节”。调用方必须把结果转换成后续资源治理动作。

## 主要符号

- `pub trait DiskUsage`：快照提供者边界；唯一方法 `EngineFileSizes(&self) -> Vec<EngineFileSize>` 每次返回一份可被排序的拥有型列表。
- `pub struct DiskQuotaResult`：纯结果值，派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`。字段为 `largeEngines: Vec<EngineId>`、`inProgressLargeEngines: usize`、`totalDiskSize: i64`、`totalMemSize: i64`。
- `pub fn CheckDiskQuota(manager: &dyn DiskUsage, quota: i64) -> DiskQuotaResult`：唯一算法入口。它只借用 provider，取得快照后在本地排序、遍历并返回结果。
- `EngineFileSize`（定义于 `pkg/ingestor/ingestctrl/lib.rs`）：输入记录含 `UUID`、`DiskSize`、`MemSize`、`IsImporting`；其中 importing 表示 Engine 正持有 import 锁。
- `EngineId`（定义于 `pkg/ingestor/ingestctrl/lib.rs`）：返回候选所用的 Engine 唯一标识。

## 执行流程

`CheckDiskQuota` 的流程如下：

1. 调用一次 `manager.EngineFileSizes()` 取得快照；空列表自然得到默认结果。
2. 原地排序。`IsImporting=true` 的记录排在前面；同一 importing 分组内按 `DiskSize + MemSize` 从小到大排列。排序键相加使用 `wrapping_add`。
3. 按排序后的顺序遍历，每条记录分别以 `wrapping_add` 累加到 `totalDiskSize` 和 `totalMemSize`。
4. 若当前累计的磁盘与内存之和严格大于 `quota`，则进入超限分支：importing 记录只增加 `inProgressLargeEngines`；非 importing 记录把 `UUID` 追加到 `largeEngines`。
5. 遍历不会提前退出，因此总量覆盖整个快照；`largeEngines` 保持“同组占用从小到大”的后缀顺序，而不是倒序或任意集合。

排序把 importing Engine 提前计入累计量，却不把它们暴露为可立即处理的 ID。这样后续较大的非 importing Engine 更容易落入超限后缀；这与 Go 实现及测试期望完全一致。

## 数据与状态

本模块没有全局或持久状态。输入是调用瞬间的 `Vec<EngineFileSize>` 快照，排序只改变该局部副本，不会回写 EngineManager。`DiskQuotaResult::default()` 的数值为零、列表为空。

真实快照链路是 `pkg/ingestor/ingestctrl/local.rs::Backend::EngineFileSizes`（以及其 `DiskUsage for Backend` 实现）委托 `pkg/ingestor/ingestctrl/engine_mgr.rs::engineFileSizes`；后者持有 Engine map 锁，遍历每个 Engine 并调用 `pkg/ingestor/ingestctrl/engine.rs::Engine::getEngineFileSize`。单个 Engine 的磁盘量来自 `engine_meta.total_size + pending_file_size`，内存量来自 `TotalMemorySize()`，`IsImporting` 来自 `isLocked()`。

`totalDiskSize` 与 `totalMemSize` 分开累计并完整返回，候选判定则使用两者之和。代码不验证负 quota、负占用或重复 `EngineId`；这些输入会按普通 `i64` 回绕算术和顺序规则处理。

## 依赖与调用关系

直接下游依赖只有 crate 内的 `EngineFileSize`、`EngineId`。核心调用链为：

`pkg/session/runtime/import_sst.rs::RuntimeBackend::disk_quota_pressure`
→ `disk_quota::CheckDiskQuota(&self.local, quota)`
→ `DiskUsage for local::Backend::EngineFileSizes`
→ `engineManager::engineFileSizes`
→ `Engine::getEngineFileSize`。

RustCodeGraph 还确认本文件被 `pkg/ingestor/ingestctrl/disk_quota_test.rs`、`pkg/ingestor/ingestctrl/local.rs` 和 `pkg/session/runtime/import_sst.rs` 使用。图查询无法为 trait 动态分派生成精确静态 callers/callees 边，因此上述 provider 链以对应实现源码核对；生产入口 `disk_quota_pressure` 则由 RustCodeGraph 符号源码直接确认。

## 错误处理与边界

接口和算法均不返回 `Result`，也不会主动记录错误。`DiskUsage` 实现若无法取得数据，只能按其自身策略返回列表；当前 `engineManager::engineFileSizes` 在 Engine map 锁 poisoned 时通过 `unwrap_or_default()` 返回空列表，因而本函数会给出“零占用、无候选”，不会区分真实空状态与取快照失败。

边界语义包括：累计量等于 quota 时不超限，因为判断使用 `>`；空快照返回默认结果；importing Engine 永不进入 `largeEngines`；全部记录仍参与总量；`wrapping_add` 使排序键、字段累计和最终比较均按二进制补码回绕。`pkg/ingestor/ingestctrl/disk_quota_test.rs::check_disk_quota_wraps_total_usage_like_go` 用 `i64::MAX + 1` 验证了最后一点。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或文件句柄。并发隔离来自快照边界：`EngineFileSizes` 返回拥有型 `Vec` 后，排序和累计完全在当前调用栈内完成，不再持有 EngineManager 的锁。

快照可能在函数返回前已经落后于真实 Engine 状态，尤其 `IsImporting`、pending file size 和内存占用会并发变化；结果因此是决策提示而非事务性配额保证。调用方在关闭、刷盘或导入候选前必须重新处理 Engine 生命周期竞争。该函数不会锁定候选，也不保证释放这些 Engine 后一定达到 quota。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ingestor/ingestctrl/disk_quota.go`。Rust 的 `DiskUsage::EngineFileSizes` 对应 Go 接口同名方法；`DiskQuotaResult` 把 Go 的四个具名返回值聚合为结构体；`CheckDiskQuota` 的排序、累计、严格 `>` 判断以及 importing/非 importing 分流与 Go 第 42–63 行一致。

Go 注释允许远端存储场景返回 `nil`，Rust 以空 `Vec` 表达同一结果。Go `int64` 运算按二进制补码回绕；Rust 为避免 debug 构建溢出 panic，在三处显式采用 `wrapping_add`，属于语义对齐而非算法差异。Go 使用 `uuid.UUID`，Rust 使用 crate 内 `EngineId(u128)`。

测试对照为 `pkg/ingestor/ingestctrl/disk_quota_test.go::TestCheckDiskQuota` 与 `pkg/ingestor/ingestctrl/disk_quota_test.rs::TestCheckDiskQuota`：两者使用同样五个 Engine 和 30000、20000、12000、5000 四档 quota，断言相同候选、in-progress 数量及总量。Rust 另有溢出回绕回归测试。

## 扩展指南

- 修改排序或候选规则时，首先改 `CheckDiskQuota`，并同步独立测试文件 `pkg/ingestor/ingestctrl/disk_quota_test.rs`；若仍要求 Go parity，也应同步核对 `disk_quota.go` 和 `disk_quota_test.go`，不要只让 Rust 测试通过。
- 增加新的占用维度时，需要同步扩展 `EngineFileSize`、`Engine::getEngineFileSize`、`DiskUsage` 的各实现、排序键、累计结果以及上层 `disk_quota_pressure` 消费逻辑，避免只计算却不暴露或只暴露却不参与 quota。
- 若要传播快照失败，当前 `Vec` 返回签名和 `DiskQuotaResult` 都不足以表达错误，需要端到端调整 trait、`Backend` 实现与调用方；不能把“锁 poisoned 时空列表”误当可靠的零占用。
- 若要提供强一致配额保证，不能仅在本纯函数中加判断；必须在 Engine 生命周期管理层设计锁定或版本校验，并评估锁竞争和导入吞吐。
- 保持测试逻辑与源文件分离；新增用例放在同目录 `disk_quota_test.rs`，不要内嵌到生产文件。

兼容风险主要是候选顺序与 Go 行为漂移；正确性风险是 importing 状态或快照失败被误解释；性能风险集中在每次复制并排序全部 Engine，复杂度约为 `O(n log n)`、额外空间为 provider 返回的 `O(n)` 快照。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件，目标目录与文件已索引。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/disk_quota.rs`：核对完整 69 行源码、6 个符号及三个直接使用文件。
- RustCodeGraph `query/node/explore`：核对 `CheckDiskQuota`、`DiskUsage::EngineFileSizes`、`DiskQuotaResult`、`EngineFileSize`、`engineManager::engineFileSizes`、`Engine::getEngineFileSize`、`RuntimeBackend::disk_quota_pressure` 的定义和调用上下文。精确 `callers/callees` 对 trait 动态分派无输出，文档未把缺失图边写成已验证静态边。
- crate 与模块边界：`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/ingestctrl/lib.rs`、`pkg/ingestor/ingestctrl/local.rs`。
- Go 对照：`pkg/ingestor/ingestctrl/disk_quota.go`。
- 测试证据：`pkg/ingestor/ingestctrl/disk_quota_test.rs`、`pkg/ingestor/ingestctrl/disk_quota_test.go`。
- 人工复核结论：本文件存在是为了把 Engine 占用采集与配额候选算法解耦；运行时对单次快照排序、累计并分类；安全扩展必须同步数据来源、算法、调用方和独立 parity 测试。
