# `pkg/kv/mpp.rs`

## 文件定位

本文件是 `astersql-kv` crate 的 MPP（Massively Parallel Processing）边界契约层。`pkg/kv/lib.rs` 以 `#[path = "mpp.rs"] mod mpp_impl` 装入它，再通过 `pub use mpp_impl::*` 将其公开，因此上层通常以 `astersql_kv::MPPTask`、`astersql_kv::MPPClient` 等名字使用这些 API，而不会直接引用 `mpp_impl`。

它位于 SQL 执行协调层与 TiFlash/计算存储客户端之间：上层协调器用这里的数据结构描述查询、任务和派发请求，下层存储客户端实现这里的 `MPPClient` trait。文件本身主要定义协议版本、跨层数据模型和 trait，不实现网络 RPC、任务切分或结果消费；当前 Rust 的具体客户端逻辑可见 `pkg/store/copr/mpp.rs`，协调执行逻辑可见 `pkg/executor/internal/mpp/local_mpp_coordinator.rs`。

crate 边界由 `pkg/kv/Cargo.toml` 确认：包名为 `astersql-kv`，库入口是 `lib.rs`；本文件直接复用 crate 根的 `Context`、`Error`、`KeyRange`、`Backoffer`、`Response` 等抽象，并从带固定 tag 的 `kvproto` 依赖公开重导出 MPP protobuf 类型。

## 核心职责

1. 以 `MppVersion`、`ToMppVersion` 和 `GetNewestMppVersion` 表达 TiDB/TiFlash MPP 协议能力范围。当前合法值为 `-1`（未指定）以及 `0..=3`，其中最新版本为 V3。
2. 以 `MPPQueryID`、`MPPTask`、`MPPDispatchRequest`、`MPPBuildTasksRequest` 描述查询身份、执行单元、派发载荷和按 key range 定位任务的输入。
3. 以 `MPPTask::ToPB` 把内部任务身份转换成 `kvproto::mpp::TaskMeta`，并保留根任务 `ID == -1` 不带 store 地址的约定。
4. 以 `MPPClient` 定义任务构造、派发、建流、取消、可见性检查和 store 计数的存储侧能力。
5. 以 `MppCoordinator` 和 `MppStatusReporter` 定义协调器作为响应流、状态上报接收者及资源生命周期所有者的接口。
6. 以 `MPPBuildTasksRequest::ToString` 生成与 Go 拼接规则一致的范围缓存键材料。

## 主要符号

- `MppVersion(pub i64)`：可排序、可复制的协议版本包装；`Default` 为 `MppVersionV0`，`ToInt64` 提供 protobuf/配置边界所需的整数。
- `MppVersionV0` 至 `MppVersionV3`、`MppVersionUnspecified`：公开版本常量；`mppVersionMax` 与 `newestMppVersion` 是内部上限哨兵和当前最新合法版本。
- `ToMppVersion(name: &str) -> (MppVersion, bool)`：大小写不敏感地接受 `UNSPECIFIED`，或解析十进制整数；越界和非数字均返回 `(MppVersionUnspecified, false)`。
- `MPPTaskMeta: Send + Sync`：节点元数据最小接口，公开 `GetAddress`；Rust 额外要求 `CloneBox`，从而使 `Box<dyn MPPTaskMeta>` 可克隆。
- `MPPQueryID`：由 `QueryTs`、进程内 `LocalQueryID`、`ServerID` 组成的查询身份，支持哈希和相等比较。
- `MPPTask`：单个执行任务的描述，包含节点、任务/查询/会话身份、表与分区信息、协议版本和静态剪枝标志；实现了深层 trait-object 克隆和 `ToPB`。
- `MppTaskStates`：`Ready -> Running -> Cancelled/Done` 的四值状态枚举，底层表示为 `u8`。
- `MPPDispatchRequest`：计划字节、目标节点、超时、schema/query/gather/task 身份、协调器地址、执行摘要开关、资源组、连接和 digest 信息的聚合载荷；`Default` 从空载荷、V0、Ready 状态开始。
- `CancelMPPTasksParam`、`EstablishMPPConnsParam<'a>`、`DispatchMPPTaskParam<'a>`：分别包装取消、建流和派发所需的参数。后两者借用上下文、请求和退避器，避免复制请求及重试状态。
- `MPPClient: Send + Sync`：存储侧接口；派发和建流都返回 `(响应, retry)`，错误另由 `Result` 表达。
- `ReportStatusRequest`、`MppStatusReporter`：包装 protobuf 状态报告，并提供可跨线程共享的只读上报入口。
- `MppCoordinator: Response + Send`：协调器同时是 `Response` 流；`Execute` 启动派发并返回 key ranges，`ReportStatus` 接受节点状态，`StatusReporter` 暴露共享上报器，`IsClosed` 和 `GetNodeCnt` 暴露生命周期/拓扑状态。`ReportsExecutionSummariesDirectly` 默认返回 `false`。
- `MPPBuildTasksRequest`：非分区范围 `KeyRanges: Option<Vec<KeyRange>>` 与分区范围 `PartitionIDAndRanges` 二选一路径，并携带 `StartTS`；`ToString` 生成确定性拼接字符串。

## 执行流程

典型链路如下：

1. 规划或协调层创建 `MPPBuildTasksRequest`，用 `KeyRanges = Some(...)` 表示非分区表，或用 `KeyRanges = None` 配合 `PartitionIDAndRanges` 表示分区表。具体 `MPPClient::ConstructMPPTasks` 实现据此把范围映射到计算节点；`pkg/store/copr/mpp.rs` 在非分区路径缺少 `KeyRanges` 时返回错误。
2. 每个节点位置实现 `MPPTaskMeta`。上层把任务身份装入 `MPPTask`，调用 `ToPB` 形成协议 `TaskMeta`；普通任务从 `Meta.GetAddress()` 写入地址，根任务 `ID == -1` 保持空地址。
3. 协调层编码计划并构造 `MPPDispatchRequest`。`pkg/executor/internal/mpp/local_mpp_coordinator.rs::appendMPPDispatchReq` 登记请求和报告槽位；`dispatchAll` 只把 `Ready` 请求改为 `Running` 并启动工作线程。
4. 具体传输层经 `MPPClient::DispatchMPPTask` 派发任务，再经 `EstablishMPPConns` 建立流。接口将“调用错误”和“是否建议重试”分开；退避状态由参数中的 `&mut Backoffer` 持有。
5. 协调器作为 `Response` 被上层持续调用 `Next` 取结果。若启用直接状态报告，外部通过 `MppStatusReporter`/`MppCoordinator::ReportStatus` 汇集 execution summaries；否则 `ReportsExecutionSummariesDirectly` 的默认值指示调用方沿流式响应消费摘要。
6. 完成或关闭时，协调实现更新任务状态、停止工作线程并调用 `CancelMPPTasks` 清理仍在运行的远端任务。`IsClosed` 与 `GetNodeCnt` 供管理和统计路径查询。

`MPPBuildTasksRequest::ToString` 是独立的缓存键流程：若 `KeyRanges` 为 `Some`，按输入顺序拼接 `range_id{序号}`、起始键字符串、结束键字符串并立即返回；若为 `None`，则按分区输入顺序先拼 `partition_id{ID}`，再拼该分区每个 range。`StartTS` 不参与此字符串。

## 数据与状态

- 查询身份分层：`MPPQueryID` 标识整次 MPP 查询，`GatherID` 标识 gather 阶段，`MPPTask.ID`/`MPPDispatchRequest.ID` 标识单个任务；`StartTs` 是读取快照时间戳。调用方必须保持这些字段跨内部结构和 protobuf 一致。
- 地址不变量：非根 `MPPTask` 的 `Meta` 必须为 `Some`，否则 `ToPB` 会因 `expect` 触发 panic；根任务以 `ID == -1` 为判据，可没有 `Meta`，其 protobuf 地址为空。
- 状态不变量：本文件仅定义 `MppTaskStates`，不自行执行状态机。当前协调器实现只派发 `Ready` 请求，将其置为 `Running`；取消路径最终把请求置为 `Cancelled`。`Done` 的具体写入责任属于实现方。
- `MPPDispatchRequest::clone` 会复制计划字节、字符串和 trait object；这便于工作线程取得独立请求快照，但大计划的克隆有内存和复制成本。
- `MPPBuildTasksRequest` 用 `Option` 保留 Go 中 `nil` 与“非 nil 空切片”的区别：`Some(vec![])` 仍走非分区路径并生成空字符串，`None` 才走分区路径。两条路径均可能生成空字符串，所以缓存使用者不能只凭结果区分空输入形态。
- 缓存键是无分隔长度编码的直接拼接，严格依赖输入顺序和 `Key::String()` 的十六进制表现；为保持 Go 兼容，不能随意排序、增加分隔符或把 `StartTS` 纳入其中。

## 依赖与调用关系

上游直接证据包括：

- `pkg/kv/lib.rs` 装入并公开重导出本文件全部符号。
- `pkg/executor/internal/mpp/local_mpp_coordinator.rs` 实现 `kv::MppCoordinator` 的协调语义，创建和克隆 `MPPDispatchRequest`，驱动 `MppTaskStates`，并使用 `ReportStatusRequest`。
- `pkg/executor/internal/mpp/executor_with_retry.rs` 持有/包装 `kv::MppCoordinator`，在恢复后继续暴露 `ReportsExecutionSummariesDirectly`。
- `pkg/store/mockstore/mockstorage/canonical_storage.rs` 与 `pkg/store/driver/kv_adapter.rs` 提供受限/不支持场景的 `kv::MPPClient` 实现，说明该 trait 也是 Storage 抽象的稳定边界。

下游依赖包括：

- crate 内的 `Context`、`Backoffer`、`KeyRange`、`PartitionIDAndRanges`、`MPPStreamResponse`、`Response`、`Error` 和 `ResultSubset`。
- `kvproto::mpp::{TaskMeta, DispatchTaskResponse, ReportTaskStatusRequest}`；本文件将三者公开重导出。
- `tiflashcompute::DispatchPolicy` 和 `tiflash::ReplicaRead`，用于构造任务时选择调度及副本读取策略。
- 标准库 `HashMap`、`Arc`、`Duration`；分别用于取消地址集合、共享状态上报器和任务构造超时。

RustCodeGraph 对 `pkg/kv/mpp.rs` 给出的文件级反向使用文件为 `pkg/executor/internal/mpp/executor_with_retry.rs`、其独立测试、`pkg/executor/internal/mpp/local_mpp_coordinator.rs` 和 `pkg/store/mockstore/mockstorage/canonical_storage.rs`。trait 同名方法的精确 callers/callees 查询未返回边，因此具体实现和更多引用由 `rg` 补充核验；不能把文件级列表解释为全部动态分派调用点。

## 错误处理与边界

- `ToMppVersion` 不抛错：解析失败或超出 `[-1, 3]` 时以 `bool = false` 报告，并用 `MppVersionUnspecified` 作为占位值；字符串 `UNSPECIFIED` 和数字 `-1` 都是合法输入。
- `MPPTask::ToPB` 的普通任务缺少 `Meta` 是编程错误，当前行为是 panic，而不是返回 `Result`。构造或反序列化任务的新路径必须在调用前维护这一不变量。
- `MPPClient` 的构造、派发、建流、可见性和 store 计数均可返回 `Error`；`CancelMPPTasks` 没有返回值，因此取消实现只能做尽力而为的清理或通过自身日志/缓存失效处理失败。
- 派发和建流的 `bool` 只表示实现建议的重试决策，不能替代 `Result`。调用者必须同时处理响应、重试标志和错误三种信息。
- `MppCoordinator::ReportsExecutionSummariesDirectly` 是兼容性默认值；新协调器若实际通过状态上报接收摘要而未覆盖为 `true`，上层可能错误地继续期待流式摘要。
- `MPPBuildTasksRequest::ToString` 不做范围合法性检查，也不防碰撞；它忠实保留 Go 的缓存键材料规则，而不是安全的通用序列化格式。

## 并发与资源生命周期

- `MPPTaskMeta`、`MPPClient` 和 `MppStatusReporter` 都要求 `Send + Sync`，可在并发派发、取消和上报路径共享；`MppCoordinator` 要求 `Send`，其会改变状态的方法使用 `&mut self`，把互斥策略留给持有者。
- `StatusReporter` 返回 `Arc<dyn MppStatusReporter>`，让协调器注册到管理器后，状态 RPC 仍可持有独立共享句柄，而不要求直接获得协调器的可变引用。
- `EstablishMPPConnsParam` 和 `DispatchMPPTaskParam` 对 `Backoffer` 使用独占可变借用，确保一次调用中的重试预算不会被无同步地并发修改；`Context`、请求和 task meta 则只读借用。
- 本文件不创建线程、channel、锁或网络连接。实际生命周期证据位于 `local_mpp_coordinator.rs`：每条 Ready 请求可启动工作线程，channel 回传响应/错误/完成事件，`stop_requested` 协调停止，关闭路径 join 工作线程并取消运行中任务。
- `MPPStreamResponse` 的资源释放由具体流和协调器负责；实现新的 `MppCoordinator` 时必须保证 `Response::Close`/终止路径不遗留远端任务或后台 worker，并使 `IsClosed` 与真实状态一致。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/mpp.go`。Rust 保留了 Go 的公开命名、版本范围、字段集合、任务状态顺序、缓存键拼接顺序及 `ID == -1` 根任务规则。

主要语言适配如下：

- Go 的 `MppVersion` 命名整数映射为 Rust tuple struct 和常量；解析仍返回“值 + 是否成功”。
- Go interface 可直接放入切片/结构；Rust 的 `MPPTaskMeta` 增加 `CloneBox`，以支持含 trait object 的 `MPPTask`、`MPPDispatchRequest` 深克隆。
- Go 的 nil `KeyRanges` 被 Rust 的 `Option<Vec<KeyRange>>` 显式表达，维持非分区/分区分支判定。
- Go `MPPClient` 返回 `(response, retry, error)`；Rust 等价地用 `Result<(response, bool), Error>` 表达。
- Go `MppCoordinator.Execute` 返回协调器自身作为 `Response` 与 key ranges；Rust 让 trait 继承 `Response`，`Execute(&mut self)` 只返回 ranges，调用方继续持有原对象。
- Rust 新增 `MppStatusReporter`、`StatusReporter` 和 `ReportsExecutionSummariesDirectly`，将可共享的状态报告句柄与摘要来源显式化；这些接口在当前 Go `pkg/kv/mpp.go` 中没有同形定义，是 Rust 协调/恢复接线所需的适配。
- Rust 的 `ReportStatusRequest.Request` 是 protobuf 值，而 Go 字段是指针；Rust 不存在 nil request 分支，调用方需提供完整默认值或实际报告。

Go 测试 `pkg/kv/version_test.go` 验证 V0-V3 与 unspecified 的解析。Rust 的 `pkg/kv/version_test.rs` 覆盖合法版本；`pkg/kv/mpp_2_aster_unit_test.rs` 进一步覆盖非法 `4`、非数字、缓存键、普通/根任务 PB 转换和 trait-object 克隆。

## 扩展指南

- 增加协议版本时，应同时更新版本常量、`mppVersionMax`/`newestMppVersion`、Go `pkg/kv/mpp.go` 及 Rust/Go 版本测试；还要核对 protobuf、session 变量校验和 TiFlash 能力协商，不能只放宽解析范围。
- 给任务或派发请求增加字段时，应同步 `Default`、手写 `Clone`、`MPPTask::ToPB`（若进入 task meta）、下层 `pkg/store/copr/mpp.rs` 的 wire 转换、上层请求构造以及 Go 对照字段。遗漏手写 Clone 会导致工作线程看到默认/旧值。
- 改动根任务约定或地址来源时，首先修改/复核 `MPPTask::ToPB`，并扩展 `pkg/kv/mpp_2_aster_unit_test.rs` 的普通、根和缺失 Meta 边界；不要把 Rust 测试嵌回生产源文件。
- 扩展 `MPPClient` 方法会影响所有实现，包括真实 copr client、driver 的 unsupported adapter、canonical mock storage 和各测试 double；应通过引用搜索逐一补齐，评估对象安全性与 `Send + Sync` 约束。
- 扩展 `MppCoordinator` 时，应同步 `local_mpp_coordinator.rs`、retry wrapper、manager/状态报告接线及其独立测试；新增默认方法前要明确旧实现的安全默认语义。
- 改动缓存键格式会影响任务分配缓存兼容性；必须同时更新 Go/Rust 对照测试并评估历史缓存命中、顺序敏感性和潜在碰撞。性能上应避免无必要地复制大范围列表、计划 `Data` 或 digest 字符串。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点；`node --file pkg/kv/mpp.rs --offset 1 --limit 500` 返回目标文件完整 428 行及 4 个文件级使用者。`query ToMppVersion` 同时定位 Rust `pkg/kv/mpp.rs:70` 和 Go `pkg/kv/mpp.go:63`，`query ConstructMPPTasks` 定位 trait 与 Go 实现；精确 trait 方法 callers/callees 查询没有输出，相关边由文件级结果与文本引用交叉验证。
- 源码与装配：`pkg/kv/mpp.rs`、`pkg/kv/lib.rs`。
- crate/依赖：`pkg/kv/Cargo.toml`。
- Go 对照：`pkg/kv/mpp.go`、`pkg/kv/version_test.go`。
- Rust 独立测试：`pkg/kv/mpp_2_aster_unit_test.rs`、`pkg/kv/version_test.rs`、`pkg/executor/internal/mpp/local_mpp_coordinator_test.rs`、`pkg/executor/internal/mpp/local_mpp_coordinator_aster_unit_test.rs`、`pkg/executor/internal/mpp/executor_with_retry_test.rs`、`pkg/executor/internal/mpp/executor_with_retry_aster_unit_test.rs`。
- 真实实现与调用链：`pkg/store/copr/mpp.rs`、`pkg/executor/internal/mpp/local_mpp_coordinator.rs`、`pkg/executor/internal/mpp/executor_with_retry.rs`、`pkg/store/mockstore/mockstorage/canonical_storage.rs`、`pkg/store/driver/kv_adapter.rs`。
- 人工复核结论：本文件存在的原因是稳定隔离 MPP 规划/协调与存储传输；运行时由上层构造并驱动这些契约、由下层实现 RPC；安全扩展的关键是同步手写转换/克隆、所有 trait 实现、Go 契约和独立测试。
