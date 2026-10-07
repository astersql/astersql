# `pkg/dxf/framework/planner/plan.rs`

## 文件定位

本文件说明的源码是 [`plan.rs`](./plan.rs)。它属于 `astersql-dxf-framework-planner` crate，是 DXF（Distributed eXecution Framework）规划层的通用数据模型与抽象契约。crate 根 `pkg/dxf/framework/planner/lib.rs` 以 `pub mod plan` 声明本模块，并通过 `pub use plan::*` 向 crate 使用者重新导出这里的类型；同一 crate 的 `planner.rs` 则消费 `LogicalPlan` 与 `PlanCtx`，完成任务 meta 序列化和任务创建。

它描述的是“逻辑计划 → 任务 meta / 物理计划 → 按 step 的子任务 meta”这条转换链，不负责执行子任务、调度节点、验证 DAG 或持久化任务。对应 Go 基线是 `pkg/dxf/framework/planner/plan.go`。Cargo 元数据还把 Go 包路径登记为 `pkg/dxf/framework/planner`。

## 核心职责

1. `PlanCtx` 汇集一次规划所需的任务身份、资源限制、阶段衔接数据、会话与存储句柄。
2. `LogicalPlan` 规定逻辑计划的任务级 meta 编解码、额外参数读取及物理计划生成接口。
3. `PhysicalPlan` 保存有序的 `ProcessorSpec` 列表，并由 `to_subtask_metas` 选出指定 step 的处理器，逐个委托其 pipeline 生成子任务 meta。
4. `ProcessorSpec`、`InputSpec`、`OutputSpec` 与 `LinkSpec` 表达处理器节点和节点间链接；本文件只承载规格，不解释或执行链接。
5. `PipelineSpec` 定义从规划上下文生成单个子任务 meta 的最小动态派发边界。

当前实现应理解为规划协议层，而不是完整 DAG 引擎：`PhysicalPlan` 不检查处理器 ID 唯一性、边是否存在、是否成环，也不依据 input/output link 做拓扑排序。

## 主要符号

- `PlanStore`：条件类型别名。启用 `kv-runtime` feature 时是 `Arc<dyn astersql_kv::Storage>`；否则是 `Arc<dyn Any + Send + Sync>`。两种形式都以 `Arc` 表示共享所有权，但无 feature 时只保留类型擦除句柄，不能直接调用 KV API。
- `PlannerError = storage::Error`：统一逻辑计划、pipeline 与物理计划导出的错误边界，和任务存储 crate 使用同一种错误类型。
- `PlanCtx`：可克隆的规划快照。重要字段包括任务标识（`task_id`、`task_key`、`task_type`）、资源约束（`thread_count`、`max_node_count`、`execute_nodes_count`）、阶段状态（`previous_subtask_metas`、`next_task_step`）、租户/路径信息（`keyspace`、`global_sort`）以及 `session_ctx`、`store`。
- `PlanCtx::default`：提供全零/空值构造；`ctx` 当前实际类型是 storage crate 的 `()` 占位，`task_type` 和 step 也分别以空字符串与 `0` 初始化。默认值适合测试和后续填充，不代表可直接提交的有效任务。
- `LogicalPlan`：公开 trait，包含 `get_task_extra_params`、`to_task_meta`、`from_task_meta`、`to_physical_plan` 四个方法。trait 本身未声明 `Send`、`Sync` 或对象生命周期约束。
- `PhysicalPlan`：只含公开的 `processors: Vec<ProcessorSpec>`；`Default` 创建空列表。`add_processor` 追加节点，`processors` 返回只读切片，`to_subtask_metas` 执行 step 过滤与 meta 生成。
- `ProcessorSpec`：一个处理器节点，含 `id`、`input`、动态 pipeline、`output` 和 `step`。`ProcessorSpec::new` 只设置 ID、step、pipeline，输入输出使用空默认值。
- `InputSpec` / `OutputSpec` / `LinkSpec`：可克隆、可比较的纯数据规格。输入包含列类型编码与上游链接，输出包含下游链接；每条链接目前只有对端 `processor_id`。
- `PipelineSpec`：公开 trait，仅有 `to_subtask_meta(PlanCtx)`；返回一个 pipeline 对应的字节 meta 或 `PlannerError`。

## 执行流程

逻辑计划的上游流程由 `pkg/dxf/framework/planner/planner.rs::Planner::run` 展示：调用方提供 `PlanCtx`、`&dyn LogicalPlan` 和任务创建器；`run` 先调用 `LogicalPlan::to_task_meta`，再解析 target scope，最终把 `PlanCtx` 中的任务字段、`get_task_extra_params` 的结果和任务 meta 交给存储边界。`from_task_meta` 与 `to_physical_plan` 是为任务恢复/后续阶段规划预留的对称契约，但 `Planner::run` 本身不调用它们。

物理计划导出由 `PhysicalPlan::to_subtask_metas(context, step)` 完成：

1. 以全部处理器数量作为结果 `Vec` 的初始容量。
2. 按 `processors` 的插入顺序线性扫描。
3. 跳过 `processor.step != step` 的节点。
4. 对每个匹配节点克隆同一份 `PlanCtx`，调用 `processor.pipeline.to_subtask_meta`。
5. 成功的 meta 按处理器插入顺序加入结果；任一调用失败时立即用 `?` 返回该错误，不再处理后续节点，也不返回此前已生成的部分结果。
6. 没有匹配节点时返回空列表，而不是错误。

`pkg/dxf/framework/planner/plan_test.rs` 证明了 step 过滤、插入顺序和上下文 `task_key` 的传递；`pkg/dxf/framework/mock/migration_aster_unit_test.rs::plan_mock_forwards_plan_context_to_pipeline_handler` 进一步验证 mock pipeline 不丢失上下文字段。

## 数据与状态

本文件自身不维护全局状态。`PhysicalPlan` 的唯一可变状态是 `processors` 向量；只有持有 `&mut PhysicalPlan` 的调用方才能通过 `add_processor` 修改它，`processors()` 仅暴露只读视图。列表顺序是可观察语义，因为它决定同一 step 的子任务 meta 顺序。

`PlanCtx::clone` 的成本并非完全常数：`String`、`HashMap<Step, Vec<Vec<u8>>>` 及其中字节数组会被克隆；`session_ctx` 按其 `Clone` 语义复制；`store` 中的 `Arc` 只增加引用计数。因此，同一调用中的各 pipeline 获得内容相同但可独立拥有的上下文值，不能通过修改某个 clone 的普通字段影响其他 pipeline。若底层 `Arc` 指向的对象具有内部可变性，则底层资源仍然共享。

输入/输出 link 目前只是描述数据。`column_types` 是不透明字节，`processor_id` 是 `i32`；本文件不校验编码格式、ID 范围、链接对称性或处理器存在性。`task_id`、step、slot/node 数等也不在这里做业务合法性检查。

## 依赖与调用关系

- 上游模块：`lib.rs` 重新导出全部符号；`planner.rs::Planner::{run, run_with_target_scope, create}` 使用 `PlanCtx` 和 `dyn LogicalPlan`。`pkg/dxf/framework/mock/plan_mock.rs` 为 `LogicalPlan`、`PipelineSpec` 提供可注入 handler 的测试替身。
- 明确的核心调用边：`PhysicalPlan::to_subtask_metas` → `PipelineSpec::to_subtask_meta`。RustCodeGraph 对此边有记录；动态 trait 实现由运行时具体 pipeline 决定。
- 协议依赖：`astersql_dxf_framework_proto` 提供 `TaskType`、`Step`、`ExtraParams`。
- 存储依赖：`astersql_dxf_framework_storage` 提供 `Error`、规划 `Context` 和 `sessionctx::Context`。当前 storage `Context` 在 `pkg/dxf/framework/storage/lib.rs` 中是 `()` 占位类型，因此源码注释所述超时/取消语义尚不能由这个具体类型承载。
- 可选 KV 依赖：Cargo feature `kv-runtime` 启用可调用的 `astersql-kv::Storage` trait 对象；默认 feature 为空，默认构建只提供类型擦除的 `PlanStore`。
- crate 使用边界：`pkg/dxf/framework/mock/Cargo.toml` 和 `pkg/dxf/importinto/Cargo.toml` 声明对本 crate 的依赖；mock 有实际 trait 实现。当前 `pkg/dxf/importinto/planner.rs` 定义的是另一套同名 `PlanCtx`、`LogicalPlan`、`PhysicalPlan` 和 `PipelineSpec`，精确检索未发现它实现或调用本文件的 trait/方法，不能把两套类型视为已经接通。

## 错误处理与边界

所有可失败接口都返回 `Result<_, PlannerError>`，而 `PlannerError` 直接别名到 `storage::Error`，没有在本层增加错误包装或上下文。`to_subtask_metas` 保留 pipeline 的原始错误，并采用首错即停策略；此前生成的 meta 位于局部变量中，会随错误返回被丢弃。

`LogicalPlan` 的序列化、反序列化和物理计划生成错误完全由实现者定义。本文件不验证 task meta 格式，也不保证 `from_task_meta` 失败后接收者保持原值；实现者必须自行定义并测试是否采用事务式更新。

主要边界还包括：空物理计划或无匹配 step 合法返回空结果；重复 processor ID、悬空 link、环、负数 ID/资源数均不会在本层报错；`ProcessorSpec::new` 不自动连接节点。调用方或更高层实现必须承担这些不变量。

## 并发与资源生命周期

`PhysicalPlan::to_subtask_metas` 是同步、单线程、串行调用；pipeline 的调用顺序稳定，不存在本文件创建的线程、异步任务、锁、通道或事务。方法借用 `&self`，调用期间处理器列表不能通过安全 Rust 并发修改。

`PipelineSpec` 与 `LogicalPlan` 未要求 `Send + Sync`，`PhysicalPlan` 又持有 `Box<dyn PipelineSpec>`，因此该抽象没有承诺计划可在线程间传递或共享。若未来需要并行生成 meta，应先明确 trait 的线程安全约束、输出顺序、首错/多错策略，并评估 `PlanCtx` 深克隆的内存开销，不能只把循环替换为并行迭代。

`store` 使用 `Arc` 延长底层句柄生命周期；最后一个 clone 释放时才释放该句柄。其余上下文字段由每次 `PlanCtx` 值拥有并在值离开作用域时释放。pipeline 以 `Box` 归 `ProcessorSpec` 独占，物理计划销毁时随节点一起销毁。

## 与 Go 版本的对应关系

`pkg/dxf/framework/planner/plan.go` 与本文件具有直接的结构对应：Go `PlanCtx`、`LogicalPlan`、`PhysicalPlan`、四种 spec/link 类型及 `PipelineSpec` 都有 Rust 对应物；`AddProcessor` 和 `ToSubtaskMetas` 的过滤、顺序与首错返回语义一致。Go 测试 `plan_test.go::TestPhysicalPlan` 的核心断言由 Rust 独立测试 `plan_test.rs::physical_plan_calls_matching_pipeline_and_returns_meta` 保留，Rust 还增加了多节点过滤和稳定顺序覆盖。

需要注意的迁移差异：

- Go 的 `context.Context` 与 `sessionctx.Context` 是实际接口；Rust 当前的 storage `Context` 是 `()`，会话上下文也由 storage 适配层提供，能力并不等同于 Go 运行时对象。
- Go 的 `kv.Storage` 可直接调用；Rust 只有启用 `kv-runtime` 才暴露 `dyn astersql_kv::Storage`，默认构建使用 `Any + Send + Sync` 占位。
- Go 的计数和处理器 ID 使用平台相关 `int`；Rust 固定为 `i32`，跨边界转换需要检查溢出。
- Go `ToPhysicalPlan` 返回 `*PhysicalPlan`；Rust 返回拥有所有权的 `PhysicalPlan`。Go 将 `PlanCtx` 按值传给 pipeline 时，其中 map/interface 仍是引用语义；Rust 明确对每个匹配 pipeline 调用 `context.clone()`，字符串、map 和字节容器会复制，`Arc` 句柄共享。
- Rust 增加了 `ProcessorSpec::new` 和只读 `PhysicalPlan::processors()` 便捷 API；Go 侧通常使用结构体字面量和公开字段。
- Go 注释把该结构称为 DAG，但 Go 与 Rust 当前都没有在这个文件内实施无环或连通性验证。

## 扩展指南

- 新增规划上下文字段时，应修改 `PlanCtx` 与 `Default`，核对 `planner.rs::Planner::create` 是否需要把字段送入持久化边界，并同步 Go `PlanCtx`、mock 转发测试及所有结构体字面量。大字段会被每个匹配 pipeline 克隆，必须评估内存和延迟。
- 新增逻辑计划生命周期操作时，应同时更新 `LogicalPlan`、`pkg/dxf/framework/mock/plan_mock.rs`、`planner_test.rs` 中的测试实现和 Go 接口；这是破坏所有 trait 实现者的 API 变更。
- 扩展 processor/link 表达能力时，优先保持 `ProcessorSpec::new` 的默认行为与公开字段兼容，并在独立测试文件 `plan_test.rs` 增加拓扑/排序/非法输入用例。若真正引入 DAG 校验，应明确校验发生在追加时还是导出时，以及重复 ID、悬空边和环分别返回什么错误。
- 修改 `to_subtask_metas` 时必须保留或有意变更三项可观察契约：仅处理目标 step、保持插入顺序、遇首个 pipeline 错误立即停止。还应新增失败 pipeline 的回归测试，因为当前 Rust 测试尚未直接覆盖错误短路。
- 若让计划跨线程或并行生成 meta，需要给 trait 对象补充适当的 `Send`/`Sync` 约束，定义确定性输出顺序，并避免无意共享可变底层资源。
- 若要把 Import Into 接到这套通用协议，应显式实现本文件的 trait 或增加适配层；不能仅依赖同名类型。变更需联合检查 `pkg/dxf/importinto/planner.rs` 及其独立测试，而不是删除其现有完整逻辑。
- Rust 测试必须继续放在同目录的独立 `plan_test.rs`，不要内嵌回生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；目标 `plan.rs` 被识别为 202 行、19 个符号，并显示被 7 个文件引用。
- RustCodeGraph 源码与符号查询：`node --file pkg/dxf/framework/planner/plan.rs`；`query PhysicalPlan --kind struct`、`query PlanCtx --kind struct`、`query LogicalPlan --kind trait`、`query PipelineSpec --kind trait`、`query to_subtask_metas`。
- RustCodeGraph 调用证据：`callees` 对 `to_subtask_metas` 记录到 `PipelineSpec::to_subtask_meta`；动态 callers 结果不完整，因此又用精确文本检索核对实现和接线点。
- 已读生产与边界文件：`pkg/dxf/framework/planner/plan.rs`、`lib.rs`、`planner.rs`、`Cargo.toml`，以及 `pkg/dxf/framework/storage/lib.rs`、`pkg/dxf/framework/mock/plan_mock.rs`、`pkg/dxf/importinto/planner.rs` 的直接相关定义/引用。
- 已读 Go 对照：`pkg/dxf/framework/planner/plan.go`。
- 已读独立测试：`pkg/dxf/framework/planner/plan_test.rs`、`plan_test.go`，以及 mock 上下文转发测试 `pkg/dxf/framework/mock/migration_aster_unit_test.rs::plan_mock_forwards_plan_context_to_pipeline_handler`。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验用于确认目标文件存在且恰有规定的十一个二级章节。
