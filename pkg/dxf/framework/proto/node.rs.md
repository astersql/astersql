# `pkg/dxf/framework/proto/node.rs`

源文件：[node.rs](./node.rs)

## 文件定位

本文件属于 `astersql-dxf-framework-proto` crate，是 DXF（分布式执行框架）共享协议模型中的“受管节点与节点资源”定义。crate 入口 `pkg/dxf/framework/proto/lib.rs` 通过 `pub mod node` 声明模块，并以 `pub use node::*` 将这里的公开符号重新导出；`pkg/dxf/framework/proto/Cargo.toml` 则把该 crate 映射到 Go 包 `pkg/dxf/framework/proto`。

在 DXF 全局结构中，`ManagedNode` 承载调度侧可见的节点身份、服务角色和 CPU 槽位数量，`NodeResource` 承载单节点 CPU、内存、磁盘容量快照及其比例换算。框架总览 `pkg/dxf/framework/doc.go` 规定“一核对应一个 slot”，单 slot 的内存和磁盘份额按节点总量除以总核数计算；本文件的 `GetStepResource` 和 `GetTaskDiskResource` 正是这一抽象的数值实现。

当前 Rust 生产接线应按实际代码区分：`pkg/dxf/framework/storage/nodes.rs` 使用本文件的 `proto::NodeResource` 保存进程内资源快照，并从 `mysql.dist_framework_meta` 构造本文件的 `proto::ManagedNode`；调度器随后在 `pkg/dxf/framework/scheduler/storage_adapter.rs::all_nodes` 将其转换成调度器自身的节点类型。仓库中还存在 taskexecutor、scheduler、testutil 等模块各自的同名资源类型，不能仅凭名称将它们视为本文件类型。

## 核心职责

- `ManagedNode` 定义 DXF 元数据层对一个 TiDB 节点的最小描述：节点 ID、服务角色和 CPU 核数。
- `NodeResource` 以整数保存节点总 CPU、总内存字节数和总磁盘字节数；`NewNodeResource` 集中构造该快照。
- `NodeResource::LimitDXFResource` 按百分比裁剪 DXF 可用 CPU，并以相同 CPU 比例裁剪内存；磁盘有意保持节点原值。
- `NodeResource::GetStepResource` 根据 `TaskBase::GetRuntimeSlots` 返回某一步可分配的 CPU/内存容量。
- `NodeResource::GetTaskDiskResource` 根据任务 runtime slots 计算磁盘份额，并先用 `quotaHint` 对节点磁盘总量封顶。
- 私有函数 `getLimitedDXFCPU` 实现百分比限额的向上取整、至少一核和不超过总核数等规则。
- `NodeResourceForTest` 提供 32 核、32 GiB 内存、100 GiB 磁盘的测试默认值。

本文件只做数据建模与同步换算，不负责资源探测、持久化、调度、实际分配或回收。进程级快照的锁与替换逻辑位于 `pkg/dxf/framework/storage/nodes.rs`，可分配资源的原子计数逻辑位于 `pkg/dxf/framework/proto/subtask.rs::Allocatable`。

## 主要符号

- `pub struct ManagedNode { ID: String, Role: String, CPUCount: i32 }`：公开节点记录。`ID` 对应 Go 注释中的执行 ID，并在元数据表中存为 `host`；`Role` 预期为 `""` 或 `"background"`，但该结构本身不校验；`CPUCount` 是节点核数/slot 数。
- `pub struct NodeResource { TotalCPU: i32, TotalMem: i64, TotalDisk: u64 }`：公开资源快照。三个字段均可由调用方直接构造或读取，本文件未封装合法性检查。
- `pub fn NewNodeResource(i32, i64, u64) -> NodeResource`：按原值填充三个字段。与 Go 返回 `*NodeResource` 不同，Rust 返回拥有所有权的值。
- `pub fn NodeResource::LimitDXFResource(&self, limit: i32) -> NodeResource`：返回新的受限快照，不修改 `self`。CPU 由 `getLimitedDXFCPU` 计算；只有 CPU 真正变化且原总 CPU 为正时才换算内存。
- `pub fn NodeResource::GetStepResource(&self, task: &TaskBase) -> StepResource`：取 `task.GetRuntimeSlots()` 作为 CPU 容量，并以 `slots / TotalCPU` 的比例计算内存容量；两者分别交给 `NewAllocatable`，初始已用量为零。
- `pub fn NodeResource::GetTaskDiskResource(&self, task: &TaskBase, quotaHint: u64) -> u64`：先取 `min(TotalDisk, quotaHint)`，再乘以 `slots / TotalCPU`。
- `fn getLimitedDXFCPU(totalCPU: i32, limit: i32) -> i32`：模块私有辅助函数。`totalCPU <= 0` 或 `limit >= 100` 时直接返回总 CPU；其他情况计算 `ceil(totalCPU * limit / 100)`，随后下限钳制为 1、上限钳制为 `totalCPU`。
- `pub static NodeResourceForTest: NodeResource`：不可变静态测试资源。它是普通共享值，不包含惰性初始化、锁或运行期探测。

文件没有 trait、枚举、条件编译项或异步函数。所有命名沿用 Go 风格；crate 根通过 `#![allow(non_snake_case, ...)]` 接纳这些公开名称。

## 执行流程

`LimitDXFResource(limit)` 的流程如下：

1. 调用 `getLimitedDXFCPU(self.TotalCPU, limit)`。
2. 若限额没有改变 CPU，或 `TotalCPU <= 0`，以原 CPU、内存和磁盘构造新快照并返回。
3. 否则用 `usableCPU / TotalCPU` 的浮点比例乘以 `TotalMem`，转换回 `i64`；转换会舍弃小数部分。
4. 返回新快照：CPU 和内存采用裁剪值，磁盘仍为 `TotalDisk`。源码注释说明该限额面向只支持 global sort、没有本地盘约束的 premium-based cluster，因此磁盘不随比例缩小。

`GetStepResource(task)` 先由 `TaskBase::GetRuntimeSlots` 决定有效 slots。该方法在 `ExtraParams.MaxRuntimeSlots > 0` 且未指定 `TargetSteps`、或当前 step 命中 `TargetSteps` 时，返回 `min(MaxRuntimeSlots, RequiredSlots)`；否则返回 `RequiredSlots`。随后本文件把 slots 直接作为 CPU `Allocatable` 容量，并按同一比例换算内存 `Allocatable` 容量。`pkg/dxf/framework/integrationtests/framework_test.rs::runtime_slot_limit_controls_step_resource` 用 16 核、16000 字节内存和 3 slots 验证结果为 CPU 3、内存 3000。

`GetTaskDiskResource(task, quotaHint)` 使用相同 slots 来源，但分母仍是节点 `TotalCPU`；基数是 `min(TotalDisk, quotaHint)`，所以提示配额只能降低可分磁盘，不能把它提高到节点总量以上。当前 Rust 搜索未发现该方法的外部调用或独立覆盖，因此它是已实现但尚无 Rust 接线证据的公开 API。

节点信息的生产流是：`pkg/dxf/framework/storage/nodes.rs::getAllNodesWithSession` 查询 `select host, role, cpu_count from mysql.dist_framework_meta order by host`，逐行构造 `proto::ManagedNode`；`TaskManager::GetAllNodes` 返回列表；`pkg/dxf/framework/scheduler/storage_adapter.rs::all_nodes` 再把 `ID/Role/CPUCount` 映射为调度器内部的 `id/role/cpu_count`。

## 数据与状态

`ManagedNode` 和 `NodeResource` 都只含拥有所有权的标量或字符串，没有内部引用和隐藏状态。字段公开意味着不变量主要由上游维护：例如 `ManagedNode::Role` 的允许值和“所有受管节点角色一致”只写在注释中，本文件不强制；`NodeResource` 也允许零或负的 CPU/内存值。

容量单位如下：

- `TotalCPU: i32`：核数/slot 数。
- `TotalMem: i64`：字节；允许的符号范围沿用 Go `int64`。
- `TotalDisk: u64`：字节。
- `StepResource::CPU` 与 `StepResource::Mem`：分别以 slot 数和字节数作为 `Allocatable.capacity`。

所有换算都会构造新值，不修改接收者。`pkg/dxf/framework/storage/nodes.rs` 另用 `RwLock<Option<proto::NodeResource>>` 管理进程级快照：`SetNodeResource` 整体替换，`GetNodeResource` 通过 `NewNodeResource` 返回字段副本。这种生命周期不属于本文件，但解释了生产代码如何保存这里的无锁值对象。

`NodeResourceForTest` 的常量使用 `bytesize::GIB`，即二进制 GiB。Go 对照使用 `docker/go-units` 的 `units.GB`；当前依赖和测试将 Rust 默认值明确固定为 GiB 量级，扩展测试时不应把名称近似当作单位完全相同的证明。

## 依赖与调用关系

直接依赖：

- `super::task::TaskBase`：提供 `GetRuntimeSlots`，决定 step 和任务磁盘换算的 slots 分子。
- `super::subtask::{NewAllocatable, StepResource}`：把计算出的 CPU/内存容量包装为带原子使用量的资源对象。
- `std::cmp::min`：用于 CPU 上限和磁盘提示配额上限。
- `bytesize::GIB`：只用于 `NodeResourceForTest` 的内存、磁盘常量。

RustCodeGraph 确认的文件内调用边为 `LimitDXFResource -> getLimitedDXFCPU` 和 `LimitDXFResource -> NewNodeResource`。图索引未解析方法调用形式的全部上游，因此使用精确引用搜索补齐了以下直接证据：

- `pkg/dxf/framework/storage/nodes.rs` 持有、读取、替换 `proto::NodeResource`，并构造 `proto::ManagedNode`。
- `pkg/dxf/framework/scheduler/storage_adapter.rs::all_nodes` 消费 storage 返回的 `ManagedNode`。
- `pkg/dxf/framework/handle/status.rs::ListManagedNodes` 通过 handle runtime 返回 `Vec<proto::ManagedNode>`。
- `pkg/dxf/framework/integrationtests/framework_test.rs` 直接调用 `NewNodeResource(...).GetStepResource(...)`。
- `pkg/dxf/framework/proto/task_test.rs` 和 `migration_aster_unit_test.rs` 直接覆盖 `LimitDXFResource`。
- `pkg/dxf/importinto/scheduler_testkit_test.rs` 使用 `NodeResourceForTest` 构造调度器测试环境。

需特别避免错误归因：`pkg/dxf/framework/taskexecutor/interface.rs`、`pkg/dxf/framework/scheduler/interface.rs` 和 `pkg/dxf/framework/testutil/context.rs` 均定义了自己的 `NodeResource` 或 `ManagedNode`。例如 `pkg/dxf/importinto/task_executor.rs` 调用的是 `node_executor::NodeResource::GetStepResource`，不是本文件的方法。

## 错误处理与边界

本文件的 API 均不返回 `Result`，也不会主动报告非法参数。调用者必须维护以下前置条件和边界认知：

- `getLimitedDXFCPU` 对 `totalCPU <= 0` 或 `limit >= 100` 视为“不限额”，直接返回 `totalCPU`。
- 当 `totalCPU > 0` 且 `limit < 100` 时，负数或零百分比最终也会被下限钳制为 1 核；该行为与 Go 对照一致，但不等价于拒绝非法百分比。
- 百分比采用向上取整。例如 16 核的 30% 为 4.8，得到 5 核；2 核的 10% 得到至少 1 核。内存按最终 CPU 比例缩放，而不是直接按原始 `limit` 缩放。
- `GetStepResource` 和 `GetTaskDiskResource` 没有保护 `TotalCPU == 0`，也不检查 runtime slots 是否为负或超过总 CPU。源码明确保留 Go 的这一前提；调用方应确保用于比例换算的总 CPU 为正且 task slots 合法。
- 浮点比例再转换为整数会丢弃小数，并可能在极端值上产生精度损失；这里没有 checked arithmetic、溢出错误或精确有理数计算。
- `ManagedNode::Role` 与节点间角色一致性不在构造时校验；筛选和一致性策略属于 scheduler 层。
- `GetTaskDiskResource` 当前没有 Rust 调用者或针对零 CPU、quotaHint 边界的独立测试证据，应把这些情况视为未验证，而不是默认已保障。

## 并发与资源生命周期

本文件自身没有锁、线程、异步任务、通道、文件句柄或网络资源。所有计算只借用 `&self` 和 `&TaskBase`，返回拥有所有权的新值，因此不会就地更新共享资源快照。

`GetStepResource` 创建的两个 `Allocatable` 各自含 `AtomicI64 used`，初始为 0；后续并发 `Alloc/Free` 的安全性由 `pkg/dxf/framework/proto/subtask.rs` 负责，而非本文件。`pkg/dxf/framework/proto/migration_aster_unit_test.rs::migration_node_and_allocatable_match_go` 在多个线程中循环分配/释放并验证最终 `Used() == 0`，这只间接覆盖本文件产物所依赖的资源容器。

进程级 `NodeResource` 生命周期由 storage 层的 `RwLock<Option<_>>` 管理。读取时构造字段副本，避免把锁守卫或内部引用暴露给调用方；写入时整体替换。`ManagedNode` 列表则由每次 SQL 查询重新构造并向上返回，没有在本文件内缓存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/proto/node.go`，Rust 保留了相同的结构名、字段名、计算顺序和主要注释：

- Go `ManagedNode`、`NodeResource` 与 Rust 对应结构字段一一匹配；Go 的 CPU 字段使用平台相关 `int`，Rust 固定为 `i32`，只在预期核数范围内语义等价，内存和磁盘则分别保持 `int64/i64` 与 `uint64/u64`。
- Go `NewNodeResource` 返回指针，Rust 返回拥有所有权的 `NodeResource` 值；因此 Rust 调用者通过移动、借用或显式重建处理共享，而不是依赖可空指针。
- 两端 `LimitDXFResource` 都先计算可用 CPU，在 CPU 不变或总 CPU 非正时复制原资源；否则按最终 CPU 比例缩放内存并保留磁盘。
- 两端 `getLimitedDXFCPU` 都使用向上取整、至少一核、至多总核数的顺序。
- 两端 `GetStepResource` 都以 `TaskBase.GetRuntimeSlots()` 作为 CPU 容量，并按 `slots / TotalCPU` 缩放内存。
- 两端 `GetTaskDiskResource` 都先取总磁盘和 quota hint 的较小值，再按 slots 比例缩放。
- Go `NodeResourceForTest` 是可替换的包级指针变量；Rust 是不可变 `static` 值。这是所有权/可变性差异，使用者不能假设 Rust 端可像 Go 一样重绑该变量。

回归证据也保持对应：Go 的 `pkg/dxf/framework/proto/task_test.go::TestTaskBaseGetRuntimeSlots` 验证 30%、100% 和至少一核三个分支；Rust 的 `pkg/dxf/framework/proto/task_test.rs::test_task_base_get_runtime_slots` 复现相同断言，`migration_aster_unit_test.rs::migration_node_and_allocatable_match_go` 再覆盖 16 核按 30% 裁剪为 5 核、1600 内存裁剪为 500。

## 扩展指南

新增或修改节点资源维度时，应从本文件的 `NodeResource`、`NewNodeResource` 和所有比例换算方法开始，并同步检查 `pkg/dxf/framework/storage/nodes.rs` 的默认值、复制逻辑及元数据写入；若维度要参与调度，还需检查 scheduler/taskexecutor 的独立资源类型和转换边界，不能只增加同名字段。

修改 slot 计算时，应明确选择接入点：任务级 runtime slot 选择属于 `pkg/dxf/framework/proto/task.rs::TaskBase::GetRuntimeSlots`，节点资源到 step 容量的映射属于本文件 `GetStepResource`，实际并发分配属于 `pkg/dxf/framework/proto/subtask.rs::Allocatable`。把规则放错层会导致调度估算和执行期容量不一致。

安全扩展至少应同步独立测试文件，而不要把测试写进 `node.rs`：

- 在 `pkg/dxf/framework/proto/task_test.rs` 增补百分比、runtime slots 和换算边界的单元测试，并同步 Go 对照 `task_test.go` 的真实语义。
- 在 `pkg/dxf/framework/proto/migration_aster_unit_test.rs` 保留跨模块移植回归。
- 涉及公开框架行为时，在 `pkg/dxf/framework/integrationtests/framework_test.rs` 增补端到端的容量断言。
- 为当前未覆盖的 `GetTaskDiskResource` 增加正数 `TotalCPU` 下的 quota 小于/大于总磁盘、不同 slots 比例测试；若要支持零 CPU，先定义与 Go 一致的新契约，再同时修改实现和测试。
- 若变更 `ManagedNode` 字段或角色语义，同步检查 `pkg/dxf/framework/storage/nodes.rs::getAllNodesWithSession`、`pkg/dxf/framework/scheduler/storage_adapter.rs::all_nodes` 以及 scheduler 的节点筛选测试。

兼容风险主要是 Go/Rust 数值舍入差异、公开字段/构造函数签名变化、不同模块同名类型漏同步；性能风险主要来自把当前常数时间纯计算改成锁操作或外部探测。保持本文件为轻量值模型，有助于避免在调度热路径引入额外共享状态。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 状态：索引包含 11467 个文件、307296 个节点和 1848419 条边；目标 `pkg/dxf/framework/proto/node.rs` 被识别为 128 行、8 个符号。
- RustCodeGraph 源码与符号：`node.rs` 全文；`NewNodeResource`、`LimitDXFResource`、`GetStepResource`、`GetTaskDiskResource`、`getLimitedDXFCPU`；下游 `subtask.rs::NewAllocatable` 与 `task.rs::GetRuntimeSlots`。
- RustCodeGraph 调用边：`LimitDXFResource -> NewNodeResource`、`LimitDXFResource -> getLimitedDXFCPU`；`storage/nodes.rs::GetAllNodes -> getAllNodesWithSession`；`scheduler/storage_adapter.rs::all_nodes` 对节点字段的转换。
- crate 与模块边界：`pkg/dxf/framework/proto/Cargo.toml`、`pkg/dxf/framework/proto/lib.rs`、根 `Cargo.toml` 的 workspace 成员和 facade 依赖。
- 框架契约：`pkg/dxf/framework/doc.go` 的 slot、节点角色和 DXF 资源管理说明。
- Go 对照：`pkg/dxf/framework/proto/node.go`、`pkg/dxf/framework/proto/task_test.go`。
- Rust 测试：`pkg/dxf/framework/proto/task_test.rs`、`pkg/dxf/framework/proto/migration_aster_unit_test.rs`、`pkg/dxf/framework/integrationtests/framework_test.rs`；引用搜索未发现 `GetTaskDiskResource` 的 Rust 调用或测试。
- 生产接线：`pkg/dxf/framework/storage/nodes.rs`、`pkg/dxf/framework/scheduler/storage_adapter.rs`、`pkg/dxf/framework/handle/status.rs`。

本任务是纯文档分析，按计划未运行 Cargo。结论限于上述静态源码、索引和引用搜索；没有验证运行期极端浮点转换、零 `TotalCPU` 行为或未接线的 `GetTaskDiskResource`。结构验证要求本文恰好包含计划指定的十一个二级标题。
