# `pkg/planner/core/operator/physicalop/fragment.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate；`Cargo.toml` 将 crate 根指定为同目录的 `lib.rs`，后者以 `mod fragment` 纳入本模块，并通过 `pub use fragment::*` 向上层再导出公开项。源码见 [`fragment.rs`](fragment.rs)，crate 装配见 [`lib.rs`](lib.rs) 与 [`Cargo.toml`](Cargo.toml)。

它位于 MPP 物理计划和执行协调器之间：输入是一棵以 `PhysicalExchangeSender` 为根的已选物理计划，输出是可派发的 `Fragment` 列表、供 UnionScan 使用的 KV ranges，以及参与执行的 TiFlash 节点地址。当前生产入口是 `pkg/executor/internal/mpp/local_mpp_coordinator.rs::NewLocalMppCoordinator`，该入口调用 `RootMppTaskGenerator::GenerateRootMPPTasks`，随后将各片段转换为 protobuf 派发请求。

本文件既提供两个 ID 分配 API，也提供会话级拓扑生成器。它不负责物理优化、DAG protobuf 编码、请求发送或执行状态机；这些职责分别留在 planner 其他模块、`MPPSink::to_pb` 和 local MPP coordinator。

## 核心职责

1. `AllocMPPTaskID` 从语句上下文的 `MPPQueryInfo.AllocatedMPPTaskID` 原子分配语句级任务 ID；`AllocMPPQueryID` 从进程级静态计数器分配本地 query ID。
2. `Fragment` 用 `Arc<Mutex<Box<dyn base::MPPSink>>>` 持有片段出口，并记录根片段和 singleton 属性；`MemoryUsage` 将自身浅层大小与 Sink 的动态内存统计相加。
3. `SessionRootMppTaskGenerator` 沿 Exchange 边界递归切分计划，为含表扫描的片段调用 `MppScanRangeEncoder` 和 `kv::MPPClient` 构造任务，为无表扫描的片段按子任务所在地址派生任务。
4. 生成器把子片段任务写入父片段的 `PhysicalExchangeReceiver`，把父片段任务追加为子 Sink 的 target tasks，并为根 Sink 安装 ID 为 `-1` 的协调器目标任务。
5. 共享生产者按 `PhysicalExchangeSender::id()` 缓存；最终再按 `Fragment::Sink` 的 `Arc` 地址去重，避免共享 CTE 生产者重复调度和重复返回。

这里的“Fragment”是已调度结果，不等同于 Go 版本在调度前保存 `TableScan`、`ExchangeReceivers`、`CTEReaders` 的中间结构；Rust 将发现、切分和调度合并在 `build_sender` 的递归过程内。

## 主要符号

- `allocate_mpp_task_id(&AtomicI64) -> i64`：内部原子加一原语，使用 `SeqCst`，返回值从 1 开始。
- `AllocMPPTaskID(&stmtctx::StatementContext) -> i64`：公开的语句级分配器。计数器生命周期由 statement context 管理；同文件生成器并不调用它，而使用自己的 `next_task_id`。
- `MPP_QUERY_ID` / `AllocMPPQueryID() -> u64`：进程级原子计数器初值为 1，因此进程内第一次调用返回 2，与 Go 的 `atomic.AddUint64` 可观察行为一致。
- `Fragment`：公开字段 `Sink`、`IsRoot` 与私有字段 `singleton`。`Clone` 只克隆 `Arc`，多个克隆共享同一可变 Sink。
- `Fragment::New`：包装 Sink 并设置根标记；singleton 初值为 `false`。
- `Fragment::init`：遍历计划；遇 `PhysicalExchangeReceiver` 时检查其下游 Sender 是否为 `PassThrough`，并以 OR 语义更新 singleton，然后停止跨该 Receiver 下钻。当前检索到的调用仅在同目录 Rust 测试中；生产切片路径在 `build_sender` 内直接执行等价的 PassThrough 判断。
- `Fragment::MemoryUsage`：锁住 Sink 并调用 `MPPSink::memory_usage`；锁中毒时以 `expect("MPP sink lock")` panic。
- `GeneratedRootMppTasks`：聚合 `fragments`、`kv_ranges`、`node_addresses` 的公开返回类型。
- `RootMppTaskGenerator`：`Send + Sync` 抽象；其方法接受原始计划、`start_ts`、`gather_id` 和 `MPPQueryID`。
- `MppScanRangeEncoder` / `EncodedMppScan`：将 `PhysicalTableScan` 隔离为存储请求、KV ranges、表 ID 和静态分区裁剪标记；这是 InfoSchema/range 编码接入边界。
- `SessionRootMppTaskGenerator`：持有 `MPPClient`、KV context、编码器、超时、派发/副本读取策略、MPP 版本、会话标识及实例内任务 ID 计数器。
- `construct_scan_tasks`：编码单个扫描、调用 `ConstructMPPTasks`、校验 meta 非空，再补齐每个 `kv::MPPTask` 的身份和会话字段。
- `derive_tasks_from_children`：无本地扫描时，以 `MPPTaskMeta::GetAddress` 去重子任务，按地址排序后构造 `TableID == -1` 的本层任务。
- `build_sender`：核心递归；负责缓存复用、边界发现、任务构造、任务连边、range 累积与片段收集。
- `BuiltFragments`：仅模块内使用的递归中间值，携带完整片段、当前根任务和累计 KV ranges。
- `collect_receivers` / `collect_table_scans`：两组 DFS 辅助函数。前者收集 Receiver 并在该边界停止，后者跳过 Receiver 子树，只收集当前片段中的 TableScan。

## 执行流程

`NewLocalMppCoordinator` 调用 `GenerateRootMPPTasks` 后，主流程如下：

1. `GenerateRootMPPTasks` 将 `original_plan` 向下转型为 `PhysicalExchangeSender`；不是 Sender 则立即返回错误。
2. 创建按 Sender plan ID 索引的缓存，并以 `is_root = true` 调用 `build_sender`。
3. `build_sender` 若命中缓存，克隆已构造结果并清空本次返回的 `kv_ranges`。这样共享生产者仍复用同一 Sink 和任务，但同一扫描范围不会重复上报。
4. `collect_receivers` 找出当前片段内的 ExchangeReceiver；对每个 Receiver 的下游 Sender 递归调用 `build_sender(..., false, ...)`，先完成子片段。
5. `collect_table_scans` 在不跨 Receiver 的范围内找 TableScan。超过一个扫描直接报错；恰有一个时走 `construct_scan_tasks`；没有扫描时走 `derive_tasks_from_children`。
6. 若任一 Receiver 的下游 Sender 为 `PassThrough`，将当前任务列表截断为一个。随后把每组子任务写回对应 Receiver。
7. 克隆当前 Sender 为 owned Sink，并写入 self tasks。根 Sender 额外获得一个 ID 为 `-1` 的 target task，表示结果返回协调器。
8. 为每个子组把当前层任务追加到其第一个（根）子片段 Sink 的 target tasks；再将子 ranges 和子片段依次并入当前结果。
9. 将构造结果放入缓存并返回。顶层随后要求累计 `kv_ranges` 非空，从所有 Sink self tasks 汇总节点地址，并按 Sink 的 `Arc` 指针去除共享片段重复项。
10. coordinator 再次校验 ranges 和节点，逐 Fragment 调用 `appendMPPDispatchReq`；后者锁住 Sink、调用 `to_pb`，结合 `IsRoot`、self tasks、store 信息和会话信息构造派发请求。

顺序上的关键不变量是“自底向上”：子片段必须先有 self tasks，父层才能按其地址派生任务，并把父任务反向写为子 Sink 的目标。

## 数据与状态

- 语句级 ID 状态位于 `StatementContext.MPPQueryInfo.AllocatedMPPTaskID`；进程级 query ID 位于静态 `MPP_QUERY_ID`；一个 `SessionRootMppTaskGenerator` 实例另有 `next_task_id`，用于本次生成的任务 ID。三者不是同一个计数器。
- `Fragment::Sink` 是共享所有权的 trait object。克隆 `Fragment` 不复制物理 Sender；因此向缓存克隆或其他克隆追加 target tasks，会修改同一底层 Sink。
- `singleton` 是片段元数据，但当前生产 `build_sender` 不读取该字段，而是直接检查收集到的 Receiver；该字段和 `init` 目前主要保留 Go 语义及测试覆盖。
- `BuiltFragments.fragments[0]` 始终代表该递归结果的根片段；父层依赖这一约定向子根 Sink 追加 target tasks。
- `kv_ranges` 只从扫描编码结果产生并沿递归累加。缓存复用时清空克隆结果中的 ranges，是避免共享生产者重复计入的必要不变量。
- `node_addresses` 是从去重前的所有片段 self tasks 中提取的地址集合，因此天然去重；缺少 `Meta` 的任务会被跳过。coordinator 后续会再次遍历并把缺少 meta 当作错误。
- 从子任务派生时先按地址去重、再按地址字符串排序，确保输出任务顺序稳定；扫描任务顺序则继承 `MPPClient::ConstructMPPTasks` 返回顺序。

## 依赖与调用关系

上游直接调用关系为：

`pkg/executor/internal/mpp/local_mpp_coordinator.rs::NewLocalMppCoordinator` → `SessionRootMppTaskGenerator::GenerateRootMPPTasks` → `build_sender`。

主要下游关系为：

- `build_sender` → `PhysicalExchangeReceiver::GetExchangeSender` / `SetTasks`，用于跨 Exchange 边界获取生产者并回填发送任务。
- `build_sender` → `PhysicalExchangeSender::Clone` / `SetSelfTasks` / `SetTargetTasks`，以及 `base::MPPSink::append_target_tasks`，用于构造有所有权的片段出口和数据流连边。
- `construct_scan_tasks` → `MppScanRangeEncoder::Encode` → `kv::MPPClient::ConstructMPPTasks`，用于把逻辑扫描范围落实为 TiFlash 地址和任务元数据。
- `derive_tasks_from_children` → `kv::MPPTaskMeta::GetAddress`，用于按执行节点合并父层任务。
- `Fragment::MemoryUsage` → `base::MPPSink::memory_usage`；协调器消费路径还会调用 Sink 的 `get_self_tasks`、`schema` 和 `to_pb`。

Cargo 直接依赖中，本文件实际使用 `base`、`expression`、`kv`、`stmtctx`、`tipb` 以及同 crate 的 Exchange Sender/Receiver/TableScan。`MppScanRangeEncoder` 将 InfoSchema、range codec 与表元数据等更重的职责留给上层实现，使此 crate 无需直接绑定具体 session 实现。

RustCodeGraph 能识别本文件和 `build_sender` 等定义，但本次 `callers GenerateRootMPPTasks` 对 trait 动态调用未返回边；因此生产入口以 `local_mpp_coordinator.rs` 的直接源码调用作为补充证据，而不是据此声称“无调用者”。

## 错误处理与边界

可恢复错误统一使用 `expression::Error`：

- 根计划不是 `PhysicalExchangeSender`：`MPP root plan must be an exchange sender`。
- `GetExchangeSender`、计划克隆或扫描编码失败：使用 `?` 原样向上传播。
- `ConstructMPPTasks` 返回 KV 错误：转换为 `expression::errors::New(error.to_string())`；当前 warning 回调为空操作，因此本层不保存 warning。
- 扫描未得到任何任务：拒绝生成不可派发片段。
- 当前片段出现多个 TableScan：拒绝违反“一片段至多一个本地扫描”的拓扑。
- 无本地扫描且子片段也没有带 meta 的任务：拒绝生成无节点来源的片段。
- 最终没有 KV ranges：拒绝根任务结果。这意味着纯派生但完全没有扫描范围的拓扑不能由此入口成功返回。

锁中毒不进入上述错误通道：`MemoryUsage`、子 target 追加以及节点收集使用 `expect("MPP sink lock")`，会 panic。协调器消费 Fragment 时则把相同锁中毒转换为共享错误。`collect_*` 是递归 DFS，没有显式循环检测或深度限制，依赖物理计划是有限无环树的上游不变量。

与 Go 主实现相比，Rust 本文件没有实现相关列重新求值、具体分区裁剪、table-reader request cache、failpoint、CTE group/source/sink 改写和本地 CTE sink/source 计数；这些不能由当前 `MppScanRangeEncoder`/缓存逻辑推断为已支持。编码器实现若要补齐扫描语义，必须明确承担相应职责并接受独立验证。

## 并发与资源生命周期

- 三处任务/query ID 自增使用 `SeqCst` 原子操作，可被多线程安全调用；`RootMppTaskGenerator` 与 `MppScanRangeEncoder` 要求 `Send + Sync`，`MPPClient` 也通过 `Arc` 共享。
- `SessionRootMppTaskGenerator::next_task_id` 属于生成器实例，`GenerateRootMPPTasks` 只接收 `&self`，因此多次或并发使用同一实例会继续共享并递增任务 ID，不会自动按一次调用复位。调用方需要让生成器生命周期与预期 ID 域一致。
- Sink 用 `Arc<Mutex<...>>` 管理共享可变状态。锁持有范围通常只覆盖单次读写，但 `MemoryUsage` 和 `to_pb` 也会在锁内调用动态方法；实现这些方法时不可反向获取同一非重入 Mutex。
- `build_sender` 的 `cache`、递归向量和 ranges 都是单次调用的栈上可变状态，不跨调用共享。共享 CTE 的持久共享来自缓存内 `Fragment` 克隆所保留的同一 `Arc`。
- `MPPTaskMeta` 以 boxed trait object 存在；派生任务会 clone meta，使父子任务可独立拥有同一节点描述。会话别名等字符串按任务克隆，代价随任务数线性增长。
- 本模块不启动线程、异步任务、通道或事务，也不拥有网络连接关闭逻辑；它同步调用 `MPPClient::ConstructMPPTasks`，实际派发和取消生命周期由 coordinator/MPP client 管理。

## 与 Go 版本的对应关系

直接对照文件是 [`fragment.go`](fragment.go) 和 [`fragment_test.go`](fragment_test.go)。对应关系如下：

- Rust `AllocMPPTaskID` 保留 Go 的语句级原子计数语义；参数从完整 `sessionctx.Context` 收窄为 `StatementContext`。Rust 生产生成器当前另用实例内计数器分配任务，不通过该公开函数。
- Rust `AllocMPPQueryID` 保留 Go 的全局初值 1、先加后取，因此首次返回 2。
- Go `Fragment` 在切片阶段保存 `TableScan`、`ExchangeReceivers`、`CTEReaders` 和 Sink；Rust `Fragment` 只保存已调度 Sink、根标志及 singleton，扫描/Receiver 通过 DFS 临时发现。
- Go 先 `buildFragments` 再 `generateMPPTasksForFragment`；Rust `build_sender` 把切片、递归调度和连边合并成一个流程。两者都沿 Receiver 递归、自底向上写 tasks，并以 Sink/plan ID 缓存共享生产者。
- Go 的 PassThrough 语义由 `Fragment::init` 设置 singleton，再在无表扫分支截断；Rust 生产路径直接检查 Receiver 下游的 Sender 并 `tasks.truncate(1)`。Rust 私有 `Fragment::init` 保留并测试了相同 OR 语义，但没有接入 `build_sender`。
- Go 根路径会把协调器目标设置到返回的根片段，并执行 `fillLocalCTECounts`；Rust 为根 Sender 设置 ID `-1` 目标，但当前文件没有 CTE 本地计数实现。
- Go 扫描路径负责相关列解析、分区裁剪、range 编码、请求缓存、warning、节点表和完整任务字段；Rust 通过 `MppScanRangeEncoder` 抽象前半段，并保留任务字段装配，但 warning 回调为空且没有请求缓存。
- Go `MemoryUsage` 统计 TableScan、Receiver slice 和各对象；Rust 已调度结构较小，只统计 `Fragment` 自身和 Sink 报告值，二者数值不可直接比较。

测试证据中，Go `TestFragmentInitSingleton` 验证 PassThrough 的 OR 语义，Rust `fragment_init_tracks_pass_through_singleton_semantics` 做等价覆盖；Go `TestFillLocalCTECountsUsesLocalTaskCounts` 所验证的能力在 Rust 生产文件中不存在，同名 Rust 测试文件只保留源码对照并明确迁移差异。

## 扩展指南

- 新增扫描编码或分区语义：优先扩展 `MppScanRangeEncoder::Encode` 的实现及 `EncodedMppScan`，不要把具体 session/InfoSchema 依赖直接塞入 `build_sender`。同步扩展 `fragment_aster_unit_test.rs` 的假 encoder，覆盖 request、ranges、表 ID、分区 ID 和错误传播。
- 新增片段边界算子：同时审查 `collect_receivers_into` 和 `collect_table_scans_into` 的停止条件，保证不会把子片段扫描误算到父片段；为“边界前后各有扫描”的结构增加独立测试。
- 修改共享 CTE 行为：保持 plan ID 缓存、复用返回清空 ranges、`Arc` 共享 Sink、最终指针去重四者一致；分别断言 `ConstructMPPTasks` 调用次数、片段数、ranges 数和 target tasks 数。
- 修改 singleton/PassThrough 行为：当前 `Fragment::init` 与 `build_sender` 有两套判定，必须同时更新或重构为单一来源；同步修改 `fragment_test.rs` 和生成器端到端测试。
- 新增错误分支时返回 `expression::Error` 并在独立测试文件覆盖，不要把 Rust 单元测试写进生产 `.rs` 文件。若改变锁策略，需同时审查 coordinator 的 Sink 锁定与 `MPPSink` 实现，避免死锁或扩大 panic 面。
- 若补齐 Go 的 CTE group/计数、相关列、分区裁剪或 request cache，应作为明确的跨模块移植任务处理，核对 Go 的完整流程和相关独立测试；不能用桩或仅编译通过替代行为验证。
- 性能审查重点包括 DFS 深度、每层 `child.root_tasks.clone()`、每任务 session 字符串克隆、地址 HashMap/排序，以及共享 Sink 上的 Mutex 竞争。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph：`status` 显示索引含本文件；`files --filter pkg/planner/core/operator/physicalop` 确认源、Go 对照及两份 Rust 测试；`node --file .../fragment.rs --offset 1/261` 阅读完整 477 行；`query GenerateRootMPPTasks`、`query build_sender`、`query collect_receivers` 确认定义；`callees GenerateRootMPPTasks` 确认 Rust 实现调用 `build_sender`。动态 trait caller 未被图索引解析，已用直接源码补证。
- 生产源码：`fragment.rs`；模块装配 `lib.rs`；crate 边界与依赖 `Cargo.toml`；`base/task_base.rs::MPPSink`；`physical_exchange_receiver.rs`；`physical_exchange_sender.rs`；`pkg/executor/internal/mpp/local_mpp_coordinator.rs::NewLocalMppCoordinator` 与 `append_mpp_dispatch_requests`。
- Go 对照：`fragment.go::{GenerateRootMPPTasks, AllocMPPTaskID, AllocMPPQueryID, generateMPPTasksForFragment, constructMPPTasksImpl}`；`fragment_test.go::{TestFragmentInitSingleton, TestFillLocalCTECountsUsesLocalTaskCounts}`。
- Rust 独立测试：`fragment_test.rs` 覆盖 query ID 单调性、singleton、构造、内存统计和 clone 共享 Sink；`fragment_aster_unit_test.rs` 覆盖根扫描任务/ranges/目标、Receiver 子片段连边和共享生产者复用。
- 仓库契约：读取了最近的 `pkg/planner/core/base/doc.go`；其要求 base 接口保持抽象、避免具体实现耦合，与本文件用 `RootMppTaskGenerator`、`MppScanRangeEncoder` 和 `MPPSink` 分隔职责的做法一致。

本任务为纯文档分析，按计划不运行 Cargo。结构验收应确认本文恰含规定的十一个二级标题；内容人工复核重点是没有把上述 Go 独有能力写成 Rust 已支持。
