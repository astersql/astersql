# `pkg/bindinfo/binding_plan_generation.rs`

## 文件定位

本文件属于 `astersql-bindinfo` crate 的候选执行计划生成层。模块由 `pkg/bindinfo/lib.rs` 声明并整体重新导出；crate 的清单 `pkg/bindinfo/Cargo.toml` 将库入口设为 `lib.rs`，关闭自动测试发现，并用 `package.metadata.porting.go-package = "pkg/bindinfo"` 标明 Go 对照包。

在应用链路中，`pkg/bindinfo/binding_handle.rs::NewBindingHandle` 用运行时构造 `newBindingAuto`，后者在 `pkg/bindinfo/binding_auto.rs` 中把 `planGenerator` 装进 `bindingAuto`。调用 `BindingPlanEvolution::ExplorePlansForSQL` 时，历史计划之外的新候选由本文件的 `PlanGenerator::Generate` 产生。因此，本文件负责“枚举候选状态并包装绑定”，而 SQL 解析、真实优化器调用、会话变量应用等环境相关工作由 `PlanRuntime` 适配器承担。

## 核心职责

1. 用 `PlanGenerator` 定义候选计划生成接口，并由 `planGenerator` 把生成结果转换为 `BindingPlanInfo`。
2. 用 `GenerationSpec` 描述可搜索空间，用 `state` 表达其中一个优化器配置点。
3. 从默认状态执行广度优先搜索（BFS），每次只改变一个 leading、索引提示、`no_decorrelate` 查询块、优化器变量或 fix-control 值。
4. 分别用 `state::Encode` 和 `genedPlan::planDigest` 去重搜索状态与执行计划，并按摘要排序以稳定返回顺序。
5. 提供变量/fix-control 调整规则和若干轻量提取辅助函数。

当前 Rust 文件不是 Go 实现的逐行内嵌优化器：`generatePlanWithSCtx` 先调用 `PlanRuntime::generation_spec` 获取已经整理好的搜索空间，`genPlanUnderState` 再调用 `PlanRuntime::plan_under_state` 生成真实计划。文件尾部的表、子查询、索引提取函数大多直接投影 `GenerationSpec`，不是 AST 遍历器的完整实现。

## 主要符号

- `PlanGenerator: Send + Sync`：公开生成接口；`Generate(defaultSchema, sql, charset, collation)` 返回 `Result<Vec<BindingPlanInfo>>`。
- `planGenerator`：默认实现，内部用 `Arc<dyn PlanRuntime>` 共享运行时；`new` 构造它，`Generate` 驱动搜索并包装绑定。
- `tableName` / `indexHint`：提示目标的数据模型。`tableName::HintName` 优先使用别名，二者的 `Display`/`String` 为状态编码提供稳定文本。
- `genedPlan`：运行时返回的计划摘要、可复现提示和二维计划文本；`PlanText` 用制表符连接列、换行连接行。
- `AnyValue`：替代 Go `any` 的显式值枚举；其浮点显示固定四位小数，是状态编码语义的一部分。
- `state`：一次搜索的全部旋钮值；`Encode` 按 leading、索引提示、`no_decorrelate`、变量值、fix 值的顺序生成逗号分隔键。
- `GenerationSpec`：SQL、表、索引提示选项、查询块、变量默认值、fix 默认值和搜索上限的运行时契约。
- `newStateWithLeading2`、`newStateWithIndexHint`、`newStateWithNoDecorrelateQB`、`newStateWithNewVar`、`newStateWithNewFix`：克隆旧状态后只修改一个维度。变量名、fix ID 或索引位置不存在时，相关函数保留克隆值。
- `generatePlanWithSCtx`、`breadthFirstPlanSearch`、`neighboring_states`、`genPlanUnderState`：规格获取、BFS、邻居枚举和单状态计划生成的主链。
- `adjustVar` / `adjustFix` / `getStartState`：搜索步进规则和起始状态构造。
- `tableNameExtractor`、`selectOffsetAssigner`、`subqueryOffsetExtractor`、`predicateColumnExtractor`：保留与 Go 结构对应的数据容器；本文件没有为它们实现 AST visitor。
- `collectSubqueryOffsets*`、`extractSelectTableNames*`、`extractNoDecorrelateQBs`、`matchesColumnTable`、`collectJoinPredicates`、`extractSelectIndexHints`：集合/匹配辅助函数；除匹配与集合累加外，提取函数当前读取 `GenerationSpec` 已有字段。

## 执行流程

1. `bindingAuto::ExplorePlansForSQL` 把当前库、SQL（或调用方传入文本）、字符集和排序规则交给 `PlanGenerator::Generate`。
2. `planGenerator::Generate` 调用 `generatePlanWithSCtx`；后者让 `PlanRuntime::generation_spec` 解析环境并生成 `GenerationSpec`，然后进入 `breadthFirstPlanSearch`。
3. `getStartState` 以无 leading、无索引提示、无 `no_decorrelate`，以及规格中的变量/fix 默认值建立初始状态。索引提示槽位数与 `index_hint_options.len()` 对齐。
4. BFS 使用 `VecDeque` 出队当前状态，调用 `genPlanUnderState`，后者直接委托 `PlanRuntime::plan_under_state`。计划以 `planDigest` 写入 `HashMap`，相同摘要会覆盖先前条目但仍只计一个候选。
5. `neighboring_states` 按固定顺序生成邻居：每个可选查询块、所有不同表组成的有序 leading 二元组、每张表的每个索引选项、每个变量的一次调整、每个 fix 的一次调整。只有 `Encode` 尚未出现的状态才入队。
6. 循环在计划数达到上限、已访问状态数达到上限或队列耗尽时停止。规格上限为 0 时分别使用 30 个计划和 10,000 个状态；返回前按 `planDigest` 升序排序。
7. `Generate` 将提示插入 SQL 第一个空白之前的首个词后；无提示时保留原 SQL。它建立 `Binding`，写入原 SQL、数据库、启用状态、历史来源、字符集/排序规则和计划摘要，再连同格式化计划文本包装为 `BindingPlanInfo`。

## 数据与状态

`GenerationSpec` 是不可变的搜索输入，`state` 是按值克隆的搜索节点。`varNames`/`varValues`、`fixIDs`/`fixValues` 依靠位置一一对应；运行时构造规格时必须维持长度和顺序，否则 `neighboring_states` 的按索引访问可能越界。`indexHints` 同样与 `index_hint_options` 的外层表顺序对齐。

`state::Encode` 不包含变量名或 fix ID，只编码它们的值；在同一份固定 `GenerationSpec` 中，名称/编号和顺序不变，因此可作为 BFS 去重键。若跨规格复用编码，则不能把它视为自描述格式。浮点数固定四位小数，差异小于显示精度的状态会合并，这是与 Go `%.4f` 一致的搜索空间压缩规则。

状态和计划均只驻留于一次调用的局部 `HashSet`、`HashMap` 与 `VecDeque`。生成出的 `Binding` 放入 `Arc`，供后续演进、评分和可选执行阶段共享；本文件不持久化绑定，也不修改绑定缓存。

## 依赖与调用关系

上游主链为：

`binding_handle.rs::NewBindingHandle` → `binding_auto.rs::newBindingAuto` → `binding_auto.rs::bindingAuto::ExplorePlansForSQL` → `PlanGenerator::Generate`。

本文件内部主链为：

`planGenerator::Generate` → `generatePlanWithSCtx` → `PlanRuntime::generation_spec` → `breadthFirstPlanSearch` → `getStartState` / `neighboring_states` / `genPlanUnderState` → `PlanRuntime::plan_under_state`。

直接 Rust 依赖来自 crate 根重新导出的 `Binding`、`BindingPlanInfo`、`PlanRuntime`、`BindError`、`Result` 及绑定状态/来源常量；标准库提供 `Arc` 和 BFS/去重容器；`serde` 为值对象提供序列化。虽然 crate 清单还声明 parser 与 hint 工具依赖，本文件当前不直接导入它们，相关解析和计划生成能力通过运行时边界进入。

## 错误处理与边界

- `generation_spec`、`plan_under_state`、`getStartState`、`neighboring_states` 中任何错误都会经 `?` 立即终止整次生成；没有跳过坏状态后继续的降级路径。
- `adjustVar` 只接受精确白名单：5 个布尔变量、5 个比例变量和 17 个代价变量。相似后缀的自定义名称也会返回 `BindError`。比例值从非正数变为 0.1，以 0.1 递增但不越过 1.0；代价值低于 1e6 时乘 5，达到或超过阈值后保持。
- `adjustFix` 仅支持 44855、45132、52869。布尔 fix 先裁剪空白并大小写无关判断 `OFF`；45132 使用严格整数解析，解析成功且值不大于 10 时原样返回，否则减半。
- `getStartState` 拒绝空变量名，但不验证变量值类型、重复名称、fix 支持范围或两组向量的其他语义；这些属于运行时规格提供者的责任。
- `newStateWithIndexHint` 对越界位置不报错而返回未改变的克隆；`newStateWithNewVar`/`newStateWithNewFix` 对未知键也如此。
- Rust `Generate` 本身没有 Go 版本的“仅接受以 SELECT 开头”检查，也不会调用 Go 的 `prepareHints`。它按第一个空白定位插入点；前导空白、注释开头或非 SELECT 文本能否进入此处，以及提示是否合法，取决于 `PlanRuntime` 和上游契约。
- 当 `max_plans` 或 `max_explore_states` 非零时直接采用调用方数值；循环条件在展开邻居前检查状态数，所以一次展开可能使 `visited_states` 超过名义上限，但不会再处理后续状态。

## 并发与资源生命周期

`PlanGenerator` 与 `PlanRuntime` 都要求 `Send + Sync`，`planGenerator` 通过 `Arc<dyn PlanRuntime>` 与 `bindingAuto` 共享同一运行时。这只是跨线程安全的接口约束；本文件的 BFS 是同步、单线程、逐状态执行，没有启动任务、创建通道或持有锁。

每次 `Generate` 调用独立分配搜索容器，函数返回即释放队列和去重表。是否复用会话、如何恢复被调节的优化器状态、计划生成所占外部资源以及并发调用之间的隔离，都由 `PlanRuntime::generation_spec`/`plan_under_state` 的实现负责，本文件无法保证。`Binding` 的 `Arc` 生命周期延伸到返回的 `BindingPlanInfo` 被释放为止。

## 与 Go 版本的对应关系

算法骨架与 `pkg/bindinfo/binding_plan_generation.go` 对齐：相同的状态字段与编码顺序、BFS 去重、30/10,000 默认上限、摘要排序、变量白名单与步进规则、三个 fix-control 规则，以及空索引提示选项用于清除某张表提示。`pkg/bindinfo/binding_plan_generation_test.go` 的 `TestAdjustFixes`、`TestAdjustVars`、`TestStartState` 在 Rust 独立测试中有对应覆盖。

关键迁移差异如下：

- Go `planGenerator` 持有 session pool，校验 SELECT、借用会话并调用 `prepareHints`；Rust 持有抽象 `PlanRuntime`，不在本文件执行这些步骤，并额外写入 `StatusEnabled`、`SourceHistory`、字符集和排序规则。
- Go `generatePlanWithSCtx` 直接解析 AST、设置当前库和 cost model v2、记录相关变量/fix，并发现表、查询块和索引；Rust 把这些职责收敛进 `PlanRuntime::generation_spec`。
- Go `genPlanUnderState` 直接修改 session variables/fix-control、临时追加 AST hints、调用 `GenBriefPlanWithSCtx` 并用 `defer` 恢复部分 hints；Rust 只调用 `PlanRuntime::plan_under_state`，因此恢复和隔离责任在运行时实现。
- Go 文件尾部是真实 AST visitor 与 infoschema 索引筛选；Rust 的同名 extractor 结构尚未接线，提取函数主要返回 `GenerationSpec` 中预先计算的字段。不能据这些符号推断 Rust 已独立实现 Go 的 AST/索引发现逻辑。
- Go 总是在 `SELECT` 后构造 `/*+ ... */`（即使提示串为空）并随后准备提示；Rust 对空提示保持原 SQL，对非空提示按首个空白插入。

## 扩展指南

- 新增优化器变量时，应同步修改 `adjustVar` 的精确白名单及其分类函数，并确保 `PlanRuntime::generation_spec` 提供正确的 `AnyValue` 类型；在 `pkg/bindinfo/binding_plan_generation_test.rs` 增加支持值、边界值和相似名称拒绝用例，同时对照 Go `adjustVar`。
- 新增 fix-control 时，应同步 `adjustFix`、规格默认值来源和独立测试，明确字符串规范化、解析失败及饱和/翻转规则。
- 新增搜索维度时，需要同时扩展 `state`、`state::Encode`、起始状态、邻居生成、`PlanRuntime::plan_under_state` 和测试。编码必须确定且能区分同一规格内的语义状态；还要评估分支因子对 10,000 状态上限和运行时调用成本的影响。
- 若把 Go 的 AST/infoschema 提取迁回本文件，应实现真正的 visitor/索引过滤而不是扩写现有投影桩，并补独立 Rust 测试覆盖别名、子查询/集合运算、不可见/非 Public/列存/倒排/主键索引及谓词首列匹配。
- 修改 hint 注入时，应覆盖前导空白、注释、非 SELECT、无空白 SQL、空提示和提示校验；还要明确是否恢复 Go 的 SELECT 限制及 `prepareHints` 语义。
- 并发或资源相关扩展应优先放在 `PlanRuntime` 实现中，并验证每次状态生成后的会话恢复；不要让 BFS 持有跨调用可变全局状态。
- 测试逻辑继续放在同目录独立文件 `pkg/bindinfo/binding_plan_generation_test.rs`，不要嵌入生产源文件。

## 验证依据

- 目标源码：`pkg/bindinfo/binding_plan_generation.rs`（RustCodeGraph `node --file` 覆盖 1–656 行）。
- crate 与装配：`pkg/bindinfo/Cargo.toml`、`pkg/bindinfo/lib.rs`。
- 上下游直接证据：`pkg/bindinfo/binding_auto.rs` 中的 `PlanRuntime`、`newBindingAuto`、`ExplorePlansForSQL`，以及 `pkg/bindinfo/binding_handle.rs::NewBindingHandle`。
- Go 对照：`pkg/bindinfo/binding_plan_generation.go`（`Generate`、状态编码、BFS、单状态生成、调整规则及 AST/索引提取全段）与 `pkg/bindinfo/binding_plan_generation_test.go`。
- Rust 独立测试：`pkg/bindinfo/binding_plan_generation_test.rs`。已编译接入的测试覆盖布尔/比例/代价调整、fix 44855/45132、起始状态编码、非白名单拒绝、45132 严格解析与原文本保留、空索引选择；文件中的 `GO_PLAN_GENERATION_TEST_DRAFT` 只是字符串草稿，不算可执行测试。
- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/bindinfo`、精确 `query` 和 `node` 确认符号及源码。对核心符号执行了 `callers`/`callees`，但命令在 30 秒内未返回结果；调用边改由上述相邻源码和局部 `rg` 交叉核对。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文恰好包含上述 11 个固定二级标题。
