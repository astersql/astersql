# `pkg/planner/core/operator/physicalop/physical_exchange_sender.rs`

## 文件定位

该文件实现 MPP（Massively Parallel Processing）执行计划片段边界上的发送端 `PhysicalExchangeSender`。它既是一个 `PhysicalPlan`，又通过 `MPPSink` trait 充当 fragment 的输出汇点：当前 fragment 的结果由它按照 PassThrough、Broadcast 或 Hash 方式发送给父 fragment 的目标任务。源码通过 `PhysicalSchemaProducer` 复用物理计划、schema、统计信息和子节点能力（`PhysicalExchangeSender.PhysicalSchemaProducer`）。

crate 边界由 `pkg/planner/core/operator/physicalop/Cargo.toml` 定义，包名为 `astersql-planner-core-operator-physicalop`；`lib.rs` 以 `mod physical_exchange_sender` 装入模块、以 `pub use physical_exchange_sender::*` 导出类型，并通过 `direct_operator_core!(PhysicalExchangeSender, PhysicalSchemaProducer)` 接入统一物理算子接口。该目录没有 `doc.go`；最近的直接包级事实来自上述 Cargo/lib 接线、同路径 Go 实现和调用代码，而不是额外的包说明文件。

## 核心职责

1. 保存交换配置：`ExchangeType`、Hash 分区列 `HashCols` 和 `CompressionMode`。
2. 保存调度阶段产生的两组任务：`Tasks` 是 sender 所在 fragment 的任务，`TargetTasks` 是接收数据的目标任务；`MPPSink` 实现使 fragment 构建器可以用统一接口写入它们。
3. 在 `ResolveIndices*` 中把 Hash 分区列绑定到唯一子节点或显式输入 schema。
4. 在 `ToPB` 中把子执行器、目标任务元数据、分区表达式、字段类型、交换类型、压缩方式及细粒度 shuffle 参数编码成 TiFlash 可消费的 `tipb::Executor`。
5. 提供计划框架所需的克隆、代价、任务挂接、EXPLAIN、相关列提取和内存估算入口。

该类型描述和编码发送边界，本身不执行网络发送；真正的 MPP 任务拓扑由 `fragment.rs` 构造，实际数据交换由下游 TiFlash 按 protobuf 配置执行。

## 主要符号

- `PhysicalExchangeSender`：公开结构体。`PhysicalSchemaProducer` 持有计划公共状态；`TargetTasks`、`Tasks` 保存已调度任务；`ExchangeType` 决定路由；`HashCols` 描述 Hash 键及 collation；`CompressionMode` 决定交换数据压缩。
- `New(ctx)`：创建类型为 `plancodec::TypeExchangeSender` 的计划，默认 `PassThrough`、无 Hash 列、无任务、`ExchangeCompressionModeNONE`。
- `Init(self, ctx, stats)`：重新安装上下文、计划类型、query block offset 和统计信息。它是构造后的初始化接线，不产生任务。
- `Clone(&self, new_ctx)`：克隆基类与 schema，深度克隆 `HashCols`，保留交换类型和压缩配置，但故意清空 `Tasks`、`TargetTasks`，避免把一次调度结果带入新的计划实例。
- `GetCompressionMode`、`GetSelfTasks`、`SetSelfTasks`、`SetTargetTasks`、`AppendTargetTasks`：调度状态访问器。追加操作直接使用 `Vec::extend`，保留顺序和重复项。
- `IsHashExchange`：仅在 `ExchangeType == tipb::ExchangeType::Hash` 时返回真。
- `ExplainInfo` / `ExplainNormalizedInfo`：输出交换类型，并按条件附加压缩方式、Hash 列、本端任务 ID 和细粒度 shuffle stream 数；规范化版本当前与普通版本完全相同。
- `ResolveIndicesItself` / `ResolveIndicesItselfWithSchema` / `ResolveIndices`：先解析公共计划，再逐个替换为相对输入 schema 解析后的 `MPPPartitionColumn`。
- `ExtractCorrelatedCols`：固定返回空列表；sender 自身不引入相关表达式。
- `GetPlanCostVer1`、`GetPlanCostVer2`、`Attach2Task`：直接委托公共物理计划实现。
- `ToPB`：本文件最主要的序列化入口，返回 `tipb::ExecType::TypeExchangeSender` 执行器。
- `MemoryUsage`：汇总 producer、三个 Vec 头部、交换类型、Hash 列内容和三个 Vec 的容量成本。
- `impl MPPSink for PhysicalExchangeSender`：把统一 sink trait 的五个方法转发给上述固有方法。

## 执行流程

计划构造阶段通常先调用 `New`/`Init`，优化器再设置 `ExchangeType`、`HashCols`、压缩方式和唯一孩子。`ResolveIndices` 先让 `PhysicalSchemaProducer` 解析公共表达式与 schema，再由 `ResolveIndicesItself` 获取第一个孩子的 schema，逐个解析 Hash 分区列索引；没有孩子时返回明确错误。

MPP 调度阶段由 `fragment.rs::SessionRootMppTaskGenerator::GenerateRootMPPTasks` 要求根计划能够下转为 `PhysicalExchangeSender`，随后调用递归的 `build_sender`。`build_sender` 收集当前 sender 下的 receiver 和 table scan，为 fragment 生成本端任务，克隆 sender 后用 `SetSelfTasks` 写入本层任务；根 fragment 的 `TargetTasks` 被设置为 ID `-1` 的协调器任务，子 fragment 则通过 `MPPSink::append_target_tasks` 追加父层任务。RustCodeGraph 给出的直接流向为 `GenerateRootMPPTasks → build_sender`。`PhysicalExchangeReceiver::GetExchangeSender` 反向验证 receiver 的唯一孩子确实是 sender。

下发阶段由 `lib.rs` 的统一 `operator_to_pb` 接线调用 `ToPB`：

1. 取唯一孩子并强制按 `kv::StoreType::TiFlash` 编码。
2. 对每个 `TargetTasks` 调用 `MPPTask::ToPB`，再序列化为字节数组。
3. 将每个 `HashCols` 的列表达式加入 partition key 列表，并将返回类型转换为 protobuf `FieldType` 后覆盖 `CollateID`。
4. 将 sender 输出 schema 的全部列类型转换为 `all_field_types`。
5. 使用 `BuildPBContext` 的表达式上下文和 client 编码分区表达式。
6. 把 AsterSQL 会话压缩枚举映射为 tipb 的 `Fast`、`HighCompression` 或 `None`。
7. 组装 `tipb::ExchangeSender` 和外层 `tipb::Executor`，补入 executor ID、细粒度 shuffle stream count 与 batch size。

## 数据与状态

`ExchangeType`、`HashCols`、`CompressionMode` 属于可随计划克隆保留的配置；`Tasks`、`TargetTasks` 属于一次 MPP 调度生成的运行期拓扑状态，`Clone` 明确不复制后两者。`HashCols` 中每项同时携带表达式列与 `CollateID`，因此 protobuf 编码必须保持表达式和类型/collation 的一一对应。

任务列表使用拥有所有权的 `Vec<kv::MPPTask>`。`Set*` 覆盖原列表；`AppendTargetTasks` 只在尾部追加，不去重。`physical_exchange_sender_test.rs::append_target_tasks_preserves_duplicates_like_go` 明确把“保序且保留重复任务”作为兼容语义。`GetSelfTasks` 返回借用切片，调用者不能绕过 `&mut self` 修改内部 Vec。

`ExplainInfo` 展示的是 `Tasks`（本端任务），不是 `TargetTasks`；空任务、无压缩、无 stream 时对应字段不会出现。`MemoryUsage` 按容量而非长度估算 Vec 后备存储，并递归累计每个 Hash 分区列的内存；测试 `memory_usage_counts_go_slice_headers_and_exchange_type` 锁定空 sender 的基础公式。

## 依赖与调用关系

上游主要有三类：优化器/物理计划构造代码创建和配置 sender；`fragment.rs` 把它作为 `MPPSink` 构建任务拓扑；统一 `PhysicalPlan` 接口通过 `direct_operator_core!` 调用 EXPLAIN、索引解析、代价计算、任务挂接和 protobuf 编码。RustCodeGraph 还显示 `PhysicalExchangeReceiver::GetExchangeSender` 读取 sender，以及 planner/executor 的计划遍历代码对该具体类型做下转判断。

下游依赖包括：`base` 的 `PhysicalPlan`、`Plan`、`Task`、`MPPSink` 与 `BuildPBContext`；`property` 的 `StatsInfo`、`MPPPartitionColumn`、`TaskType` 和列说明格式化；`kv::MPPTask`/`StoreType`；`expression` 的 schema、表达式、字段类型与 protobuf 转换；`tipb` 的交换和执行器消息；`vardef` 的压缩枚举；`protobuf::Message` 的任务元数据序列化。Cargo 清单对这些依赖分别以工作区 path crate、固定 `protobuf = 2.8.0` 和固定 tipb Git revision 声明。

关键调用边为：`GenerateRootMPPTasks → build_sender → Clone/SetSelfTasks/SetTargetTasks/append_target_tasks`；计划 protobuf 主链为统一 `operator_to_pb → PhysicalExchangeSender::ToPB → child.to_pb / MPPTask::ToPB / ExpressionsToPBList / ToPBFieldTypeWithCheck`。本文件不拥有网络连接、线程或异步任务。

## 错误处理与边界

所有可失败的计划操作统一返回 `expression::Error`。`Clone` 传播基类克隆失败；`ResolveIndices` 先传播公共解析错误，再传播 Hash 列解析错误。`ResolveIndicesItself` 和 `ToPB` 都要求至少一个孩子，但 Rust 版本不会像 Go 的 `Children()[0]` 那样越界崩溃，而是分别返回 `exchange sender requires one child`。

`ToPB` 还会在以下位置提前返回：任务 protobuf 序列化失败；Hash 列或输出 schema 列缺少返回类型；字段类型不适用于指定 store；`BuildPBContext` 缺少 client；分区表达式编码失败。注意孩子始终按 TiFlash store 编码，而字段类型转换仍使用调用者传入的 `store`，这是与 Go 实现一致的形状，不应在扩展时随意合并两者。

当前代码不会验证“非 Hash 交换时 `HashCols` 必须为空”，也不会去重目标任务；它会按现有状态照常编码。`ToPB` 只读取第一个孩子，对额外孩子没有本地拒绝逻辑，因此“一元算子”约束应由构造/校验链维持。未知或 NONE 压缩值落到 tipb `None`。`ExplainNormalizedInfo` 当前没有隐藏任务 ID 等额外规范化处理，而是原样复用 `ExplainInfo`。

## 并发与资源生命周期

`PhysicalExchangeSender` 自身没有锁、原子变量、channel、后台任务或 I/O 资源；可变操作要求 `&mut self`。调度器把它装入 `Fragment.Sink` 后，由 fragment 层的 `Arc<Mutex<Box<dyn MPPSink>>>` 提供共享和互斥，而锁不在本文件内。`fragment_test.rs::fragment_clone_shares_the_same_underlying_sink` 验证 fragment 克隆共享同一个 sink，并通过 mutex 串行更新任务状态。

生命周期上，计划配置先建立，索引解析后进入调度；调度克隆 sender 以隔离原始计划，并安装本端/目标任务；随后 `ToPB` 只读地生成拥有自身数据的 protobuf。`Clone` 清空调度任务是重要的不变量，否则旧查询的任务地址和 ID 可能泄漏到新调度。任务元数据在 `ToPB` 时被立即序列化为字节，不在返回执行器中保留对 `kv::MPPTask` 的借用。

## 与 Go 版本的对应关系

同路径 `physical_exchange_sender.go` 是直接对照实现。字段集合、默认/初始化方式、交换类型、任务访问器、EXPLAIN 条件、索引解析顺序、代价委托和 protobuf 字段总体一一对应。Rust 的 `PhysicalSchemaProducer` 对应 Go 内嵌的 `BasePhysicalPlan` 能力；Rust `Vec<kv::MPPTask>` 对应 Go `[]*kv.MPPTask`，因此 Rust 值语义会克隆任务，而 Go 保存指针。

两边克隆都不复制已调度的 `Tasks` 与 `TargetTasks`，并保留 `ExchangeType`、`HashCols` 和 `CompressionMode`。Rust 对 `HashCols` 逐项调用 `Clone`，Go 则复制 slice 头并共享列指针；语义目标相同，但后续原地修改列对象时的别名行为不同。Rust 测试 `cloning_a_sender_preserves_plan_configuration_but_not_scheduled_tasks` 锁定任务清空和压缩保留行为。

Rust 增加了显式错误边界：缺少孩子、返回类型或 PB client 时返回错误，而 Go 在其中部分路径依赖非空前置条件并可能索引/解引用失败。压缩映射在 Go 中委托 `ToTipbCompressionMode`，Rust 在 `ToPB` 内显式 match。Go `MemoryUsage` 对 nil receiver 返回 0；Rust 方法要求有效引用，因此不存在 nil receiver 分支。Go 的 `fragment_test.go::TestFragmentInitSingleton` 与 Rust `fragment_test.rs::fragment_init_tracks_pass_through_singleton_semantics` 都证明：存在 PassThrough 接收边界时 fragment 为 singleton，而全 Broadcast 不会触发 singleton。

## 扩展指南

新增交换策略时，至少需要同步修改 `tipb::ExchangeType` 的匹配、`ExplainInfo` 的稳定文本、`ToPB` 的编码语义，以及 fragment 调度对任务数/拓扑的判断；同时在独立测试文件中覆盖默认值、EXPLAIN、调度拓扑和 protobuf 输出。不要把 Rust 单元测试写入本源文件，应扩展 `physical_exchange_sender_test.rs`，跨 fragment 行为放入 `fragment_test.rs`，必要时同步 Go 对照测试。

新增 sender 状态时，应先判断它是计划配置还是调度状态：前者通常要进入 `Clone`、`MemoryUsage`、EXPLAIN/ToPB，后者通常必须像任务列表一样在克隆时清空。新增 Vec/拥有型字段要按容量和元素内容更新 `MemoryUsage`。新增 Hash 键属性时，要同时维护表达式、字段类型、collation 的相同顺序，并补齐缺失元数据的错误测试。

调整 `ToPB` 时需保持孩子固定下推 TiFlash、调用者 store 用于字段类型检查、目标任务顺序不变、executor ID 与细粒度 shuffle 参数完整。兼容风险主要是 protobuf 字段和 EXPLAIN 文本变化；正确性风险主要是 Hash 列索引/类型/collation 错位和错误的目标任务拓扑；性能风险主要是无意深拷贝任务/表达式、重复序列化，以及错误的 stream count 或 batch size。

## 验证依据

- 源码：`pkg/planner/core/operator/physicalop/physical_exchange_sender.rs`，完整核对结构体、全部固有方法和 `MPPSink` 实现。
- 模块与 crate：`pkg/planner/core/operator/physicalop/lib.rs`；`pkg/planner/core/operator/physicalop/Cargo.toml`。确认模块导出、统一物理算子接线、crate 名和直接依赖。
- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`node --file ...physical_exchange_sender.rs` 返回完整 377 行及 14 个使用文件；`explore 'PhysicalExchangeSender ...'` 定位 sender 的方法、receiver 关系和测试；`explore 'MPPSink ... GenerateRootMPPTasks build_sender'` 给出 `GenerateRootMPPTasks → build_sender` 调用边及 `operator_to_pb` 接线。
- 直接调用证据：`pkg/planner/core/operator/physicalop/fragment.rs` 的 `GenerateRootMPPTasks`/`build_sender`；`physical_exchange_receiver.rs::GetExchangeSender`；`pkg/planner/core/base/task_base.rs::MPPSink`。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_exchange_sender.go`；`pkg/planner/core/operator/physicalop/fragment_test.go`。
- Rust 测试：`physical_exchange_sender_test.rs` 覆盖追加重复任务和空 sender 内存公式；`physical_exchange_sender_aster_unit_test.rs` 覆盖 trait 任务访问、EXPLAIN 与克隆清空任务；`fragment_test.rs` 覆盖 PassThrough singleton、sink 内存委托及共享锁生命周期。
- 本任务为纯文档分析，按计划不运行 Cargo；结构验证另以任务指定命令检查文件存在且恰有十一个固定二级标题。
