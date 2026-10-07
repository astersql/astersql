# `br/pkg/restore/log_client/import_retry.rs`

## 文件定位

`import_retry.rs` 属于 `astersql-br-pkg-restore-log-client` library crate。crate 根由 `br/pkg/restore/log_client/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定；`lib.rs` 通过 `#[path = "import_retry.rs"] pub mod import_retry` 挂载本文件，并用 `pub use import_retry::*` 暴露其公开符号。Cargo 清单没有为本模块设置 feature 开关，因此它随该 crate 正常编译。

在日志恢复链路中，本文件位于“计算待恢复文件的全局键范围”和“向各 TiKV region 发送 Apply 请求”之间。直接生产调用者是 `import.rs` 的 `LogFileImporter::ImportKVFiles`：它构造 `RangeController`、安装 `RangeCtlMetricListener`，再把每个 region 上的文件过滤与 Apply 动作包装成 `RegionFunc` 交给 `ApplyFuncToRange`。本文件不负责组装 ImportSST 请求，也不负责读取外部存储；它只负责 region 枚举、错误分类、退避和重试游标选择。

## 核心职责

本文件有两组相互配合的职责：

1. `RangeController` 把一个键区间分页展开为 region，逐个调用业务回调，并在拓扑或 RPC 发生变化时决定重试当前 region、从范围起点重扫，或立即停止。
2. `RPCResult` 把本地/传输错误、ImportSST 消息和结构化 store error 统一成可分类结果；`RetryStrategy` 将分类结果压缩为 `StrategyGiveUp`、`StrategyFromThisRegion`、`StrategyFromStart` 三种控制流。

`CreateRangeController` 还承担一个正确性边界：先用 `TruncateTS` 去掉传入结束键的时间戳，再调用 `PrefixNextKey` 形成扫描的排他上界。Go 文件在该处明确指出，省略这一步可能少扫 region 并造成数据丢失；因此调用方应传原始结束键，而不应自行重复推进上界。

## 主要符号

- `RegionFunc = Box<dyn FnMut(&Context, &mut RegionInfo) -> RPCResult + Send>`：单 region 的业务回调。`FnMut` 允许闭包维护计数等状态，`&mut RegionInfo` 允许重试路径更新 leader，`Send` 允许回调跨线程所有权边界，但本文件本身不创建线程。
- `RangeCtlEventListener: Send`：定义 `OnRequestRegion`、`OnRetryRegion`、`OnRetryRange`、`OnRegionSuccess` 四个观察点。`RangeCtlNopListener` 是默认空实现；`RangeCtlMetricListener` 把四个事件映射到静态 Prometheus `Counter`。
- `RangeController`：保存 `start`、经调整的 `end`、共享的 `Arc<dyn SplitClient>`、累计 `errors`、外层 `RetryState` 和事件监听器。Rust 字段当前均为 `pub`，但安全扩展仍应优先通过构造器和 `SetEventListener` 保持不变量。
- `CreateRangeController`：建立默认使用 Nop listener 的控制器，并规范化结束键。
- `ApplyFuncToRange`：范围级入口；检查重试预算，调用 `PaginateScanRegion`，然后按顺序处理每个 region。
- `applyFuncToRegion`：单 region 状态机；调用业务闭包、累计错误，并按 `RetryStrategy` 递归进入当前 region 或整个范围。
- `tryFindLeader` / `handleRegionError`：处理 NotLeader、ServerIsBusy、RegionNotInitialized 等可恢复 store 错误。前者使用独立的 4 次、2 秒初始、10 秒上限退避策略查询 `GetRegionByID`。
- `RPCResultFromPBError`、`RPCResultFromError`、`RPCResultOK`：分别构造 ImportSST/store 错误、本地或传输错误、成功结果。
- `RPCResult::{OK, Error, StrategyForRetry, StrategyForRetryStoreError, StrategyForRetryGoError}`：判断成功、生成可读消息并完成策略分类。
- `IsMemoryLimited`：仅当存在 `ServerIsBusy` 结构且顶层 `errorpb::Error.Message` 包含 `"memory is limited"` 时返回真；不能改为读取嵌套 busy 消息。

本文件没有条件编译项、模块级常量或异步函数。

## 执行流程

生产主链如下：

1. `LogFileImporter::ImportKVFiles` 在 `import.rs` 中计算所有日志文件重写后的最小 `startKey` 和最大 `endKey`，创建最大 45 次、100 ms 初始、15 s 上限的 `RetryState`。
2. 它调用 `CreateRangeController`；构造器对结束键执行 `TruncateTS` 与 `PrefixNextKey`，接管调用方传入的 `Arc<dyn SplitClient>`，并安装 `RangeCtlNopListener`。
3. 调用者立刻通过 `SetEventListener` 换成 `RangeCtlMetricListener`，再创建一个拥有文件、规则、加密信息、后端和 importer client 副本的 `RegionFunc`。
4. `ApplyFuncToRange` 先用 `ShouldRetry` 检查共享预算，再通过 `PaginateScanRegion(start, end, ScanRegionPaginationLimit)` 获取完整 region 列表。每个 region 先触发 `OnRequestRegion`，再交给 `applyFuncToRegion`。
5. 回调返回成功时触发 `OnRegionSuccess` 并继续下一 region。失败时 `onError` 给错误附加 region id，并通过 `multierr::Append` 留在累计错误链中。
6. `StrategyGiveUp` 立即返回累计错误；`StrategyFromThisRegion` 先调用 `handleRegionError`，修复成功则触发 `OnRetryRegion` 并重试同一 `RegionInfo`；修复失败则触发 `OnRetryRange` 并重新执行 `ApplyFuncToRange`。
7. `StrategyFromStart` 触发 `OnRetryRange`、消耗一次指数退避，然后放弃当前扫描游标并从原始范围重新分页扫描。递归范围调用成功后，外层以 `cont = false` 停止遍历旧 region 列表，避免重复处理旧拓扑。

`handleRegionError` 的优先级很重要：memory-limit busy 固定等待 15 秒后重试当前 region；NotLeader 若响应携带新 leader，则原地更新并立即重试；若未携带 leader，则先消耗外层退避并用 `tryFindLeader` 查询 PD；查询失败升级为整段重扫。其它当前-region错误先消耗外层退避再重试。

## 数据与状态

`RangeController` 的 `start`/`end` 在构造后作为每次整段重扫的稳定边界；局部重试修改的是循环中的 `RegionInfo` 副本，主要是其 `Leader`。一旦选择 `StrategyFromStart`，新的 `PaginateScanRegion` 结果替换旧拓扑视图。

`rs: RetryState` 是当前 region 重试和整段重扫共享的外层预算。`ShouldRetry` 只检查状态，实际消耗发生在 `ExponentialBackoff`；直接使用响应内新 leader 的分支不退避。`tryFindLeader` 另建 `NewBackoffRetryAllExceptStrategy`，因此 leader 查询的内部尝试不直接消耗外层计数，但失败后可能导致外层重新扫描。

`errors: Option<Error>` 保存历次失败。即使后续 region 重试成功，旧错误也不会主动清空；它只在最终放弃或预算耗尽时作为诊断链返回。分页扫描本身的错误通过 `?` 直接返回，不加入该链。若预算耗尽但尚无累计错误，Rust 实现返回 `"retry exhausted"` 哨兵，避免返回空错误。

`RPCResult` 约定三类字段：`Err` 表示本地或传输错误，`ImportError` 表示 ImportSST PB 的文本，`StoreError` 保存 TiKV 结构化错误。`OK` 要求三者全部为空。若 `Err` 存在，它在策略分类和错误文本中优先；PB 构造器则同时保存 `ImportError` 与 `StoreError`，使结构化 store 分类仍然可用。

## 依赖与调用关系

上游关系：

- `lib.rs` 声明并公开再导出本模块；`import_retry_test.rs` 作为独立测试模块由同一 crate 根在 `cfg(test)` 下挂载。
- `import.rs::LogFileImporter::ImportKVFiles` 是生产调用入口：使用 `CreateRangeController`、`RangeCtlMetricListener`、`RegionFunc`、`RPCResultFromError` 和 `RPCResultOK`，最终调用 `ApplyFuncToRange`。
- `import.rs` 的 region Apply 结果还通过 `RPCResultFromPBError` 保留 ImportSST 响应中的 store error。

下游关系：

- `astersql_br_pkg_restore_utils::TruncateTS` 与 `utils_retry::PrefixNextKey` 规范化结束键。
- `SplitClient::{ScanRegions/GetRegionByID}` 的适配层经 `PaginateScanRegion`、`CheckRegionEpoch` 提供拓扑与 leader 信息。
- `utils_retry::{RetryState, WithRetryV2, NewBackoffRetryAllExceptStrategy}` 提供外层和 leader 查询的退避状态。
- `errorpb`、`import_sstpb`、`metapb` 提供结构化错误、ImportSST 结果和 region/peer 数据；`grpc_status::FromError` 与 `Code` 分类传输错误。
- `multierr::Append` 聚合多次失败；`logutil` 记录 leader 查询失败；`failpoint::Inject` 为 memory-limit 长等待提供测试钩子。

RustCodeGraph 对 `import_retry.rs` 的文件级“used by”只识别到测试文件，但精确源码检索确认 `import.rs` 在导入列表和 `ImportKVFiles` 主链中实际使用这些公开符号。因此文件级图边不足以否定生产接线，调用关系以符号引用和源码为准。

## 错误处理与边界

- `RPCResult::StrategyForRetryGoError` 仅把 gRPC `Unavailable`、`Aborted`、`ResourceExhausted`、`DeadlineExceeded` 视为当前-region瞬时错误；其它状态或无法解析状态的普通错误均 `GiveUp`。
- store error 中的 `ServerIsBusy`、`RegionNotInitialized`、`NotLeader` 在当前 region 重试；其它 store/import 错误默认 `FromStart`。三类错误字段全空却请求分类属于调用方逻辑错误，策略为 `GiveUp`，`Error` 返回 BUG 哨兵。
- NotLeader 携带 leader 时直接替换 `region.Leader`；不携带时向 PD 查询。查询结果缺 region、epoch 改变或没有 leader都会报错，其中 epoch mismatch 由 `isNonRetryErrForFindLeader` 标为 leader 查询内部不可重试，随后外层重扫。
- memory-limit 判定读取顶层 store error 文本，等待 15 秒是给 TiKV 回收内存的窗口；普通 busy 走指数退避。改变字段来源或等待方式会偏离 Go 行为。
- `CreateRangeController` 对空结束键执行 `PrefixNextKey` 后不再表示正无穷；Rust 测试因此用 `"zzz"` 作为高位结束键。新增调用方不能假定空 `end` 会扫描到键空间末端。
- 当前实现用递归表达 region 重试和范围重扫。预算会限制带退避的失败路径，但响应直接给出 leader 的连续重试不调用 `ExponentialBackoff`；业务回调应保证 leader 更新后能收敛，避免异常服务端持续返回同一“新 leader”造成深递归。

## 并发与资源生命周期

本文件执行同步、串行的 region 遍历，没有启动异步任务。`thread::sleep` 会阻塞当前工作线程；memory-limit 分支最长固定阻塞 15 秒，普通重试按 `RetryState` 阻塞。若未来迁移到异步运行时，不能直接沿用阻塞 sleep。

`metaClient` 由 `Arc<dyn SplitClient>` 持有，控制器和 `tryFindLeader` 闭包可安全共享其所有权；实际线程安全保证由 `SplitClient` trait 实现承担。`RegionFunc` 和 listener 要求 `Send`，但都以 `&mut` 串行调用，不存在本文件内部的并发访问。`RangeCtlMetricListener` 保存静态 Counter 引用，不拥有或释放指标注册对象。

控制器拥有回调之外的重试状态与错误链；回调由调用方以 `&mut RegionFunc` 借入，其捕获资源在 `ApplyFuncToRange` 返回后随调用方闭包销毁。`RangeController` 没有显式 `Drop` 或连接关闭职责；Importer gRPC 生命周期由 `LogFileImporter::Close` 管理。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `import_retry.go`：`RegionFunc`、事件 listener、`RangeController`、三种 `RetryStrategy`、`RPCResult` 构造/分类，以及 NotLeader、busy、epoch 变化和 gRPC code 分支均保留。Go 的 `CreateRangeController` 同样执行 `TruncateTS + PrefixNextKey`，`ImportKVFiles` 同样使用 45 次、100 ms 到 15 s 的外层退避并挂四类指标。

语言适配上的主要差异是：Rust 用 `Arc<dyn SplitClient>` 和拥有值的 `RetryState` 代替 Go interface 与指针；用 `Option<Error>`/`Option<errorpb::Error>` 表达 nil；用 `Box<dyn FnMut + Send>` 表达可变回调；用 `(bool, Option<Error>)` 表达 Go 的多返回值。Rust 还在预算耗尽且错误链为空时生成明确哨兵错误。

当前 Rust 没有移植 Go 的 `prepareLogCtx`，因此没有把 `startKey`/`endKey` 字段注入派生日志 context；它直接克隆原 `Context`。Rust 的 `tryFindLeader` 依赖本地 stub 的 `Result<RegionInfo>`，以 `Region.is_none()` 代表 Go 的 nil region。Rust 的 `grpc_status::FromError` 也由 stub 从错误表示中恢复 code；扩展错误包装时需确认它仍能识别根因，才能保持 Go `errors.Cause + status.FromError` 的语义。

测试对应关系以 `import_retry_test.rs` 和 `import_retry_test.go` 为依据。Rust 独立测试覆盖扫描窗口、响应携带新 leader、普通 busy、四类 gRPC code、epoch 重扫、leader 查询缺 region 后的范围重扫、分页 leader，以及顶层/嵌套 memory-limit 文本差异；Go 测试还包含更完整的 region split 与 failpoint 长等待路径。该差异说明 Rust 当前测试面并非逐用例完全等量，但核心策略分支已有直接证据。

## 扩展指南

- 新增可重试错误类型时，先确定它应保持当前 region 还是必须刷新整个拓扑，再修改 `StrategyForRetryStoreError` 或 `StrategyForRetryGoError`；同步在独立文件 `import_retry_test.rs` 添加策略与实际控制流回归，并对照 `import_retry.go`/`import_retry_test.go`，不要只让分类函数返回目标枚举。
- 新增 region 修复动作时接入 `handleRegionError`。保持 memory-limit 优先级和 NotLeader 的“响应 leader 优先、PD 查询兜底”不变量，并评估它是否消耗外层 `RetryState`。
- 新增可观测事件时同时更新 `RangeCtlEventListener`、Nop 实现、Metric 实现以及 `import.rs` 中 listener 构造；trait 方法变化会影响所有实现者。
- 修改扫描边界时同时复核 `CreateRangeController`、`PaginateScanRegion` 和 `test_scan_success`。尤其不要移除 `TruncateTS + PrefixNextKey`，除非上游键约定及 Go 行为一起迁移并有防数据遗漏测试。
- 若消除递归或改成异步状态机，应保持两项行为：范围重扫成功后不得继续旧 region 列表；同一累计错误链和重试预算必须跨重扫保存。性能评估应关注阻塞 sleep、全范围重复扫描和深递归风险。
- 测试逻辑必须继续放在独立的 `import_retry_test.rs`，不要嵌入生产源文件。若生产行为与 Go 对齐发生变化，也应同步更新 Go 对照说明或明确记录有意差异。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标文件被成功定位。
- RustCodeGraph `explore "br/pkg/restore/log_client/import_retry.rs RetryState ImportRetryableErrorStrategy"`：获得目标源、Go 对照、范围控制器相关调用上下文。
- RustCodeGraph `node --file br/pkg/restore/log_client/import_retry.rs --offset 1 --limit 260` 与 `--offset 248 --limit 220`：完整核对本文件 428 行源码、54 个符号及状态机分支。
- RustCodeGraph `query CreateRangeController`、`query ApplyFuncToRange`、`query RPCResultFromError` 与 `files --filter br/pkg/restore/log_client`：核对 Rust/Go 同名符号、模块文件集合和图索引限制。
- RustCodeGraph `node` 读取 `lib.rs`、`import.rs`、`import_retry_test.rs`：核对模块挂载、生产入口和 Rust 独立测试。包目录不存在 `doc.go`，无额外 Go 包契约文件可读。
- 直接读取 `br/pkg/restore/log_client/Cargo.toml`、`import_retry.go`、`import_retry_test.go`，并用精确符号检索核对生产引用：确认 crate 边界、Go 语义、测试边界和 `import.rs` 的实际接线。
- 未运行 Cargo 或代码测试：本任务是纯文档分析，计划明确禁止 Cargo；交付验证仅执行固定章节结构检查与人工事实复核。
