# `pkg/ingestor/ingestctrl/region_job.rs`

## 文件定位

源码：[region_job.rs](./region_job.rs)；独立 Rust 测试：[region_job_test.rs](./region_job_test.rs)；Go 对照：[region_job.go](./region_job.go) 与 [region_job_test.go](./region_job_test.go)。

本文件属于 `astersql-ingestor-ingestctrl` crate；crate 入口 `pkg/ingestor/ingestctrl/lib.rs` 以公开模块 `region_job` 导出它，`pkg/ingestor/ingestctrl/Cargo.toml` 则把该 crate 映射到 Go 包 `pkg/ingestor/ingestctrl`。上层包说明 `pkg/ingestor/doc.go` 将 ingestor 定义为直接向底层存储导入 SST、准备 Region 拆分与 TiKV import mode 环境的组件。本文件位于这条导入链的 Region 作业控制层：它不执行 TiKV RPC，而是生成按 Region 划分的 `RegionJob`，决定 ingest 失败后的阶段，保存延时重试作业，并为本地引擎按 Store 负载挑选作业。

Rust 当前实际入口之一是 `pkg/dxf/importinto/write_ingest_backend.rs::GlobalSortWriteIngestBackend::ImportEngine`：transport 扫描给定键范围后调用 `newRegionJobs`，再把作业交给 `pkg/ingestor/ingestctrl/import_pipeline.rs::do_import`。后者仅在 `ImportOptions::local_engine` 为真时启用 `storeBalancer`，并始终创建 `regionJobRetryer`。本文件没有条件编译项，也没有网络、磁盘或 Cargo feature 分支。

## 核心职责

1. `newRegionJob` / `newRegionJobs` 把排序且可覆盖作业范围的 `LocatedRegion` 与半开作业区间相交，生成初始阶段为 `RegionScanned` 的 `RegionJob`。
2. `WriteRequest` / `newWriteRequest` 保存 Lightning 内部写请求所需的元数据、资源组、任务类型、请求来源和事务来源标记。
3. `getNextStageOnIngestError` 把可重试 ingest 错误归类为“重写”“仅重试 ingest”或“重新扫描 Region”。
4. `regionJobRetryer` 用按到期时间排序的并发安全堆保存延时重试作业，并提供取消、关闭和未处理作业回收接口。
5. `storeBalancer` 根据一个作业全部 peer Store 的当前负载之和选取作业；挑选时增加负载，完成或清理时由调用方释放负载。

这些职责只管理调度元数据与状态。本文件不实现写 SST、Ingest RPC、Region 扫描重试、重试次数上限或资源引用计数；它们分别由 `job_worker.rs`、调用方提供的 transport、`import_pipeline.rs` 与 `RegionJob` 的资源方法承担。

## 主要符号

- `defaultKVBatchCount: usize = 512`：默认写批次 KV 数；本文件内不消费该常量，供 crate 的其他导入逻辑使用。
- `LocatedRegion { region: RegionInfo, key_range: KeyRange }`：已经由上游定位并转换成 ingestctrl 表示的 Region 元数据和半开键范围。空 `key_range.end` 表示无上界。
- `newRegionJob(...) -> RegionJob`：公开构造器，写入 Region、数据、作业范围和 Region 拆分阈值，并把阶段设为 `RegionJobStage::RegionScanned`。
- `newRegionJobs(...) -> Vec<RegionJob>`：公开的双指针式区间归并函数；要求 Region 与作业范围已排序，正确性依赖调用方提供可覆盖的输入。
- `minEnd(a, b) -> Vec<u8>`：内部结束键求小函数；将空键按正无穷处理。
- `WriteRequest` 与 `newWriteRequest(...)`：公开值对象及构造器。`request_source` 生成 `internal_lightning:{taskType}`，`txn_source` 固定为 `1`。RustCodeGraph 目前只发现独立测试调用该构造器，未发现生产调用者。
- `getNextStageOnIngestError(&Error) -> RegionJobStage`：公开错误分类器；生产调用者是 `job_worker.rs::RegionJobBaseWorker::runJob`。
- `RetryEntry`：私有堆条目，以 `wait_until` 和单调 `sequence` 定序；反转 `Ord` 以在标准大顶堆中实现最早到期优先，序号同时稳定相同到期时间的先后。
- `regionJobRetryer`：公开类型，内部字段私有；`push`、`popReady`、`close`、`cleanupUnprocessedJobs` 构成完整生命周期。
- `storeBalancer`：公开类型，内部保存待调度作业、Store 负载与入队序号；公开方法是 `push`、`jobLen`、`pickJob`、`releaseStoreLoad` 和 `storeLoad`。

公开 API 保留了 Go 风格命名，crate 根通过 `#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]` 接受这些名称。文件中没有 trait 或 enum 定义；`PartialEq`、`Eq`、`PartialOrd`、`Ord` 和 `Default` 都是对内部队列条目或控制器的实现。

## 执行流程

Region 作业生成流程如下：

1. `GlobalSortWriteIngestBackend::ImportEngine` 对每个待导入范围调用 transport 的 `Scan`，取得 `Vec<LocatedRegion>`。
2. `newRegionJobs` 逐个处理 `sortedJobRanges`，用共享 `region_index` 跳过完全位于当前作业范围左侧的 Region；遇到完全位于右侧的 Region 时处理下一个作业范围。
3. 对相交范围，起点取两者较大值，终点由 `minEnd` 取较小值；仅当终点无界或 `start < end` 时创建非空作业。
4. `newRegionJob` 克隆 Region 和整份 `data`，设置 `[start, end)`、拆分阈值及 `RegionScanned` 阶段。外部 global-sort 入口当前传入空 `data`，实际 ingest 数据随后由 `import_pipeline::do_import` 通过 `JobResources` 关联到作业。
5. `do_import` 为每个生成的作业建立资源引用并提交给 worker；需要重新扫描时，`ImportEngine` 提供的 rescan 闭包再次调用 `newRegionJobs`，同时保留时间戳和拆分阈值。

失败与重试流程如下：

1. `job_worker.rs::runJob` 在 `Wrote` 阶段调用 ingest；若发生可重试错误，保存错误原因并调用 `getNextStageOnIngestError`。
2. 包含 `KVIngestFailed` 的错误回到 `RegionScanned`，意味着 SST 写入也要重做；`ServerIsBusy`、`RequestTooNew`、`Timeout` 或不含 `epoch`/`region` 的一般 `Retryable` 错误保持 `Wrote`，只重试 ingest；其他错误变为 `NeedRescan`。
3. `import_pipeline.rs::dispatch_results` 对 `RegionScanned`/`Wrote` 作业增加重试计数，计算最大 30 秒的指数退避并调用 `regionJobRetryer::push`。超过上限直接返回最后错误；`NeedRescan` 在该结果分发位置被视为不可到达，因为 worker 层的 process/rescan 路径负责处理它。
4. `do_import` 的 retry 线程重复调用 `popReady`。作业到期后重新走统一 `submit` 路径；取消或结束时调用 `cleanupUnprocessedJobs`，逐个执行 `job.done()` 释放资源。

本地引擎的均衡流程是：`submit` 调用 `storeBalancer::push`；balancing 线程调用 `pickJob`，将作业涉及的 peer Store 负载全部加一，然后把作业发送给 worker。`import_pipeline::Worker` 用 `StoreLoadGuard` 保证处理完成或中途失败时调用 `releaseStoreLoad`。取消后，balancing 线程等待生产者和 retry 线程停止，再排空队列、释放负载并结束每个作业。

## 数据与状态

键范围统一采用 `KeyRange` 的半开区间 `[start, end)`，空 `end` 代表正无穷。`newRegionJobs` 不重新排序、不检查连续性，也不验证输入 Region 是否完整覆盖作业范围；其单调 `region_index` 依赖调用者遵守排序前提。输出作业共享相同的拆分阈值，但每个作业都拥有克隆的 `RegionInfo`、范围和 `Vec<KvPair>`。

`RegionJobStage` 是作业状态机的关键外部类型：本文件构造 `RegionScanned` 并返回 `RegionScanned`、`Wrote` 或 `NeedRescan` 的错误决策；`Ingested` 由 worker 在成功后设置。错误分类目前依赖 `Error::to_string()` 中的英文标记，而不是结构化 TiKV 错误类型，因此文案变化会改变状态迁移。

重试器的 `queue`、`closed` 与 `sequence` 分别承担待处理数据、终止状态和稳定顺序。`close` 只标记关闭并唤醒等待者，不排空堆；错误/取消路径必须再调用 `cleanupUnprocessedJobs` 取得所有权并完成资源回收。`cleanupUnprocessedJobs` 返回堆的 drain 顺序，调用方不得依赖它按到期时间排序。

均衡器以 `(入队序号, RegionJob)` 保存作业，但当前 `pickJob` 只按负载和迭代顺序取最小值，没有显式使用入队序号作为负载相等时的比较键。选中作业后使用 `swap_remove`，所以待调度列表不保持 FIFO。`storeLoadMap` 中的计数代表已挑选但尚未释放的作业数；同一作业的重复 Store ID 会被重复计数，和测试中的 `[2, 2]` peer 列表行为一致。

## 依赖与调用关系

直接 Rust 依赖全部来自标准库与同 crate 模块：

- `std::collections::{BinaryHeap, HashMap}` 保存重试堆和 Store 负载；`Mutex`、`Condvar` 与原子类型提供线程安全；`Instant`/`Duration` 驱动到期等待。
- `crate::job_worker::{RegionInfo, RegionJob, RegionJobStage}` 提供作业和状态机主体。
- crate 根的 `CancellationToken`、`Error`、`KeyRange`、`KvPair`、`Result` 提供取消、错误和通用数据模型。

主要上游边经 RustCodeGraph 核对为：

- `pkg/dxf/importinto/write_ingest_backend.rs::GlobalSortWriteIngestBackend::ImportEngine -> newRegionJobs`，同时覆盖初次 Region 扫描和 worker 的 rescan 闭包。
- `pkg/ingestor/ingestctrl/job_worker.rs::RegionJobBaseWorker::runJob -> getNextStageOnIngestError`。
- `pkg/ingestor/ingestctrl/import_pipeline.rs::dispatch_results -> regionJobRetryer::push/close`。
- `pkg/ingestor/ingestctrl/import_pipeline.rs::do_import -> regionJobRetryer::popReady/cleanupUnprocessedJobs`，以及 `storeBalancer::push/pickJob/releaseStoreLoad`。

`Cargo.toml` 的 `[lib] path = "lib.rs"` 证明 crate 边界；本文件本身只需无条件依赖中的标准 crate 内部类型，没有直接使用列在 Windows target 下的外部包。`newWriteRequest` 目前只有 `region_job_test.rs::TestNewWriteRequest` 这一条 Rust 调用边，因此应视为已实现但尚未接入生产 RPC 的兼容构造器。

## 错误处理与边界

- `newRegionJobs` 返回空向量而不是错误。外部 global-sort 入口会把“Region 扫描没有产生覆盖作业”转成 `Error::InvalidData`，但直接调用者必须自行验证覆盖性。
- 区间交集过滤 `start == end`，不会创建空作业；空结束键按无上界处理。未排序、重叠或存在缺口的输入没有显式报错，可能导致遗漏、重复或错误归属。
- `getNextStageOnIngestError` 不返回 `Result`，所有无法识别或非重试错误都保守映射为 `NeedRescan`。调用方只应在已经判定为可重试的分支使用它，当前 `runJob` 满足此前提。
- `regionJobRetryer::push` 在已关闭或互斥锁 poisoned 时返回 `false`；调用者据此保留并完成作业资源。`popReady` 将锁 poisoned 映射为 `Error::Poisoned`，将取消映射为 `Error::Cancelled`。
- 空重试堆上的 `popReady` 立即返回 `Ok(None)`，不无限等待；未来作业由外层循环再次调用。未到期时每次最多等待 10 ms，从而即使取消没有直接通知 Condvar，也能及时重新检查 token。
- `storeBalancer` 的可失败方法只因互斥锁 poisoned 返回 `Error::Poisoned`。空队列返回 `Ok(None)`；查询方法 `jobLen`/`storeLoad` 在 poisoned 时退化为零，适合观测但可能隐藏锁错误，不能作为强一致性判定。
- `releaseStoreLoad` 遇到未知 Store 会继续释放后续 Store；已有计数用 `saturating_sub` 防止下溢。这与 Go 的“记录不变量违规后继续最佳努力清理”语义一致，但 Rust 当前不记录日志。

## 并发与资源生命周期

`regionJobRetryer` 允许多个生产者 `push` 和一个或多个消费者 `popReady`。`push` 在加锁前后各检查一次 `closed`，避免关闭窗口仍然入队；Acquire/Release 用于发布关闭状态，序号只需 Relaxed，因为唯一性而非跨线程数据发布是其目的。`Condvar::notify_one` 唤醒一个等待者处理更早到期的新条目，`close` 的 `notify_all` 则终止所有等待者。

取消令牌不是 Condvar 的直接唤醒源，所以 `popReady` 把长退避切成最多 10 ms 的等待片段。该设计以少量周期唤醒换取取消响应，不会持锁执行作业。返回 `RegionJob` 后，队列已转移所有权；清理时 drain 也把所有权交给 `do_import`，由它调用 `job.done()`，保持资源引用次数平衡。

`storeBalancer` 用两个互斥锁分别保护作业列表和负载表。`pickJob` 按“jobs 后 loads”的顺序同时持锁并原子完成选择、移除和负载增加；`push` 只锁 jobs，`releaseStoreLoad` 只锁 loads，不形成反向锁序。负载的最终释放依赖 `StoreLoadGuard` 和取消排空逻辑，调用 `pickJob` 的新代码必须保证所有退出路径调用 `releaseStoreLoad`，否则后续调度会永久高估相关 Store。

独立测试 `TestStoreBalancerNoRace` 用 8 个线程并发 pick/release 200 个作业，验证队列清空且所有 Store 归零；`TestRegionJobRetryer` 验证延迟等待、取消、到期弹出和 close 后拒绝 push。测试与生产源码保持在独立的 `region_job_test.rs` 中。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/ingestctrl/region_job.go`，相关 Go 测试是 `region_job_test.go`。核心语义对应如下：

- 两版 `newRegionJobs` 都以已排序、连续、可覆盖的 Region 与作业范围为前提，并按边界交集生成作业。Go 版从 TiKV 编码键解码 Region 边界；Rust 的 `LocatedRegion` 已要求上游提供 ingestctrl 的原始 `KeyRange`，本函数不负责编解码。
- Go 的 `newWriteRequest` 直接构造 protobuf `sst.WriteRequest` 并设置 resource control context；Rust 用普通 `WriteRequest` 值对象表示相同字段，其中 `txn_source = 1` 对应 `LightningPhysicalImportTxnSource`，但当前生产链尚未调用它。
- Go 的 `getNextStageOnIngestError` 能从结构化 `IngestAPIError` 取出 `NewRegion` 并在其覆盖旧 Region 时直接回到写阶段；Rust 只返回阶段并按字符串分类，不能携带新 Region。这是迁移能力差异，不应把 Go 的结构化错误恢复能力视为 Rust 已支持。
- Go `regionJobRetryer` 自带 context、后台 `run` goroutine、channel 和 WaitGroup 清理；Rust 重试器只提供并发安全堆，后台线程、重试计数、退避和 `job.done()` 都由 `import_pipeline.rs` 组合。Rust 用 10 ms 分段 Condvar 等待响应取消。
- Go `storeBalancer` 自带输入/输出 channel 和两个 goroutine，内部用 `sync.Map`；Rust 类型只保留队列与打分原语，调度线程和 channel 位于 `import_pipeline::do_import`。两版都按所有 peer Store 的负载之和择低，并允许挑选瞬间之后负载已变化的近似最优结果。
- Go 明确警告 balancer 无背压、不应与 external engine 搭配以免 OOM；Rust 仅在 `options.local_engine` 为真时创建 balancer，external/global-sort 路径直接使用零容量 worker channel，自然避开该风险。

Rust 独立测试大量保留 Go 同名用例和注释，但并非所有测试都直接执行生产逻辑：例如 `TestCancelBalancer` 只保存 Go 的取消/WaitGroup 期望结构。判断 Rust 已支持的行为时，应以生产实现和真正调用目标符号的用例为准。

## 扩展指南

- 修改 Region/作业区间切分时，优先改 `newRegionJobs`/`minEnd`，并在 `region_job_test.rs::TestNewRegionJobs` 增加空上界、边界相等、跨多个 Region 的表驱动用例；若要接受非连续或未排序输入，应新增显式校验和 `Result`，而不是悄悄排序后掩盖上游错误。还需同步核对 Go `TestNewRegionJobs`。
- 新增 TiKV 错误类型或恢复阶段时，应同时修改 `getNextStageOnIngestError`、`job_worker.rs::runJob` 和 `TestGetNextStageOnIngestError`。优先推进结构化错误而非继续增加大小写敏感的字符串匹配，并评估是否需要像 Go 一样传回新 Region。
- 扩展重试策略时，退避计算与次数上限在 `import_pipeline.rs::dispatch_results`，队列排序/等待在本文件；两处必须一起审查。新增队列关闭语义时要覆盖 push/close 竞态、取消延迟、同到期时间顺序和 cleanup 后资源引用归零。
- 修改 Store 打分时，入口是 `storeBalancer::pickJob`；必须保持选中和负载增加的原子性，并同步 `releaseStoreLoad` 与 `StoreLoadGuard`。若引入 FIFO tie-break，应显式把 `nextJobIndex` 纳入比较，并更新 `TestStoreBalancerPick` 和 `TestStoreBalancerNoRace`。
- 若接入 `newWriteRequest` 到生产 RPC，应先确认 `txn_source = 1` 与目标协议常量一致，并让 `meta` 从当前无类型字节变为可验证的协议边界；同步扩展 `TestNewWriteRequest`，避免只验证字符串拼接。
- 新测试继续放在同目录独立文件 `region_job_test.rs`，不要内嵌进生产源文件。涉及 Go 语义变更时也要核对 `region_job.go`/`region_job_test.go`，防止 Rust 版本无意简化状态机或资源生命周期。

## 验证依据

- RustCodeGraph 状态：索引覆盖本仓库 11,467 个文件，其中 `pkg/ingestor/ingestctrl/region_job.rs` 完整索引为 355 行；符号图列出常量、两个公开数据类型、四个主要函数、重试器和均衡器的全部方法。
- RustCodeGraph 调用证据：`GlobalSortWriteIngestBackend::ImportEngine -> newRegionJobs`；`RegionJobBaseWorker::runJob -> getNextStageOnIngestError`；`dispatch_results -> regionJobRetryer::push/close`；`do_import -> popReady/cleanupUnprocessedJobs` 与 `storeBalancer::push/pickJob/releaseStoreLoad`。
- 已读生产路径：`pkg/ingestor/ingestctrl/region_job.rs`、`lib.rs`、`job_worker.rs::runJob`、`import_pipeline.rs::{dispatch_results, do_import}`、`pkg/dxf/importinto/write_ingest_backend.rs::ImportEngine`。
- 已读边界声明：`pkg/ingestor/ingestctrl/Cargo.toml` 与 `pkg/ingestor/doc.go`。
- 已读 Go 对照：`pkg/ingestor/ingestctrl/region_job.go` 的 `newRegionJobs`、`newWriteRequest`、`getNextStageOnIngestError`、`regionJobRetryer`、`storeBalancer`；已读 Go 测试路径 `pkg/ingestor/ingestctrl/region_job_test.go` 并以 Rust 测试内的同名移植说明交叉核对。
- 已读 Rust 独立测试：`pkg/ingestor/ingestctrl/region_job_test.rs`；直接覆盖错误阶段映射、重试到期/取消/关闭、Region 边界归并、Store 选择与并发释放、缺失 Store 的最佳努力清理、写请求字段。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的 `rg` 命令确认本文恰有十一个固定二级标题，并人工复核文档只陈述上述源码、调用图、Cargo、Go 和测试能够支持的事实。
