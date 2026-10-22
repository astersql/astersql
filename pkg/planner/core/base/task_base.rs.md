# `pkg/planner/core/base/task_base.rs`

## 文件定位

这一分层遵循 `pkg/planner/core/base/doc.go` 的包契约：基础接口应保持抽象、不依赖具体 core 实现，新方法追加到末尾以便实现者对齐。

## 核心职责

1. `Task` 把“物理计划 + 执行形态 + 优化期状态”隔离成对象安全的 trait object，供优化器用 `Box<dyn Task>` 传递、复制、比较行数、访问计划、转为 Root 形态并携带警告。
2. `Task` 同时给 MPP 路径提供分区布局的查询和更新点，让上层 Join/聚合等算子能判断子任务的分区是否满足要求。
3. `INVALID_TASK` 保留 Go 全局无效任务的槽位，但当前 Rust 仅声明为 `None`，且全仓 Rust 搜索未发现初始化或读取点；它不能被当成已可用的公共单例。
4. `MPPSink` 把 ExchangeSender 类算子抽象为 MPP fragment 的数据出口，统一暴露压缩策略、本端任务和目标任务列表。

## 主要符号

- `pub trait Task`：对象安全的物理任务接口。`count` 返回估算行数；`copy` 返回新的 trait object；`plan`/`plan_mut` 分别给出不可变/可变物理计划；`invalid` 判断候选是否可用；`convert_to_root_task` 接收 `ContextRef` 并返回 Root 执行形态；`memory_usage` 估算字节数；`append_warning` 保存非致命 `crate::Error`。
- `Task::mpp_partition_type`：默认返回 `property::AnyType`，表示没有对父算子声明更强的分区保证。
- `Task::mpp_hash_cols`：默认返回空 `Vec<MPPPartitionColumn>`，每次调用交付独立所有权。
- `Task::set_mpp_partition`：默认为 no-op；只有需要保存 MPP 元数据的实现者才覆写。
- `pub static mut INVALID_TASK: Option<Box<dyn Task>>`：未同步的全局可变槽位，初值为 `None`。对它的任何安全使用都必须由后续接线层额外解决初始化时序、并发和对象访问问题。
- `pub trait MPPSink: crate::PhysicalPlan`：所有 sink 必须同时是物理计划。`get_compression_mode` 返回 Exchange 压缩模式；`get_self_tasks` 借用内部任务切片；`set_self_tasks`/`set_target_tasks` 整体替换有序列表；`append_target_tasks` 按输入顺序追加。

## 执行流程

`Task` 的实际使用主链在 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 中可见：

1. 物理算子用 `RootTask::New` 或 `RootTask::NewWithMpp` 构造 `Box<dyn Task>`，并在向上挂接算子时携带子计划和来源任务。
2. 候选生成路径先用 `invalid()` 过滤失效结果，用 `count()` 读取基数，必要时通过 `plan_mut()` 对已选物理树做局部调整。`pkg/planner/core/optimizer_runtime.rs` 也在选择候选时调用 `invalid()`。
3. MPP 路径在多个算子挂接点读取 `mpp_partition_type()` 和 `mpp_hash_cols()`，与子属性要求比较；当 Exchange 或其他算子改变数据分布时，调用 `set_mpp_partition()` 记录新布局。
4. 需要回到 TiDB 本地执行时，算子调用 `convert_to_root_task(context)`。当前生产实现 `physicalop::RootTask` 对该操作返回自身的深度独立复制。

MPP fragment 调度链是另一条直接流程：`pkg/planner/core/operator/physicalop/fragment.rs::Fragment::New` 接收 `Box<dyn base::MPPSink>` 并放入 `Arc<Mutex<_>>`；fragment 生成器通过 sink trait 读写本端与目标 `kv::MPPTask`。生产实现位于 `physical_exchange_sender.rs::impl MPPSink for PhysicalExchangeSender`，各 trait 方法转发给 ExchangeSender 的固有方法。

## 数据与状态

`Task` 本身不持有字段，但它约束实现者同时管理四类状态：当前物理计划、估算行数/失效标记、非致命警告和 MPP 分区元数据。当前唯一搜索到的生产 Rust 实现 `pkg/planner/core/operator/physicalop/task.rs::RootTask` 保存 `plan`、可选 `source`、`warnings`、Root 残余条件/选择率及 `mpp_partition_type`/`mpp_hash_cols`。其 `copy()` 会克隆物理计划、来源任务、警告、表达式和 MPP 列，因此当前实现并不是源文件注释所说的“共享同一物理计划指针”式浅拷贝；扩展者必须以实现代码为准。

`MPPSink` 的任务列表语义是有序的：`set_*` 获取 `Vec` 所有权并替换原列表，`append_target_tasks` 保留现有目标并追加。`get_self_tasks` 返回借用切片，不允许调用者绕过 setter 直接改写。

## 依赖与调用关系

- 上游入口：`pkg/planner/core/base/lib.rs` 将本模块公开符号重导出；`plan_base.rs::PhysicalPlan::attach_to_task` 以 `Vec<Box<dyn Task>> -> Box<dyn Task>` 把 Task 纳入物理算子挂接契约。
- 主要实现：`pkg/planner/core/operator/physicalop/task.rs::impl Task for RootTask`；`pkg/planner/core/operator/physicalop/physical_exchange_sender.rs::impl MPPSink for PhysicalExchangeSender`。RustCodeGraph 对本文件限定的 `Task` 节点确认其位于第 26 行并包含 11 个方法。
- `Task` 主要消费者：`base_physical_plan.rs` 的各算子构造/挂接路径，`physical_apply.rs`、`physical_projection.rs` 等算子的 Root 转换，以及 `optimizer_runtime.rs` 的失效候选检查。
- `MPPSink` 主要消费者：`fragment.rs::Fragment` 和 MPP fragment 生成流程；下游数据类型是 `kv::MPPTask`和 `vardef::ExchangeCompressionMode`。
- `INVALID_TASK` 的 Go 上游接线在 `pkg/planner/core/core_init.go`，Rust 当前无对应调用边。因此不应为它推断 Rust 优化器中的实际生命周期。

## 错误处理与边界

本文件没有 `Result` 返回值，也不主动产生错误。`append_warning(crate::Error)` 专门用于携带可继续优化的非致命诊断；致命错误由调用者所在的计划构建流程传播。`invalid()` 是另一条非异常失败通道，表示“此候选不可选”，不等同于系统错误。

默认 MPP 方法是重要边界：未覆写时会报告 `AnyType`/空 hash 列并忽略 setter。这一默认值能让非 MPP 任务实现 trait，但如果新 MPP 任务忘记覆写，上层将丢失分区保证或插入多余 Exchange。`plan()`/`plan_mut()` 无 `Option`，所以实现者不得在“无计划”状态下假造引用；应让 `invalid()` 与可访问的存储结构保持一致。

## 并发与资源生命周期

`Task` 和 `MPPSink` 都没有 `Send`/`Sync` 超约束，本文件本身不承诺跨线程可用性。`Box<dyn Task>` 表达单一所有者，`copy()` 由实现者决定内部克隆语义；不能仅根据 trait 签名假定共享或深拷贝。`get_self_tasks()` 的切片借用期绑定于 sink，而获取所有权的 setter 明确划分了旧/新任务列表的销毁时点。

`Fragment` 在本 trait 之外用 `Arc<Mutex<Box<dyn MPPSink>>>` 提供共享和互斥访问；`fragment_test.rs::fragment_clone_shares_the_same_underlying_sink` 验证 clone 共享同一 `Arc`。因此并发安全边界是 fragment 容器的责任，不是 `MPPSink` trait 自身的保证。

`INVALID_TASK` 是特殊风险：`static mut` 需要 `unsafe` 读写，且当前没有锁、一次性初始化器或已证明的调用点。在引入真实接线前，应优先重设计为可安全初始化的共享状态，而不是直接读写该槽位。

## 与 Go 版本的对应关系

Rust 额外加入了三个 MPP 分区方法和 `plan_mut`；它们并非当前 Go base `Task` 接口的对应成员，而是 Rust 物理算子接线所需的局部扩展。

Rust `INVALID_TASK` 仍是未接线槽位，Rust 优化路径主要通过 `Task::invalid()` 和 `Option`/`Result` 表达候选失败，不应声称该全局单例已完成移植。

差异是 Go 使用 `[]*kv.MPPTask`，Rust 使用值语义的 `Vec<kv::MPPTask>` 和借用切片 `&[kv::MPPTask]`；顺序、整体替换和追加契约保持一致。

实现层尚未与 Go 完全同形：Go `physicalop/task_base.go` 显式由 `RootTask`、`MppTask`、`CopTask` 实现 base `Task`；当前 Rust 搜索只找到 `physicalop/task.rs::RootTask` 实现本 base trait，Cop 状态由辅助结构处理，MPP 元数据被收纳到 `RootTask` 中。

## 扩展指南

- 增加 `Task` 方法前，先验证能否放在具体实现或扩展 trait；若确需新抽象，按 `doc.go` 契约追加到末尾，同步更新 `physicalop/task.rs::impl Task for RootTask` 和所有后续新实现。若要保持 Go 公共契约对齐，同时评估 `task_base.go` 是否应有对应方法。
- 新增 MPP 任务实现时，必须显式覆写三个 MPP 方法，并验证 hash 列克隆、分区比较、Exchange 插入与 setter 更新后的读回。默认 no-op 不适合需要传播分区保证的实现。
- 扩展 `MPPSink` 时，同步修改 `PhysicalExchangeSender` 的 trait impl、`Fragment` 调度路径和 `physical_exchange_sender_aster_unit_test.rs`；保留任务顺序、set 替换/append 追加以及 clone 不携带已调度任务的现有契约。
- 若要启用 `INVALID_TASK`，不应直接增加零散 `unsafe` 读写；应先确定安全的一次性初始化与共享模型，补上独立 Rust 回归测试，并核对 Go `core_init.go` 和所有哨兵比较场景。
- 与本抽象直接相关的测试应保持在独立文件：Task 实现回归放在 `pkg/planner/core/operator/physicalop/task_test.rs` 或专用的 `*_test.rs`，MPPSink 契约放在 `physical_exchange_sender_aster_unit_test.rs`，fragment 共享/内存行为放在 `fragment_test.rs`；不把测试嵌入本生产文件。
