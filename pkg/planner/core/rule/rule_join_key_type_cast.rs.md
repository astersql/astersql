# `pkg/planner/core/rule/rule_join_key_type_cast.rs`

## 文件定位

本文件属于 `astersql-planner-core-rule` crate；crate 入口 `pkg/planner/core/rule/lib.rs` 以 `pub mod rule_join_key_type_cast` 导出它。它在 `rule_init.rs` 定义的精简逻辑计划 IR（`Plan`、`PlanKind`、`Expr`、`FieldType`）上实现 Join 键类型改写规则，而不是直接操作 Go 版的 `base.LogicalPlan`/`logicalop.LogicalJoin`。

公开入口是零大小类型 `JoinKeyTypeCast` 及其 `LogicalRule` 实现。`name()` 返回稳定注册名 `join_key_type_cast`，该名称也出现在 `rule_init.rs::default_rule_names`。不过，仓库搜索到对本文件 `JoinKeyTypeCast.optimize` 的直接调用均位于 Rust 测试；真实优化器运行时在 `pkg/planner/core/optimizer_runtime.rs` 中通过另一套 `LogicalRule::JoinKeyTypeCast` 和 `rewrite_join_key_type_casts` 接线。因此，本文件当前是可独立验证的规则实现和移植语义载体，不能仅凭模块导出或默认名称清单断言它已直接驱动生产优化主链。

## 核心职责

规则解决 signed INT 与 VARCHAR 等值连接因隐式统一成 `DOUBLE` 而妨碍整数索引键识别的问题。它识别以下已经物化的形态：Join 两个孩子都是 Projection；等值条件两端是 `Float` 列；每个列都可按 Projection schema 位置追溯到 `CAST(原始列 AS Float)`；原始类型恰为一侧 `SignedInt`、另一侧 `Text`。

匹配后，规则把 Join 等值键改成整数域：整数侧追加原列直通表达式，文本侧追加 `CAST(text AS SignedInt)`，再把等值条件参数替换为这两个整数键。同时在文本侧 Projection 下插入 Selection，使用 `CAST(CAST(text AS SignedInt) AS Float) = CAST(text AS Float)` 过滤无法无损进入整数比较域的文本值。这样保留 Go 规则的核心语义：只对可安全按整数比较的文本参与连接。

规则刻意跳过非 `eq`（包括 null-safe equality）、非二元条件、非双 `Float` 键、缺失 Projection 模式、非 signed-int/text 配对，以及文本位于外连接保留侧的情形。源码顶部还明确说明：当前表达式模型不能区分隐式 CAST 与用户显式 CAST，因此二者物化成相同 Projection 后都会被改写。

## 主要符号

- `pub struct JoinKeyTypeCast`：无状态规则标记类型，是本文件唯一公开类型。
- `impl LogicalRule for JoinKeyTypeCast`：`name()` 返回 `join_key_type_cast`；`optimize(plan)` 取得计划所有权，调用 `rewrite_join_keys(&mut plan)`，返回 `(plan, changed)`。错误类型沿 trait 统一为 `String`。
- `rewrite_join_keys(plan)`：核心后序遍历与 Join 重写入口。先递归处理孩子，再处理当前节点，使深层 Join 的新增列 ID 已计入上层扫描。
- `ProjectedCast`：内部匹配结果，记录原始表达式、原始类型及所在孩子下标；`Clone` 仅用于安全保存匹配信息，不承载共享状态。
- `column_id(expression)`：只接受 `Expr::Column`，提取列 ID；其他表达式返回 `None`。
- `projected_float_cast(plan, output_id, child_index)`：用输出列 ID 在 Projection schema 中定位同位置表达式，并验证它严格为“列到 `Float` 的 CAST”。
- `append_projection_expression(children, child_index, expression, output_id)`：同步向 Projection 表达式列表和 schema 追加新输出；调用前已验证节点类型。
- `insert_guard_selection(projection, predicates)`：把 Projection 原孩子整体迁入一个新 Selection，并保持原孩子 schema、Projection 行数估计；新 Selection 的 `keys` 为空且 `used_stats` 新建为空表。
- `maximum_column_id(plan)`：递归取整棵子树 schema 的最大列 ID，用于从 `max + 1` 分配文本侧新整数列。

文件没有模块级常量、条件编译项、异步函数或额外 trait 定义。

## 执行流程

1. `JoinKeyTypeCast::optimize` 把计划交给 `rewrite_join_keys`；后者先递归改写所有孩子，并用按位或累计 `changed`。
2. 当前节点处理前，`maximum_column_id(plan).saturating_add(1)` 计算安全的新列 ID 起点。非 Join 节点在递归结果基础上直接返回。
3. Join 必须恰有两个孩子且两者均为 Projection，否则不处理。随后根据 Join 类型计算保留侧：`LeftOuter` 和 `AntiSemi` 保留左侧，`RightOuter` 保留右侧，`Inner`、`Semi` 无保留侧。
4. 逐个扫描 `equal_conditions`。条件必须是函数名为 `eq` 的二元 `Expr::Scalar`，两端结果类型均为 `Float`，且两端都是列引用。
5. `projected_float_cast` 分别从左右 Projection 找到对应的 `CAST(column AS Float)`。原始列必须形成 `SignedInt`/`Text` 配对；若文本位于保留侧，则跳过本条件，避免 guard 删除本应由外连接保留的行。
6. 整数侧复用原列及其 ID；文本侧从当前 `next_column_id` 创建 `SignedInt` 输出列，并递增分配器。两个 Projection 分别追加原整数列和文本转整数 CAST。
7. 为文本侧累计 guard，但暂不修改树形结构。随后按原左右孩子顺序替换等值条件参数：左参数始终来自左孩子，右参数始终来自右孩子。
8. 所有等值条件处理完毕后，每个有 guard 的孩子只插入一个 Selection，其中可包含多个谓词；最后返回递归结果与当前改写结果的合并值。

重要不变量是 Projection 的 `expressions` 与 `schema` 同步追加、Join 条件左右顺序不变、每个新文本整数键获得不同列 ID、同一文本侧的多个 guard 合并进一个 Selection。

## 数据与状态

规则没有跨调用状态。所有变更都发生在 `optimize` 拥有的 `Plan` 值内；`rewrite_join_keys` 通过可变借用就地修改子树。

核心数据为：`guards: [Vec<Expr>; 2]` 按左右孩子暂存守卫条件；`next_column_id` 是当前 Join 子树内的单调、饱和递增分配器；`ProjectedCast` 是一次条件匹配期间的快照。`maximum_column_id` 仅检查 schema，不扫描表达式中的列 ID，因此其正确性依赖精简 IR 的列标识已反映在相应计划 schema 中。

新 Selection 继承其原孩子的 schema 与 Projection 的 `estimated_rows`，但不复制键和统计使用记录；这反映当前精简 IR 的构造策略。Join 自身 schema 不追加内部 Join 键，因为新增列只用于等值条件，不作为 Join 输出；这一点与 Go 实现“不调用 `MergeSchema`，等待后续列裁剪重建 schema”的意图相同。

## 依赖与调用关系

本文件的标准库依赖只有 `std::collections::BTreeMap`，用于初始化 guard Selection 的空 `used_stats`。其余依赖均来自同 crate 的 `rule_init`：表达式与类型模型 `Expr`/`FieldType`、连接种类 `JoinType`、规则接口 `LogicalRule`、计划模型 `Plan`/`PlanKind`。

直接内部调用链为 `JoinKeyTypeCast::optimize -> rewrite_join_keys`；`rewrite_join_keys` 递归调用自身，并调用 `maximum_column_id`、`column_id`、`projected_float_cast`、`append_projection_expression` 与 `insert_guard_selection`。RustCodeGraph 对 `rewrite_join_keys` 和 `projected_float_cast` 建立了函数节点，但 callers/callees 命令没有返回额外跨文件调用边；仓库文本搜索补充确认直接 `optimize` 调用位于 `rule_join_key_type_cast_test.rs` 和 `rule_aster_unit_test.rs`。

crate 边界由 `pkg/planner/core/rule/Cargo.toml` 确认：库入口为 `lib.rs`，包名为 `astersql-planner-core-rule`。本文件没有直接引用 Cargo 外部 crate；它通过 crate 内共享 IR 间接工作。`lib.rs` 还把 `rule_join_key_type_cast_test.rs` 作为独立 `#[cfg(test)]` 模块接入，符合生产逻辑与测试分文件的仓库约束。

## 错误处理与边界

绝大多数不匹配情况是正常边界，以 `continue`、`None` 或 `Ok(changed)` 静默跳过，不视为错误。这使规则保持保守：它只改写能完整证明来源的条件。递归链使用 `?` 传播 `Result<bool, String>`，但当前文件内的辅助函数都不创建 `Err`；错误通道是 `LogicalRule` 接口兼容面。

唯一的显式断言是追加 Projection 表达式时对已分类列调用 `column_id(...).expect(...)`。该断言由 `projected_float_cast` 已确认原表达式为 `Expr::Column` 的前置条件保证；若未来放宽匹配器而未同步这里，可能触发 panic。

列 ID 使用 `saturating_add`，避免整数溢出 panic；代价是若最大 ID 已为 `i64::MAX`，多个新列可能碰撞。源码没有对这一极端状态返回错误。Projection schema 与 expressions 长度不一致、输出 ID 不存在、CAST 参数不是列等情况均由 `projected_float_cast` 返回 `None`。无孩子的 Projection 插入 guard 时会给 Selection 空 schema，属于当前构造函数明确允许但通常不应由有效计划产生的边界。

语义限制还包括：精简 `FieldType::SignedInt` 不表示 Go 类型宽度，因此本文件无法复现 Go `classifyCastPair` 对 `BIGINT`（`TypeLonglong`）的额外排除；`JoinType` 也没有 Go 的 `LeftOuterSemiJoin` 和 `AntiLeftOuterSemiJoin` 变体。扩展类型模型前，不应声称这些 Go 分支已由本文件覆盖。

## 并发与资源生命周期

本规则是同步、单线程、纯内存的计划树变换：没有锁、原子变量、任务、通道、事务、网络或文件资源。`optimize` 独占 `Plan`，递归过程中 Rust 的可变借用保证同一时刻不会并发修改同一子树。

插入 guard 时，`std::mem::take` 把 Projection 原有孩子所有权移入新 Selection，再把 Selection 作为 Projection 唯一孩子装回；没有悬挂引用或额外资源清理。`guards` 和 `ProjectedCast` 均在单次 `rewrite_join_keys` 栈帧内生存，函数返回时释放。时间开销主要来自遍历计划树、每个 Join 扫描等值条件，以及 `maximum_column_id` 对当前子树的递归扫描；若树中有很多 Join，重复扫描可能形成额外开销，但当前源码未缓存最大列 ID。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/rule/rule_join_key_type_cast.go`。主要映射如下：`JoinKeyTypeCastRewriter` 对应 `JoinKeyTypeCast`；`Optimize`/`Name` 对应同名 trait 方法；`rewriteJoinTypeCasts` 与 `rewriteJoinEqConds` 的职责合并进 `rewrite_join_keys`；`projCastInfo`/`findCastInProj`/`classifyCastPair` 分别对应 `ProjectedCast`、`projected_float_cast` 和 Rust 内联的类型配对分支。

两版共享的行为包括后序遍历、要求两侧 Projection、仅处理双 DOUBLE 等值键、排除 null-safe equality、只接受 signed INT 与字符串组合、跳过文本位于外连接保留侧的情况、保持 Join 参数左右顺序、在整数侧保留原列标识、为文本转整数分配新列标识、合并同侧 guard，并且不把内部键加入 Join 输出 schema。

差异来自 IR 能力而非文档推断：Go 使用会话分配器 `AllocPlanColumnID`，Rust 通过子树最大 schema ID 分配；Go 保留真实字段类型、表达式上下文、查询块偏移和统计信息，Rust 使用精简枚举与简单字符串函数名；Go 明确排除 unsigned INT 和 BIGINT，Rust能排除 `UnsignedInt`，但 `SignedInt` 无宽度信息，不能单独排除 BIGINT；Go 覆盖更多外/半连接类型，Rust 仅覆盖 `JoinType` 已定义的五种。Go 规则已在 `optimizer.go` 的 `optRuleList` 中注册且位于谓词下推之后，Rust 本文件的生产接线则没有从直接调用搜索中得到确认，真实 Rust 运行时使用 `optimizer_runtime.rs` 的平行实现。

## 扩展指南

若新增可改写类型组合，应先修改 `rewrite_join_keys` 的 `original_type` 分类，并同步评估 guard 是否仍能证明转换无损；不要只扩展 `projected_float_cast`，否则可能把不安全的表达式送入后续 `expect`。若支持更多 CAST 形态，应保持“Projection schema 位置与 expression 位置一致”的验证，并明确区分显式/隐式 CAST 的语义。

若扩展 Join 类型，应同步更新 `rule_init.rs::JoinType` 与 `preserved_child` 判断；判断标准是 guard 能否放到非保留侧，而不是简单按 Join 名称归类。若为 `FieldType` 增加整数宽度，应对齐 Go 的 BIGINT 精度边界，尤其覆盖 `2^53 + 1` 一类 DOUBLE 无法精确表示的值。

列 ID 分配策略若改变，应修改 `maximum_column_id` 和新列创建处，并验证嵌套 Join、多个等值条件及接近 `i64::MAX` 的行为。若改变 Selection 构造，应检查 schema、keys、estimated_rows 和 used_stats 的继承策略。

测试必须继续放在独立文件，优先扩展 `pkg/planner/core/rule/rule_join_key_type_cast_test.rs`；涉及共享规则不改写的不变量可同步 `rule_aster_unit_test.rs`，涉及真实优化入口则同步 `pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs`。至少覆盖左右类型互换、多个条件合并 guard、null-safe equality、unsigned/BIGINT、各 Join 保留侧、schema/表达式错位和嵌套 Join，且与 Go 对照行为逐项核验。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标源码可由 `node --file` 完整读取。
- RustCodeGraph `query rewrite_join_keys --kind function` 与 `query projected_float_cast --kind function`：确认核心函数、签名和源码位置；`query JoinKeyTypeCast --kind struct` 同时定位 Rust 类型与 Go 对照类型。
- RustCodeGraph callers/callees 查询未返回额外调用边；因此跨文件接线结论由仓库搜索核验，并在本文中限定为“搜索到的直接调用”。
- 生产源码：`pkg/planner/core/rule/rule_join_key_type_cast.rs`；共享 IR 与 trait：`pkg/planner/core/rule/rule_init.rs`；模块入口：`pkg/planner/core/rule/lib.rs`；crate 声明：`pkg/planner/core/rule/Cargo.toml`。
- Go 对照：`pkg/planner/core/rule/rule_join_key_type_cast.go`；Go 优化器注册与顺序：`pkg/planner/core/optimizer.go`。
- 独立 Rust 测试：`pkg/planner/core/rule/rule_join_key_type_cast_test.rs` 验证成功改写、外连接保留侧跳过和无 Projection 时不改写；`pkg/planner/core/rule/rule_aster_unit_test.rs` 验证非目标数字类型保持不变；`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 验证真实逻辑计划入口把混合键恢复为整数并增加文本侧 guard。
- 人工复核结论：本文分别回答了规则为何存在、精确匹配和变换流程、当前接线边界、Go 差异、失败/跳过路径及安全扩展位置；未把源码中不存在的生产接线、类型精度或并发行为描述为已支持。
