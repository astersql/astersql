# `pkg/planner/core/rule/rule_outer_join_to_semi_join.rs`

## 文件定位

本文件位于 `astersql-planner-core-rule` crate，模块由 [`lib.rs`](lib.rs) 公开为 `rule_outer_join_to_semi_join`。它实现一套面向 [`rule_init.rs`](rule_init.rs) 精简计划 IR 的外连接改写规则：识别 `Selection -> Left/Right Outer Join`，在严格条件下把 Join 改为 `AntiSemi`。crate 名称、库入口和直接依赖见 [`Cargo.toml`](Cargo.toml)；本文件实际编译代码只依赖同 crate 的 `rule_init::{Expr, JoinType, LogicalRule, Plan, PlanKind}`，没有直接使用 Cargo 清单中的 operator crate。

文件第 16—411 行是以注释形式保存的 Go 来源说明和伪签名，不参与编译；真正 Rust 实现从 `use crate::rule_init...` 开始。它也不是完整 Rust 优化器的直接执行实现：[`optimizer_runtime.rs`](../optimizer_runtime.rs) 的 `LogicalRule::OuterJoinToSemiJoin` 分支调用另一套 `outer_join_to_semi_join_descendants`，操作真实 `logicalop::LogicalPlanRef`。因此本文把本文件称为“精简 IR 规则”，不能把它的覆盖范围等同于应用主链。

同目录没有 `doc.go`，故包级契约以模块入口、规则注册、Go 对照和测试为准。

## 核心职责

- `OuterJoinToSemiJoin` 通过 `LogicalRule` 暴露稳定名称 `"outer_join_to_semi_join"` 和优化入口。
- `convert` 后序遍历精简 `Plan` 树，使深层候选先于父节点被处理。
- 只对恰有一个子节点和一个谓词的 `Selection` 尝试改写；其直接子节点必须是有连接条件的二元 Left/Right Outer Join。
- 谓词必须严格为一元 `is_null(直接列引用)`，列必须只属于 Join 的 inner 子树。
- 被测 inner 列还必须出现在可判定为拒空的列间 Join 比较中：等值条件接受除 `null_eq` 外的二元直接列比较；other 条件仅接受 `gt/ge/lt/le/ne`。
- Right Outer Join 改写前会交换两个孩子，并交换每个二元等值条件的参数，使原右侧（保留侧）成为 AntiSemi 的第一个孩子。
- 改写后 Join 和 Selection 的 `schema` 都缩为第一个孩子的 schema，并报告 `changed = true`。

当前代码并未删除 Selection，也未清除它的 `is_null` 谓词；它不支持 Selection 与 Join 之间的 Projection、不支持 Go 的 inner `NOT NULL` 推断分支，也不为被移除的 inner 输出列生成 NULL Projection。这些是本文件当前事实，不应由顶部注释中的完整 Go 算法推断为已经实现。

## 主要符号

- `pub struct OuterJoinToSemiJoin`：无字段的公开规则类型，没有实例状态。
- `impl LogicalRule for OuterJoinToSemiJoin`：
  - `name(&self) -> &'static str` 返回 `"outer_join_to_semi_join"`。
  - `optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String>` 取得计划所有权，调用 `convert(&mut plan)`，总是返回 `Ok((plan, changed))`。
- `fn convert(plan: &mut Plan) -> bool`：文件私有的后序递归和改写主体；返回当前子树是否发生过至少一次改写。
- `fn column_comparison_rejects(condition: &Expr, inner_column: &i64, allowed_functions: Option<&[&str]>) -> bool`：文件私有条件检查器，只接受“二元 Scalar 且两侧均为直接 Column”的形状，并按可选函数白名单和 `null_eq` 例外判断目标 inner 列是否参与比较。

本文件没有模块级常量、条件编译项、自定义错误类型、异步函数或 unsafe 代码。公开 API 只有规则类型及其 trait 方法；两个辅助函数均为模块私有。

## 执行流程

1. `optimize` 把 owned `Plan` 以独占可变引用交给 `convert`。
2. `convert` 先遍历全部 `children`。折叠表达式 `convert(child) || seen` 保留任一后代的改写结果，同时不会因 `seen == true` 跳过后续孩子。
3. 当前节点若不是 Selection、孩子数不是 1、谓词数不是 1，立即返回后代累计的 `changed`。
4. 检查唯一孩子是否为 Join 且 `equal_conditions` 或 `other_conditions` 至少一项非空；无条件 Join 不改写。
5. 根据 Join 类型决定 inner 下标：Left Outer 的 inner 是孩子 1，Right Outer 的 inner 是孩子 0；其他 Join 类型和非二元 Join 均退出。
6. 从两个子计划的 `schema` 构造 inner/outer 列集合。谓词必须匹配 `Expr::Scalar { function: "is_null", args: [Expr::Column { id, .. }], .. }`，并同时满足 inner 含该 ID、outer 不含该 ID。
7. 遍历 Join 条件证明该 inner 列被拒空：
   - `equal_conditions` 调用 `column_comparison_rejects(..., None)`，即函数名不限，但明确拒绝 `null_eq`，且要求二元直接列对；
   - `other_conditions` 使用 `gt/ge/lt/le/ne` 白名单。
8. 若没有任何条件通过，保留原节点，只返回后代的改写状态。
9. Right Outer 分支逐个交换长度为 2 的等值条件参数，再交换两个 Join 孩子；other 条件不重排。
10. 将 Join 类型改为 `AntiSemi`，把 Join schema 设为第一个孩子 schema，再把 Selection schema 设为同一 schema；Selection 节点和谓词仍保留。最终返回 `true`。

## 数据与状态

规则读取和修改 [`rule_init.rs`](rule_init.rs) 的 owned `Plan`：`kind` 保存算子及 Join 条件，`children` 保存子树，`predicates` 保存 Selection 谓词，`schema: Vec<i64>` 同时承担输出列集合和列归属判断。`Plan::all_columns()` 实际只把当前节点 `schema` 转成 `BTreeSet`，不会递归扫描后代。

关键不变量与假设如下：

- 计划由 owned `Vec<Plan>` 表示，形成有限无环树；后序遍历确保嵌套候选先改写。
- inner/outer 归属仅凭列 ID 是否存在于两个孩子 schema 判断。目标 ID 若也出现在 outer schema，会被拒绝，以避免共享/冲突 ID 导致错误归属。
- `column_comparison_rejects` 不校验比较两端分别来自 inner 与 outer；只要两端均为直接列且其中一端 ID 等于目标 inner 列即可。它也不验证 `equal_conditions` 的函数名确为 `eq`，只排除 `null_eq`。这是精简 IR 当前边界。
- Right Outer 规范化只交换等值表达式参数与孩子，不存在 Go `LogicalJoin` 的 LeftConditions/RightConditions 字段可同步。
- 改写覆盖 Join/Selection 的 schema，但不改 `predicates`、`keys`、`estimated_rows`、`used_stats`，也不调整 Selection 的孩子数量。
- `changed` 是整棵子树的“是否发生结构/类型改写”汇总；即使当前节点不匹配，后代改写仍会使其为 true。

## 依赖与调用关系

上游装配与调用证据：

- [`lib.rs`](lib.rs) 公开生产模块，并在 `#[cfg(test)]` 下装配 [`rule_outer_join_to_semi_join_test.rs`](rule_outer_join_to_semi_join_test.rs) 和综合测试 [`rule_aster_unit_test.rs`](rule_aster_unit_test.rs)。RustCodeGraph 也把这两份文件列为本文件的使用者。
- 测试通过 `OuterJoinToSemiJoin.optimize(plan)` 直接进入本实现。
- [`rule_init.rs`](rule_init.rs) 的 `default_rule_names()` 包含同名字符串，表达精简规则顺序，但没有构造或运行规则对象的调度器。
- 应用主优化器的 Go 注册位于 [`optimizer.go`](../optimizer.go)：`optRuleList` 在 Join reorder 后注册 `&rule.OuterJoinToSemiJoin{}`，对应 `FlagOuterJoinToSemiJoin`。
- 完整 Rust 优化器枚举和顺序位于 [`optimizer_runtime.rs`](../optimizer_runtime.rs)，但其分支直接调用该文件内的 `outer_join_to_semi_join_descendants`，没有调用这里的 `OuterJoinToSemiJoin::optimize`。这是同名语义的另一实现，不是本文件调用者。

本文件内部调用链为 `LogicalRule::optimize -> convert`，`convert` 递归调用自身并调用 `column_comparison_rejects`。下游只依赖精简 IR 的 `Plan::all_columns`、Join/表达式枚举和标准集合操作，没有 I/O 或外部服务。

## 错误处理与边界

`LogicalRule::optimize` 的统一签名允许 `Err(String)`，但本实现没有失败路径，始终返回 `Ok`；不满足前置条件均采取“不改写”的保守策略。明确拒绝的情况包括：非 Selection、非单孩子/单谓词、无 Join 条件、非 Left/Right Outer、非二元 Join、非 Scalar 或非 `is_null`、参数不是唯一直接 Column、列不唯一属于 inner、没有拒空比较、`null_eq`、other 条件函数不在白名单、比较参数不是两个直接 Column。

[`rule_outer_join_to_semi_join_test.rs`](rule_outer_join_to_semi_join_test.rs) 覆盖两项关键边界：表达式参与等值条件而非直接列对时不改写；Right Outer 改写会交换孩子和等值参数。[`rule_aster_unit_test.rs`](rule_aster_unit_test.rs) 还覆盖无 Join 条件不改写，以及有效 Left Outer inner key 会变成 AntiSemi。测试没有验证改写后仍保留的 Selection 谓词是否可对缩减 schema 解析，也没有覆盖 other 条件白名单、`null_eq`、共享列 ID、多层树或异常参数数目。

由于本任务是只读逻辑说明，未把这些覆盖缺口解释为待修复缺陷；它们是扩展和验证时需要正视的风险。完整应用主链另由 `optimizer_runtime.rs` 的 SQL 入口测试覆盖，不可用来证明本精简实现的每个细节。

## 并发与资源生命周期

规则同步、单线程、纯内存运行。`OuterJoinToSemiJoin` 无字段，不持有共享状态、锁、原子变量、通道、任务、事务或外部句柄。`optimize` 取得整棵计划所有权，`convert` 通过独占 `&mut Plan` 逐节点修改；Rust 借用规则阻止同时别名写入。

递归深度与计划树高度一致，代码没有显式深度限制或迭代回退，极深人工计划存在栈增长风险。每个候选 Selection 会为两个孩子 schema 分配 `BTreeSet`；Right Outer 会原地交换表达式参数和子计划，不复制整棵子树。退出作用域后临时集合自动释放，不存在显式清理或回滚流程。若改写进行到 schema 更新，当前函数没有后续可能失败的操作，因此无需事务性恢复。

## 与 Go 版本的对应关系

直接语义来源是 [`rule_outer_join_to_semi_join.go`](rule_outer_join_to_semi_join.go)。共同点是：规则名一致；后序查找 Selection；要求单一 `IS NULL` 条件和有条件的 Left/Right Outer Join；排除 `NullEQ`；只把直接列间比较视作拒空条件；Right Outer 需要交换孩子和等值参数；成功后变为 AntiSemi。

主要差异必须明确：

- Go 操作完整 `base.LogicalPlan`，能处理 `Selection -> Projection -> Join` 的恒等列投影；本文件只认 Selection 直接连接 Join。
- Go 的第二条安全路径会根据 inner child 的函数依赖 NotNullCols 或字段 NOT NULL flag 接受非 Join-key 列；精简 IR 没有相应元数据，本文件只支持 Join 条件拒空列。
- Go 成功后移除 Selection；若父 schema 仍需要 inner 列，则构造 Projection，以正确类型的 NULL 替代 inner 输出列。本文件保留 Selection 和原谓词，只缩减 schema，也不生成 NULL。
- Go Right Outer 分支还交换 LeftConditions/RightConditions；精简 `PlanKind::Join` 没有这些字段。
- Go 明确排除 `LogicalApply`；精简 IR 没有 Apply 类型，因此无法表达该边界。
- Go `joinCondNullRejectsInnerCol` 对 equal 条件依赖 `expression.IsColOpCol`，但未在该函数中按名称限制为普通等号；本文件同样只排除 `null_eq`。other 条件两端均限定为列且仅允许 GT/GE/LE/LT/NE。

完整 Rust 应用主链的 [`optimizer_runtime.rs`](../optimizer_runtime.rs) 更接近 Go：它会移除 Selection、支持声明 NOT NULL、按需生成 Projection，并处理真实 logicalop schema/name；它还有嵌套 Left Outer 旋转逻辑。该运行时实现是重要上下文，但不是本文件的函数或下游调用。

## 扩展指南

若扩展精简 IR 规则，首要修改点是 `convert` 的候选形状识别与成功改写尾部；比较语义集中在 `column_comparison_rejects`。安全扩展应先对照 Go 文件和 `optimizer_runtime.rs`，然后决定是否需要扩充 [`rule_init.rs`](rule_init.rs) 的数据模型，而不是凭列 ID 猜测完整语义。

- 支持 Projection 时，应只允许可证明为恒等列映射的表达式，并维护 Selection、Projection 与 Join schema 的列 ID 对应。
- 支持 inner NOT NULL 场景前，需要在精简 IR 中显式承载来源字段或函数依赖的非空性；不能从外连接输出 schema 推断，因为 inner 输出会被置为 nullable。
- 若真正删除 Selection，必须决定原输出是否包含 inner 列；包含时应生成带正确字段类型和可空性的 NULL Projection。当前 `schema: Vec<i64>` 不足以表达字段类型，可能需要先演进 IR。
- 收紧等值条件时，应考虑显式只允许 `eq`，并验证比较两侧分别来自 outer/inner；这会改变当前可接受输入，需有回归测试证明与 Go 意图一致。
- Right Outer 扩展任何侧相关字段时，必须与孩子交换同步处理；遗漏侧条件、输出名或缓存会产生方向错误。
- 保持测试与生产代码分离。直接规则测试放在 [`rule_outer_join_to_semi_join_test.rs`](rule_outer_join_to_semi_join_test.rs)，跨规则精简 IR 场景可放 [`rule_aster_unit_test.rs`](rule_aster_unit_test.rs)，真实 SQL/应用主链场景放在 [`optimizer_logical_entry_aster_unit_test.rs`](../optimizer_logical_entry_aster_unit_test.rs)。

兼容风险主要是 NULL 语义和输出 schema；错误改写会改变查询结果。性能上该规则当前每个节点访问一次，但候选 Selection 会构造集合并扫描全部 Join 条件；新增投影映射或 FD 分析时应避免无界重复遍历。规则顺序也有影响：Go/Rust 主链把它放在 Join reorder 后、Correlate 前，调整接线时应保留经验证的相互作用。

## 验证依据

本文依据以下本地证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file ... --offset/--limit` 完整读取目标文件，并确认它由 `rule_outer_join_to_semi_join_test.rs`、`rule_aster_unit_test.rs` 使用。
- RustCodeGraph `query OuterJoinToSemiJoin` 定位本文件公开类型、Go 同名类型、测试导入和运行时枚举；`explore` 定位 Go 的 `recursivePlan -> dealWithSelection -> startConvertOuterJoinToSemiJoin` 关系及 Rust 测试调用。由于 `optimize`/`convert` 是高重名符号，精确 `callers`/`callees` 未返回可用边，调用链改由已索引源码和精确文件搜索交叉核验。
- 目标实现：[`rule_outer_join_to_semi_join.rs`](rule_outer_join_to_semi_join.rs) 的实际编译区 `OuterJoinToSemiJoin`、`convert`、`column_comparison_rejects`，并人工区分前置注释中的 Go 来源。
- crate/模块边界：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`rule_init.rs`](rule_init.rs)；目标目录不存在 `doc.go`。
- Rust 独立与综合测试：[`rule_outer_join_to_semi_join_test.rs`](rule_outer_join_to_semi_join_test.rs)、[`rule_aster_unit_test.rs`](rule_aster_unit_test.rs)、[`optimizer_logical_entry_aster_unit_test.rs`](../optimizer_logical_entry_aster_unit_test.rs)。后者验证完整运行时的真实 SQL Left/Right Outer 改写，不等同于直接测试本文件。
- Go 对照和主链接线：[`rule_outer_join_to_semi_join.go`](rule_outer_join_to_semi_join.go)、[`optimizer.go`](../optimizer.go)、[`logical_rules.go`](logical_rules.go)；Rust 完整主链对照为 [`optimizer_runtime.rs`](../optimizer_runtime.rs) 的规则枚举、调度分支和 `outer_join_to_semi_join_descendants`。
- 人工复核重点：后序遍历、全部拒绝分支、Left/Right inner 方向、比较函数白名单、Right Outer 交换、schema 更新、Selection 保留事实、Go/精简 Rust/完整 Rust 三者边界及独立测试位置。

本任务为纯文档分析，不运行 Cargo。结构验证应确认本文恰有任务要求的十一个固定二级标题。
