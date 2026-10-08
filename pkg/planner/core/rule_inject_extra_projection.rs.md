# `pkg/planner/core/rule_inject_extra_projection.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 入口 `pkg/planner/core/lib.rs` 以 `pub mod rule_inject_extra_projection` 导出该模块，并在 `cfg(test)` 下装配独立测试 `rule_inject_extra_projection_test.rs`。它操作的不是 Go 侧 `base.PhysicalPlan` trait object，而是 `crate::task` 中的轻量值类型 `PlanNode`、`PlanKind` 和 `Expression`（`pkg/planner/core/task.rs:24-213`）。

模块意图与 Go 同名规则一致：在物理计划中将聚合、排序、TopN、名义排序和 MPP `UnionAll` 所需的表达式提前物化为 `Projection`。不过当前 RustCodeGraph 的 `InjectExtraProjection` 节点没有生产调用者，仓库内精确 Rust 引用也只有本文件和独立测试；因此当前事实是“模块已公开、规则及单元测试已存在，但未发现它接入 Rust 优化器生产主链”，不能把 Go 的运行时接线当成 Rust 已接线。

## 核心职责

- `InjectExtraProjection` / `projInjector::inject` 自底向上遍历 `PlanNode` 树，然后按节点种类分派改写，保证父节点处理时其普通 `children` 已完成同一规则。
- `injectProjBelowUnion` 只处理 `StoreType::TiFlash` 的 `UnionAll`，在需要时为每个分支增加类型对齐投影。
- `InjectProjBelowAgg` 将聚合函数或分组项中的非列表达式放到聚合子节点之上的投影中，并把原表达式替换为投影输出列。
- `InjectProjBelowSort` 为 `Sort`/`TopN` 的标量排序键建立下层物化投影，再用上层投影裁掉仅供排序使用的附加列。
- `TurnNominalSortIntoProj` 删除不实际排序的 `NominalSort`：只含列时直接透传孩子，否则用上下两层投影触发表达式计算并恢复原 schema。

这些职责均是树结构改写；本文件不执行表达式、不分配真实计划列 ID，也不直接访问会话、统计管理器或存储。

## 主要符号

- `pub fn InjectExtraProjection(plan: PlanNode) -> PlanNode`：整棵树的公开入口，构造无状态注入器并调用 `inject`。
- `pub struct projInjector` 与 `pub fn NewProjInjector() -> projInjector`：零字段、零大小的规则对象及其构造器。命名保留 Go 移植风格。
- `pub fn projInjector::inject(&self, p: PlanNode) -> PlanNode`：核心分派。先递归替换 `p.children`，再匹配 `UnionAll`、`HashAgg`/`StreamAgg`、`Sort`/`TopN`、`NominalSort`；其他 `PlanKind` 原样返回。
- `pub fn injectProjBelowUnion(p: PlanNode) -> PlanNode`：TiFlash Union 分支 schema 对齐。
- `pub fn InjectProjBelowAgg(p, funcs, groups) -> PlanNode`：聚合表达式物化与重复表达式复用。
- `pub fn InjectProjBelowSort(p, items) -> PlanNode`：排序键物化以及输出 schema 裁剪。
- `pub fn TurnNominalSortIntoProj(p, onlyColumn, items) -> PlanNode`：名义排序消除；`onlyColumn` 为真时直接返回唯一孩子。
- 私有 `projection`：创建 `PlanKind::Projection`，由表达式返回类型生成 schema，并继承孩子的 `stats` 与 `expected_count`。
- 私有 `is_scalar`、`is_constant`、`same_expression`：分别以 `function_count > 0`、无列且无函数、五个 `Expression` 字段相等来进行轻量表达式分类/去重。
- 私有 `column`、`default_type`：生成列引用；缺失返回类型时回退到 `TypeCode::Null`、长度/小数位为 0、非 unsigned 的字段类型。

文件没有模块级常量、trait、条件编译项或全局可变状态。

## 执行流程

1. 调用 `InjectExtraProjection` 后，`projInjector::inject` 通过所有权移动递归改写每个普通孩子，形成后序遍历。
2. 对 `UnionAll`，只有节点 `store == TiFlash` 才进入分支对齐。孩子列数与父 schema 不同，或两者完全相同，均保持孩子不变；列数相同但 schema 有差异时逐列生成表达式：相同类型使用列引用，不同类型生成名为 `cast(col_i)`、目标类型为父列类型的表达式，最后强制投影 schema 等于父 schema。
3. 对 `HashAgg`/`StreamAgg`，入口复制节点的 `agg_funcs` 和 `group_items` 作为输入。若两组中完全没有 `function_count > 0` 的表达式，则不改写。存在标量表达式且恰有一个孩子时，常量保留在聚合项中但不进入投影；其他表达式按 `same_expression` 去重加入投影，并替换成对应下标的列引用。投影成为聚合唯一孩子。
4. 对 `Sort`/`TopN`，没有标量排序键或孩子数不是 1 时原样返回。否则下层投影先复制孩子 schema 的所有列，再按排序项顺序附加每个标量表达式；排序项相应改成附加列引用。上层投影只选择原计划 schema 的列，因此外部可见列不包含临时排序列，最终结构为 `top projection -> sort/topN -> bottom projection -> original child`。
5. 对 `NominalSort`，必须恰有一个孩子。`onlyColumn == true` 时直接移除名义排序并返回孩子；否则下层投影保留孩子所有列并附加标量排序表达式，上层投影再恢复孩子 schema，结构为 `top projection -> bottom projection -> original child`。`projInjector::inject` 当前固定传入 `false`，只有直接调用公开 helper 才能走 `true` 分支。
6. 其他节点只保留已经递归改写过的孩子，不改变自身字段。

## 数据与状态

规则以拥有所有权的 `PlanNode` 为输入和输出。改写期间使用局部 `Vec<Expression>`、`Vec<FieldType>` 和闭包，没有跨调用缓存。`projection` 会复制孩子的 `StatsInfo` 与 `expected_count`；排序的顶层投影以原排序节点为孩子，因此继承排序节点统计，而底层投影继承原孩子统计。聚合与 Union 的新增投影同样继承被包装孩子的这两项信息。

表达式模型来自 `pkg/planner/core/task.rs:73-81`，只包含名称、可选列下标、函数计数、虚拟列标记和可选返回类型。`same_expression` 比较这五项，并不进行语义等价推导。schema 是 `Vec<FieldType>`；缺失表达式返回类型不会报错，而由 `default_type` 写入 Null 类型。新增投影除 schema、统计、期望行数、表达式和孩子外，其余 `PlanNode` 字段采用 `PlanNode::new(Projection)` 的默认值。

## 依赖与调用关系

直接代码依赖只有 `crate::task::{Expression, PlanKind, PlanNode}`，并在 Union 与类型辅助函数中引用同模块的 `StoreType`、`FieldType` 和 `TypeCode`。`pkg/planner/core/Cargo.toml` 将本目录定义为 `astersql-planner-core`（`lib.rs` 为库入口，`autotests = false`），该文件本身没有使用任何外部 crate，也没有受 `nextgen` feature 条件控制。

RustCodeGraph 给出的内部边包括：

- `InjectExtraProjection -> NewProjInjector -> projInjector::inject`；
- `inject -> injectProjBelowUnion / InjectProjBelowAgg / InjectProjBelowSort / TurnNominalSortIntoProj`；
- 三个物化 helper 调用私有 `projection`，并按需调用 `is_scalar`、`is_constant`、`same_expression`、`column`、`default_type`；
- `InjectProjBelowAgg`、`InjectProjBelowSort`、`TurnNominalSortIntoProj` 和 `injectProjBelowUnion` 的已索引 Rust 调用者是 `inject` 及各自的独立测试；公开总入口没有已索引的 Rust 上游调用者。

Go 主链不同：`pkg/planner/core/optimizer.go:467` 在 `postOptimize` 中调用 Go `InjectExtraProjection`，`pkg/executor/coprocessor.go:200` 也为下推到 TiDB 的计划调用它。这两条 Go 边只能说明对照实现的设计位置，不能证明 Rust 模块已经接线。

## 错误处理与边界

所有接口均直接返回 `PlanNode`，没有 `Result`、错误码或 panic 分支。结构不满足预期时采取保守回退：聚合仅在一个孩子时插入投影；排序和名义排序要求恰好一个孩子；Union 孩子列数不等时不尝试 `zip` 后的部分转换。未知节点种类保持自身不变。

需要注意的边界与风险：

- `default_type` 会把缺失类型静默降为 Null，占位模型无法表达真实类型推导错误；扩展时不应把该回退解释成 Go 的完整类型系统行为。
- Union 通过完整 `FieldType` 相等判断转换，但轻量 `FieldType` 没有 Go `RetType` 的 nullable flag；Rust 也只构造描述性的 `cast(col_i)` 表达式，不调用 Go 的 `BuildCastFunction4Union`。
- 聚合的 `function_count` 只是是否含标量函数的摘要，`agg_funcs` 也不是 Go 的 `AggFuncDesc` 参数/ORDER BY 嵌套结构。当前去重基于字段相等，无法替代 Go 表达式上下文中的 `Equal`。
- `projInjector::inject` 仅递归 `PlanNode.children`；它没有 Go 对 TiFlash `PhysicalTableReader.TablePlan` 的特殊递归与扁平化，也没有 `DisableProjectionPostOptimization` failpoint。
- 直接 helper 是公开 API，调用者传入的 `funcs`、`groups` 或 `items` 可以与 `p` 内部字段不一致；总入口通过克隆本节点字段规避了该问题。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道、I/O、事务或外部资源。`projInjector` 无状态，可从语言层面独立创建并调用；每次改写消费一棵计划树并返回新根，子树通过 `Vec` 的所有权移动重新挂接。临时表达式和 schema 向量随函数返回转入新计划节点，未使用全局缓存或引用共享，因此不存在本文件内的锁顺序、取消或清理协议。

性能上，遍历主体对节点数近似线性；但聚合去重对每个非恒定表达式在已收集向量中执行线性 `position`，最坏为表达式数的平方量级。计划树递归深度也等于树深度，极深的人工计划可能消耗较多调用栈。

## 与 Go 版本的对应关系

直接对照为 `pkg/planner/core/rule_inject_extra_projection.go`。Rust 保留了入口、注入器和四类 helper 的总体分工，以及“先孩子、后父节点”“排序键上下双投影”“NominalSort 消除”“MPP Union 类型对齐”等核心形状，但属于基于轻量 `PlanNode` 的移植，尚非 Go 完整语义的等价实现。

主要差异如下：

- Go 入口有 `DisableProjectionPostOptimization` failpoint，并已接入 `postOptimize` 与 coprocessor；Rust 无 failpoint且未发现生产调用者。
- Go 注入器还遍历 TiFlash `PhysicalTableReader.TablePlan` 并重建扁平计划列表；Rust 只遍历通用 `children`。
- Go Union 以 `Mpp` 标志为门槛，比较完整返回类型和 NOT NULL 标志并构造真实 cast；Rust以 `store == TiFlash` 近似门槛，使用简化 `FieldType` 和描述性表达式，且对列数不等直接跳过。
- Go 聚合先调用 `coreusage.WrapCastForAggFuncs`，遍历每个聚合函数的参数及 ORDER BY 项，分配唯一计划列 ID，并按各自规则复用表达式；Rust 将每个 `Expression` 当作扁平聚合项，不做 cast 包装或列 ID 分配，并统一按 `same_expression` 去重非恒定项。
- Go 创建投影时保留 session context、query block offset、child required property，并按期望行数缩放统计，还会用 `refine4NeighbourProj` 合并相邻投影；Rust只复制轻量统计和 `expected_count`，不做相邻投影整理。
- Go `NominalSort` 的 `OnlyColumn` 来自实际节点；Rust `PlanKind::NominalSort` 没有对应字段，总入口固定按 `false` 路径处理。

`pkg/planner/core/casetest/rule/rule_inject_extra_projection_test.go` 及其 Rust 对照测试验证的是 `WrapCastForAggFuncs` 的类型/模式矩阵，并不直接覆盖本文件的轻量 helper。直接行为证据应以 `pkg/planner/core/rule_inject_extra_projection_test.rs` 为准。

## 扩展指南

若要新增受支持算子，优先在 `projInjector::inject` 的后序分派中增加 `PlanKind` 分支，并把具体构造逻辑放在独立 helper；同时在 `pkg/planner/core/rule_inject_extra_projection_test.rs` 增加同目录独立测试，不要把测试嵌入生产源文件。若新增节点拥有不在 `children` 中的嵌套计划，还必须显式递归该字段，参考 Go 的 TiFlash TableReader 特例。

若要提高 Go 语义一致性，应先扩展 `crate::task` 的表达式/字段/计划上下文能力，再逐项移植：聚合参数和 ORDER BY 层级、真实 cast 构造、nullable 比较、唯一列 ID、required property、统计缩放、相邻投影整理、failpoint，以及生产优化器/coprocessor 接线。不要在本文件用字符串名称继续模拟这些完整能力。

修改各路径时至少同步验证：非 TiFlash Union 不变、相同 schema 不插入投影、类型不匹配只转换对应列、常量不物化、重复表达式复用、无标量表达式不改写、错误孩子数保守返回、排序附加列最终被上层投影裁剪、NominalSort 两个分支，以及递归确实先处理子树。兼容风险集中在输出 schema/列下标和 Go 语义差异；性能风险集中在重复表达式查找与多余相邻投影。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被完整索引为 247 行。
- RustCodeGraph 源码/调用查询：`node --file pkg/planner/core/rule_inject_extra_projection.rs`、`node InjectExtraProjection`、`node injectProjBelowUnion`、`node InjectProjBelowAgg`、`node InjectProjBelowSort`、`node TurnNominalSortIntoProj`、`node projection`，用于确认符号实现、内部调用边、测试调用者和生产入口缺失。
- 数据模型查询：`node --file pkg/planner/core/task.rs --offset 1 --limit 230`，用于核对 `StoreType`、`FieldType`、`Expression`、`PlanKind`、`PlanNode` 及默认值。
- crate 与装配：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs:97-99`。
- Go 实现与真实 Go 调用点：`pkg/planner/core/rule_inject_extra_projection.go`、`pkg/planner/core/optimizer.go:467`、`pkg/executor/coprocessor.go:200`。
- 直接 Rust 测试：`pkg/planner/core/rule_inject_extra_projection_test.rs`，覆盖 Union、聚合、Sort 和 NominalSort 的主要分支。
- 邻近 Go/Rust 对照测试：`pkg/planner/core/casetest/rule/rule_inject_extra_projection_test.go` 与 `.rs`，只证明 `WrapCastForAggFuncs` 对照，不作为本文件 helper 已实现该能力的证据。
- 未运行 Cargo：本任务只新增说明文档，计划明确禁止运行 Cargo；结构检查和人工事实复核是本任务的验证手段。
