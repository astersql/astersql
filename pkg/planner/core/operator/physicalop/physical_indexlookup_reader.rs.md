# `pkg/planner/core/operator/physicalop/physical_indexlookup_reader.rs`

## 文件定位

本文件定义 Rust 物理计划中的 `PhysicalIndexLookUpReader`，表示“先通过索引侧找到 handle，再由表侧按 handle 回表”的双读 reader。它属于 `astersql-planner-core-operator-physicalop` crate；`Cargo.toml` 将该 crate 的入口设为同目录 `lib.rs`，后者通过 `mod physical_indexlookup_reader` 纳入实现，再用 `pub use physical_indexlookup_reader::*` 对外导出类型。

`lib.rs` 中的 `ConcretePhysicalOperator for PhysicalIndexLookUpReader` 和 `impl_concrete_physical_plan!` 把该结构注册为 `base::PhysicalPlan`：计划树观察到的孩子顺序固定为 `IndexPlan`、`TablePlan`。`pkg/planner/core/flat_plan.rs` 因此把第一个孩子标记为 build side，把第二个孩子标记为 probe side，并把表侧标记为 index-lookup probe child。执行阶段的直接入口之一是 `pkg/executor/physical_plan_runtime.rs::execute_node`；当前 Rust 行执行器会从 reader 下钻到 `TablePlan`，而不是在本文件内执行真正的两阶段 KV 查询。

## 核心职责

- `PhysicalIndexLookUpReader` 汇聚索引侧计划、表侧计划、输出 schema/统计信息，以及分页、保序、handle 列、下推 limit、分区信息和期望行数等规划元数据。
- `New` 建立类型为 `plancodec::TypeIndexLookUp` 的空 reader；`Init` 重新绑定上下文与 query-block offset，并在已有 `TablePlan` 时令输出 schema 和统计信息跟随表侧。
- `Clone` 深拷贝两侧 trait-object 子计划及列/分区元数据，供计划上下文切换和计划缓存等场景使用。
- `ExtractCorrelatedCols`、`ResolveIndices`、`ToPB`、成本接口和 `MemoryUsage` 为统一物理计划 trait 提供所需操作。
- 本文件并不完整实现 Go 版的 index-lookup pushdown、flattened plan、动态分区访问对象或统计预加载；这些差异是当前迁移状态，不应被解释为已支持。

## 主要符号

### `PhysicalIndexLookUpReader`

唯一公开类型。其主要字段可分为四组：

1. 计划骨架：`PhysicalSchemaProducer` 保存基础计划、schema 和统计；`IndexPlan`、`TablePlan` 是可缺省的两个子计划。
2. 执行策略：`IndexLookUpPushDown`、`Paging`、`KeepOrder` 和 `PushedLimit` 描述是否下推、分页、保序及提前截断。
3. handle 与分区：`ExtraHandleCol` 表示整数 row handle，`CommonHandleCols` 表示聚簇主键列，`PlanPartInfo` 携带分区计划信息。
4. 成本提示：`ExpectedCnt` 表示期望输出行数；本文件仅保存该值，实际成本消费可见 `pkg/planner/core/plan_cost_ver1.rs::getCost4PhysicalIndexLookUpReader` 等调用点。

### 构造、复制和元数据方法

- `New(ctx: ContextRef) -> Self`：创建两个子计划均为 `None`、布尔标记为 `false`、计数为零的 reader。
- `Init(self, ctx, offset) -> Self`：重设 session context、计划类型和 query-block offset；仅当表侧存在时同步 schema 与 stats。
- `Clone(&self, new_ctx) -> Result<Self, expression::Error>`：通过 `clone_physical` 递归复制 `IndexPlan` 和 `TablePlan`，复制 schema、列和分区信息，并传播任一复制错误。
- `ExtractCorrelatedCols() -> Vec<CorrelatedColumn>`：先收集表侧、再追加索引侧相关列；不去重，也不改变子计划中的列。
- `MemoryUsage() -> i64`：累计 producer、三个布尔量、一个 `u64`、两个子计划、handle 列、limit 和分区信息；`Option` 为空时贡献零。

### 展示、成本和序列化方法

- `AccessObject` 当前固定返回 `"index lookup"`，没有复现 Go 版动态分区访问对象计算。
- `ExplainInfo` 优先使用显式 `PushedLimit`；没有时递归搜索 `IndexPlan` 中的 `PhysicalLimit`，生成 `limit embedded(offset:..., count:...)`。`ExplainNormalizedInfo` 当前恒为空字符串。
- `GetIndexNetDataSize` 以“索引侧行数 × schema 列数 × 8 字节”估算网络数据量；`GetAvgTableRowSize` 以“表侧列数 × 8 字节”估算行宽。二者缺少相应子计划时返回零。
- `GetCost` 只是将传入的两侧成本、上述网络量和平均表行宽相加。trait 实际暴露的 `GetPlanCostVer1/2` 则委托给 `BasePhysicalPlan`。
- `ToPB` 优先序列化 `TablePlan`，否则序列化 `IndexPlan`；两侧都不存在时返回 `expression::errors::New("index lookup has no child plan")`。

## 执行流程

1. 规划阶段创建 `PhysicalIndexLookUpReader::New(ctx)`，再设置 `IndexPlan` 和 `TablePlan`。例如 `pkg/planner/core/operator/physicalop/index_join_probe.rs` 在非 single-scan 路径中构造两个带过滤条件的子计划，并把输出统计写入 producer。
2. 调用 `Init(ctx, offset)` 将基础计划固定为 `TypeIndexLookUp`。若表侧已经存在，reader 的输出 schema 与统计直接克隆自表侧，体现最终结果来自回表阶段。
3. `lib.rs` 的 `ConcretePhysicalOperator` 实现向通用计划框架暴露两个孩子；`set_children_operator` 将第一个输入安装为索引侧、第二个输入安装为表侧，并在表侧存在时更新输出 schema。
4. 计划检查/改写期间，`ExtractCorrelatedCols` 遍历两侧，`ResolveIndices` 先解析自身 producer，再按索引侧、表侧顺序调用各子计划的 `resolve_indices`。任一步失败即停止并返回错误。
5. EXPLAIN 通过 `ExplainInfo` 报告嵌入 limit；计划成本由调用方或 `GetPlanCostVer1/2` 路径计算。`pkg/planner/core/flat_plan.rs` 展平时把索引侧视为 build、表侧视为 probe。
6. 当前内置行执行器在 `physical_plan_runtime.rs` 识别 reader 后要求 `TablePlan` 存在，并递归执行表侧；若缺失则报 `PhysicalIndexLookUpReader has no TablePlan`。因此“索引产出 handle、表侧回表”的真实存储交互并不由本文件的运行时代码完整实现。

`BuildIndexLookUpTask` 是本文件内的任务包装辅助：它克隆当前 reader，使用 `tasks` 的第一个元素作为 `RootTask` 的可选子任务；克隆失败会触发 `expect("index lookup clone")`，而多余 task 会被丢弃。它与 Go 版接收 `CopTask`、据此构造 reader 并按需插入 projection 的同名函数语义并不等价。

## 数据与状态

`PhysicalIndexLookUpReader` 自身没有独立缓存或隐藏全局状态；所有可变状态都在结构字段和 `PhysicalSchemaProducer.BasePhysicalPlan` 中。重要不变量如下：

- 完整的 index lookup 应同时具有 `IndexPlan` 和 `TablePlan`，且孩子顺序不能交换；多个消费方依赖索引为位置 0、表为位置 1。
- 输出 schema/统计原则上跟随 `TablePlan`。`Init` 会同时同步二者，但 `set_children_operator` 只同步 schema，因此在改写孩子后，调用方仍需确保统计正确。
- `ExtraHandleCol` 与 `CommonHandleCols` 是两种 handle 表达方式；代码没有强制它们互斥，也没有在本文件中校验列属于表侧 schema。
- `PushedLimit` 为 `Copy` 风格值，既参与 EXPLAIN，也参与内存统计；当前 `ToPB` 不单独编码它。
- `ExpectedCnt`、`Paging`、`KeepOrder` 在本类型中主要是供外围成本/执行逻辑读取的元数据。本文件没有依据这些字段改写子计划。

`MemoryUsage` 是估算而非分配器精确值：它计入三个布尔量和一个 `u64` 的静态尺寸，递归计入两个拥有的子计划与列对象；trait object/`Vec`/`Option` 容器自身的全部容量开销并未逐项建模。独立测试 `memory_usage_counts_reader_scalars_and_pushed_limit_like_go` 只锁定空 reader 与 `PushedLimit` 的增量。

## 依赖与调用关系

crate 直接依赖中，本文件实际使用 `base`（计划 trait、上下文、task、PB context）、`costusage`（成本结果与选项）、`expression`（列、相关列及错误）、`property`（task type）、`kv`（store type）、`plancodec`（算子类型）和 `tipb`（PB executor）；`Cargo.toml` 中这些依赖均由 `physicalop` crate 声明。

上游/消费者的直接证据包括：

- `physicalop/lib.rs`：模块装配、公开再导出、trait 注册和双孩子顺序的权威定义。
- `index_join_probe.rs`：构造 index/table 两侧并生成 reader 的生产调用点。
- `pkg/planner/core/flat_plan.rs`：把 reader 的 index/table 孩子分别标记为 build/probe，并携带 TiKV Cop reader 上下文。
- `pkg/planner/core/plan_cost_ver1.rs`、`plan_cost_ver2.rs`：识别该具体类型，读取两侧行数、分页、期望行数、保序和下推 limit 计算成本。
- `pkg/executor/physical_plan_runtime.rs`：流式判断、直接表扫描识别和行执行都通过 reader 的 `TablePlan` 下钻；缺少表侧时返回运行时错误。
- `pkg/executor/builder.rs`、`statement_ru_plan_walk.rs`、`pkg/planner/core/cache_snapshot.rs` 和 `base_physical_plan.rs` 也按具体类型消费或遍历该节点。

RustCodeGraph 的 file node 报告目标文件被 18 个文件使用；对精确方法执行 `callers`/`callees` 未返回可展示的边，因此本文没有把缺失的图边当作“无人调用”，而是使用索引文件节点、类型查询与上述直接调用点交叉验证。

## 错误处理与边界

- `Clone`、`ResolveIndices`、`GetPlanCostVer1/2` 和 `ToPB` 使用 `Result` 向上传播表达式/计划错误；`ResolveIndices` 按 producer、索引侧、表侧的顺序短路。
- `ToPB` 明确处理两个子计划都不存在的情形；只有索引侧时仍可序列化索引侧，两个都存在时却只序列化表侧。这是当前实现边界，不等于完整 index-lookup executor PB 的构建。
- `BuildIndexLookUpTask` 对 clone 错误使用 `expect`，因此错误会 panic；空 `tasks` 合法地映射为 `RootTask` 的 `None` 子任务。
- `GetIndexNetDataSize`、`GetAvgTableRowSize` 对缺失孩子返回 `0.0`，可能掩盖未完整组装的计划，调用方不能以零成本推断计划有效。
- `ExplainInfo` 的 fallback 会递归遍历索引子树，并在 `tp() == "Limit"` 但无法 downcast 为 `PhysicalLimit` 时，以该节点统计行数构造 count；负数统计会经 `max(0.0)` 截为零。
- `LoadTableStats` 是空函数，`AccessObject` 是固定描述；需要真实统计或动态分区访问对象的调用方不能依赖它们获得 Go 版结果。

## 并发与资源生命周期

本类型不创建线程、异步任务、锁、通道、事务或文件/网络资源；方法均为同步计划操作。`ContextRef` 是共享上下文句柄，`Clone` 将新的 context 传给基础计划和两侧子计划，但本文件不控制 context 的关闭生命周期。

两个子计划由 `Option<Box<dyn PhysicalPlan>>` 独占；替换或丢弃 reader 时由 Rust 所有权规则递归释放。`set_children_operator` 在只有一个新孩子时保留旧 `TablePlan`，在有两个孩子时替换两侧；这一行为使部分计划改写可只更新索引侧，但调用者必须意识到旧表侧仍被保留。`Clone` 创建独立子计划和列对象，不共享这些可变结构；`PlanPartInfo::Clone` 的具体深浅语义由其实现决定。

`Paging` 与 `KeepOrder` 只是状态标志，本文件没有启动分页 worker 或排序/归并任务。实际并发度和批处理成本由外围成本模型与执行器负责，不能从这些布尔字段推导当前存在后台资源。

## 与 Go 版本的对应关系

同路径 Go 文件定义同名类型和基础方法，但 Rust 当前只覆盖了核心骨架：

- 已对应：schema producer、index/table 根计划、分页、额外/common handle、下推 limit、分区信息、期望行数、保序字段；`Clone`、相关列提取、基础 EXPLAIN、初始化时跟随表侧 schema/stats、成本接口和内存估算均有对应意图。
- 状态差异：Go 还保存 `IndexPlans`、`TablePlans` 和 `IndexPlansUnNatureOrders` 等 flatten 结果；Rust 类型没有这些字段，直接通过两个根计划的 `children()` 遍历。
- pushdown 差异：Go `Init` 调用 `tryPushDownLookUp`，会检查 `KeepOrder`、构造下推计划、清零 TiDB 表侧行数并更新状态；Rust `Init` 不接收 pushdown 来源，也不执行这些分支，虽然保留 `IndexLookUpPushDown` 字段。
- 统计与访问对象差异：Go `LoadTableStats` 从首个 table scan 预载统计，`AccessObject` 根据动态分区裁剪返回访问对象；Rust 前者为空，后者固定为字符串。
- 估算差异：Go 通过 `cardinality::GetAvgRowSize` 和实际统计估算网络量/行宽；Rust 按每列 8 字节近似。
- 任务构建差异：Go `BuildIndexLookUpTask` 从 `CopTask` 填充所有字段，并在非聚合场景按需插入 projection；Rust 同名方法仅克隆现有 reader 并用第一个 task 包装 `RootTask`。
- 序列化差异：Go 的 flattened 计划供 executor PB 构造使用；Rust `ToPB` 只把一个现有子计划转为 PB，尚未体现完整双读协议。

因此本文件可视为已经接入统一物理计划框架、并支持若干 Rust 运行时路径的迁移实现，而不是 Go 功能的完整逐项等价版本。

## 扩展指南

1. 若补全真正的 index lookup 执行或 PB 编码，首要修改点是 `ToPB`、`BuildIndexLookUpTask` 及执行器中的 reader 分支；必须保持 `IndexPlan`/`TablePlan` 的顺序契约，并覆盖缺失任一侧的错误。
2. 若移植 Go pushdown，应该在 `Init` 周边引入明确的 pushdown 决策参数和失败降级路径，同时处理 `KeepOrder`、common handle、统计归零和 hint warning；不能只设置 `IndexLookUpPushDown = true`。
3. 若补全统计/访问对象，分别实现 `LoadTableStats` 和 `AccessObject`，并核对动态分区裁剪、别名、空分区结果与 table/index scan 的安全 downcast。
4. 若修改 schema 或孩子替换规则，应同步审查 `ConcretePhysicalOperator::{children_operator,set_children_operator,set_child_operator}`、`flat_plan.rs` 和 `physical_plan_runtime.rs`，避免表侧 schema/stats 与实际孩子脱节。
5. 若改变成本或内存口径，应同时检查 `plan_cost_ver1.rs`、`plan_cost_ver2.rs`，并明确是否计入容器容量、flattened state、handle 额外宽度及 pushed limit。
6. 测试必须继续放在独立文件。优先扩展 `physical_indexlookup_reader_test.rs`；跨模块行为分别使用 `pkg/planner/core/resolve_indices_test.rs`、`pkg/executor/typed_index_lookup_test.rs` 或相关成本/执行器测试。需要新增的最低边界包括：两侧 clone 错误、相关列合并顺序、缺子计划的 `ToPB`、递归 limit 提取、孩子替换后的 schema/stats，以及未来 pushdown/统计加载路径。

兼容风险集中在 EXPLAIN 文本、孩子位置、schema/统计同步和 Go/Rust 行为差异；性能风险集中在行宽近似、分页/保序成本以及未来若重复递归遍历或重复克隆子计划。扩展时不应为了通过单元测试继续简化 Go 分支。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`node --file pkg/planner/core/operator/physicalop/physical_indexlookup_reader.rs --offset 1 --limit 400` 返回完整 280 行实现并报告 18 个使用文件；`query PhysicalIndexLookUpReader --limit 20 --json` 定位 Rust/Go 类型及成本、构建、索引解析相关符号；精确 struct 的 `callers`/`callees` 查询无输出，故调用关系由文件节点与直接源码引用补证。
- 目标实现：`pkg/planner/core/operator/physicalop/physical_indexlookup_reader.rs`，核对 1 个公开 struct、17 个固有方法、无条件编译项。
- crate/装配：`pkg/planner/core/operator/physicalop/Cargo.toml`、`pkg/planner/core/operator/physicalop/lib.rs`，核对 crate 名、依赖、模块导出、trait 注册和孩子顺序。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_indexlookup_reader.go`，核对字段、pushdown、flatten、统计加载、访问对象、成本及任务构建差异。
- 独立 Rust 测试：`pkg/planner/core/operator/physicalop/physical_indexlookup_reader_test.rs`，现有三个测试分别验证 EXPLAIN limit 文本、固定内存与 pushed-limit 增量、`Init` 复制表侧 stats。
- 直接调用/消费证据：`pkg/planner/core/operator/physicalop/index_join_probe.rs`、`pkg/planner/core/flat_plan.rs`、`pkg/planner/core/plan_cost_ver1.rs`、`pkg/planner/core/plan_cost_ver2.rs`、`pkg/executor/physical_plan_runtime.rs`、`pkg/planner/core/resolve_indices.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo；完成前执行任务规定的 11 章节结构命令，并以 `git diff --check` 检查 Markdown whitespace。
