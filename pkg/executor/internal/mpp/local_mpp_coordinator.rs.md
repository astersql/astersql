# `pkg/executor/internal/mpp/local_mpp_coordinator.rs`

## 文件定位

本文件属于 `astersql-executor-internal-mpp` crate（见同目录 `Cargo.toml`），是 TiDB 进程内 MPP（Massively Parallel Processing）执行的本地协调器实现。crate 入口 `pkg/executor/internal/mpp/lib.rs` 将 `NewLocalMppCoordinator`、`MppCoordinatorPlan`、`MppDispatchSession`、`MppStoreInfo`、`MppReportSink` 和 `NoopMppReportSink` 重新导出；外层 `executor_with_retry.rs` 再把构造出的 `Box<dyn kv::MppCoordinator>` 注册、执行、按需重建，并作为 `kv::Response` 消费。

它位于“物理计划已选定、TiFlash 根任务即将生成”与“调用者逐包读取 MPP 结果”之间：`NewLocalMppCoordinator` 调用 `SessionRootMppTaskGenerator::GenerateRootMPPTasks` 切分 Fragment 和 KV range，编码 TiFlash DAG 与派发请求；`LocalMppCoordinator::Execute` 启动派发，`kv::Response::Next` 汇聚根任务流，`Close` 取消剩余任务并处理 execution summary。文件本身不负责优化计划、选择 MPP 与否或决定重试策略。

源文件已经带有 `// Copyright 2026 AsterSQL.`，并非门面或桩：其 1553 行实现包含生产传输适配、状态机、请求编码、拓扑标记和结果类型。

## 核心职责

1. `NewLocalMppCoordinator` 把计划身份、会话快照、TiFlash store 拓扑和上报回调组装成一个可执行的 `kv::MppCoordinator`；空 KV range、无 TiFlash task、缺失 task metadata 都在入口阶段拒绝。
2. `append_mpp_dispatch_requests` / `prepare_dispatch_requests_from_root` 为每个 `MPPTask` 克隆 protobuf executor 树，按动态/静态分区信息改写扫描 table id，填充 exchange same-zone 标志，序列化 `tipb::DagRequest`，并携带事务、协议、schema、resource group、连接和 digest 元数据。
3. `LocalMppCoordinator::dispatchAll` 为 Ready 请求各启一个工作线程，经 `CoordinatorTransport` 派发；只有 root 请求建立流并持续拉包，所有线程通过 `mpsc::channel<WorkerEvent>` 向 `Next` 汇聚。
4. `Next` 在每个成功数据包返回前调用 `CheckVisibility(start_ts)`；任一工作线程错误成为可见错误、设置 `dispatch_failed` 并取消本 gather 的运行中任务。
5. `MppReportHandle` 接收按 task id 路由的 `ReportStatus`，拒绝未知/重复上报，保存首个 task 错误和 execution summaries；`Close` 在允许的计划形状下等待全部 report，交给 `MppReportSink` 记录统计、合并 RU、补 dummy summary。
6. `TaskZoneInfoHelper` 为 ExchangeSender/ExchangeReceiver 生成 same-zone 布尔数组，使 TiFlash 能利用 TiDB、当前 task、交换对端的 zone 关系。
7. `mppResponse` 实现 `kv::ResultSubset`，拥有 protobuf 包和运行时统计，并缓存内存大小估算，供恢复层安全缓冲。

## 主要符号

- `MppReportSink`：语句侧统计边界。`RecordOneCopTask` 记录完整三计数 summary，`MergeTiFlashRUConsumption` 合并每个 task 的 RU，`FillDummySummaries` 补齐未记录 plan id，`ReportTimeout` 记录超时；`NoopMppReportSink` 是无副作用实现。
- `CoordinatorResponseStream` / `CoordinatorTransport`：将状态机与具体 RPC 解耦。生产实现 `MppClientCoordinatorTransport` 包装 `kv::MPPClient`，在 `DispatchMPPTask` 和 `EstablishMPPConns` 返回 retry 标志时循环；非 root task 返回 `None`，root task 返回 `MppClientResponseStream`。
- `MppReportHandle`：`Mutex<MppReportState> + Condvar + AtomicBool` 的独立上报器，同时实现 `kv::MppStatusReporter`。它能在协调器本体被外层互斥锁持有、`Close` 等待报告时继续接收 gRPC report。
- `LocalMppCoordinator`：核心状态机。请求列表和 task 状态由可变协调器独占；工作线程、事件接收器、停止标记、report handle 管理并发；`executed`、`closed`、`dispatch_failed`、`all_reports_handled` 保证阶段和幂等性。
- `DispatchSessionInfo` / `MppDispatchSession`：前者是文件内部请求编码快照，后者是公开构造参数；字段覆盖时区、flags、summary 开关、除法精度、schema 版本、资源组、连接身份、SQL/plan digest 和 TiDB zone。
- `PreparedDispatchRequests`：返回请求及 task/store id 的批次容器；task/store id 当前由协调器保存，主要对应 Go 的派发日志数据，但本文件 Rust 实现不输出该日志。
- `TiFlashStore`、`TiFlashStoreInfo`、`MppStoreInfo`：store 地址到 `store_id`/DC zone 的边界；`add_tiflash_store_info` 从 `placement::DCLabelKey` 读取 zone。
- `TaskZoneInfoHelper`：缓存 executor id 到对端 zone 数组；`fill_same_zone_flag_for_exchange` 递归遍历 tipb executor DAG。
- `need_report_execution_summary`：只有从当前根向下的路径先遇到 `PhysicalLimit`，随后到达目标 plan id 的 `PhysicalTableReader` 时才允许 coordinator 直收 summary。
- `mppResponse`：成功或错误结果载体；生产返回路径使用 `new`，`from_error`/`Error` 等接口保留了错误响应能力，但当前线程错误直接通过 `WorkerEvent::Error` 返回。
- `MppCoordinatorPlan` / `NewLocalMppCoordinator`：公开构造门面；计划对象、start/query/gather id 与 plan ids 被一次性移入协调器。

## 执行流程

1. 外层工厂准备 `SessionRootMppTaskGenerator`、`BuildPBContext`、`MppCoordinatorPlan`、会话快照和 store 列表，调用 `NewLocalMppCoordinator`。
2. `GenerateRootMPPTasks` 生成 fragments、KV ranges 和已知节点地址。入口拒绝空 range；随后遍历每个 fragment 的 self tasks，补齐并校验所有 task metadata，并以地址集合大小作为 `node_count`。
3. `new_local_mpp_coordinator` 根据协议版本初始化状态：低于 `MppVersionV2` 时清空 coordinator 地址；仅“地址非空且 statement plan 中存在 Limit -> 目标 TableReader 路径”时设置 `report_execution_info`。
4. 每个 fragment 进入 `appendMPPDispatchReq`：锁住 `Fragment::Sink`，调用 `to_pb(..., StoreType::TiFlash)` 得到 root executor，再为每个 task 进入 `prepare_dispatch_requests_from_root`。
5. 请求准备阶段按 `PartitionTableIDs` 或非静态裁剪的 `TableID` 改写扫描节点；为交换节点填 same-zone；按 root/non-root 选择 `TypeChunk`/`TypeChBlock`；序列化 DAG 并构造初始状态为 `MppTaskReady` 的 `MPPDispatchRequest`。每个请求同时在 `MppReportHandle` 中安装唯一 report 槽位。
6. 外层 `ExecutorWithRetry::setupMPPCoordinator` 取得 `StatusReporter`、注册 `(query_id, gather_id)`，然后调用 `Execute`。`Execute` 保存 context 并调用 `dispatchAll`；重复 Execute、已关闭状态或 Execute 后追加请求都会报错。
7. `dispatchAll` 在持有 `&mut self` 的串行阶段把每个 Ready 请求改成 Running，再各启线程。线程先 `Dispatch`；非 root 到此结束，root 逐次 `stream.Next()`。成功包发 `WorkerEvent::Response`，RPC/包内错误发 `Error`，最后总会尝试发 `Finished` 并关闭 stream。
8. 调用者反复执行 `kv::Response::Next`。Response 事件先做 MVCC visibility 检查再包装为 `mppResponse`；Error 事件保存首错、标记派发失败、取消所有运行中 task 并返回错误；Finished 将仍为 Running 的请求标为 Done 并减少活跃线程。所有线程结束后 join，返回 `None`。
9. `Close` 首次调用时标记 closed、设置停止标志、取消运行中请求、join 线程；随后幂等调用 `handleAllReports`。若开启直报且派发未失败，它等待全部报告或超时；全齐时记录完整 summaries、合并 RU、补 dummy summaries，超时则只调用 `ReportTimeout` 并成功返回。
10. 外层 `ExecutorWithRetry` 负责把 `Next` 错误交给恢复策略；可恢复时关闭/注销旧 gather、分配新 gather id、经 factory 重建本文件的协调器并重新 Execute。本文件本身不决定是否恢复。

## 数据与状态

- 请求生命周期是不变量 `Ready -> Running -> Done` 或 `Ready/Running/Done -> Cancelled`。`dispatchAll` 只派发 Ready 请求；`cancelMppTasks` 只把当时 Running 请求的地址加入 cancel map，但最终把全部请求标为 Cancelled。首请求已 Cancelled 时直接返回，避免重复 RPC。
- `executed` 禁止二次 Execute 和执行后追加 fragment；`closed` 使 Close 幂等并阻止首次执行；`all_reports_handled` 防止重复消费 report；`dispatch_failed` 阻止失败派发后再等待 summary。
- `active_workers` 与 `workers` 必须同步维护：每次 spawn 前加一，每个 `Finished` 减一，归零时 join。即使 sender 全部断开，`Next` 也会归零并 join。
- `MppReportState.requests` 以 task id 唯一索引，`request_index` 用来恢复请求顺序。`reported_request_count` 只在首次合法 report 时递增；`first_error_message` 只保存第一个非空 task 错误。
- `MppReportHandle::wait_and_collect` 在 Condvar 谓词 `reported_request_count < expected` 上等待，超时后生成一次性快照；`waiting` 仅供测试观察等待已开始。
- `completed_execution_summaries` 仅在全部 report 按时到齐并成功处理后写入；超时路径不保存部分 summary。summary 缺少 `time_processed_ns`、`num_produced_rows` 或 `num_iterations` 时不调用 `RecordOneCopTask`，但仍参与 RU 合并。
- same-zone 缓存以 executor id 为键。executor id 或当前 task zone 未知时保守填 `true`；root ExchangeSender 直接比较 TiDB zone；解码失败、store 未知或缓存长度异常也采用不阻止同 zone 优化的保守值。
- `mppResponse::memory_size` 以 `Cell<i64>` 延迟缓存 `ExecDetails` 固定大小与 protobuf `compute_size`。它不是跨线程共享类型；结果在主消费路径中独占传递。

## 依赖与调用关系

上游主链（由 RustCodeGraph 文件关系和 `executor_with_retry.rs` 核对）：

`CoordinatorFactory::Build`（具体工厂） -> `NewLocalMppCoordinator` -> `Box<dyn kv::MppCoordinator>` -> `ExecutorWithRetry::setupMPPCoordinator` -> `Execute`；随后 `ExecutorWithRetry::Next`/`Close` 转发到协调器。注册表同时保存 `StatusReporter`，使 `MppCoordinatorManager::ReportStatus` 无需获取协调器主互斥锁即可调用 `MppReportHandle::ReportStatus`。

主要下游关系：

- 计划侧：`SessionRootMppTaskGenerator::GenerateRootMPPTasks`、`Fragment::Sink`、`PhysicalPlan::to_pb`、`PhysicalLimit`、`PhysicalTableReader`。
- KV/RPC 侧：`kv::MPPClient::{DispatchMPPTask, EstablishMPPConns, CancelMPPTasks, CheckVisibility}`，以及 `kv::{MppCoordinator, Response, ResultSubset, MppStatusReporter}` 契约。
- 协议侧：`kvproto::mpp::{MppDataPacket, TaskMeta}` 与 `tipb::{Executor, DagRequest, TiFlashExecutionInfo, ExecutorExecutionSummary}`；`protobuf::Message` 负责编解码。
- 统计侧：`CopRuntimeStats`、`ExecDetails` 和注入的 `MppReportSink`。
- 拓扑侧：`astersql-ddl-placement::DCLabelKey` 提供 zone 标签键。

同目录 `Cargo.toml` 将该 crate 定位到 `pkg/executor/internal/mpp/lib.rs`，并声明 planner core/base/physicalop、KV、distsql、infoschema、store、execdetails、kvproto、tipb 等依赖；本文件直接使用其中 planner、KV、placement、execdetails、kvproto、protobuf 和 tipb。crate 没有针对本文件的 feature 门控，条件编译只有若干 `#[cfg(test)]` 观察/注入接口。

## 错误处理与边界

- 构造边界明确拒绝空 `kv_ranges`、没有任何 TiFlash task、task 缺 metadata；请求准备拒绝 fragment sink 锁中毒、plan protobuf 构造/序列化失败、需要改写的 table/index scan 没有 partition id、join 缺 outer child。
- 传输适配按 `kv::MPPClient` 的 retry 标志重试 dispatch/establish；dispatch protobuf response 自带 error 时直接转 `SharedError`。此处没有独立退避策略，退避状态由 `kv::Backoffer` 和 client 契约持有。
- stream 包内错误优先使用已经收到的第一个 `ReportStatus` task 错误，保持 Go 的错误文案优先级；否则使用包内 error。任一错误都经 `Next` 触发取消。
- `CheckVisibility` 在数据已经从工作线程到达、交给调用者之前执行；失败直接返回错误。Rust 当前原样传播 visibility 错误，不像 Go 版本把它统一替换为 `ErrQueryInterrupted`。
- Mutex/Condvar 中毒转为明确错误；worker panic 由 `joinWorkers` 汇总为 `MPP dispatch worker panicked`。线程向已关闭 channel 发送失败被有意忽略，因为主消费方已结束。
- `ReportStatus` 对未知 task、重复 task 和 protobuf 解码失败报错。report 等待超时是非致命边界：只上报指标，不调用 record/merge/fill，也不保存部分 summaries。
- 未知 tipb executor 在 `fill_same_zone_flag_for_exchange` 中保持子树不变并成功返回，以对齐 Go “记录后继续”的前向兼容行为；相反，`update_executor_table_id` 遇到未知 executor 会报错，因为无法安全确定应沿哪条子树改写表 ID。
- `Close` 优先保留 cancel/join 错误；随后仍尝试处理 report，最后用 `close_result.and(report_result)` 返回。若前者失败，report 错误不会替换它。

## 并发与资源生命周期

协调器要求 `Execute`、`Next`、`Close` 通过外层 `Arc<Mutex<Box<dyn MppCoordinator>>>` 串行访问；文件内部只把每个已标为 Running 的请求克隆给独立 OS 线程。因此“派发胜出”和“取消胜出”由对协调器的独占访问确定，不会在同一请求上把 Cancelled 任务重新派发。`local_mpp_coordinator_test.rs` 的三个 race 测试覆盖非 Ready 跳过、dispatch 胜出后 cancel 包含 store、Close 胜出后 Execute 失败。

每个工作线程拥有请求副本、context、transport、sender 和停止标记。`stop_requested` 使 Close 后的拉流循环退出；stream 始终尝试 Close。协调器保留 `JoinHandle`，正常耗尽、错误或显式 Close 都通过 `joinWorkers` 回收，不把后台线程遗留给 Drop。需要注意：`LocalMppCoordinator` 本身没有 `Drop`；可靠清理由 `ExecutorWithRetry::Drop -> Response::Close` 保证，直接使用者也必须调用 Close。

report 通道与响应通道刻意分离。`MppReportHandle` 自含 Mutex/Condvar，可被注册表持有的 `Arc<dyn MppStatusReporter>` 并发调用；因此 Close 即使在外层协调器 mutex 内等待，也能被 report 唤醒。`local_mpp_coordinator_aster_unit_test.rs::report_status_wakes_close_without_coordinator_mutex` 验证这一不死锁性质。

资源边界包括：所有 worker join、每个 root response stream 幂等 Close、每次 gather 至多一次 Cancel RPC、外层注册表在失败/Close/恢复时注销。Rust 本文件没有 Go `sendToRespCh` 中的 memory tracker consume/release，也没有 `nextImpl` 的三秒 killed 轮询；取消主要依赖显式 Close、工作线程错误或上层生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/internal/mpp/local_mpp_coordinator.go`，Go 测试是 `local_mpp_coordinator_test.go`。主要一一对应关系如下：

- Go `localMppCoordinator`、`NewLocalMPPCoordinator`、`Execute`、`Next`、`Close` 对应 Rust `LocalMppCoordinator`、`new_local_mpp_coordinator`/公开 `NewLocalMppCoordinator` 及相同 trait 方法。
- Go `appendMPPDispatchReq` 对应 Rust `appendMPPDispatchReq` + `append_mpp_dispatch_requests` + `prepare_dispatch_requests_from_root`；Rust 把 sessionctx/config/infoschema 读取前移为显式的 `MppDispatchSession`、store 列表和 plan 参数，更容易独立验证。
- Go `taskZoneInfoHelper`、`needReportExecutionSummary` 和 `mppResponse` 分别对应 Rust `TaskZoneInfoHelper`、`need_report_execution_summary`、`mppResponse`。同路径 Rust 测试复刻 Go 的 Limit/TableReader 形状和 zone quick-fill 用例。
- Go goroutine、`respChan`、`finishCh`、`WaitGroup` 被 Rust worker thread、`mpsc::channel<WorkerEvent>`、`AtomicBool`、`JoinHandle` 替换；状态转换和“取消只包含 Running store”的意图保持一致。
- Go `reqMap + reportStatusCh + mu` 被 Rust `MppReportHandle(Mutex + Condvar)` 替换，从而让 status reporter 可独立于主协调器锁被注册。

当前可见差异必须在扩展时保留意识：

- Go `nextImpl` 定时检查 session killed 标志和 context cancellation；Rust 本文件没有等价轮询。
- Go `sendToRespCh` 对响应内存向 `memTracker` 记账并捕获 panic；Rust 只提供 `ResultSubset::MemSize`，记账由外层恢复/内存层承担，线程 panic 最终由 join 报错。
- Go 有 `needTriggerFallback` 和 `enableCollectExecutionInfo` 的会话/config 分支；Rust 请求使用 `ReportExecutionSummary`，但没有在本文件中把网络错误替换成 TiFlash timeout 的 fallback 分支。
- Go report 超时后会冻结已收到的部分 report，并可记录 raw statement summaries；Rust 超时后仅 `ReportTimeout` 并返回，不消费部分 summaries。全部到齐时，两者都只把三项必需计数完整的 summary 作为 cop task，同时合并 RU、补 dummy。
- Go `needReportExecutionSummary` 还显式解包 `PhysicalShuffleReceiverStub`、CTE seed/recur 及多种 statement wrapper（通过 `getActualPhysicalPlan`）；Rust 入口要求调用者直接传 `statement_plan: Option<Box<dyn PhysicalPlan>>`，本函数只按通用 children 遍历及 Limit/TableReader 特判。新增特殊物理节点时需验证 children 暴露是否足够。
- Go 使用可配置/常量 `receiveReportTimeout`（当前 100 ms）；Rust 通过构造参数 `report_timeout` 注入。

## 扩展指南

- 新增派发字段或会话元数据：修改 `MppDispatchSession`、内部 `DispatchSessionInfo`、`NewLocalMppCoordinator` 字段映射和 `prepare_dispatch_requests_from_root` 的 `MPPDispatchRequest`/`DagRequest` 构造；同步 `coordinator_prep_aster_unit_test.rs`，并与 Go `appendMPPDispatchReq` 对照默认值、开关和 digest/resource group 规则。
- 新增 tipb executor 类型：同时审查 `update_executor_table_id` 和 `TaskZoneInfoHelper::fill_same_zone_flag_for_exchange`。前者必须明确扫描改写路径，后者必须明确 child 遍历和 exchange 行为；不要简单落入 unknown 分支。同步独立测试文件，避免把测试写进生产源文件。
- 修改任务状态或取消策略：保持 Ready/Running/Done/Cancelled 单向语义、Execute/Close 的外层串行契约以及“只取消已 Running store”的规则；同步 `local_mpp_coordinator_test.rs` 的 race 回归测试和 `local_mpp_coordinator_aster_unit_test.rs` 的失败取消测试。
- 修改 report 协议：保持 task id 唯一性、重复/未知拒绝、首错优先级、Condvar 唤醒不依赖主协调器锁；同步 report 解码、超时、并发唤醒测试，并评估 Go 的部分 report 快照语义是否需要补齐。
- 修改传输/重试：从 `CoordinatorTransport` 和 `MppClientCoordinatorTransport` 接入，不把具体 client 逻辑散入状态机。必须验证非 root 不建流、root TaskMeta 字段完整、retry 循环终止条件、stream Close 和 visibility 检查。
- 修改结果对象或缓冲：同步 `mppResponse` 的所有权、`MemSize` 缓存与 `recovery_handler.rs` 的容量语义；相关独立测试还包括 `mpp_response_aster_unit_test.rs`。
- 修改公开构造签名或 crate 边界：同步 `lib.rs` 再导出、具体 `CoordinatorFactory` 实现和 `executor_with_retry.rs`，并检查所有 Cargo manifest 仍使用一致依赖。该类改变可能影响 distsql/executor 接线，不应只改单文件。
- 保持仓库约束：Rust 生产逻辑和测试分别放置；与 Go 行为尽可能一致；不删除 PingCAP Apache License；修复后的 Rust 文件保留顶部 AsterSQL 版权行；代码变更后先 `cargo fmt --all`。本次仅写文档，没有修改代码或运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/executor/internal/mpp` 确认本模块 16 个索引文件；目标文件识别 113 个符号。
- RustCodeGraph `query LocalMppCoordinator` / `query MppCoordinator`：定位 Rust `LocalMppCoordinator`（源码第 427 行）、`MppCoordinatorPlan`（第 1440 行）、公开 `NewLocalMppCoordinator`（第 1456 行）、`kv::MppCoordinator` trait 和外层 `ExecutorWithRetry`/注册表符号。
- RustCodeGraph `node --file pkg/executor/internal/mpp/local_mpp_coordinator.rs` 分段读取完整 1-1553 行，核对传输、report、状态机、请求编码、zone、计划判定、响应和公开入口；文件关系显示其被 `executor_with_retry.rs` 等 6 个 Rust 文件使用。
- 直接读取 `pkg/executor/internal/mpp/Cargo.toml` 与 `lib.rs`，核对 crate 名、依赖、模块声明、公开再导出和独立测试模块；本目录没有 `doc.go`，因此无额外包契约可读。
- RustCodeGraph 分段读取 `executor_with_retry.rs`，核对 `setupMPPCoordinator -> Execute`、`Next`、`Close`、注册/注销和恢复重建主链。
- RustCodeGraph 分段读取 Go `local_mpp_coordinator.go`（1-982）和 `local_mpp_coordinator_test.go`（1-179），核对移植语义、并发模型、request 字段、same-zone、report、错误与已知差异。
- RustCodeGraph 分段读取 Rust `local_mpp_coordinator_test.rs`（1-353）和 `local_mpp_coordinator_aster_unit_test.rs`（1-748），核对计划形状、zone quick-fill、派发/取消竞态、client retry、并行交错流、失败取消、首错优先、report 去重/解码、超时和并发唤醒。
- 未运行 Cargo：任务和总计划明确规定纯文档分析不运行 Cargo。验收以事实复核和固定 11 章节结构命令为准。
