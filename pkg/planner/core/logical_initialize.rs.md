# `pkg/planner/core/logical_initialize.rs` 逻辑说明

## 文件定位

`pkg/planner/core/logical_initialize.rs` 是 `astersql-planner-core` crate 内的私有占位模块。`pkg/planner/core/lib.rs` 通过 `mod logical_initialize;` 将它纳入编译，但没有 `pub mod` 或再导出，因此它不构成 crate 的公开 API。`pkg/planner/core/Cargo.toml` 指定 `lib.rs` 为库入口，并用 `package.metadata.porting.go-package = "pkg/planner/core"` 标记 Go 包对应关系。

该文件当前只有版权、许可证和说明性注释，没有 `use`、常量、静态量、类型、trait、函数、`impl` 或条件编译项。RustCodeGraph 将它识别为含 1 个文件节点、0 个代码符号，并报告 `used by 0 files`；这里的“模块被 `lib.rs` 声明”和“没有符号级使用边”应分开理解。

## 核心职责

本文件的唯一职责是保留与 `pkg/planner/core/logical_initialize.go` 对应的源码槽位和移植说明。Go 对照文件同样只有许可证与 `package core` 声明，没有初始化逻辑。

文件注释明确指出：逻辑算子的 `Init` 实现位于各自的算子定义旁。当前 Rust 真实实现集中在 `pkg/planner/core/operator/logicalop/*.rs`，例如 `LogicalUnionAll::Init` 位于 `logical_union_all.rs`，公共基座构造函数 `NewBaseLogicalPlan` 位于 `base_logical_plan.rs`。因此，本文件不是规划器初始化入口、注册器或生成代码承载文件，也不会在 SQL 规划主链中执行。

## 主要符号

本文件没有可列举的代码符号，也没有公开 API：

- 模块名 `logical_initialize` 仅由 `pkg/planner/core/lib.rs` 的私有 `mod logical_initialize;` 产生。
- 文件内不存在模块级常量、类型、trait、函数、`impl` 或 `#[cfg(...)]` 项。
- RustCodeGraph 对该文件的符号与调用查询没有产生函数调用者或被调用者；这符合空模块事实。

与其意图直接相关、但不属于本文件的真实符号包括：

- `LogicalUnionAll::Init`（`pkg/planner/core/operator/logicalop/logical_union_all.rs`）：把默认算子的 `LogicalSchemaProducer.BaseLogicalPlan` 替换为已初始化基座，并返回算子自身。
- `NewBaseLogicalPlan`（`pkg/planner/core/operator/logicalop/base_logical_plan.rs`）：调用 `PlanContext::alloc_plan_id`，保存 `ContextRef`、算子类型、计划 ID 和查询块偏移，其余字段采用 `BaseLogicalPlan::default()`。
- 其他算子的 `Init`：分散在 `logical_apply.rs`、`logical_join.rs`、`logical_projection.rs`、`logical_selection.rs`、`logical_table_dual.rs` 等算子文件中，而不是汇总到本文件。

## 执行流程

本文件自身没有运行时执行流程。编译期只发生以下装配：

1. Cargo 以 `pkg/planner/core/lib.rs` 作为 `astersql-planner-core` 的库入口。
2. `lib.rs` 声明私有模块 `logical_initialize`。
3. 编译器解析该文件；因为其中没有代码项，不会生成可供业务调用的初始化函数，也不会增加运行时分支。

逻辑算子的实际初始化流程应从具体算子的 `Init` 阅读。以 `LogicalUnionAll::default().Init(ctx, offset)` 为例，`Init` 调用 `NewBaseLogicalPlan(ctx, "Union", offset)`；后者从上下文分配计划 ID，建立带上下文和查询块偏移的 `BaseLogicalPlan`，然后写回算子的 schema producer。相关行为不经过 `logical_initialize.rs`。

## 数据与状态

本文件不声明、读取或修改任何数据与状态；没有全局变量、缓存、schema、统计信息、计划节点或上下文所有权。

初始化后实际存在的状态由 `BaseLogicalPlan` 持有，定义在 `pkg/planner/core/operator/logicalop/base_logical_plan.rs`，包括上下文、算子类型、计划 ID、查询块偏移、任务缓存、子节点、schema、输出名、统计信息、函数依赖集合和 TiFlash 标志等。`NewBaseLogicalPlan` 只显式设置上下文、类型、ID 和查询块偏移，其他字段来自默认值。上述状态是相关架构背景，不应误归为本占位文件所有。

## 依赖与调用关系

直接依赖关系非常窄：

- 上游装配：`pkg/planner/core/lib.rs` 私有声明 `mod logical_initialize;`。
- crate 边界：`pkg/planner/core/Cargo.toml` 定义 `astersql-planner-core`，默认 feature 为空，`nextgen` feature 与本空模块没有条件分支关系。
- 下游依赖：本文件没有 `use`、函数调用、类型引用或模块再导出，因此没有符号级下游依赖。
- 调用边：RustCodeGraph 对目标文件报告 0 个符号和 `used by 0 files`；对相邻真实构造函数 `NewBaseLogicalPlan` 的节点信息则显示它调用 `alloc_plan_id`，并被多个逻辑算子 `Init` 调用。这进一步表明调用链位于 `operator/logicalop`，不位于本文件。

由于模块是私有且为空，其他 crate 无法通过稳定公开路径依赖它。删除或填充该模块会影响源码布局或未来接线，但当前没有运行时调用者需要兼容。

## 错误处理与边界

本文件没有返回值、`Result`、panic、断言或错误转换，因此不存在本地错误处理路径。它也不会吞掉、包装或传播规划错误。

边界必须如实理解：

- “逻辑初始化”这个文件名不表示所有逻辑算子的统一初始化入口。
- 文件注释所称的各算子 `Init` 才是实际实现；不能从本占位文件推断所有算子已经完整移植或具有相同签名。
- 具体 `Init` 多为直接返回 `Self`，公共基座构造依赖 `PlanContext::alloc_plan_id`；上下文实现的失败或 panic 语义必须到对应上下文和算子文件核对。
- 本任务没有运行 Cargo，也没有以编译成功替代源码与调用图证据。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源，也不拥有需要释放的资源，因此没有独立的并发协议或生命周期清理逻辑。

相关真实初始化会把 `base::ContextRef` 保存到 `BaseLogicalPlan`。测试中的上下文通常以 `Arc` 构造，并以原子计数器模拟 `alloc_plan_id`，例如 `pkg/planner/core/operator/logicalop/logical_union_all_test.rs`；这说明计划上下文可被多个节点共享，但共享与 ID 分配语义属于 `ContextRef`、`PlanContext` 和 `BaseLogicalPlan`，不是本文件提供的保证。

## 与 Go 版本的对应关系

`pkg/planner/core/logical_initialize.go` 只有许可证和 `package core`，没有声明、初始化函数或副作用；Rust 文件以空私有模块保持同路径语义，二者在“无运行时行为”这一点上对齐。

Go 的真实逻辑算子初始化同样位于具体算子旁。例如 `pkg/planner/core/operator/logicalop/logical_union_all.go` 的 `LogicalUnionAll.Init` 调用 Go 版 `NewBaseLogicalPlan`；Rust 的 `logical_union_all.rs` 采用相同局部布局和同样的基座初始化意图。两版仍有语言级差异：Go 返回指针并把 `self` 传给基座，Rust 消费并返回 `Self`，且当前 Rust `BaseLogicalPlan` 不保存 Go 式自引用，而通过 trait 和所属算子访问。

相关测试也位于真实算子旁，而非本文件旁。Rust 的 `logical_union_all_test.rs`、`logical_partition_union_all_test.rs` 以及 `optimizer_logical_entry_aster_unit_test.rs` 会通过具体 `Init` 构建计划；Go 的 `operator/logicalop/logicalop_test/hash64_equals_test.go` 等测试也直接调用各算子 `Init`。它们验证算子初始化后的行为或等价性，不构成本空模块的单元测试。

## 扩展指南

新增或修改某个逻辑算子的初始化时，应优先编辑 `pkg/planner/core/operator/logicalop/<算子>.rs` 中该类型的 `Init`，并按 Go 同名算子核对类型标签、查询块偏移、上下文、计划 ID 和算子特有字段；公共状态构造应落在 `NewBaseLogicalPlan`，不要把分散的算子逻辑集中复制到本占位文件。

测试应放在对应的独立 Rust 测试文件中，遵守源文件与单元测试分离要求。例如修改 `LogicalUnionAll::Init` 时同步更新 `logical_union_all_test.rs`，修改跨算子初始化契约时考虑 `optimizer_logical_entry_aster_unit_test.rs` 或 `operator/logicalop/logicalop_test/*` 的相关用例，并与 Go 同名测试意图保持一致。

只有在 Go 上游或 Rust 架构确实新增了包级统一初始化职责时，才应向本文件加入代码。届时需要同时：将 API 可见性写清楚；在 `lib.rs` 评估是否再导出；在本文件对应的独立测试模块中覆盖调用顺序、重复调用、失败边界和并发约束；检查 Cargo feature 是否需要门控。主要兼容风险是改变计划 ID、类型标签或查询块偏移；性能风险是把每节点的轻量构造变成全局注册、锁竞争或重复分配。

## 验证依据

本说明依据以下直接证据完成：

- RustCodeGraph `status`：索引包含目标仓库；目标文件查询显示 23 行、1 个文件节点、0 个代码符号和 `used by 0 files`。
- RustCodeGraph `node --file pkg/planner/core/logical_initialize.rs`：确认文件只有许可证和占位说明。
- RustCodeGraph 对 `NewBaseLogicalPlan` 的 `query`/`node`：确认 Rust 定义位置、`alloc_plan_id` 下游调用，以及多个逻辑算子 `Init` 上游调用。单独的 `callers`/`callees` ID 查询返回空数组，与 `node` 的 Trail 摘要不一致，因此本文只把 Trail 和源码交叉验证后的边用于架构定位，不把空数组解释为没有真实调用。
- `pkg/planner/core/lib.rs`：确认私有模块声明；`pkg/planner/core/Cargo.toml`：确认 crate 名称、入口、feature 和 Go 包元数据。
- `pkg/planner/core/logical_initialize.go`：确认 Go 同路径文件无声明。
- `pkg/planner/core/operator/logicalop/logical_union_all.rs`、`base_logical_plan.rs` 及对应 Go 文件：确认真实 `Init` 与公共基座构造的位置和语义。
- `pkg/planner/core/operator/logicalop/logical_union_all_test.rs`、`logical_partition_union_all_test.rs`、`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 和 Go 的 `operator/logicalop/logicalop_test/hash64_equals_test.go`：确认相关测试直接覆盖具体算子初始化，而非本空模块。

人工复核结论：这个文件为保持 Go/Rust 包布局对应而存在；它在编译时作为空私有模块被解析，但运行时不执行；安全扩展的默认位置是具体算子 `Init` 和 `NewBaseLogicalPlan`，不是向本文件堆叠无关初始化逻辑。
