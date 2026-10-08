# `pkg/planner/core/operator/physicalop/physical_stream_agg.rs`

## 文件定位

本文件定义物理流式聚合的具体 Rust 类型 `PhysicalStreamAgg`。它属于 Cargo 包 `astersql-planner-core-operator-physicalop`（`pkg/planner/core/operator/physicalop/Cargo.toml`），由同目录 `lib.rs` 声明为 `mod physical_stream_agg` 并公开再导出。`lib.rs` 中的 `agg_operator_core!(PhysicalStreamAgg)` 又把它接入 `base::Plan`、`base::PhysicalPlan` 和该 crate 的 `ConcretePhysicalOperator` 通用接口，因此优化器、任务树和 PB 编码路径能够把它作为一个具体物理计划节点处理。

流式聚合要求输入按分组键有序，以连续扫描相同分组的方式计算聚合；相较 `PhysicalHashAgg`，它不需要为全部分组维护哈希表，但其可用性依赖子计划提供正确顺序。这个文件是薄具体类型层：共享的聚合描述、分组表达式、schema、统计信息、子节点和 MPP 字段都存放在 `BasePhysicalAgg` 中；候选生成、属性筛选、部分/最终聚合拆分等逻辑不在本文件内。

## 核心职责

- 用 `PhysicalStreamAgg { BasePhysicalAgg }` 为公共聚合基座赋予“流式聚合”这一具体类型身份。
- 通过 `Clone` 深拷贝/重绑计划上下文，通过 `MemoryUsage` 暴露公共基座的内存估算。
- 通过 `ToPB` 选择 `tipb::ExecType::TypeStreamAgg`，复用 `aggregate_to_pb` 将分组表达式、聚合函数及必要的 TiFlash 子执行器编码成 protobuf。
- 通过 `GetCost` 提供与 Go v1 局部成本公式一致的 CPU 加 DISTINCT 状态内存估算；通过 `GetPlanCostVer1`、`GetPlanCostVer2` 接入完整计划成本缓存/递归计算。
- 通过 `Attach2Task` 进入 `base::PhysicalPlan::attach_to_task` 的任务树挂接流程。

本文件不负责从逻辑聚合枚举 StreamAgg 候选。Go 同路径文件中的 `getStreamAggs`、`getEnforcedStreamAggs` 没有位于本 Rust 文件；Rust 的构造入口可见 `BasePhysicalAgg::InitForStream`，更高层的属性与任务处理分散在相邻规划代码中。

## 主要符号

- `pub struct PhysicalStreamAgg`：唯一模块级类型，仅公开包含 `BasePhysicalAgg`。没有额外流式状态、常量、trait 定义或条件编译项。
- `GetPointer(&mut self) -> &mut BasePhysicalAgg`：把具体算子降到可变公共聚合基座，供共享逻辑原地更新聚合函数、分组项、schema、统计信息或子属性。
- `Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error>`：调用 `BasePhysicalAgg::CloneWithSelf`，深拷贝聚合相关数据并把表达式/计划换绑到 `new_ctx`；失败原样传播。
- `MemoryUsage(&self) -> i64`：完全委托给 `BasePhysicalAgg::MemoryUsage`。Rust 方法要求有效的 `&self`，没有 Go `nil` receiver 返回零值的分支。
- `ToPB(&self, ctx, store) -> Result<Box<tipb::Executor>, expression::Error>`：调用共享 `aggregate_to_pb`，唯一特化参数是 `TypeStreamAgg`。
- `GetCost(&self, input_rows, is_root, _flag) -> f64`：本文件唯一直接计算行为；`_flag` 当前不参与公式。
- `GetPlanCostVer1` / `GetPlanCostVer2`：委托到 `BasePhysicalPlan`。后者把 `inl: &[bool]` 继续传下去。
- `Attach2Task(&self, tasks) -> Box<dyn Task>`：调用 trait 的 `attach_to_task`；实际动态分派由 `lib.rs` 的宏接线完成。

所有方法都是固有 `impl PhysicalStreamAgg` 的公开方法；公共 `Plan`/`PhysicalPlan` trait 实现由 `lib.rs` 的 `agg_operator_core!` 与 `impl_concrete_physical_plan!` 生成，而不是写在本文件中。

## 执行流程

1. 规划阶段以 `BasePhysicalAgg::InitForStream` 构造节点：设置计划类型为 `StreamAgg`，安装输出 schema 和孩子所需物理属性，再包装成 `PhysicalStreamAgg`。输入有序性的建立与校验属于候选生成/物理属性规划，不由本文件运行时检查。
2. 通用物理计划接口由 `agg_operator_core!(PhysicalStreamAgg)` 转接：schema producer、解释信息、索引解析和相关列提取都落到 `BasePhysicalAgg`；成本、内存和 PB 转换回调到本文件相应方法。
3. 局部 v1 成本计算时，`GetCost` 从计划上下文读取 session variables。根任务使用 `GetCPUFactor()`，cop 任务使用 `GetCopCPUFactor()`；CPU 成本为 `input_rows * cpu_factor * GetAggFuncCostFactor(false)`。
4. 同一公式用 `input_rows / stats_count()` 得到每组平均输入行数，再乘固定 DISTINCT 因子 `0.8`、会话内存因子和 DISTINCT 聚合函数数目；CPU 与内存两项相加返回。
5. 编码执行计划时，`ToPB` 把 `BasePhysicalAgg` 交给 `aggregate_to_pb`。共享函数把 `GroupByItems` 转为 PB 表达式、逐个把 `AggFuncs` 转为聚合 PB 表达式，并创建类型为 `TypeStreamAgg` 的 executor；TiFlash 路径还编码第一个子节点、executor id 与细粒度 shuffle stream 数。
6. 挂接任务时，`Attach2Task` 进入 `PhysicalPlan::attach_to_task`。宏生成的具体 trait 实现把调用路由到通用算子挂接逻辑；相邻 `base_physical_plan.rs` 还处理 StreamAgg 的 root/cop/MPP 边界、可下推性以及 partial/final 聚合构造。

## 数据与状态

`PhysicalStreamAgg` 自身只有一个字段 `BasePhysicalAgg`。后者持有：

- `PhysicalSchemaProducer`，进而包含计划上下文、统计信息、schema、子节点和孩子所需属性；
- `AggFuncs`，即聚合函数描述符列表，包括模式与 DISTINCT 标志；
- `GroupByItems`，即决定流式分组边界的表达式；
- `MppRunMode` 和 `MppPartitionCols`，供相邻 MPP 聚合规划逻辑使用。

本类型没有运行时聚合缓冲区；实际按行聚合发生在由 PB 计划驱动的执行端。本文件也没有内部缓存。完整计划成本缓存位于 `BasePhysicalPlan`，所以 `GetPlanCostVer1/2` 必须在可变借用上转发；`GetCost` 只是纯数值计算，不写状态。

成本公式隐含 `stats_count()` 是有效的输出组数估计。源码没有在这里对零、负数、NaN 或无穷输入做校验：浮点除法会按 Rust/IEEE-754 语义产生无穷或 NaN，并继续进入结果。调用方应提供规划器产生的有效非负基数统计。

## 依赖与调用关系

上游直接证据如下：

- `BasePhysicalAgg::InitForStream`（`base_physical_agg.rs`）是具体构造入口。
- `lib.rs` 声明并再导出模块，`agg_operator_core!(PhysicalStreamAgg)` 为其生成通用计划接口；成本接口会先尝试全局 cost router，再回落到本文件的转发方法。
- `cache_snapshot.rs` 的 `capture`/`restore` 识别并重建该类型；`cache_snapshot_test.rs` 验证包含 StreamAgg 的计划树往返后仍保留分组项和孩子层级。
- `base_physical_plan.rs` 多处以 `downcast_ref::<PhysicalStreamAgg>()` 参与任务挂接、聚合下推和成本分派；MPP root 特例明确让原 StreamAgg 留在 TiDB，而不是按 HashAgg 的通用路径拆到 MPP 子节点。
- `physical_table_reader.rs`、`physical_index_reader.rs` 把 StreamAgg 识别为 reader 内部计划类型之一；执行器运行时测试也直接构造 StreamAgg 计划。

下游依赖包括 `base` 的计划上下文、计划/任务 trait 与 PB 构建上下文，`costusage::{CostVer2, PlanCostOption}`，`property::TaskType`，`kv::StoreType`，`expression::Error` 和 `tipb::Executor`。这些都由 `physicalop/Cargo.toml` 的工作区路径依赖或固定 `tipb` Git revision 提供。

RustCodeGraph 的 `PhysicalStreamAgg` 节点 trail 确认了 `InitForStream`、缓存快照恢复、`physical_stream_agg_test` 和执行器运行时测试的实例化边。当前索引对精确 `callers`/`callees` 命令无输出并超时，因此方法级调用关系以上述节点 trail、模块宏接线及精确源码搜索交叉核验。

## 错误处理与边界

- `Clone` 传播 `CloneWithSelf` 的 `expression::Error`，不吞掉表达式或 schema 克隆失败。
- `ToPB` 传播共享转换函数的全部错误，包括缺少 PB client、分组表达式转换失败、聚合函数不支持目标存储，以及 TiFlash 路径缺少孩子/孩子 PB 编码失败。共享函数当前缺孩子错误文本写的是 “TiFlash hash aggregation requires a child”，即使调用者是 StreamAgg；这是现有真实行为，不应在文档中改写为已特化错误。
- `GetPlanCostVer1/2` 原样传播基础计划的成本计算错误。
- `GetCost` 和 `Attach2Task` 不返回 `Result`。前者不验证统计基数；后者的更底层通用实现对计划克隆使用 `expect` 的路径可能在克隆失败时 panic，具体算子方法本身没有恢复策略。
- `ToPB` 在本文件不检查输入排序，也不拒绝某种 store；合法性应在物理属性选择和挂接/下推阶段建立。
- 本文件没有 `unsafe`、显式 panic、条件编译或静默降级分支。

## 并发与资源生命周期

本类型不创建线程、异步任务、锁、通道、事务或外部句柄。`GetCost`、`MemoryUsage` 和 `ToPB` 只借用现有计划；`GetPlanCostVer1/2` 的可变借用确保同一 Rust 引用下的成本缓存更新是独占的，但类型本身没有声明额外同步保证。

`Clone(new_ctx)` 产生独立拥有的 `PhysicalStreamAgg`，并通过 `CloneWithSelf` 克隆表达式、schema 和子计划所需状态；`ContextRef` 按共享引用语义换绑到传入上下文。`Attach2Task` 接收并消费 `Vec<Box<dyn Task>>`，通用挂接流程克隆孩子计划、建立新的父子关系并返回拥有该计划的 boxed task。PB 编码返回拥有的 `Box<tipb::Executor>`；临时表达式列表和聚合函数 PB 随 executor 所有权移交。

流式聚合的主要资源收益是算法层面无需维护全量分组哈希表；但这个文件只描述计划节点，并不持有执行期的流式状态或内存配额。

## 与 Go 版本的对应关系

同路径 `physical_stream_agg.go` 是直接对照来源。类型字段、`GetPointer`、`Clone`、`MemoryUsage`、`ToPB`、`GetCost`、两个计划成本入口和 `Attach2Task` 在 Rust 中都有对应项，方法命名也保留 Go 风格以方便迁移核对。

主要对应与差异是：

- Rust `ToPB` 与 `PhysicalHashAgg` 共用 `aggregate_to_pb`，并通过 `TypeStreamAgg` 区分 executor；Go 在本文件中展开表达式转换与 executor 构造。可观察目标相同，但 Rust 共享实现还统一写入细粒度 shuffle stream 数。
- Go `GetCost` 转调 `utilfuncp.GetCost4PhysicalStreamAgg`；Rust 在本文件内直接实现相同核心公式。`physical_stream_agg_test.rs::get_cost_matches_go_cpu_and_distinct_memory_accounting` 分别覆盖 root/coprocessor CPU 因子及一个 DISTINCT 聚合的 `0.8` 内存因子。
- Go `GetPlanCostVer1/2` 和 `Attach2Task` 转调 `utilfuncp` 的 StreamAgg 专用函数；Rust 方法表面先委托公共基础计划/trait，具体类型路由和 StreamAgg 分支位于 `lib.rs`、`base_physical_plan.rs` 等相邻模块，不能仅凭本文件断言所有 Go 专用分支都在这里逐行复刻。
- Go `MemoryUsage` 接受 nil receiver 并返回零；安全 Rust 的 `&self` 调用不存在 nil receiver。
- Go 文件还定义 `getStreamAggs` 和 `getEnforcedStreamAggs`，负责 Flash/index join 限制、顺序属性、FinalMode、GROUP BY 表达式形态、DISTINCT 下推和 hint 强制候选等筛选。本 Rust 文件没有这两个函数；这些 Go 行为不能算作本文件已实现。

## 扩展指南

- 若新增只属于 StreamAgg 的计划字段，首先扩展 `PhysicalStreamAgg`，并同步 `Clone`、`MemoryUsage`、缓存快照 `capture/restore`、解释输出和必要的 PB 编码；同时更新 `lib.rs` 的缓存快照契约描述。不要把字段仅放进 Go 对照或测试桩。
- 若改变 PB 格式或存储能力，修改 `ToPB` 或共享 `aggregate_to_pb` 时必须同时评估 HashAgg；至少补充独立测试覆盖 TiKV/TiFlash、缺少 client、缺少 TiFlash child 和不支持的聚合表达式。测试应继续放在独立 `*_test.rs` 文件，不能内嵌到生产源文件。
- 若改变成本公式，修改 `GetCost` 并同步 `physical_stream_agg_test.rs` 的 root/cop、DISTINCT 数量、零 DISTINCT 和统计边界用例；还要核对 Go `GetCost4PhysicalStreamAgg` 的当前语义以及全局 cost router 是否绕过本地公式。
- 若改变候选可用条件或排序要求，应从产生 StreamAgg 的逻辑到物理转换/属性代码入手，而不是只改这个薄包装；重点验证 GROUP BY 顺序、强制 sort、index join、DISTINCT 下推及 TiFlash/MPP 边界。
- 若改变任务拆分或下推，检查 `base_physical_plan.rs` 中所有 `PhysicalStreamAgg` downcast 分支，并补 planner 集成测试和执行器物理计划运行测试。尤其要防止把只适用于 HashAgg 的 MPP partial/final 路径错误复用给 StreamAgg。
- 性能风险主要来自错误的基数/成本导致计划误选、失去输入有序性导致语义错误，以及 PB 共享转换的额外克隆；兼容风险集中在 Go 成本、计划缓存快照和 TiDB/TiKV/TiFlash executor 形状差异。

## 验证依据

已核对的直接文件与符号：

- `physical_stream_agg.rs`：完整类型和八个公开方法；无条件编译项、无额外模块级常量。
- `physicalop/Cargo.toml`：crate 名、`lib.rs` 入口、`base`/`costusage`/`expression`/`kv`/`property`/`tipb` 等依赖，以及 Go 包映射 metadata。
- `physicalop/lib.rs`：模块声明、公开再导出、`agg_operator_core!(PhysicalStreamAgg)` 和缓存快照类型契约。
- `base_physical_agg.rs`：`BasePhysicalAgg` 数据布局、`InitForStream`、克隆/索引解析等共享聚合行为。
- `physical_hash_agg.rs::aggregate_to_pb`：StreamAgg 实际调用的共享 PB 编码实现。
- `base_physical_plan.rs`：trait 默认任务挂接、StreamAgg 的 downcast 分派、root/cop/MPP 与 partial/final 边界。
- `physical_stream_agg.go` 与 `pkg/planner/core/plan_cost_ver1.go::getCost4PhysicalStreamAgg`：Go 类型、候选生成职责和 v1 成本语义对照。
- `physical_stream_agg_test.rs`：root/cop CPU 因子及 DISTINCT 内存成本回归；`cache_snapshot_test.rs`：计划缓存往返；`pkg/executor/physical_plan_runtime_test.rs`：StreamAgg 执行计划构造；`pkg/planner/core/enforce_mpp_test.rs`：TiFlash/MPP StreamAgg 形状证据。

RustCodeGraph 查询包括 `status`、`query PhysicalStreamAgg`、`query physical_stream_agg`、主要方法的 `query/node`；索引包含目标 Rust/Go/测试节点，并在结构节点 trail 中报告 `InitForStream`、缓存恢复与测试实例化边。精确 `callers/callees` 查询在当前索引上超时且没有返回方法级边，故没有据此声称不存在调用者，而是用已返回的 node trail 与上述源码接线补证。

本任务只新增文档，按计划不运行 Cargo。结构验收应确认目标文件存在，且上述十一个固定二级标题各出现且仅出现一次；人工复核重点是区分本文件的薄包装职责、相邻模块的通用接线和 Go 文件尚未归属于本文件的候选生成逻辑。
