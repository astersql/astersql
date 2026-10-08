# `pkg/planner/core/operator/physicalop/physical_union_scan.rs`

## 文件定位

本文对应源码 [`physical_union_scan.rs`](./physical_union_scan.rs)。该文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`，包入口在 `pkg/planner/core/operator/physicalop/lib.rs` 中声明并公开重导出 `physical_union_scan` 模块。它实现逻辑 `LogicalUnionScan` 对应的物理计划节点 `PhysicalUnionScan`：该节点位于规划器与执行器之间，用来描述“读取底层快照后，还必须合并当前事务内未提交行”的 root 侧算子。

物理计划枚举的统一路由位于 `base_physical_plan.rs`：识别到 `logicalop::LogicalUnionScan` 后调用本文件的 `ExhaustPhysicalPlans4LogicalUnionScan`。执行阶段则由 `pkg/executor/physical_plan_runtime.rs` 对 `PhysicalUnionScan` 做类型分派，分别取得子节点快照行和 `PhysicalTableSource::UnionScanRows` 提供的事务本地行，再按 handle 合并。

## 核心职责

1. 用 `PhysicalUnionScan` 保存公共物理计划状态、过滤条件 `Conditions` 和唯一定位行的 `HandleCols`。
2. 由 `ExhaustPhysicalPlans4LogicalUnionScan` 将逻辑节点转换为单个物理候选，并拒绝 TiFlash/MPP 属性，因为 UnionScan 必须在 TiDB/root 侧观察事务本地状态。
3. 提供构造、初始化、克隆、相关列提取、 Explain、索引解析、任务附着、代价委托、禁止下推及内存估算等物理计划协议实现。
4. 保持条件表达式、handle 列和 schema 在规划、缓存克隆、索引解析及执行之间一致；真正的行过滤与合并算法不在本文件，而在 `pkg/executor/physical_plan_runtime.rs`。

## 主要符号

- `pub struct PhysicalUnionScan`：一元物理算子。`PhysicalSchemaProducer` 承载上下文、计划 ID、schema、统计、孩子和孩子所需属性；`Conditions: Vec<ExprBox>` 是需要作用于快照行和事务本地行的过滤条件；`HandleCols: Box<dyn HandleCols>` 描述整数 handle 或 common handle 的列集合。
- `PhysicalUnionScan::New(ctx)`：创建 `TypeUnionScan` 基础计划，条件为空，handle 默认为 `IntHandleCols::default()`。
- `PhysicalUnionScan::Init(ctx, stats, offset, props)`：在已有对象上写入上下文、类型、 query block offset、统计和孩子所需属性，不重新构造对象。`physical_union_scan_test.rs::init_reuses_constructor_plan_id_like_go_init` 验证计划 ID 不会被二次分配。
- `Clone(new_ctx)`：通过 `CloneWithNewCtx` 克隆基础计划，复制可选 schema，逐项 `CloneExpr` 条件，并调用 `CloneHandleCols` 深拷贝 handle 描述；失败类型是 `expression::Error`。
- `ExtractCorrelatedCols()`：遍历全部条件，以 `expression::ExtractCorColumns` 提取并克隆相关列。
- `ExplainInfo()` / `ExplainNormalizedInfo()`：分别使用带求值上下文的排序表达式说明和归一化排序说明，输出稳定的 UTF-8 lossy 字符串。
- `ResolveIndices()`：先解析公共 schema producer，再以第一个孩子的 schema 重写每个条件和 `HandleCols` 的列下标；没有孩子时只完成公共部分并返回成功。
- `Attach2Task(tasks)`：委托 `base::PhysicalPlan::attach_to_task`。具体 UnionScan 任务接线会将孩子转为 root task，并维护 Projection/Selection 的相对位置，证据见 `pkg/planner/core/task.rs::attach2Task4PhysicalUnionScan`。
- `GetPlanCostVer1` / `GetPlanCostVer2`：把代价计算交给 `BasePhysicalPlan`；本文件不另加 UnionScan 专属代价项。
- `ToPB(...)`：固定返回错误，明确禁止将此算子编码并下推到存储层。
- `MemoryUsage()`：累计 schema producer、条件 `Vec` 头、各表达式以及 handle 描述的内存估算。
- `ExhaustPhysicalPlans4LogicalUnionScan(logical, property)`：从逻辑节点产生物理候选的模块级入口。

## 执行流程

规划阶段的主流程如下：

1. `base_physical_plan.rs` 的逻辑到物理路由识别 `LogicalUnionScan`，调用 `ExhaustPhysicalPlans4LogicalUnionScan`。
2. 若请求属性 `IsFlashProp()`，函数通过会话变量发出 MPP enforced 警告并返回空候选；UnionScan 不进入 TiFlash/MPP。
3. 若逻辑节点缺少 `SCtx`，返回空候选。否则克隆物理属性的必要字段，并通过 `AdmitIndexJoinProp` 检查/调整 index join 属性；不被接受时也返回空候选。
4. 使用上下文创建 `PhysicalUnionScan`，深克隆逻辑条件与 handle 列，复制逻辑 schema；随后 `Init` 写入逻辑统计、 query block offset 和唯一孩子所需属性，返回一个物理候选。
5. 任务附着时，UnionScan 被放到 root task。`task.rs::attach2Task4PhysicalUnionScan` 对 `Selection(Projection(x))` 和以 Projection 为根的孩子进行重排，确保 Projection 最终仍在 UnionScan 之上，同时保留 task warnings 与 index-join 标记。
6. 索引解析时，条件表达式和 handle 列都相对子节点 schema 转换成运行时列下标。
7. 执行器在 `physical_plan_runtime.rs::execute_node` 或 `stream_rows_while` 中先执行唯一孩子取得快照行，再调用 `UnionScanRows` 取得事务本地行；两侧应用同一组 `Conditions`，按 `HandleCols` 排序合并，相同 handle 由本地脏行覆盖快照行。底层直接表扫描为降序时，合并顺序也随之反转。

## 数据与状态

`PhysicalUnionScan` 自身没有事务缓冲区，也不拥有行数据。它保存的是不可序列化到存储执行器的计划元数据：公共物理计划状态、表达式对象和 handle 列描述。事务本地行由执行期的 `PhysicalTableSource::UnionScanRows` 提供，因此本文件只规定“如何描述合并”，不负责读取事务写缓冲。

`Conditions` 的顺序在克隆和解析时保持不变；Explain 为获得稳定展示会调用排序说明函数，但不会改写原向量。`HandleCols` 是 trait object，可表示整数或多列 common handle；执行器依次比较 `IterColumns()` 返回的列。schema 来自逻辑节点，解析列下标时以第一个物理孩子的 schema 为准。

`Init` 会覆盖上下文、类型、offset、统计及孩子属性，但保留 `New` 已分配的计划 ID。缓存快照支持也会保存/恢复条件和 handle；`cache_snapshot_test.rs` 验证 round trip 后条件数量、handle 唯一 ID 和孩子结构不变。

## 依赖与调用关系

上游入口：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 将 `LogicalUnionScan` 路由到 `ExhaustPhysicalPlans4LogicalUnionScan`。
- `logicalop::LogicalUnionScan` 提供上下文、条件、handle、schema、统计和 query block offset。
- `pkg/planner/core/operator/physicalop/lib.rs` 通过 `direct_operator_core!(PhysicalUnionScan, PhysicalSchemaProducer)` 接入统一 `PhysicalPlan` trait，并公开模块符号。

本文件的直接依赖：

- `base`：计划上下文、`Plan`/`PhysicalPlan`/`Task`、PB 构建上下文。
- `expression`：表达式克隆、相关列提取、Explain、索引解析和错误类型。
- `planner_util::HandleCols`：handle 抽象及克隆、解析、内存统计。
- `property`、`costusage`：统计、物理属性、task 类型和两个版本的代价返回值。
- `logicalop`：逻辑 UnionScan；`plancodec`：节点类型；`kv`/`tipb`：下推接口签名。

下游消费者：

- `pkg/executor/physical_plan_runtime.rs` 读取 `Conditions` 与 `HandleCols` 完成过滤和合并。
- `pkg/planner/core/task.rs` 处理 root task 附着和投影重排。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs` 捕获并恢复此节点；`statement_ru_plan_walk.rs` 和代价遍历也按具体类型识别它。

`Cargo.toml` 将本模块放在独立 physicalop crate 内，并以路径依赖连接 `base`、`logicalop`、`expression`、`property`、`planner_util`、`costusage`、`kv` 等；PB 类型来自固定 revision 的 `tipb` Git 依赖。本文件没有条件编译项或 feature 专属分支。

## 错误处理与边界

- MPP/TiFlash 属性不是错误返回：会记录 enforced-MPP 警告并给出空候选。无计划上下文或 `AdmitIndexJoinProp` 拒绝属性时同样返回空候选，因此调用方必须把“没有候选”当作规划不可行分支。
- `Clone`、`ResolveIndices`、代价委托和 `ToPB` 使用 `expression::Error`。表达式或 handle 索引解析失败会立即传播；此前已经成功解析的较早元素不会回滚，独立的 `resolve_indices_test.rs::union_scan_keeps_earlier_condition_resolution_on_error` 记录了这一逐项更新语义。
- `ResolveIndices` 缺少孩子时返回 `Ok(())`，不会尝试解析条件/handle；真正执行时 `physical_plan_runtime.rs::one_child` 会将缺少或多余孩子报告为运行时错误。
- `ToPB` 总是返回“root-only、不可下推”错误，这是防止错误越过事务可见性边界的硬约束。
- 执行器会拒绝负 handle 下标、越过行宽的 handle 列，并传播条件求值/布尔转换错误；SQL NULL 条件按不保留处理。这些错误属于下游执行实现，不由本文件吞掉。
- Rust `MemoryUsage` 以有效对象为前提；不同于 Go 指针方法，它没有 nil receiver 分支。默认 handle 始终存在，因此无需 Go 中的 nil 检查。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、网络连接或事务，也不直接管理事务提交/回滚。计划对象在构建阶段通过 `&mut self` 或消费 `self` 完成初始化和索引解析，之后执行器通常以共享引用读取；线程安全能力由 `ContextRef`、表达式对象、`HandleCols` 及 `PhysicalPlan` trait 的实现共同约束。

资源生命周期主要体现为所有权：`Conditions`、`HandleCols` 和 `PhysicalSchemaProducer` 随节点释放；`Clone` 为新上下文建立独立表达式与 handle 副本，避免计划缓存或并行使用时共享可变列下标状态。执行阶段产生的快照行、本地行和合并结果由执行器持有，不驻留在 `PhysicalUnionScan` 中。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/physical_union_scan.go`。两版都包含公共物理计划、`Conditions`、`HandleCols`，都拒绝 Flash 属性、通过 `admitIndexJoinProp` 传递孩子属性，并实现相关列提取、Explain、内存估算、任务附着和索引解析。

需要注意的表达差异：

- Go `ExhaustPhysicalPlans4LogicalUnionScan` 返回 `([]base.PhysicalPlan, bool, error)`；Rust 返回 `Vec<Box<dyn PhysicalPlan>>`，以空向量表示此处无候选，未携带 Go 的 `hintCanWork` 与 error 通道。Rust 额外防守 `SCtx()` 缺失。
- Go 构造字面量后调用 `Init`，Rust 分为 `New` 与消费式 `Init`；测试证明 Rust 不因这两步重复分配计划 ID。
- Go 条件赋值在源码中直接复用 slice/接口值；Rust 在逻辑转物理及 `Clone` 中显式深克隆表达式和 handle，以满足所有权和独立可变索引状态。
- Go 的任务附着与索引解析通过 `utilfuncp` 接线；Rust 本文件暴露方法并由 trait/任务框架委托到对应实现。Projection 重排和逐项索引解析语义有独立 Rust 测试覆盖。
- Rust 显式提供 `ExplainNormalizedInfo`、两个代价委托和拒绝 `ToPB` 的实现；Go 文件中的相应公共能力部分来自嵌入的 `BasePhysicalPlan` 或仓库其他接线，而非都写在该文件内。
- Go `MemoryUsage` 支持 nil receiver 和 nil `HandleCols`；Rust 值始终存在，并将 `Vec<ExprBox>` 头、表达式和 handle 的估算全部计入。`physical_union_scan_test.rs::memory_usage_includes_go_conditions_slice_header` 验证空条件时的基线。

## 扩展指南

- 新增计划字段时，应同时更新 `PhysicalUnionScan`、`New`/`Init`、`Clone`、`MemoryUsage`、Explain（若用户可见）、`cache_snapshot.rs` 的 capture/restore，以及 Go 对照语义；字段含列引用时必须纳入 `ResolveIndices`，含表达式时还应评估 `ExtractCorrelatedCols`。
- 修改逻辑到物理枚举时，入口是 `ExhaustPhysicalPlans4LogicalUnionScan`。必须保持 root-only 约束、MPP 警告行为、`AdmitIndexJoinProp` 处理以及 schema/统计/offset/孩子属性的完整传递。
- 修改合并或过滤语义时，不应只改本文件；真实执行路径在 `pkg/executor/physical_plan_runtime.rs`。需同步验证升/降序、common handle、多条件、NULL、重复 handle、脏行覆盖、缺失孩子和越界列。
- 修改附着形状时同步更新独立的 `pkg/planner/core/task_test.rs`；修改索引解析时同步更新 `pkg/planner/core/resolve_indices_test.rs`；修改构造、克隆、内存或物理执行时分别扩展 `physical_union_scan_test.rs`、`unary_aster_unit_test.rs`、`cache_snapshot_test.rs` 或 `pkg/executor/physical_plan_runtime_test.rs`。Rust 测试应继续保存在独立测试文件，不嵌入生产源文件。
- 兼容性风险集中在事务内读一致性、Projection 列位置、handle 去重和 MPP 禁用边界；性能风险集中在表达式深克隆、内存估算遗漏以及执行器当前对两侧行向量排序/合并的成本。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；使用 `node --file` 阅读了目标文件全部 245 行，并查询 `PhysicalUnionScan` 与 `ExhaustPhysicalPlans4LogicalUnionScan`。图查询未返回该枚举函数的直接 callers/callees，因此以已索引文件节点和精确仓库引用搜索补足调用证据。
- 生产源码：`pkg/planner/core/operator/physicalop/physical_union_scan.rs`；模块/trait 接线：`pkg/planner/core/operator/physicalop/lib.rs`、`base_physical_plan.rs`；任务与索引接线：`pkg/planner/core/task.rs`、`pkg/planner/core/resolve_indices.rs`；执行实现：`pkg/executor/physical_plan_runtime.rs`；缓存实现：`pkg/planner/core/operator/physicalop/cache_snapshot.rs`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`，包名为 `astersql-planner-core-operator-physicalop`，并声明上述路径依赖和 `tipb` 依赖。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_union_scan.go`；相关 Go 任务接线/测试位于 `pkg/planner/core/task.go`、`pkg/planner/core/task_test.go`，索引解析位于 `pkg/planner/core/resolve_indices.go`。
- 独立 Rust 测试：`physical_union_scan_test.rs`（计划 ID、内存基线）、`unary_aster_unit_test.rs`（trait 与 clone）、`cache_snapshot_test.rs`（条件/handle/孩子 round trip）、`pkg/planner/core/task_test.rs`（Projection 重排和 task 元数据）、`pkg/planner/core/resolve_indices_test.rs`（部分解析后错误）、`pkg/executor/physical_plan_runtime_test.rs::union_scan_merges_by_handle_and_dirty_rows_shadow_snapshot_rows`（有序合并与脏行覆盖）。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令确认目标文档存在且恰有 11 个固定二级章节，并人工复核文档未把执行器逻辑误写为本文件内部实现。
