# `pkg/executor/internal/mpp/executor_with_retry.rs`

## 文件定位

本文件属于 `astersql-executor-internal-mpp` crate；crate 根由 `pkg/executor/internal/mpp/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 将本模块声明为私有模块并公开再导出其中的重试执行器、注册表、工厂接口和配置。它位于 MPP 响应消费链的协调器上层：实现 `kv::Response`，把一个 `kv::MppCoordinator` 包装成带有限缓冲和可恢复重建能力的响应源（`ExecutorWithRetry`、`impl kv::Response`，源文件第 163–382 行）。

当前 Rust 接线状态需要和接口实现本身区分：仓库内非测试 Rust 源码没有找到 `NewExecutorWithRetry` 的调用者，也没有生产环境的 `CoordinatorFactory` 实现；可见实例化均位于 `executor_with_retry_test.rs` 和 `executor_with_retry_aster_unit_test.rs`。因此该文件已经提供可测试的行为契约并由 `lib.rs` 导出，但不能仅凭当前代码声称它已经接入 Rust SQL 请求主链。Go 对照文件则由自身保存会话、计划和 infoschema，并直接构造 `localMppCoordinator`（`executor_with_retry.go:71-111,256-260`）。

## 核心职责

1. 以 `(MPPQueryID, gather_id)` 唯一标识每轮 gather，并通过 `CoordinatorRegistry` 注册协调器和独立的状态上报接口，使外部 `ReportStatus` 能路由到当前轮次（`CoordinatorUniqueId`、`MppCoordinatorManager::ReportStatus/Register/Unregister`）。
2. 在构造和恢复时经 `CoordinatorFactory::Build` 创建协调器，调用 `Execute` 获取 KV 范围，并拒绝节点数不大于零的无效执行（`NewExecutorWithRetry`、`setupMPPCoordinator`）。
3. 恢复开启时，在结果尚未暴露给调用者前预拉有限条响应；错误可恢复时关闭并注销旧协调器、分配新 gather、重新执行，并丢弃旧轮次缓冲，避免重复结果（`nextWithRecovery`）。
4. 对调用者实现 `kv::Response::Next/Close`，保证缓冲 FIFO 输出、关闭幂等、内存 tracker 脱离以及注册项清理；`Drop` 兜底调用 `Close`。
5. 将“是否由协调器直接报告 execution summaries”的判断动态转发给恢复后的当前协调器（`ReportsExecutionSummariesDirectly`），避免恢复后继续使用旧协调器的路由属性。

## 主要符号

- `CoordinatorUniqueId { query_id, gather_id }`：注册键。`query_id` 标识整次 MPP 查询，`gather_id` 标识该查询的一轮派发；恢复重建会改变后者。
- `SharedMppCoordinator = Arc<Mutex<Box<dyn kv::MppCoordinator>>>`：执行器与注册表共享同一个协调器对象。`Arc` 提供共享所有权，`Mutex` 串行化 `Execute/Next/Close` 与状态访问。
- `SharedMppStatusReporter = Arc<dyn kv::MppStatusReporter>`：从协调器取得的独立状态上报句柄，注册表通过它处理状态上报，无需长期持有协调器互斥锁。
- `CoordinatorRegistry`：可注入的注册/注销边界。`Register` 可能失败，`Unregister` 无返回值。
- `MppCoordinatorManager`：本文件提供的进程内注册表实现，内部为 `Mutex<HashMap<CoordinatorUniqueId, RegisteredCoordinator>>`。`Register` 拒绝重复键；`ReportStatus` 先克隆 reporter、释放 map 锁，再调用 reporter；`Len` 在锁中毒时返回零，`Unregister` 在锁中毒时静默不处理。
- `CoordinatorFactory::Build(gather_id)`：重建边界。工厂必须用传入的新 gather ID 构造一轮新的 `kv::MppCoordinator`；当前生产 Rust 代码中未发现实现。
- `MppRecoveryConfig`：`use_auto_scaler` 控制默认内存限额恢复 handler 是否匹配，`enabled` 控制恢复总开关，`holder_capacity` 是可缓冲响应条数；默认分别为 `false/false/2`。
- `ExecutorWithRetry<'parent>`：核心状态机。它拥有当前协调器、工厂、注册表、原子 gather 分配器、执行 context、自己的 tracker、`RecoveryHandler`、公开的 `KVRanges`、查询/轮次 ID、节点数和关闭标志。`PhantomData<&'parent mut Tracker>` 将生命周期绑定到父 tracker，防止包装器比父 tracker 活得更久。
- `NewExecutorWithRetry`：公开构造器。创建恢复处理器和子 tracker，初始化状态，随后立即执行首次 `setupMPPCoordinator(false)`；首次建连失败则不返回执行器。
- `setupMPPCoordinator`、`nextWithRecovery`：分别负责单轮协调器生命周期和跨轮恢复循环，是主要内部状态转换点。
- `kv::Response::Next/Close` 与 `Drop::drop`：对外数据消费和资源释放接口。`recovery_handler_mut`、`gather_id` 仅在 `cfg(test)` 下开放。

## 执行流程

构造流程如下（`NewExecutorWithRetry`）：

1. 用配置和父 tracker 创建 `RecoveryHandler`；同时创建同 label 的执行器 tracker，并通过原始父指针 `AttachTo`。
2. 初始化空协调器、空 `KVRanges`、`gather_id = 0`、`node_count = 0` 和 `closed = false`。
3. 调用 `setupMPPCoordinator(false)`：原子递增 gather 分配器得到从 1 开始的新 ID；工厂构造协调器；先取得 `StatusReporter`，再以 `(query_id, gather_id)` 注册共享协调器与 reporter；随后调用 `Execute(context)`。
4. `Execute` 成功后读取 `GetNodeCnt`。节点数必须大于零，成功返回的 ranges 写入公开字段 `KVRanges`。

每次 `Next` 的流程如下：

1. 调用 `nextWithRecovery`。恢复未启用时立即返回，不预拉数据。
2. 恢复启用且 `RecoveryHandler::CanHoldResult` 为真时循环调用当前协调器的 `Next`：数据进入 holder，流结束则停止，错误则构造含原错误和当前节点数的 `RecoveryInfo`。
3. `Recovery` 失败时直接返回原始 MPP 错误；成功时调用 `setupMPPCoordinator(true)`。恢复 setup 会先取走、关闭并注销旧协调器，再分配新 gather、注册并执行新协调器。若重建失败，对外仍返回触发恢复的原始 MPP 错误，重建错误不替换根因。
4. 重建成功后 `ResetHolder`，丢弃旧轮次尚未暴露的结果，再继续从新协调器预拉。
5. 预拉结束后，`Next` 优先从 holder FIFO 弹出一条；holder 为空时直接调用当前协调器 `Next`。

关闭流程（`Close`）先检查 `closed` 以保证幂等，然后清空恢复缓冲、detach 执行器 tracker、取走并关闭协调器，最后无论协调器关闭成功与否都调用 `Unregister`，并返回协调器的关闭结果。若调用者遗漏显式关闭，`Drop` 会执行同一流程并忽略错误。

## 数据与状态

- `gather_allocator.fetch_add(1, Ordering::SeqCst) + 1` 为所有共享该 allocator 的执行器提供全序且不重复的 gather ID。构造和每次成功进入重建步骤都会消耗一个 ID；分配后的后续失败不会回退计数。
- `query_id` 在执行器生命周期中不变，`gather_id` 每轮改变；二者共同防止迟到的旧轮次状态被误投到新协调器。
- `coordinator: Option<_>` 表达是否存在已建立的当前轮次。恢复时先 `take` 旧值；`Execute` 或节点数校验失败会注销并清空该字段。注册失败发生在赋值之前，此时字段仍为空。
- `node_count` 在每次成功 `Execute` 后刷新，并作为 `RecoveryInfo::NodeCnt` 提供给恢复策略；默认恢复策略可将它用于 AutoScaler 拓扑恢复（`recovery_handler.rs:199-218`）。
- `KVRanges` 仅保存首次构造时 `Execute` 返回的扫描范围；恢复重建返回的 ranges 在 `nextWithRecovery` 中未写回。因此调用者看到的是初始轮次范围，这是当前实现事实。
- holder 的状态和内存记账由 `RecoveryHandler` 管理：容量满或第一次结果弹出后不可继续 hold；弹出按响应 `MemSize` 扣减；重置清队列并 detach holder tracker（`recovery_handler.rs:86-129,222-269`）。
- `closed` 只保护显式/析构重复关闭，不禁止关闭后再次调用 `Next`；关闭后 `coordinator` 为 `None`，这类调用会得到“MPP coordinator is not initialized”。

## 依赖与调用关系

上游边界：

- `lib.rs` 公开再导出 `ExecutorWithRetry`、`NewExecutorWithRetry`、注册表/工厂接口及配置，供 crate 外部接线。
- RustCodeGraph 显示目标文件由 `executor_with_retry_aster_unit_test.rs` 和 `local_mpp_coordinator.rs` 引用；精确仓库搜索进一步确认，非测试 Rust 代码当前没有构造器调用或生产工厂实现。因此 `local_mpp_coordinator.rs` 提供实际 `kv::MppCoordinator` 行为与 reporter，但本文件到它的生产构造桥尚未出现。
- 两个独立测试文件分别通过测试工厂和测试注册表调用构造器，覆盖行为契约，而不是依赖具体本地协调器。

下游边界：

- `astersql-kv` 提供 `Context`、`MPPQueryID`、`KeyRange`、`ResultSubset`、`Response`、`MppCoordinator` 与 `MppStatusReporter` 契约。
- `recovery_handler.rs` 提供 `RecoveryHandler`、`RecoveryInfo` 和 `NewRecoveryHandler`，负责恢复策略、结果 FIFO 与缓冲内存记账。
- `astersql-util-memory::tracker` 提供执行器子 tracker 的创建、挂接和脱离。
- `astersql-errors` 统一动态错误类型和新错误构造。
- 标准库 `Arc/Mutex/AtomicU64/HashMap` 分别承担共享所有权、互斥、gather 分配及注册索引。

`Cargo.toml` 将 crate 映射到 Go 包 `pkg/executor/internal/mpp`，并声明上述直接依赖；它没有 feature 条件。源文件自身唯一条件编译项是两个 `cfg(test)` 检查入口。

## 错误处理与边界

- 所有可失败公开路径使用 `errors::SharedError`。协调器未初始化、协调器或注册表 mutex 中毒、重复注册、工厂失败、状态目标不存在、`Execute/Next/Close` 失败及节点数非法都有明确错误。
- `MppCoordinatorManager::ReportStatus` 找不到键时返回 `MppCoordinator not exists`；它克隆 reporter 后才调用，避免 reporter 回调期间占用注册表锁。
- `Register` 先检查重复键，不覆盖已有项。`Unregister` 无法报告锁中毒；`Len` 也把锁中毒折叠为零，这两个行为适合诊断时特别留意。
- setup 的清理并非所有阶段完全对称：`Execute` 失败和节点数非法都会注销并清空 `coordinator`；但 `Register` 失败时新建的局部协调器随作用域释放而不会调用其 `Close`，因为它尚未存入 `self.coordinator`。这是扩展错误清理时应覆盖的边界。
- 恢复时旧协调器 `Close` 的结果被有意忽略，但锁中毒会中断恢复；注销发生在关闭之后。该顺序与 Go 注释“先关闭，再注销以避免关闭过慢期间丢失路由”一致。
- 恢复 handler 拒绝错误或超过次数时，`nextWithRecovery` 保留原 MPP 错误作为对外错误；重建失败也同样保留原错误。`executor_with_retry_test.rs::recovery_rebuild_failure_returns_the_original_mpp_error` 专门锁定后一个契约。
- 节点数必须大于零；相应检查发生在协调器注册和执行之后，失败时会注销该轮。

## 并发与资源生命周期

- 注册表 map 的所有访问受一个 `Mutex` 保护。协调器本体另有每实例 mutex；`ReportStatus` 使用独立 reporter，避免通过注册表拿协调器锁执行状态回调。
- `ExecutorWithRetry` 的可变响应消费仍依赖 `&mut self` 串行进行。共享协调器允许注册表和执行器持有同一所有权，但实际协调器方法由 mutex 串行化；不应在持有其 guard 时回调可能再次索取同一锁的代码。
- gather 分配器使用 `SeqCst`，强于仅保证唯一性所需的顺序，但给出跨线程一致的全序。恢复过程中先使旧 ID 注销，再注册新 ID；短暂窗口内没有当前轮次的路由。
- 执行器 tracker 在构造时 attach，`Close` 时 detach；holder 有独立子 tracker，由恢复处理器在 reset/drop 时 detach。`PhantomData` 只表达父 tracker 生命周期，不替代运行时 detach。
- `Close` 在设置 `closed = true` 后即使协调器关闭失败也会注销；第二次关闭直接成功，不会重复关闭或注销。测试 `held_responses_remain_fifo_and_close_unregisters_once` 验证关闭计数和注册/注销次数。
- `Drop` 是最终兜底，但吞掉关闭错误；需要观察关闭失败的调用者必须显式调用 `kv::Response::Close`。

## 与 Go 版本的对应关系

共同语义：

- 两版都处于 `kv.Response` 与 MPP coordinator 之间，先有限缓冲，只有结果尚未对外可见时才允许恢复；恢复成功后丢弃旧缓冲并重建 gather（Go `executor_with_retry.go:35-66,114-128,183-248`；Rust `ExecutorWithRetry::nextWithRecovery` 与 `Next`）。
- 两版都在每轮 setup 前分配新 gather ID，以查询 ID 加 gather ID 注册；执行后要求 TiFlash 节点数大于零；恢复失败和重建失败都向上返回原始 MPP 错误。
- 两版的 `ReportsExecutionSummariesDirectly` 都跟随当前协调器，恢复后可反映新协调器属性。

Rust 的抽象差异：

- Go 构造器读取全局配置与 session fallback，固定 holder 容量为 2，并保存 session、计划、infoschema、startTS 等以直接调用 `NewLocalMPPCoordinator`。Rust 把恢复开关/容量作为 `MppRecoveryConfig` 注入，把构造细节抽成 `CoordinatorFactory`，把注册表抽成 `CoordinatorRegistry`；是否满足 Go 的配置判定需由上层生产接线负责，而当前仓库中未找到该接线。
- Go 使用全局 `mppcoordmanager.InstanceMPPCoordinatorManager`；Rust 在本文件内另有可注入 `MppCoordinatorManager`，条目同时保存共享协调器和 reporter。Rust 当前 manager 与 `pkg/executor/mppcoordmanager` 中的另一套 manager 是不同类型，不能混同。
- Go 恢复关闭调用具体 `localMppCoordinator.closeWithoutReport`；Rust 只能通过 trait 调用通用 `Close` 并忽略其返回值。这是否在所有生产 coordinator 上完全等价，目前因缺少生产工厂接线而未验证。
- Go 的普通 `Close` 没有显式幂等标志；Rust 增加 `closed` 和 `Drop` 兜底。Rust 在 `Execute` 或节点数检查失败时主动注销并清空协调器，比对照 Go 当前函数中的直接返回更积极清理注册状态。
- Go 文件包含恢复相关 failpoint 和日志；Rust 文件没有对应 failpoint/日志，测试通过可注入工厂、注册表和 handler 建模故障。

## 扩展指南

- 接入生产请求链时，最可能新增的是实现 `CoordinatorFactory` 的适配器，并在已有 MPP gather 构造处调用 `NewExecutorWithRetry`。适配器必须把 `gather_id` 注入 `NewLocalMppCoordinator` 所需计划/会话参数，同时复现 Go 对 `DisaggregatedTiFlash && UseAutoScaler && !AllowFallbackToTiKV` 的启用判定。不要在本文件硬编码上层 session/plan，除非同步重新评估可测试边界。
- 增加恢复类型时，应优先扩展 `recovery_handler.rs` 的 handler 选择与执行，并保持“任何结果暴露后不再恢复”的不变量；同步在独立的 `recovery_handler_aster_unit_test.rs` 和本文件对应测试中覆盖匹配、次数、旧缓冲丢弃及原错误保留。
- 修改 setup 生命周期时，必须覆盖工厂失败、重复注册、`Execute` 失败、零节点、锁中毒和关闭失败，并明确每一阶段是否需要 `Close/Unregister`。尤其应为当前未覆盖的注册失败资源清理补独立测试，而不要把测试内嵌到生产 `.rs`。
- 修改注册键或状态路由时，应同步 `CoordinatorUniqueId`、`MppCoordinatorManager`、gRPC 路由接线及重复/迟到 report 测试，确保旧 gather 不能污染新 gather。
- 修改缓冲容量或输出次序时，应保留 FIFO、容量有界和结果首次暴露后禁止恢复三个契约；性能风险主要是持锁调用协调器、过大 holder 增加内存与首行延迟，以及 `SeqCst` 分配在高并发下的争用。
- 测试应继续放在独立文件：一般回归放 `executor_with_retry_test.rs`，更完整的注册、FIFO、幂等关闭和重建状态机覆盖放 `executor_with_retry_aster_unit_test.rs`。若完成生产接线，还应增加调用侧的集成测试，证明配置映射、真实 `LocalMppCoordinator` 工厂和状态上报路由连通。

## 验证依据

- 目标源码：`pkg/executor/internal/mpp/executor_with_retry.rs`，RustCodeGraph `node --file ... --offset 1 --limit 500` 返回完整 388 行及“由测试和 local coordinator 使用”的索引信息。
- 符号/调用查询：RustCodeGraph `query NewExecutorWithRetry`、`query setupMPPCoordinator`、`query MppCoordinatorManager`、`callers/callees NewExecutorWithRetry`、`callers/callees setupMPPCoordinator`，以及聚焦 `local_mpp_coordinator.rs NewExecutorWithRetry CoordinatorFactory CoordinatorRegistry ExecutorWithRetry` 的 `explore`。可信边包括构造器到 `NewRecoveryHandler`/首次 setup、setup 到 `Build/Register/Unregister/Execute/GetNodeCnt`，以及测试到构造器/`Next`/`Close`；通用名称产生的跨模块候选未作为结论。
- crate 与模块边界：`pkg/executor/internal/mpp/Cargo.toml`、`pkg/executor/internal/mpp/lib.rs`。目录不存在 `doc.go`，故以 crate 根注释和 Cargo 元数据作为最近模块契约。
- 直接依赖实现：`pkg/executor/internal/mpp/recovery_handler.rs`；RustCodeGraph 完整读取其恢复次数、handler、FIFO、内存记账和 tracker 生命周期。
- Go 对照：`pkg/executor/internal/mpp/executor_with_retry.go`，RustCodeGraph 完整读取 267 行；对照了构造配置、setup、恢复、关闭、gather 分配和本地协调器构造。
- 独立 Rust 测试：`pkg/executor/internal/mpp/executor_with_retry_test.rs` 验证重建失败保留原 MPP 错误；`pkg/executor/internal/mpp/executor_with_retry_aster_unit_test.rs` 验证重复注册、FIFO、关闭幂等、恢复重建、旧缓冲丢弃、gather 递增和当前协调器 summary 路由。当前目录没有同名 Go 测试文件，Go 行为依据来自生产对照文件及 Rust 回归契约。
- 生产接线检查：`rg -n "NewExecutorWithRetry|ExecutorWithRetry|MppRecoveryConfig|CoordinatorFactory" --glob '*.rs'` 只命中目标、`lib.rs` 与两个测试文件，因此将 Rust 生产调用者和生产工厂明确标为当前未见，而非推测已接线。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证为固定 11 章节结构检查、引用路径检查、范围检查和 diff 人工复核。
