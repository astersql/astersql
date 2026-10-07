# `pkg/dxf/framework/testutil/context.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-testutil` crate，是 DXF（Distributed eXecution Framework）集成测试复用的上下文与内存集群夹具。crate 入口 `pkg/dxf/framework/testutil/lib.rs` 将 `context` 声明为公开模块并再导出其公开 API，因此测试通常可直接从 `astersql_dxf_framework_testutil` 引入 `NewTestDXFContext`、`TestDXFContext`、`TestContext` 等符号。

它不实现真正的任务调度、持久化或执行器业务逻辑。真实副作用由调用方提供的 `Arc<dyn DxfRuntime>` 承担；本文件只编排资源覆盖、节点扩缩容、owner 选举、异步下线和测试观测。直接使用者包括 `pkg/dxf/framework/integrationtests/framework_test.rs`、`pkg/dxf/framework/integrationtests/framework_ha_test.rs`，而 `pkg/dxf/framework/testutil/disttest_util.rs` 会将执行过的子任务写入这里定义的 `TestContext`。

## 核心职责

- 定义测试所需的最小任务模型：`Step`、`TaskState`、`SubtaskState`、`TaskBase`、`Task`、`Subtask`，以及步骤哨兵 `STEP_INIT`、`STEP_ONE`、`STEP_TWO`、`STEP_DONE`。
- 以 `DxfRuntime` 隔离测试夹具与执行器、调度器、存活节点发布、资源配置和轮询间隔等外部副作用。
- 通过 `TestDXFContext` 模拟多 TiDB 节点集群，维护节点 ID、owner 集合和节点顺序，并提供扩容、缩容、owner 切换、异步关闭与节点抽样操作。
- 通过 `TestContext` 按 `(task_id, step)` 去重记录已执行的子任务，并提供线程安全的计数与调用序号。
- 用 `CheckIntervalGuard` 和 `TestDxfInner::drop` 保证测试结束时恢复轮询间隔、节点资源并回收后台线程和节点。

## 主要符号

- `DxfError(String)`：本测试边界的轻量错误类型，实现 `Display` 与 `std::error::Error`；运行时失败和夹具校验失败均以 `Result<_, DxfError>` 返回。
- `Step(i64)` 与步骤常量：`STEP_INIT = -1`、`STEP_ONE = 1`、`STEP_TWO = 2`、`STEP_DONE = -2`。`TaskBase::is_done` 将 `Failed`、`Reverted`、`Succeed` 视为终态。
- `TaskState`、`SubtaskState`、`TaskBase`、`Task`、`Subtask`：独立于具体存储实现的测试数据模型；字段保留任务类型、并发度、作用域、元数据、执行节点与摘要等集成测试所需信息。
- `NodeResource::for_cpu`：按给定 CPU 数生成固定的 32 GiB 内存和 100 GiB 磁盘配额。
- `CheckIntervals`、`ReduceCheckInterval`、`CheckIntervalGuard`：将七类调度/执行检查间隔缩短到 10–200 ms；守卫析构时调用 `DxfRuntime::set_check_intervals` 恢复旧值。
- `DxfRuntime`：`Send + Sync` 的注入接口，覆盖资源切换、executor/scheduler 启停与取消、存活 executor ID 更新及检查间隔替换。
- `TestContext`：内部使用 `RwLock<HashMap<(i64, Step), HashSet<i64>>>` 去重收集子任务，使用 `AtomicU64` 产生从 0 开始的调用序号。公开入口是 `CollectSubtask`、`CollectedSubtaskCnt` 和 `next_call_time`。
- `ClusterState`：受单个 `Mutex` 保护的节点索引、owner ID 集合、按加入顺序保存的节点和 FIFO 回收 ID 队列；`NODE_ID_POOL_CAPACITY` 将回收队列限制为 100。
- `TestDxfInner`：由多个 `TestDXFContext` clone 共享的运行时、集群状态、后台线程句柄、恢复守卫、原始资源和原子计数器；其 `Drop` 是最终清理点。
- `TestDXFContext`：公开的可克隆集群句柄。主要方法包括 `ScaleOut[By]`、`ScaleIn[By]`、`ChangeOwner`、`AsyncChangeOwner`、`AsyncShutdown`、`GetRandNodeIDs`、`GetNodeIDByIdx`、`NodeCount`、`WaitAsyncOperations` 与 `test_context`。
- `NewTestDXFContext`：按显式节点数、CPU 数和间隔策略构造夹具；`NewDXFContextWithRandomNodes`：在闭区间内基于当前纳秒值选择节点数，并固定为 16 CPU、启用短间隔。

## 执行流程

1. `NewTestDXFContext` 或 `NewDXFContextWithRandomNodes` 进入 `TestDXFContext::new`。后者先校验 `min_count <= max_count`，再选择闭区间内的节点数。
2. `new` 先用 `DxfRuntime::set_node_resource` 保存旧资源并安装测试资源；按参数可调用 `ReduceCheckInterval` 保存并替换轮询间隔，然后创建共享的 `TestDxfInner`。
3. 初始化循环为每个节点调用 `get_node_id` 和 `ScaleOutBy`。节点 ID 优先从 FIFO 回收队列取得，否则由 `AtomicI32` 生成 `:4000`、`:4001`……。
4. `ScaleOutBy` 先启动 executor；若显式指定 owner，再启动 scheduler。scheduler 启动失败时会尝试停止刚启动的 executor。随后在锁内拒绝重复 ID并登记节点，再通过 `update_live_ids` 向运行时发布完整存活 ID 列表。
5. 若发布存活列表失败，`ScaleOutBy` 会撤销内存登记、回收 ID，并尽力停止刚启动的 scheduler/executor，然后把原错误返回。
6. 初始化末尾及普通 `ScaleOut` 末尾调用 `elect_if_needed`：仅当集群非空且没有 owner 时，用 `election_counter % node_count` 轮转选择节点，先启动 scheduler，再在锁内标记 owner；如果节点在启动期间已消失，则停止该 scheduler。
7. `ScaleInBy` 在锁内移除节点、重建下标、移除 owner 标记并回收 ID；随后发布存活列表，停止 executor，必要时停止 scheduler，最后补选 owner。不存在的 ID 是幂等成功。`ScaleIn` 重复移除当前尾节点，空集群时提前结束。
8. `ChangeOwner` 先在锁内清空所有 owner 标记，再逐个停止旧 scheduler，最后补选一个 owner；`AsyncChangeOwner` 将同一操作放入后台线程并保存 `JoinHandle`。
9. `AsyncShutdown` 在短临界区内确认节点及 owner 状态，释放集群锁后先取消 executor/scheduler，再启动后台线程执行 `ScaleInBy`。`WaitAsyncOperations` 取走当前句柄并逐一 join。
10. 最后一个 `TestDXFContext` clone 离开作用域时触发 `TestDxfInner::drop`：join 遗留线程、停止所有剩余节点、丢弃间隔守卫以恢复旧间隔，并恢复原始节点资源。

## 数据与状态

集群事实由 `ClusterState` 统一维护：`nodes` 保留稳定的加入顺序，`node_indices` 加速按 ID 定位，`owner_ids` 允许测试构造多 owner 情景，三者在增删节点后必须一致。删除节点后会重新枚举 `nodes` 构造索引，避免 `Vec::remove` 引起的下标漂移。`TidbNode.owner` 决定缩容和析构时是否需要停止 scheduler。

节点 ID 回收模拟 Go/Kubernetes 场景中地址复用。回收队列最多保存 100 个 ID，超出容量的 ID 被丢弃；`context_test.rs::recycled_node_id_pool_matches_go_capacity` 以 101 个节点证明再次扩容时第 101 个 ID 会新分配为 `:4101`。`GetRandNodeIDs` 的“随机”是由 `election_counter` 提供环形起点后顺序截取，不保证密码学或统计随机性，但保证结果去重、数量不超过现有节点数。

`TestContext` 的 key 直接使用 `(task_id, Step)`，value 使用 `HashSet<subtask_id>`，所以重复收集同一子任务不会增加计数，不同任务或步骤互不混淆。`call_time.fetch_add(1, SeqCst)` 返回递增前的值，适合实现“首次失败、随后成功”等测试脚本。

## 依赖与调用关系

该源文件自身只依赖 Rust 标准库；跨 crate 行为通过 `DxfRuntime` trait 注入。所在 crate 的 `Cargo.toml` 声明了 scheduler、taskexecutor、storage、mock、proto、testkit 等 DXF/测试依赖，但这些具体 crate 没有直接导入 `context.rs`，而是由同 crate 的其他辅助模块和集成测试组合使用。

主要上游调用关系如下：

- `framework_test.rs::framework_owner_change_and_scale_lifecycle_preserves_live_nodes` 构造 `NewTestDXFContext`，依次切换 owner、扩容和缩容，并验证存活节点发布。
- `framework_test.rs::framework_parallel_task_observation_keeps_distinct_task_and_step_counts` 并发调用 `TestContext::CollectSubtask`，验证任务/步骤隔离和未知步骤计数为零。
- `framework_ha_test.rs` 通过 `NewDXFContextWithRandomNodes`、`AsyncShutdown`、`WaitAsyncOperations` 和多 owner `ScaleOutBy` 覆盖高可用场景。
- `testutil/disttest_util.rs::RegisterTaskTypeForRollback` 将 `CollectSubtask` 作为步骤执行器的观测副作用。
- `testutil/migration_aster_unit_test.rs` 通过记录型 `DxfRuntime` 验证启动、停止、资源恢复、间隔恢复及不存在节点的异步关闭。

主要下游调用边均落在 `DxfRuntime`：构造调用 `set_node_resource`/`set_check_intervals`，扩缩容调用 executor/scheduler 的 start/stop/cancel，节点集合变化调用 `update_live_executor_ids`。因此运行时实现必须使这些方法可在不同测试线程调用，并正确处理重复清理或补偿调用。

## 错误处理与边界

- `NewDXFContextWithRandomNodes` 对反向区间返回 `DxfError("minimum node count exceeds maximum")`；合法闭区间宽度至少为 1，因此取模分母不为零。
- 所有注入运行时错误均通过 `Result` 传播。`ScaleOutBy` 对 scheduler 启动失败、重复节点 ID、存活 ID 发布失败提供局部补偿，但补偿中的 stop 错误被忽略，以保留原始失败。
- `ScaleInBy` 对不存在节点返回 `Ok(())`；`AsyncShutdown` 也将不存在节点视为幂等空操作。`ScaleIn` 请求数超过现存节点数时安全结束。
- `ScaleInBy` 先修改内存状态，再调用 `update_live_ids`。若该调用失败，函数立即返回，已经移除的节点不会在此路径停止，属于运行时实现/测试使用者需要注意的非事务性边界。
- `ChangeOwner` 若停止任一旧 scheduler 失败，会提前返回，内存中 owner 标记已经清空，后续 owner 补选尚未执行。
- `WaitAsyncOperations` 将线程 panic 映射为 `DxfError("asynchronous DXF operation panicked")`，线程函数自身的 `DxfError` 继续传播；`Drop` 中的 join、stop 和资源恢复错误则被有意忽略，因为析构无法返回错误。
- `GetNodeIDByIdx` 对越界索引主动 panic；调用者应先用 `NodeCount` 建立边界。标准库锁使用 `unwrap`，因此持锁线程 panic 造成的 poison 也会继续 panic。

## 并发与资源生命周期

`TestDXFContext` 通过 `Arc<TestDxfInner>` 安全克隆；真正的清理只在最后一个 clone 释放时发生。集群复合状态由一个 `Mutex<ClusterState>` 串行化，子任务观测使用 `RwLock` 支持多读，ID、选举偏移和调用序号使用顺序一致性原子操作。`DxfRuntime: Send + Sync` 是跨线程调用的契约。

调用外部运行时时通常不持有集群锁：尤其 `AsyncShutdown` 先读取 owner 状态后解锁，再发 cancel，避免运行时回调与集群锁形成死锁。`elect_if_needed` 也先在锁内选择节点、解锁启动 scheduler、再重新加锁确认节点仍存在；这一两阶段流程显式处理并发缩容竞态。

后台操作的 `JoinHandle<Result<(), DxfError>>` 保存在 `joins` 中。显式调用 `WaitAsyncOperations` 可以获得错误；未显式等待时，`TestDxfInner::drop` 仍会 join，但忽略结果。`CheckIntervalGuard` 先于原始资源恢复被释放，最终清理顺序是：后台线程、节点组件、检查间隔、节点资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/testutil/context.go`。两版共同保留了 `:4000` 起始编号、容量 100 的可回收节点 ID 池、按节点维护 executor/owner、无 owner 时补选、异步关闭先 cancel 后等待、子任务去重计数、32 GiB/100 GiB 默认资源以及相同的短轮询间隔数值。

Rust 版不是对 Go 结构的字段级复制。Go `TestDXFContext` 直接持有 `testing.TB`、`kv.Storage`、`storage.TaskManager`、gomock controller，以及真实 `taskexecutor.Manager`/`scheduler.Manager`；初始化还负责 mock store、session pool 和 failpoint。Rust 版把这些环境职责抽象为 `DxfRuntime`，`TidbNode` 只保存 ID/owner，因此其当前定位是可测试的生命周期编排层，而非完整 Go 测试环境的内嵌重建。

恢复机制也不同：Go 通过 `testing.TB.Cleanup` 和 `WaitGroupWrapper` 清理；Rust 通过 `CheckIntervalGuard`、`Drop for TestDxfInner` 与显式 `WaitAsyncOperations` 实现 RAII。Go 随机节点数和节点抽样使用 `math/rand`；Rust 的构造器用系统时间纳秒取模，节点抽样用原子计数器控制环形起点。因此只能依赖区间、数量和去重语义，不应假设两版生成相同序列。

Go 的 `CallTime` 是由调用者在锁约束下管理的普通整数；Rust 的 `next_call_time` 是 `AtomicU64`。Go 的任务/子任务类型来自 `framework/proto`，Rust 当前在本文件定义迁移所需的轻量模型。相关等价性由 `migration_aster_unit_test.rs` 和 Rust 集成测试覆盖，而不是由类型同一性保证。

## 扩展指南

- 新增节点生命周期动作时，优先扩展 `DxfRuntime`，在 `TestDXFContext` 中只编排顺序；同步提供记录型 runtime 测试，验证成功、失败补偿和析构清理。trait 新方法会影响所有实现者，包括 `context_test.rs`、`migration_aster_unit_test.rs`、`framework_test.rs` 与 `framework_ha_test.rs` 中的桩。
- 修改节点增删逻辑时必须同时维护 `nodes`、`node_indices`、`owner_ids`、`TidbNode.owner` 与存活 ID 发布；特别检查外部调用失败后的内存状态和 runtime 状态是否仍可清理。
- 修改异步流程时不要在调用 `cancel_*`、`stop_*`、`start_scheduler` 等外部方法期间长期持有 `ClusterState` 锁；新增线程必须进入 `joins`，并同时验证显式等待与 Drop 清理。
- 新增轮询间隔字段时同步修改 `CheckIntervals`、`ReduceCheckInterval`、所有 `DxfRuntime` 实现及恢复测试，确保 guard 保存并恢复完整旧值。
- 修改 Go 对齐语义时同步核对 `context.go`；若差异是 Rust 有意设计（如 trait 注入、确定性环形抽样），应在独立 Rust 测试中固定可观察契约，而不是假称实现细节完全一致。
- 测试应继续放在独立文件：局部回归优先加入 `pkg/dxf/framework/testutil/context_test.rs` 或 `migration_aster_unit_test.rs`；跨组件的 owner/HA/并发行为放入 `pkg/dxf/framework/integrationtests/framework_test.rs` 或 `framework_ha_test.rs`，不要把测试内嵌回 `context.rs`。
- 兼容风险集中在公开的 Go 风格符号名、步骤/状态值、节点 ID 规则和幂等边界；性能风险集中在删除节点时重建索引、全量 clone 存活 ID、顺序一致性原子及单集群锁竞争，但这些路径面向测试，优化前应优先保持可预测语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/dxf/framework/testutil` 确认目标源、模块入口和独立测试均已索引；`node --file pkg/dxf/framework/testutil/context.rs` 分段核对了全部 642 行和主要符号；文件节点报告被 28 个文件使用。对 `context.rs::NewTestDXFContext` 的精确 `callers` 查询在限定时间内未返回，因此精确上游边使用仓库 `rg` 结果补证，未据此臆造图边。
- 源码与 crate 边界：`pkg/dxf/framework/testutil/context.rs`、`pkg/dxf/framework/testutil/lib.rs`、`pkg/dxf/framework/testutil/Cargo.toml`。
- Go 对照：`pkg/dxf/framework/testutil/context.go`；重点核对节点 ID 池、扩缩容、owner、异步关闭、清理、子任务收集和检查间隔。
- 独立 Rust 测试：`pkg/dxf/framework/testutil/context_test.rs` 验证 ID 池容量；`pkg/dxf/framework/testutil/migration_aster_unit_test.rs` 验证常量、节点生命周期、资源与间隔恢复、缺失节点幂等行为。
- 集成证据：`pkg/dxf/framework/integrationtests/framework_test.rs` 验证 owner/扩缩容/存活节点和并发子任务观测；`pkg/dxf/framework/integrationtests/framework_ha_test.rs` 验证随机节点范围、异步下线、多 owner 与并发收集；`pkg/dxf/framework/testutil/disttest_util.rs` 展示 `CollectSubtask` 的实际注册入口。
- 本任务是纯文档分析，按计划未运行 Cargo；使用规定的 11 标题结构命令验证文档形状，并人工复核符号、流程、已知差异和未事务化失败边界均有上述代码或测试依据。
