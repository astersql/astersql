# [`pkg/planner/core/operator/physicalop/physical_exchange_receiver.rs`](./physical_exchange_receiver.rs)

## 文件定位

本文件定义 MPP 物理计划中的网络接收边界 `PhysicalExchangeReceiver`。它属于 crate `astersql-planner-core-operator-physicalop`（同目录 `Cargo.toml`），由 `lib.rs` 的 `mod physical_exchange_receiver` 纳入 crate，再经 `pub use physical_exchange_receiver::*` 对上层规划器公开。`lib.rs` 中的 `direct_operator_core!(PhysicalExchangeReceiver, PhysicalSchemaProducer)` 还把本文件的方法接入统一的 `base::Plan` 与 `base::PhysicalPlan` 动态分发接口。

Receiver 在同一个内存计划树里仍保留一个 `PhysicalExchangeSender` 孩子，供 MPP 拓扑生成器发现下游片段；但在生成 TiFlash protobuf 时，`PhysicalExchangeReceiver::ToPB` 只编码接收端本身，不递归编码这个 Sender。因此它同时是“规划树中的连接点”和“下推执行器树的片段叶子”。直接的拓扑证据见 `fragment.rs::collect_receivers_into`、`MppTaskGenerator::build_sender` 与本文件 `GetExchangeSender`/`ToPB`。

## 核心职责

- 用 `PhysicalSchemaProducer` 承载公共物理计划状态、输出 Schema、统计信息、孩子以及 EXPLAIN 标识。
- 用线程安全任务快照保存下游发送端的 `kv::MPPTask`；`fragment.rs::build_sender` 构建子片段后调用 `SetTasks` 回填这些元数据。
- 暴露并校验 Receiver 与其唯一 Sender 孩子的结构关系，供片段划分、PassThrough 单节点判定和任务拓扑生成使用。
- 为 EXPLAIN 输出细粒度 shuffle 的 `stream_count`，并将任务元数据、输出字段类型、执行器 ID、shuffle 流数和批大小编码为 `tipb::Executor`。
- 通过 schema producer 或基类复用索引解析、任务挂接和本地成本入口；真正的 Go 对齐成本计算由 `lib.rs::impl_concrete_physical_plan!` 优先路由到 planner core 的全局成本路由器。

本文件不建立网络连接，也不接收运行时数据；它描述计划与 MPP 调度元数据，实际执行发生在下游 TiFlash 执行环境。

## 主要符号

- `PhysicalExchangeReceiver { PhysicalSchemaProducer, tasks }`：公开的 schema producer 加私有的 `Arc<RwLock<Vec<kv::MPPTask>>>`。任务值由 Receiver 的克隆之间按具体构造方式决定是否共享；当前 `Clone` 会创建新的空锁和空列表。
- `New(ctx)`：创建类型为 `plancodec::TypeExchangeReceiver`、初始任务为空的节点。它只初始化基类和类型，不负责设置 Schema、统计信息或 Sender 孩子；这些由构造调用方完成。
- `Clone(new_ctx)`：调用 `BasePhysicalPlan::CloneWithNewCtx`，复制已有 Schema（若存在），但刻意不复制运行时任务列表。
- `GetExchangeSender()`：读取第一个孩子并向下转型为 `PhysicalExchangeSender`；没有孩子或类型不符时返回 `expression::Error`。
- `SetTasks(tasks)` / `Tasks()`：分别在写锁下整体替换任务列表、在读锁下克隆并返回快照。
- `ExplainInfo()` / `ExplainNormalizedInfo()`：流数为零时返回空串，否则返回 `stream_count: N`；规范化输出当前与普通输出一致。
- `ResolveIndices()`、`ExtractCorrelatedCols()`、`Attach2Task()`：分别委托 schema producer 解析列索引、声明本节点没有表达式相关列、调用通用物理计划任务挂接。
- `GetPlanCostVer1()` / `GetPlanCostVer2()`：本地回退实现委托 `BasePhysicalPlan`；经 trait 调用时，`lib.rs` 会先尝试 `routed_plan_cost_ver1/2`。
- `ToPB(context, store)`：编码 `tipb::ExchangeReceiver` 和外层 `tipb::Executor`。传入的 `_store` 不参与决策，字段类型明确按 `kv::StoreType::TiFlash` 转换。
- `MemoryUsage()`：返回 schema producer 的内存估算加任务向量容量乘 `size_of::<kv::MPPTask>()`。

## 执行流程

1. 规划器在 MPP 属性需要网络重分布或汇聚时创建 Sender/Receiver 对。`base_physical_plan.rs` 的多处 MPP 任务构造先建立 `PhysicalExchangeSender`，再以 `PhysicalExchangeReceiver::New` 创建 Receiver、复制输出 Schema/统计信息，并把 Sender 设置为 Receiver 的孩子。
2. `lib.rs` 的 `direct_operator_core!` 为 Receiver 实现公共 `Plan`/`PhysicalPlan` trait，使计划遍历、克隆、成本计算、EXPLAIN、索引解析和 PB 编码都能通过 trait object 到达本文件的方法。
3. `fragment.rs::collect_receivers_into` 从当前 Sender 子树向下收集 Receiver，遇到 Receiver 后不再进入其下游片段；`collect_table_scans_into` 也在 Receiver 边界停止。
4. `fragment.rs::MppTaskGenerator::build_sender` 对每个 Receiver 调用 `GetExchangeSender`，递归生成下游 Sender 片段。若下游 Sender 为 `PassThrough`，当前片段任务会收敛到一个；随后调用 `receiver.SetTasks(child.root_tasks.clone())`，把下游任务写回 Receiver。
5. 生成 TiFlash executor 时，trait 的 `to_pb` 被宏接线到 `PhysicalExchangeReceiver::ToPB`。该方法先取得任务快照并逐个执行 `MPPTask::ToPB` 和 protobuf 序列化，再把 Schema 各列的返回类型转换为 TiFlash `FieldType`，最后组装 Receiver 和外层 Executor；它不会递归编码 Receiver 的 Sender 孩子。
6. EXPLAIN 通过 trait 路由到 `ExplainInfo`；成本查询则优先走 planner core 安装的全局路由。Go v1/v2 对齐实现分别把孩子成本与网络成本组合，广播在 v2 中还会放大网络成本；只有路由器未安装时才回落到本文件的基类委托。

## 数据与状态

`PhysicalSchemaProducer` 是长期计划状态，包含 `BasePhysicalPlan`、Schema、孩子、统计信息、计划 ID、细粒度 shuffle 流数等公共字段。本文件读取其中的 `TiFlashFineGrainedShuffleStreamCount`、Schema 和 explain ID；Schema/统计/孩子由上游构造过程写入。

`tasks` 是调度阶段生成的易变状态。它保存拥有所有权的 `kv::MPPTask` 值，`SetTasks` 以整体替换而非追加的方式更新，`Tasks` 返回深度由 `MPPTask::clone` 决定的向量快照，调用方不会持有锁保护下的引用。`Clone` 明确丢弃这一状态，防止计划克隆携带旧查询的运行时任务；`physical_exchange_receiver_test.rs::clone_does_not_copy_runtime_tasks_like_go` 固化了这个不变量。

PB 输出中的 `encoded_task_meta` 来自任务快照；`field_types` 与当前 Schema 列顺序一一对应；`executor_id` 来自 `BasePhysicalPlan::explain_id`；shuffle 流数来自计划，batch size 来自 `BuildPBContext`。`_store` 被忽略意味着 Receiver 的输出协议固定面向 TiFlash。

## 依赖与调用关系

上游直接调用者包括：

- `base_physical_plan.rs`：在 MPP Join、聚合、属性满足和锁列恢复等路径创建 Receiver，设置 Schema/统计并挂接 Sender；这是 Receiver 进入真实物理计划的主要位置。
- `fragment.rs::Fragment::init`：通过 `GetExchangeSender` 检测 PassThrough，决定片段是否必须 singleton。
- `fragment.rs::MppTaskGenerator::build_sender`：递归跨 Receiver 构建子片段，并用 `SetTasks` 回填下游根任务。
- `fragment.rs::collect_receivers_into` / `collect_table_scans_into`：把 Receiver 当作片段边界。
- `lib.rs::direct_operator_core!` 与 `impl_concrete_physical_plan!`：把本文件的具体方法适配为 `base::Plan`/`base::PhysicalPlan` trait，实现动态调用入口。

主要下游依赖包括：`base::{Plan, PhysicalPlan, Task, BuildPBContext}` 的公共计划协议，`PhysicalSchemaProducer`/`BasePhysicalPlan` 的状态和默认行为，`PhysicalExchangeSender` 的结构约束，`kv::MPPTask` 的任务元数据，`expression::ToPBFieldTypeWithCheck` 的字段类型转换，以及 `tipb`/`protobuf` 的执行器编码。`Cargo.toml` 对应声明了 `base`、`costusage`、`expression`、`kv`、`property`、`plancodec`、`protobuf` 与带 `protobuf-codec` feature 的 `tipb`。

RustCodeGraph `status` 显示目标文件已索引；文件查询报告 23 个符号并列出 33 个使用文件。当前索引能定位结构体和整文件源码，但没有为该 inherent `impl` 的方法提供可寻址调用边，因此上述方法级边由精确源码搜索及相邻实现核验，不把索引缺失误写成“没有调用者”。

## 错误处理与边界

- `Clone` 传播 `CloneWithNewCtx` 的 `expression::Error`；不会在失败时产生半成品 Receiver。
- `GetExchangeSender` 把“无第一个孩子”和“第一个孩子不是 Sender”统一转换为 `exchange receiver child must be a sender`。它只验证第一个孩子，不显式拒绝额外孩子，因此“恰有一个 Sender”仍主要是上游构造不变量。
- `ToPB` 在任一任务 protobuf 序列化失败时立即返回转换后的 `expression::Error`；任一 Schema 列缺少 `RetType` 时返回 `exchange receiver column has no return type`；字段类型不支持 TiFlash 时传播转换错误。由于先完整收集两个向量，失败不会返回部分 Executor。
- `SetTasks`、`Tasks`、`MemoryUsage` 对锁中毒使用 `expect("exchange receiver task lock")`，会 panic，而不是恢复或返回业务错误。
- `New` 产生的空 Receiver 尚不能安全用于 `GetExchangeSender` 或完整 PB 编码：调用方必须先设置正确孩子和可编码 Schema。空任务列表本身可以编码，代表没有发送任务元数据。
- `ToPB` 是片段边界：Sender 孩子用于拓扑而非嵌入当前 protobuf。改变这一点会重复或跨片段编码执行树。

## 并发与资源生命周期

`tasks` 使用 `Arc<RwLock<_>>`，允许持有同一字段 Arc 的并发读写，并让 `Tasks` 的多个读取并行；写入以整个向量替换为原子临界区。当前公开 `Clone` 不共享原 Arc，而是创建空的独立 Arc，因此“克隆计划”与“共享任务状态”是刻意分离的生命周期。结构体销毁时，最后一个 Arc 所有者释放任务向量。

锁只覆盖取代或克隆任务列表的短临界区；任务序列化在 `Tasks()` 返回后进行，不占用读锁，避免昂贵 protobuf 工作阻塞调度写入。代价是编码得到的是某一时刻的快照；调用协议应在拓扑生成完成并 `SetTasks` 后再调用 `ToPB`。本文件没有线程、异步任务、channel、事务或显式网络句柄；Sender/Fragment 的 `Arc<Mutex<...>>` 生命周期由 `fragment.rs` 管理。

`MemoryUsage` 根据向量 capacity 估算任务存储，而非只按 len；它不递归计算任务内部堆分配，也不包含 Arc/RwLock 的所有实现开销，因此应视作与当前 Rust 表示匹配的近似值。

## 与 Go 版本的对应关系

同路径 `physical_exchange_receiver.go` 是直接语义对照，crate 的 `package.metadata.porting.go-package` 也指向同一 Go package。

- 两版都把 Receiver 作为被动接收边界，孩子保持 Sender，并编码任务元数据、字段类型、executor ID 与细粒度 shuffle 参数。
- Go `Init(ctx, stats)` 同时设置类型与统计；Rust `New(ctx)` 只设置类型，调用点另行设置 Schema/统计。这是构造 API 差异，不代表统计语义可以省略。
- Go 公开 `Tasks []*kv.MPPTask` 并另有私有 `frags []*Fragment`；Rust 只保留受锁保护的 `Vec<kv::MPPTask>`，没有 `frags` 字段。Rust 的 fragment 拓扑由 `fragment.rs` 的返回结构持有，不能假定 Go 的 `frags` 生命周期仍存在于 Receiver 内。
- 两版克隆都不复制运行时任务：Go 的 `CloneWithSelf` 只克隆基类，Rust 明确新建空任务锁；Rust 独立测试直接验证这一点。
- Go `GetExchangeSender` 使用索引和类型断言，结构错误会 panic；Rust 返回 `expression::Error`，使 fragment 生成链可以传播可诊断错误。
- Go 的成本方法通过 `utilfuncp` 函数指针调用 core；Rust 的公共 trait 实现通过 `routed_plan_cost_ver1/2` 做依赖倒置。core 中的 Go/Rust成本实现都将 Receiver 识别为网络成本节点，但本文件自己的 `GetPlanCostVer1/2` 只是路由未安装时的基类回退。
- Go `MemoryUsage` 对 nil Receiver 返回零，并统计两个 slice、指针容量及 fragment 内存；Rust 值方法不存在 nil 接收者，只统计 schema producer 与任务值向量容量。两者受数据表示差异影响，数值不应直接逐字节比较。
- Go 字段类型转换直接接收 `column.RetType`；Rust 在调用转换前显式拒绝缺失的返回类型。PB 序列化失败在两版都会作为错误返回。

相关 Go 测试 `fragment_test.go::TestFragmentInitSingleton` 验证 PassThrough Receiver 的 singleton 语义；Rust 对照测试在 `fragment_test.rs::fragment_init_tracks_pass_through_singleton_semantics`。Rust 的 `fragment_aster_unit_test.rs::receiver_boundary_creates_child_fragment_and_links_tasks` 还验证 Receiver 会拆出子片段并关联目标任务。

## 扩展指南

- 增加 Receiver 自有状态时，至少同步修改 `New`、`Clone`、`MemoryUsage` 和 `ToPB`；先决定该状态属于可克隆计划状态、单次查询运行时状态，还是跨克隆共享状态，不能默认沿用 `Arc`。
- 改变任务填充方式时，以 `fragment.rs::MppTaskGenerator::build_sender` 的 `SetTasks` 调用为主要接入点，并保持“下游片段先构建、Receiver 后回填”的顺序。需覆盖空任务、PassThrough、Broadcast、共享 CTE 和多片段场景。
- 改变 Sender/Receiver 结构约束时，同时修改 `GetExchangeSender`、`collect_receivers_into`、`collect_table_scans_into` 和 fragment 初始化逻辑；应决定多孩子是拒绝还是支持，并给出独立错误测试。
- 增加 PB 字段时，修改 `ToPB`，并核对 Go 同路径实现、tipb protobuf 版本和 `BuildPBContext`；保持 Receiver 不递归编码 Sender 的边界。字段顺序、可空返回类型、任务序列化错误和 TiFlash 类型兼容是主要风险。
- 改变成本语义时，不应只改本文件的基类委托；还要检查 `lib.rs` 的成本路由、`core_init.rs` 的安装过程以及 `plan_cost_ver1.rs`/`plan_cost_ver2.rs` 的 Receiver 分支，并与 Go 对照实现同步。
- 测试继续放在独立文件：Receiver 自身行为扩展 `physical_exchange_receiver_test.rs`；拓扑/资源生命周期行为扩展 `fragment_test.rs` 或 `fragment_aster_unit_test.rs`；成本行为放在 planner core 的独立成本测试。不要把测试嵌入本生产源文件。

兼容风险主要是 protobuf 字段与 Go 行为漂移、计划克隆误带旧任务、Receiver 边界被错误穿透；性能风险主要是频繁克隆大任务列表、锁竞争以及重复网络 Exchange；正确性风险主要是 Schema 字段类型/顺序与实际发送数据不一致。

## 验证依据

- 目标实现：`pkg/planner/core/operator/physicalop/physical_exchange_receiver.rs`，逐行核对结构体及 14 个公开方法、错误分支、锁和 PB 字段。
- crate 与 trait 接线：`pkg/planner/core/operator/physicalop/Cargo.toml`；`lib.rs` 的模块声明、再导出、`ConcretePhysicalOperator`、`direct_operator_core!` 和 `impl_concrete_physical_plan!`。
- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter ...physical_exchange_receiver.rs` 报告目标文件含 23 个符号并被 33 个文件使用；`query PhysicalExchangeReceiver --kind struct` 同时定位 Rust/Go 定义；限定方法节点和调用边查询没有结果，故使用精确源码搜索补足方法级证据。
- 直接调用与拓扑：`fragment.rs::Fragment::init`、`MppTaskGenerator::build_sender`、`collect_receivers_into`、`collect_table_scans_into`；`base_physical_plan.rs` 中 Receiver/Sender 构造路径。
- Go 对照：`physical_exchange_receiver.go`；成本实现 `pkg/planner/core/plan_cost_ver1.go::getPlanCostVer1PhysicalExchangeReceiver` 与 `plan_cost_ver2.go::getPlanCostVer2PhysicalExchangeReceiver`；Rust core 成本分支 `plan_cost_ver1.rs`、`plan_cost_ver2.rs`。
- 独立测试：`physical_exchange_receiver_test.rs::clone_does_not_copy_runtime_tasks_like_go`；`fragment_test.rs::fragment_init_tracks_pass_through_singleton_semantics`；`fragment_aster_unit_test.rs::receiver_boundary_creates_child_fragment_and_links_tasks`；Go `fragment_test.go::TestFragmentInitSingleton`。
- 目标目录不存在 `doc.go`，因此没有可读取的包级 Go 合约文件；包边界以 Cargo、`lib.rs`、直接实现和测试为准。
- 本任务是只新增说明文档的分析任务，按计划不运行 Cargo。交付前执行任务文件指定的 11 章节结构命令，并人工复核唯一生产物、链接路径、事实来源及未验证边界。
