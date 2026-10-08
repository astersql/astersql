# `pkg/planner/core/rule_aggregation_push_down.rs`

## 文件定位

本文件定义逻辑优化规则 `AggregationPushDownSolver`，目标是在简化的 Rust `LogicalPlan` 上把可分解聚合穿过 `Join` 或 `UnionAll`，在孩子侧形成 Partial 聚合、在原位置保留 Final 聚合，从而在连接或并集之前减少行数。模块由 `pkg/planner/core/lib.rs` 公开声明，所属 crate 是 `astersql-planner-core`（`pkg/planner/core/Cargo.toml`）。

当前接线边界需要特别注意：仓库内对该 Rust 类型的直接使用只出现在独立测试 `pkg/planner/core/rule_aggregation_push_down_test.rs`；未找到把它注册进 Rust 生产优化器规则列表的调用。Go 对照实现则由 `pkg/planner/core/optimizer.go` 的 `optRuleList` 注册。Rust 生产规划路径中另有 `pkg/planner/core/optimizer_runtime.rs` 的聚合下推相关逻辑，因此本文件应视为基于简化计划模型的规则实现/迁移单元，不能仅凭模块公开性断言它已经进入完整应用主链。

## 核心职责

- 判定函数能否跨 Join 或 UNION ALL 分解：`isDecomposableWithJoin`、`isDecomposableWithUnion` 同时拒绝带 `order_by` 的聚合，并使用不同函数白名单。
- 将聚合函数及分组列归属到 Join 左右孩子：`getAggFuncChildIdx`、`collectAggFuncs`、`collectGbyCols`、`splitAggFuncsAndGbyCols`。
- 构造两阶段聚合：`decompose` 把孩子侧设为 `Partial1`、父侧设为 `Final`，并把 `COUNT` 的 Final 函数改为 `SUM`。
- 对 Join、UNION ALL 及其后代递归改写：入口为 `Optimize`，主体是 `aggPushDown`，普通容器节点遍历由私有方法 `recurse` 完成。
- 规避不安全或无收益改写：拒绝不支持的 Join、跨左右两侧取列的聚合、全 `FirstRow`、直接嵌套 Join、以及分组列已覆盖孩子唯一键的情形。

## 主要符号

- `AggregationPushDownSolver { next_column_id }`：唯一模块级类型。`next_column_id` 为新 Partial 输出分配本求解器实例内单调递增的列标识；`Default` 从 0 开始。
- `Optimize(&mut self, LogicalPlan) -> Result<(LogicalPlan, bool)>`：公开规则入口，返回改写后的计划及是否发生下推；错误类型复用 `rule_aggregation_elimination.rs` 中的 `Result<T> = std::result::Result<T, String>`。
- `aggPushDown`：识别根部 `Aggregation`，分别处理 `UnionAll`、有效 `Join` 或递归孩子，是本文件的控制中心。
- `isDecomposableWithJoin`：Join 白名单为 `Max`、`Min`、`FirstRow`，以及非 DISTINCT 的 `Sum`、`Count`。
- `isDecomposableWithUnion`：UNION ALL 白名单额外包含 `Avg` 与 `ApproxCountDistinct`；当前实现没有单独禁止 DISTINCT，但仍禁止 `order_by`。
- `getAggFuncChildIdx`：按参数中的列下标返回左侧 `0`、右侧 `1`、两侧 `-1`、无列引用 `2`。它依赖左右 schema 长度及拼接后的全局列下标约定。
- `collectAggFuncs`：全有或全无地验证聚合函数可分解性；常量聚合在 LeftOuter 时归左，其余归右；外连接保留侧之外的特定表达式参数必须全是直接列引用。
- `collectGbyCols` / `addGbyCol` / `add_unique`：按左 schema 长度拆分、去重分组列；Join 等值键由 `aggPushDown` 另外加入两侧分组集合。
- `decompose`：克隆描述符并生成 Partial/Final 对；Final 参数引用形如 `partial_<id>` 的新列。
- `tryToPushDownAgg`：执行收益/安全门槛检查，构造孩子侧 `LogicalAggregation`，补充承载分组列的 `FirstRow`，空分组时加入常量 `0`，并返回父层应使用的 Final 函数。
- `tryAggPushDownForUnion` / `pushAggCrossUnion` / `splitPartialAgg`：为 UNION ALL 的每个孩子克隆 Partial 聚合，并把父聚合切换为 Final。
- `getDefaultValues`、`makeNewAgg`：公开的 Go 对齐辅助函数；当前 Rust 文件内部没有调用它们。前者以字符串表达外连接默认值，后者只返回 `tryToPushDownAgg` 构造的计划部分。
- `Name`：返回稳定规则名 `aggregation_push_down`；当前未见 Rust trait 实现或注册者调用它。

## 执行流程

1. 调用者以一个 `LogicalPlan` 创建/复用求解器并调用 `Optimize`；入口直接进入 `aggPushDown`。
2. 若当前节点不是 `Aggregation`，`recurse` 只穿过 `Projection`、`Join`、`UnionAll`、`Expand`，对子节点递归并用逻辑或合并 `changed`；其他节点保持不变且停止向下。
3. 若聚合孩子是 `UnionAll`，先验证所有函数都满足 UNION 分解白名单，再为每个分支调用 `pushAggCrossUnion` 创建 Partial 聚合，父聚合函数切为 Final，父 `COUNT` 改为 `SUM`。
4. 若聚合孩子是 Inner、LeftOuter 或 RightOuter Join，先用 `splitAggFuncsAndGbyCols` 判定每个函数只依赖一侧并拆分函数/分组列。任何函数不可分解或同时引用两侧，整个 Join 下推放弃。
5. Join 等值条件中的左右列分别补入孩子分组列。若一侧存在 `COUNT`/`SUM`，另一侧聚合不得下推，因为连接倍增会改变这类聚合的结果。
6. 每个允许侧进入 `tryToPushDownAgg`：空函数集合、全 `FirstRow`、孩子本身为 Join、或分组列覆盖唯一键时不改写；否则为各函数生成 Partial/Final 对，并为分组列附加 `FirstRow` 输出。
7. 右侧 Final 引用的列下标加上改写后左 schema 长度；随后按原函数的左右归属依次替换父聚合中的描述符，重建 Join schema，并以任一侧确实产生 Final 函数作为 `changed`。
8. 聚合孩子不符合上述直接改写形态时，规则递归处理孩子，再把孩子装回原聚合。

## 数据与状态

本文件不定义完整 TiDB 计划对象，而是复用 `rule_aggregation_elimination.rs` 的简化 `LogicalPlan`、`LogicalAggregation`、`AggFuncDesc`、`AggFuncName`、`AggMode`，以及 `task.rs` 的 `Expression`、`FieldType`、`JoinType`。列依赖由 `Expression.column: Option<usize>` 表示：Join 右侧列使用“左 schema 长度 + 右侧局部下标”的拼接坐标；这也是右侧 Final 参数必须增加 `right_offset` 的原因。

求解器唯一可变状态是 `next_column_id`。每次 `decompose` 前由 `tryToPushDownAgg` 读取并递增；同一实例多次优化时不会复位。Partial 聚合设置 `no_eliminate = true`，避免后续聚合消除规则立即撤销这次改写。新 Partial schema 的顺序是聚合输出在前、分组列输出在后，`output_columns` 相应设为连续范围。

Join schema 和 Final 引用依赖上述位置不变量。`collectGbyCols` 只提取 `group_by_items` 中直接带 `column` 的表达式；复杂表达式中的隐含列不会被递归提取。`other_conditions` 被原样保留，但其中列不会像 Go 版本那样加入分组集合。

## 依赖与调用关系

上游：`pkg/planner/core/lib.rs` 公开模块；RustCodeGraph 将 `pkg/planner/core/rule_aggregation_push_down_test.rs` 识别为该类型的导入者。仓库文本搜索未发现测试之外的 Rust `AggregationPushDownSolver` 实例化或 `Optimize` 调用，因此生产上游在本文件层面未验证。

内部主调用链为 `Optimize -> aggPushDown`。Join 分支继续调用 `checkValidJoin -> splitAggFuncsAndGbyCols -> collectAggFuncs/collectGbyCols`，再调用 `tryToPushDownAgg -> decompose`；UNION ALL 分支调用 `tryAggPushDownForUnion -> pushAggCrossUnion -> splitPartialAgg`。`recurse` 反向调用 `aggPushDown` 形成树遍历。

下游类型主要来自同 crate 的 `rule_aggregation_elimination` 与 `task` 模块；`Cargo.toml` 没有为本文件增加专属外部依赖。crate 本身是 workspace 成员式包，`lib.rs` 为库入口、`autotests = false`，测试通过 `#[cfg(test)] mod rule_aggregation_push_down_test` 显式接入。

Go 生产上游是 `pkg/planner/core/optimizer.go` 的 `optRuleList`；Go 规则调用完整 `base.LogicalPlan`、`logicalop`、`expression/aggregation` 和 session 变量。该链只能作为语义对照，不能证明 Rust 简化类型已经接入相同运行时。

## 错误处理与边界

`Optimize`、`aggPushDown` 及 UNION 辅助函数使用字符串错误传播。`pushAggCrossUnion` 在遇到不可分解函数时返回 `"aggregate cannot be decomposed across UNION ALL"`；正常入口已先做同样检查，因此该错误主要保护直接公开调用或未来调用顺序变化。Join 的不适用情形通常以 `(原计划, false)` 或 `None` 静默表示“规则不生效”，而不是错误。

关键拒绝边界包括：带排序项的聚合、Join 白名单外函数、Join 上的 DISTINCT `SUM/COUNT`、函数同时引用左右孩子、外连接可空侧上的非纯列参数、Semi/AntiSemi Join、多层直接 Join 孩子、全 `FirstRow`、以及已由唯一键保证分组唯一的孩子。

`tryToPushDownAgg` 对分组列类型使用 `expect("group-by column must belong to the child schema")`；传入越界分组列会 panic，而不是返回错误。构造 output schema 时相同越界列又会被 `filter_map` 跳过，因此调用者必须维持列下标属于孩子 schema 的不变量。空分组会强制添加常量分组表达式，以避免孩子侧聚合生成单行默认值而改变 Join 行为。

当前实现还存在明确的迁移边界：未读取 `AllowAggPushDown` 会话开关；未执行 Go 的聚合消除、投影穿越、外连接可空标记重置、默认值挂接、Join key 信息重建或表达式列替换；Join 的非等值/左右条件也未进入孩子分组键。扩展或接线前必须先决定这些差异是简化模型约束还是待移植行为。

## 并发与资源生命周期

规则完全同步执行，不创建线程、异步任务、锁、通道、文件句柄或事务。计划以所有权传入并通过模式匹配拆解，再构造新树返回；临时 `Node` 仅用于安全取出 `LogicalAggregation.child`，不会留在最终正常返回的计划中。

`AggregationPushDownSolver` 持有可变列 ID 计数器，因此一次优化需要 `&mut self`。类型没有内部同步；如果调用方跨线程共享实例，必须自行互斥。更稳妥的生命周期是一棵计划使用一个求解器实例，以免跨计划累计的 `next_column_id` 与外部列 ID 空间产生冲突。计划节点、schema、函数描述会按需 clone，宽计划或多分支 UNION 的内存/复制成本随节点和表达式数量增长。

## 与 Go 版本的对应关系

Rust 方法名基本逐一对应 `pkg/planner/core/rule_aggregation_push_down.go`：分解白名单、函数侧归属、Join 类型门槛、`COUNT -> SUM`、唯一键收益判断、UNION ALL 分支下推和递归遍历均保留了核心意图。`pkg/planner/core/rule_aggregation_push_down_test.rs::decomposable_function_lists_match_go` 直接验证两类白名单；其余测试验证父层 Final 改写、COUNT/SUM 对另一侧下推的抑制，以及常量聚合在 Inner Join 中归右。

两者不是等价的完整数据结构移植。Go `AggregationPushDownSolver` 嵌入 `aggregationEliminateChecker` 并在生产 `optRuleList` 注册；Rust 类型只保存 `next_column_id`，当前直接调用者仅测试。Go 通过 session 的 `AllowAggPushDown` 控制 Join/UNION 下推，Rust 无此参数。Go 会从等值、左右及 other Join conditions 收集分组列，Rust 只从聚合分组项和 `equal_conditions` 收集。Go 还处理 Projection、`LogicalPartitionUnionAll`、表达式替换、外连接 null-generating 侧类型标记与默认值、key info 重建，以及下推前后的聚合消除；本文件未实现这些完整行为。

模式拆分也更简化：Go 使用 `physicalop.BuildFinalModeAggregation`，可表达 Partial1/Partial2 等模式和多输出分解；Rust `decompose` 固定每个函数产生一个 Partial 和一个 Final。Rust 的 UNION 白名单允许 `Avg`，但 `splitPartialAgg` 仅切模式，没有在本文件内展示 Go 构建器所做的 SUM/COUNT 多结果拆分，故不能据此宣称完整 AVG 语义已与 Go 等价。

Go 的 `Optimize` 当前返回的 `planChanged` 初始化为 `false` 且未更新；Rust 则实际返回是否产生 Partial/Final 改写。这是接口可观察差异。Go 相关回归入口为 `pkg/planner/core/logical_plans_test.go::TestEagerAggregation` 及其 testdata；它验证完整 Go 优化链，不直接执行此 Rust 文件。

## 扩展指南

新增聚合函数时，先分别评估 Join 与 UNION ALL 的代数可分解性，再同步修改 `isDecomposableWithJoin` / `isDecomposableWithUnion`；若 Final 不是对单个 Partial 值应用同名函数，还必须扩展 `decompose` 或改用能产生多输出的描述结构。至少在独立文件 `pkg/planner/core/rule_aggregation_push_down_test.rs` 增加允许、拒绝、DISTINCT、ORDER BY 和空输入用例，不能把测试内嵌到生产源文件。

扩展 Join 表达式支持时，优先修改列提取模型，而不是只放宽白名单：`getAggFuncChildIdx` 和 `collectGbyCols` 当前只能识别顶层 `Expression.column`。新增非等值条件处理需同步 `aggPushDown` 的分组列收集，并覆盖 LeftOuter/RightOuter 可空侧、常量参数及 COUNT/SUM 连接倍增回归。

若要接入生产 Rust 优化主链，应先定位 `optimizer_runtime.rs` 的现有聚合下推路径，避免并行规则重复改写；还需明确会话开关、全局列 ID 分配、schema/key/nullability 更新和规则顺序。接线不能只注册 `Optimize`，因为当前简化 `LogicalPlan` 与完整运行时计划类型不同。

性能方面应监控 UNION 分支 clone、schema clone、`Vec::contains` 去重的二次复杂度，以及过宽分组键抵消预聚合收益。正确性方面最敏感的是列下标重映射、外连接空匹配默认值、复杂表达式列依赖和 AVG/多输出聚合分解。

## 验证依据

- 源码全貌：`pkg/planner/core/rule_aggregation_push_down.rs`，核对了 `AggregationPushDownSolver` 的全部公开方法、私有 `recurse` 与 `add_unique`，文件无条件编译分支、模块级常量或 trait 实现。
- 类型与计划模型：`pkg/planner/core/rule_aggregation_elimination.rs`、`pkg/planner/core/task.rs`；确认 `Result<String>`、`LogicalPlan`、聚合描述符、表达式列下标和 Join 枚举的真实定义。
- crate 与模块装配：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`；确认 crate 名、库入口、显式测试模块以及模块公开性。目标包根目录无 `doc.go`，最近发现的 `pkg/planner/core/base/doc.go` 属于子 crate/package，并非本文件所属包契约。
- RustCodeGraph：执行了 `status`（索引含 11,467 文件、307,296 节点、1,848,419 边）、`query AggregationPushDownSolver`、`node rule_aggregation_push_down.rs::AggregationPushDownSolver`、`callers`、`callees`。图确认测试导入及主要内部调用边；`files --filter`/首次 `explore` 未返回目标结果，故按技能规则对图未覆盖部分使用源码和 `rg` 补证。
- Rust 测试：`pkg/planner/core/rule_aggregation_push_down_test.rs` 的四个测试覆盖函数白名单、Join 下推的 Partial/Final 改写、跨侧 COUNT/SUM 限制和常量聚合归属。
- Go 对照与回归：`pkg/planner/core/rule_aggregation_push_down.go`、`pkg/planner/core/optimizer.go`、`pkg/planner/core/logical_plans_test.go::TestEagerAggregation`；用于核对注册位置、完整 Go 分支及与 Rust 简化实现的差异。
- 调用点核验：仓库范围 Rust 文本搜索仅找到目标定义、独立测试使用和 `optimizer_runtime.rs` 的对照注释，未发现生产注册或调用；因此文档将生产接线标为“未验证/当前未找到”，没有推断已支持。
- 本任务是纯文档分析，按总计划不运行 Cargo；完成条件使用任务指定的 11 章节结构命令验证。
