# `pkg/planner/core/initialize.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate，由 `pkg/planner/core/lib.rs` 以私有模块 `mod initialize` 编译，再通过 `pub use initialize::*` 对外重导出其公开项。它位于计划数据结构与规划器上下文之间：`LoadData` 和 `ImportInto` 定义在 `pkg/planner/core/common_plans.rs`，而计划 ID 分配边界 `PlanContext::alloc_plan_id` 定义在 `pkg/planner/core/base/plan_base.rs`。

`pkg/planner/core/Cargo.toml` 声明本 crate 的库根为 `lib.rs`，并将 `astersql-planner-core-base` 以 `base-dependency` 引入。本文件不受 `nextgen` feature 或其他条件编译项控制。

## 核心职责

本文件仅为两种简化命令计划补齐 Go `baseimpl.Plan` 中最基本的计划身份：规划器上下文、类型标识、唯一计划 ID 和查询块偏移。它不构建 schema、表达式、物理算子或执行器，也不负责解析 `LOAD DATA` / `IMPORT INTO` 的业务参数。

这个窄边界的直接用途是使 `LoadData` 和 `ImportInto` 能按 Go 命名习惯提供 `SCtx`、`TP`、`ID` 和 `QueryBlockOffset` 查询。当前 Rust 生产构建链的 `PlanBuilder::buildLoadData` 与 `PlanBuilder::buildImportInto` 返回 `BuiltPlan::Command`，未构造这里的 `LoadData` / `ImportInto`；因此本文件的 API 已实现且有单元测试，但不应被描述为已接入当前生产语句构建主链。

## 主要符号

- `InitializedPlan`：公开、可克隆的基础身份值；字段保持私有，迫使调用者经由访问器观察。
- `InitializedPlan::new(ctx, tp, query_block_offset)`：私有构造器，调用 `ctx.alloc_plan_id()` 一次并固化全部四个身份字段。
- `InitializedPlan::{SCtx, TP, ID, QueryBlockOffset}`：零变更访问器，分别返回上下文引用、静态类型字符串、ID 和查询块偏移。
- `LoadData::Init` 与 `ImportInto::Init`：按值接收 `self`，用新建的 `InitializedPlan` 覆盖 `Plan: Option<_>`，然后按值返回已初始化计划。类型字符串分别固定为 `"LoadData"` 和 `"ImportInto"`，查询块偏移都固定为 `0`。
- 两组 `SCtx` / `TP` / `ID` / `QueryBlockOffset` 方法：将访问转发到 `Plan` 内的 `InitializedPlan`；若未初始化则使用明确的 `expect` 消息终止。
- `Debug`：输出 `tp`、`id` 和 `query_block_offset`，通过 `finish_non_exhaustive` 不暴露 `ctx` 内容。
- `PartialEq` / `Eq`：除三个标量字段相等外，还要求两个 `ContextRef` 由 `Arc::ptr_eq` 判定为同一实例，而非进行上下文内容比较。

## 执行流程

1. 调用者准备一个 `base::ContextRef`（即共享的计划上下文）和一个默认或已填充的 `LoadData` / `ImportInto`。
2. `Init` 选定固定类型字符串和偏移 `0`，调用 `InitializedPlan::new`。
3. `new` 通过 `PlanContext::alloc_plan_id` 向上下文申请一个 ID，将返回值与上下文、类型和偏移一起存入新身份。
4. `Init` 将身份包装成 `Some` 写入 `self.Plan`；如果该字段原来已有值，旧身份被替换，且新的 ID 仍会被分配。
5. 调用者从返回的计划值上调用访问器；访问器先要求 `Plan` 为 `Some`，再返回身份字段。

## 数据与状态

`InitializedPlan` 保存的状态是：`ctx: base::ContextRef`、`tp: &'static str`、`id: i32` 与 `query_block_offset: i32`。`ContextRef` 的共享所有权允许计划与其他算子持有同一上下文；类型使用静态字符串，不需要单独的堆分配或生命周期管理。

`LoadData::Plan` 与 `ImportInto::Plan` 是 `Option<InitializedPlan>`，其 `Default` 值为 `None`，因而类型系统允许未初始化状态存在。初始化之后没有公开 setter，但因 `Plan` 字段本身是 `pub`，crate 外调用者仍可替换或清空它；所以“访问器安全”依赖调用方维持“先 `Init`、后访问”的运行时不变量。

## 依赖与调用关系

下游直接依赖只有 `base_dependency` 中的 `ContextRef` 和 `PlanContext::alloc_plan_id`，以及 crate 内 `common_plans.rs` 定义的 `LoadData` / `ImportInto`。`lib.rs` 将本文件的公开符号重导出，而 `common_plans.rs` 反向以 `crate::InitializedPlan` 作为两个计划的 `Plan` 字段类型，形成 crate 内的模块级交叉引用。

RustCodeGraph 对目标文件识别出 20 个符号，包括两个 `Init` 和三组访问器；针对 `LoadData::Init` 符号 ID 的 `callers` / `callees` 查询均返回空集。由于重名 inherent method 的图边可能不完整，又用文本搜索复核：仓库内直接调用这两个 `Init` 的已确认位置仅有 `pkg/planner/core/initialize_test.rs`，未找到 Rust 生产调用者。

## 错误处理与边界

构造路径不返回 `Result`：`alloc_plan_id` 的 trait 签名直接返回 `i32`，因此本文件没有可传播的可恢复错误。明确的失败边界是在 `Plan == None` 时调用任一计划访问器：代码会分别以 `"LoadData must be initialized"` 或 `"ImportInto must be initialized"` panic。

类型标识和偏移是硬编码契约，调用者不能通过本 API 定制。ID 正确性取决于注入的 `PlanContext` 分配器；本文件不检查重复、溢出或跨上下文唯一性。`Debug` 故意不打印上下文，降低泄露庞大或敏感会话状态的风险。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或 I/O 资源。上下文通过 `Arc` 持有，克隆 `InitializedPlan` 只会增加共享引用计数，不会分配新 ID；只有再次调用 `Init` 才会触发新的 `alloc_plan_id`。

ID 分配的并发语义完全由 `PlanContext` 实现保证。独立测试中的 `TestPlanContext` 用 `AtomicI32::fetch_add(Ordering::SeqCst)` 模拟线程安全且严格有序的分配，但这是测试替身的性质，不是 `initialize.rs` 自身强加的具体内存序契约。当计划和所有克隆被丢弃后，`Arc` 计数递减；本文件没有额外清理步骤。

## 与 Go 版本的对应关系

Go 直接对照文件是 `pkg/planner/core/initialize.go`。Go 的 `LoadData.Init` 和 `ImportInto.Init` 同样按值接收器工作，通过 `baseimpl.NewBasePlan(ctx, <type>, 0)` 创建基础计划，再返回新值指针。Rust 版保留了上下文、类型、ID 和零偏移四个核心语义；`InitializedPlan::new` 对应 `baseimpl.NewBasePlan` 中这部分的初始化。

差异主要有四点：

- Go 计划结构嵌入 `physicalop.SimpleSchemaProducer`，Rust 的这两个结构仅持有 `Option<InitializedPlan>` 和简化业务字段，不是完整物理算子移植。
- Go 的类型值来自 `plancodec.TypeLoadData` / `TypeImportInto`，Rust 目前使用等值的字面量；若编码常量变更，两处需同步核对。
- Go `NewBasePlan` 通过会话变量 `PlanID.Add(1)` 取 ID，Rust 把策略抽象为 `PlanContext::alloc_plan_id`。
- Go 同文件还初始化 `Analyze` 和 `ScalarSubqueryEvalCtx`。Rust `initialize.rs` 未移植 `Analyze::Init`；标量子查询由 `pkg/planner/core/scalar_subq_expression.rs` 的 `ScalarSubqueryEvalCtx::New` 构造，因为 Rust 构造时还必须同时提供物理子计划、请求上下文和信息模式。

没有找到专门验证这两个 Go `Init` 的独立 Go 测试；Rust 回归契约由 `pkg/planner/core/initialize_test.rs` 单独承载。

## 扩展指南

- 若要为新的简化计划复用此身份，应在对应计划类型中添加 `Option<InitializedPlan>`，实现与现有流程一致的 `Init` 和访问器，并在独立的 `*_test.rs` 文件中覆盖 ID、类型、偏移与上下文同一性；不要把测试内嵌到本源文件。
- 若要将 `LoadData` / `ImportInto` 接入真实 Rust 规划主链，修改点不只是本文件；还必须核对 `planbuilder.rs` 的 `BuiltPlan::Command` 设计、下游执行器预期的计划类型，以及 Go `common_plans.go` 的业务字段。不能只把 `Init` 调用塞入构建器就宣称完成语义移植。
- 若要允许可变查询块偏移或类型，优先修改 `InitializedPlan::new` 的调用约定并与 Go `plancodec` 契约对齐，同时扩展 `initialize_test.rs`。
- 若要改变未初始化行为，必须明确选择保持 panic、返回 `Option` 或返回 `Result`，并检查所有访问器调用点。这是 API 兼容性变更，不是内部重构。
- 性能上的主要风险是重复 `Init` 造成无意义的 ID 消耗，或将静态类型字符串替换为频繁分配的动态字符串。正确性风险则是类型名与 Go / EXPLAIN 编码不一致，或在跨查询共享了错误的 `ContextRef`。

## 验证依据

- RustCodeGraph `status`：项目索引可用，共 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/core/initialize.rs` 确认目标已索引。
- RustCodeGraph `node --file pkg/planner/core/initialize.rs --offset 1 --limit 500`：读取了全部 158 行，验证结构体、trait 实现、构造器、两个 `Init` 和访问器实现。
- RustCodeGraph 精确 `query`：确认目标文件的 20 个符号；对 `LoadData::Init` 的符号 ID 运行 `callers` 和 `callees` 均得到 `[]`，随后以 `rg` 复核重名方法的真实调用面。
- Rust 源码与边界：`pkg/planner/core/common_plans.rs`、`pkg/planner/core/base/plan_base.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/planbuilder.rs`、`pkg/planner/core/scalar_subq_expression.rs`。
- crate 声明：`pkg/planner/core/Cargo.toml`，确认 crate 名、库根、`base-dependency` 路径、feature 边界和 Go 包对照元数据。
- Go 对照：`pkg/planner/core/initialize.go`、`pkg/planner/core/common_plans.go`、`pkg/planner/core/operator/baseimpl/plan.go`，分别核对初始化入口、完整计划结构和 `NewBasePlan` 语义。
- 独立 Rust 测试：`pkg/planner/core/initialize_test.rs` 用起始值为 40 的原子 ID 分配器验证两次初始化依次得到 41 和 42，并验证类型、零偏移和 `Arc` 指针同一性。按任务约束未运行 Cargo，因此这里记录的是测试源码证据，不是本次执行结果。
