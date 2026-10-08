# `pkg/planner/core/operator/physicalop/physical_max_one_row.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-core-operator-physicalop`，crate 根为同目录的 `lib.rs`；`Cargo.toml` 的 `package.metadata.porting.go-package` 将它映射到 Go 包 `pkg/planner/core/operator/physicalop`。`lib.rs` 通过 `mod physical_max_one_row` 纳入模块，并以 `pub use physical_max_one_row::*` 公开本文件的类型与枚举函数。

它位于“逻辑标量子查询约束”到“可执行的一元物理计划”的转换位置：`LogicalMaxOneRow` 表示结果最多一行的逻辑要求，本文件生成 `PhysicalMaxOneRow`；执行阶段则由 `pkg/executor/builder.rs::buildMaxOneRow` 构造 `pkg/executor/select.rs::MaxOneRowExec`。因此，本文件负责规划形态、属性和元数据，不直接读取行或抛出“子查询多于一行”的运行时错误。

目标包没有 `doc.go`；包边界以 `Cargo.toml`、`lib.rs` 和相邻实现为准。

## 核心职责

- 定义无专属业务字段的 `PhysicalMaxOneRow`，以 `PhysicalSchemaProducer` 承载通用物理计划状态、Schema、统计信息和子节点属性。
- 通过 `New` 和 `Init` 设置 `plancodec::TypeMaxOneRow`、会话上下文、查询块偏移、统计信息，以及唯一子节点的所需物理属性。
- 通过 `ExhaustPhysicalPlans4LogicalMaxOneRow` 将 `LogicalMaxOneRow` 枚举为物理候选；当父属性要求排序或 TiFlash/MPP 时拒绝生成候选。
- 将子节点 `ExpectedCnt` 固定为 `2.0`。这不是把语义改成“最多两行”，而是让下游最多探测到第二行，以便执行器判定是否违反“最多一行”。
- 支持跨上下文克隆、内存估算和 EXPLAIN 接口；通用 `PhysicalPlan` 能力由 `lib.rs` 中的 `impl_schema_leaf_operator!(PhysicalMaxOneRow)` 宏接线。

## 主要符号

- `pub struct PhysicalMaxOneRow`：公开物理算子。唯一字段 `PhysicalSchemaProducer` 也是公开的；本文件没有模块常量、枚举、trait 定义或条件编译项。
- `PhysicalMaxOneRow::New(ctx: ContextRef) -> Self`：创建类型为 `TypeMaxOneRow`、查询块偏移初始为 `0` 的基础计划包装。
- `PhysicalMaxOneRow::Init(self, ctx, stats, offset, child) -> Self`：消费并返回算子值，重设上下文、类型、查询块偏移和统计信息，并登记恰好一个子属性。
- `PhysicalMaxOneRow::Clone(&self, new_ctx) -> Result<Self, expression::Error>`：调用基础计划的 `CloneWithNewCtx`，再对可选 Schema 做深层 `Clone`，返回绑定新上下文的独立算子。
- `PhysicalMaxOneRow::MemoryUsage(&self) -> i64`：将内存估算委托给 `PhysicalSchemaProducer::MemoryUsage`。
- `PhysicalMaxOneRow::ExplainInfo(&self) -> String`：返回空字符串；该算子没有要附加到 EXPLAIN 的专属字段。
- `ExhaustPhysicalPlans4LogicalMaxOneRow(logical, required) -> Vec<Box<dyn PhysicalPlan>>`：公开的逻辑到物理枚举入口，成功时返回唯一候选，拒绝或缺少上下文时返回空向量。

## 执行流程

1. `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的枚举路由在遇到 `logicalop::LogicalMaxOneRow` 时调用 `ExhaustPhysicalPlans4LogicalMaxOneRow`。
2. 枚举函数首先检查父属性。`required.IsSortItemEmpty()` 为假，或 `required.IsFlashProp()` 为真时，MaxOneRow 无法满足该属性；若逻辑计划有上下文，则调用 `RaiseWarningWhenMPPEnforced`，随后返回零个候选。
3. 属性允许后，函数从逻辑计划获取并克隆 `ContextRef`；上下文缺失时安全地返回零个候选。
4. 函数创建默认子属性，把 `ExpectedCnt` 设置为 `2.0`，并从父属性传播 `CTEProducerStatus` 与 `NoCopPushDown`。排序项和 Flash 属性不会传播，因为带这些要求的路径已在前一步被拒绝。
5. 函数用逻辑计划的 Schema、统计信息和 `QueryBlockOffset` 初始化 `PhysicalMaxOneRow`，再以 `Box<dyn PhysicalPlan>` 返回唯一候选。统计信息缺失时使用 `StatsInfo::default()`。
6. 后续实现规则为该一元计划挂接子计划。旧 Cascades 路径 `pkg/planner/cascades/old/implementation_rules.rs::ImplMaxOneRow::OnImplement` 也遵循 `ExpectedCnt = 2.0`，并用 `NewMaxOneRowImpl` 包装候选。
7. 执行器构建器把物理计划及其唯一子节点交给 `ExecutorKind::MaxOneRow`。`MaxOneRowExec::Next` 首次读取零行时补一行 NULL；读取一行时再探测一次；首次已有多行或二次探测仍有行时返回“Subquery returns more than 1 row”；之后的 `Next` 不再产出数据。

## 数据与状态

`PhysicalMaxOneRow` 自身不保存计数器、行缓存或执行状态。它的持久状态全部位于 `PhysicalSchemaProducer` 及其 `BasePhysicalPlan` 中，主要包括计划上下文、计划类型、计划 ID、查询块偏移、Schema、统计信息、子节点需求属性，以及通用计划的子节点集合。

枚举期间新建的子属性有三个关键值：`ExpectedCnt = 2.0`，`CTEProducerStatus` 与父属性一致，`NoCopPushDown` 与父属性一致。两行期望值是检测上限提示，不是输出基数承诺；真正的最多一行约束由运行时执行器维护。

`Clone` 会重建 Schema 生产者，并在源 Schema 存在时调用 `schema.Clone()`，避免只替换上下文却遗失输出列描述。`Init` 则将子属性装入一个单元素 `Vec<Box<PhysicalProperty>>`，体现该算子必须是一元算子。

## 依赖与调用关系

上游直接接线包括：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：按 `LogicalMaxOneRow` 类型路由到 `ExhaustPhysicalPlans4LogicalMaxOneRow`。
- `pkg/planner/cascades/old/implementation_rules.rs::ImplMaxOneRow`：旧 Cascades 实现路径直接调用 `PhysicalMaxOneRow::New(...).Init(...)`。
- `pkg/planner/core/operator/physicalop/lib.rs`：公开模块符号，并用宏为该类型实现通用物理计划、Schema 生产者和类型擦除能力。

本文件的直接下游依赖为：`base::{ContextRef, PhysicalPlan}` 提供上下文和物理计划 trait；`logicalop::LogicalMaxOneRow` 提供上下文、Schema、统计和查询块偏移；`property::{StatsInfo, PhysicalProperty}` 描述代价/枚举属性；`plancodec::TypeMaxOneRow` 提供稳定计划类型标识；当前 crate 的 `BasePhysicalPlan` 与 `PhysicalSchemaProducer` 承载共用实现；`expression::Error` 是克隆错误类型。

后续消费者包括 `pkg/executor/builder.rs::buildMaxOneRow`、`pkg/planner/core/plan_cost_ver2.rs` 中的 MaxOneRow 成本分支，以及 `pkg/executor/statement_ru_plan_walk.rs` 中将该算子按根节点 Limit 类别计量的分支。RustCodeGraph 的文件关系还列出 `pkg/executor/statement_ru_plan_walk.rs`、`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/logical_plans_test.rs` 和 `pkg/planner/core/plan_cost_ver2.rs` 对本文件的使用。

## 错误处理与边界

- 排序需求或 Flash/MPP 属性不受支持时，枚举返回空候选；只有在会话确实强制 MPP 时，`RaiseWarningWhenMPPEnforced` 才会记录对应警告。`physical_max_one_row_test.rs` 验证了强制 MPP 场景的精确警告文本。
- 逻辑计划没有上下文时返回空候选，不 panic。统计信息缺失则退化为默认统计；Schema 在枚举路径上由 `logical.Schema().Clone()` 明确复制。
- `CloneWithNewCtx` 的失败通过 `expression::Error` 原样向上传播；本文件不吞掉或改写该错误。
- 本文件不直接检查实际行数。零行补 NULL、多行错误、只评估一次等边界在 `pkg/executor/select.rs::MaxOneRowExec` 中实现；把 `ExpectedCnt` 改为 `1.0` 或用 `LIMIT 1` 替代会掩盖第二行错误路径。
- 结构上要求唯一子节点。`Init` 只接收一个 `PhysicalProperty`，执行器构建端也通过 `child_plan()` 获取必需子节点；缺失子节点由构建器报告“max one row”构建失败，而非由本文件处理。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源；计划对象只在规划期构造、克隆和传递。`ContextRef` 是共享上下文引用，`New`/`Init` 保存它，`Clone` 显式替换为调用者提供的新上下文。

计划的子节点资源尚未在本文件打开。执行期生命周期由 `MaxOneRowExec` 管理：`Open` 打开唯一子执行器并把 `evaluated` 重置为 `false`；第一次 `Next` 完成全部零/一/多行判定，之后返回空结果。该单次评估状态不在物理计划间共享，因此本文件没有同步原语需求。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/physical_max_one_row.go`。

- 两版都定义 `PhysicalMaxOneRow`，以通用基础计划承载状态；都把类型设为 `plancodec.TypeMaxOneRow`，保存统计和查询块偏移，并把子节点 `ExpectedCnt` 设为 `2`。
- 两版枚举都拒绝非空排序项和 Flash 属性，并使用相同的 MPP 阻塞警告文本；都传播 `CTEProducerStatus` 与 `NoCopPushDown`。
- Go 的 `Init` 接受可变数量的子属性并返回指针；Rust `Init` 消费值且只接受一个子属性，更直接地表达一元不变量。Go 枚举返回“计划列表、hintCanWork、error”三元组，Rust 路由使用计划向量表达候选存在与否，不在该函数签名中返回 hint 标记或错误。
- Go `MemoryUsage` 对 nil 接收者返回零；Rust 方法借用有效引用，类型系统排除了 nil 接收者。Go 的 `CloneWithSelf` 保留具体动态类型；Rust 使用 `CloneWithNewCtx` 后显式复制 Schema。
- Rust 文件另有 `ExplainInfo` 空字符串实现；Go 对照文件没有同名专属方法。
- `pkg/executor/test/executor/executor_test.go::TestMaxOneRow` 验证多行标量子查询返回 `[executor:1242]Subquery returns more than 1 row`。Rust 的已接线回归 `pkg/executor/test/executor/executor_test.rs::scalar_subquery_rejects_more_than_one_row` 验证同一语义；同文件中较早的 `TestMaxOneRow` 是保留 Go 文本的迁移占位，不应当作可执行行为证据。

## 扩展指南

- 若要改变候选属性约束，应优先修改 `ExhaustPhysicalPlans4LogicalMaxOneRow`，并同步旧 Cascades 的 `ImplMaxOneRow::{Match, OnImplement}`，防止两条优化路径产生不同计划；尤其要保持二次探测所需的 `ExpectedCnt = 2.0`。
- 若增加算子字段，需同步 `New`、`Init`、`Clone`、`MemoryUsage`、`ExplainInfo`，以及 `lib.rs` 宏生成接口所依赖的生产者结构；同时核对 Go 对照文件是否有相同字段或行为。
- 若改变 Schema 或上下文克隆语义，应扩展独立测试 `physical_max_one_row_test.rs`，覆盖源/克隆计划的 Schema 独立性、上下文替换和错误传播。Rust 单元测试必须继续放在独立测试文件，不能嵌入本源文件。
- 若改变排序或 MPP 支持范围，应补充接受与拒绝两类属性测试，并核对警告只在强制 MPP 时出现；同时检查 `base_physical_plan.rs` 的统一枚举路由和旧 Cascades 规则。
- 若改变运行时零/一/多行行为，实际修改点在 `pkg/executor/select.rs::MaxOneRowExec`，还需同步 `pkg/executor/test/executor/executor_test.rs::scalar_subquery_rejects_more_than_one_row` 及 Go `TestMaxOneRow`。不能用 `LIMIT 1` 作为性能简化，因为那会丢失多行错误语义。
- 性能关注点主要是第二次取数和 Schema/计划克隆；兼容性关注点是计划类型标识、EXPLAIN 形态、MPP 警告文本和错误语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，Rust 与 Go 均已纳入。
- RustCodeGraph `query PhysicalMaxOneRow`：定位到 Rust `physical_max_one_row.rs::PhysicalMaxOneRow` 与 Go 同名结构。
- RustCodeGraph `query ExhaustPhysicalPlans4LogicalMaxOneRow`：定位到 Rust/Go 两版枚举函数；`node --file pkg/planner/core/operator/physicalop/physical_max_one_row.rs` 读取了完整 126 行，并报告五个使用文件。图的 `callers/callees` 精确查询在本次运行中超时，因此调用边又由下列源码接线逐一核对，未把超时解释成“无调用者”。
- 源与 crate：`pkg/planner/core/operator/physicalop/physical_max_one_row.rs`、`Cargo.toml`、`lib.rs`、`base_physical_plan.rs`。
- 直接规划路径：`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/implementation/simple_plans.rs`、`pkg/planner/core/plan_cost_ver2.rs`。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_max_one_row.go`。
- 独立规划测试：`pkg/planner/core/operator/physicalop/physical_max_one_row_test.rs`，验证强制 MPP 拒绝和警告文本。
- 运行时及回归边界：`pkg/executor/builder.rs`、`pkg/executor/select.rs`、`pkg/executor/test/executor/executor_test.rs`、`pkg/executor/test/executor/executor_test.go`、`pkg/executor/statement_ru_plan_walk.rs`。
- 人工复核结论：该文件存在是为了把 `LogicalMaxOneRow` 转换为可挂接、可计价、可构建执行器的物理计划；它通过二行期望值保留运行时多行检测能力，并以属性拒绝、上下文缺失和克隆错误传播覆盖规划边界。扩展时必须同步统一枚举、旧 Cascades、独立 Rust 测试和 Go 语义基线。
