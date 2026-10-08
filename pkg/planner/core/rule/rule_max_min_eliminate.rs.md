# `pkg/planner/core/rule/rule_max_min_eliminate.rs`

## 文件定位

本文件属于 `astersql-planner-core-rule` crate（同目录 `Cargo.toml`），由 `lib.rs` 以 `pub mod rule_max_min_eliminate` 公开。它在 `rule_init.rs` 定义的精简逻辑计划 IR 上实现 `MAX`/`MIN` 标量聚合消除：保留聚合外壳，把输入缩减为排序后的至多一行；多个聚合满足索引前提时，拆成多路聚合并用 Inner Join 合并。

文件必须分成两部分理解。第 16～281 行是注释中的 Go 迁移草稿，不参与 Rust 编译；第 282～461 行才是当前可执行实现。可执行部分只依赖 crate 内的 `AggKind`、`Expr`、`FieldType`、`LogicalRule`、`Plan` 和 `PlanKind`，不直接使用 `Cargo.toml` 中列出的外部 planner/operator/ranger 依赖。

该精简规则不是完整 Rust SQL 优化主链当前执行的实现。生产式逻辑计划入口 `pkg/planner/core/optimizer_runtime.rs` 的 `LogicalRule::MaxMinEliminate` 分支调用的是该文件外的 `eliminate_single_max_min_descendants`；本文件的 `MaxMinEliminator` 当前直接调用证据来自独立规则测试和综合规则测试。Go 主链则在 `pkg/planner/core/optimizer.go::optRuleList` 中注册 `rule.MaxMinEliminator`。

## 核心职责

- `MaxMinEliminator` 提供稳定规则名 `"max_min_eliminate"`，并通过统一 `LogicalRule` 接口接收、返回 owned `Plan`。
- `eliminate` 后序遍历计划树，只对无 `GROUP BY`、聚合列表非空、恰有一个子节点、且所有聚合都是单参数 `MAX`/`MIN` 的 `Aggregation` 执行改写。
- 单聚合不要求索引：有列引用的参数被包装为 `Sort`，随后总是尝试增加 `Limit { count: 1 }`，原 `Aggregation` 保留以保证空输入仍产生聚合结果。
- 多聚合只接受裸 `Expr::Column`，且每个列都必须能由 `DataSource` 的首列索引提供顺序；通过检查后，每个聚合得到独立的 `Aggregation -> Limit -> Sort -> 克隆输入` 分支，最终由无连接条件的 Inner Join 合并。
- `index_can_produce_order` 提供当前精简 IR 的保守索引判定：支持直接 `DataSource`，或穿过一层及多层 `Selection` 递归到数据源。

文件不负责 SQL 解析、真实 access path/range 构造、物理计划选择或聚合执行。注释草稿描述的 Go 完整能力也不能视为当前可执行 Rust 已支持。

## 主要符号

- `pub struct MaxMinEliminator`：无字段、无共享状态的公开规则对象。
- `impl LogicalRule for MaxMinEliminator`：
  - `name(&self) -> &'static str` 返回 `"max_min_eliminate"`，与 `rule_init.rs::default_rule_names` 中的阶段名一致。
  - `optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String>` 调用 `eliminate` 原地改写计划，但有意固定返回 `changed = false`，对齐 Go `Optimize` 不请求额外优化轮次的契约。
- `fn eliminate(plan: &mut Plan) -> bool`：文件私有的后序递归及全部结构改写入口。其内部布尔值用于递归汇总与放弃当前节点时保留“子树已改写”信息，但公开 `optimize` 不向外暴露该值。
- `fn index_can_produce_order(plan: &Plan, expression: &Expr) -> bool`：文件私有索引顺序判定；要求表达式恰好引用一个列，并要求某个索引列向量的第一项等于该列 ID。

文件没有模块级常量、自定义错误类型、条件编译项、异步函数或实际外部 I/O。第 16～281 行出现的 Go 风格方法名只是注释证据，不是 Rust 符号。

## 执行流程

1. `optimize` 取得整棵 `Plan` 的所有权，以可变引用调用 `eliminate`，忽略其内部返回值，最后返回 `Ok((plan, false))`。
2. `eliminate` 首先递归所有子节点并汇总是否发生内部改写，因此父节点判断发生在子树之后。
3. 当前节点不是 `Aggregation` 时直接返回子树结果；是聚合但存在分组、没有聚合函数、或子节点数不是 1 时也不改写当前节点。
4. 逐个验证聚合：参数数必须等于 1，种类必须为 `AggKind::Max` 或 `AggKind::Min`。`distinct` 字段不参与拒绝判断，因为 `MAX/MIN DISTINCT` 与普通 `MAX/MIN` 在结果上等价。
5. 单聚合路径先取走唯一子节点。参数引用至少一列时构造 `Sort`：`MAX` 用字符串函数名 `"desc"`，`MIN` 用 `"asc"`，排序表达式的占位返回类型为 `FieldType::Bool`；常量参数不增加 Sort。随后构造 `Limit { count: 1 }` 并设为原聚合的唯一子节点。
6. 多聚合路径克隆聚合列表并保存输出 schema。每个聚合参数必须是裸 `Expr::Column`，且 `index_can_produce_order` 必须成功；任一失败就把先前取走的原输入放回，整个当前聚合保持原形，但已完成的子树改写不会回滚。
7. 每个通过检查的聚合生成独立分支：克隆原输入，按聚合种类排序，加 `Limit 1`，再套单个聚合函数的 `Aggregation`。分支 schema 从原输出 schema 的同一偏移取至多一个列 ID。
8. 所有分支成功后，用一个 `PlanKind::Join { join_type: Inner, equal_conditions: [], other_conditions: [] }` 替换原聚合。该节点可含多个 children，语义为多路笛卡尔积；schema 按分支顺序拼接。

## 数据与状态

规则操作 `rule_init.rs::Plan` 的 owned 树。`kind` 决定算子，`schema` 是列 ID 列表，`children` 保存子树，`keys`、`estimated_rows` 和 `used_stats` 是当前精简 IR 的派生元数据。算法没有全局状态；所有临时状态都在递归栈和局部 `Vec` 中。

单聚合生成的 Sort 继承输入的 schema、keys、估算行数和统计摘要；Limit 保留 schema，但把 keys 清空、估算行数设为 `1.0`、统计摘要设为空。多聚合的每个聚合分支同样把 keys 和统计摘要清空并设一行估算，最终 Join 也将 keys/统计清空、估算行数设为 `1.0`。这些是当前代码的显式赋值，不代表经过完整基数估算或 key 推导。

索引模型是 `DataSource.indexes: BTreeMap<i64, Vec<i64>>`。判定只看每个索引列向量的第一列，不使用索引 ID；`BTreeMap` 提供稳定遍历顺序，但这里只需要 `any` 的布尔结果。Selection 的 `predicates` 不参与可下推性或等值前缀分析。

重要不变量包括：聚合外壳保留，从而空输入仍由聚合产生一行；多聚合必须全部通过才改写当前节点；失败路径会恢复唯一输入；输出分支顺序与原聚合函数顺序一致。当前 `FieldType` 没有 ENUM/SET 变体，也没有 nullable flag，因此本文件无法表达 Go 版本相应的拒绝和非空过滤语义。

## 依赖与调用关系

直接下游依赖只有 `crate::rule_init`：`LogicalRule` 提供统一入口，`Plan`/`PlanKind` 表示计划树，`AggKind` 表示聚合种类，`Expr::columns` 收集表达式列，`FieldType::Bool` 用于构造排序方向占位表达式。`eliminate` 调用自身和 `index_can_produce_order`；后者也可穿过 `Selection` 递归调用自身。

模块与上游证据如下：

- `pkg/planner/core/rule/lib.rs` 公开本模块，并在 `#[cfg(test)]` 下装配 `rule_max_min_eliminate_test.rs` 与 `rule_aster_unit_test.rs`。
- `rule_max_min_eliminate_test.rs` 直接构造 `MaxMinEliminator`，覆盖多聚合索引改写、单聚合无索引改写和空聚合不改写。
- `rule_aster_unit_test.rs::scalar_max_min_does_not_require_an_index` 再次验证单聚合不要求索引且公开 changed 标志为 false。
- `rule_init.rs::default_rule_names` 包含 `"max_min_eliminate"`，但它只是名称顺序清单，没有构造或调用本类型。
- 全仓库 Rust 精确检索未找到测试之外对 `MaxMinEliminator` 的构造调用。完整逻辑优化主链在 `optimizer_runtime.rs::LOGICAL_RULES` 中有 `MaxMinEliminate` 阶段，但 match 分支调用其本地 `eliminate_single_max_min_descendants`，不是本文件的精简 `eliminate`。
- Go 主链 `pkg/planner/core/optimizer.go` 在投影消除之后、常量传播之前注册 `&rule.MaxMinEliminator{}`。

`Cargo.toml` 声明 crate 名、`lib.rs` 入口及普通/Windows 条件依赖；本文件实际编译代码没有直接引用这些外部 crate。目标目录不存在 `doc.go`，因此没有更近的包级契约。

## 错误处理与边界

`optimize` 的统一接口允许返回 `String` 错误，但本实现没有错误分支，始终返回 `Ok`。不满足改写前提时采用保守 no-op，而不是报错：包括有 GROUP BY、空聚合、子节点数不是 1、非 MAX/MIN、多参数聚合、多聚合参数不是裸列、缺少首列匹配索引，以及索引检查遇到 Selection/DataSource 以外的节点。

边界与当前限制包括：

- 单聚合参数只要求“一个聚合参数”；该参数可以是复杂表达式。只要 `Expr::columns()` 非空就排序，完全无列引用则只加 Limit。
- 多聚合索引判定不分析 Selection 条件是否能转成 access conditions，不支持等值条件固定索引前缀后由后续列提供顺序，也不区分整数句柄、公共句柄、索引长度或 range 构造错误。
- 当前实现不为 nullable 参数增加 `IS NOT NULL` 过滤；也无法识别 ENUM/SET 并拒绝按值排序。这些能力存在于 Go 对照和 `optimizer_runtime.rs` 的完整计划实现中，不存在于本文件可执行部分。
- 精简 `PlanKind` 没有 CTE 变体，因此本文件会遍历所有可表示子树；Go 和完整 Rust 运行时会在 CTE 边界停止。
- 多聚合生成一个多子节点 Join，而 Go 逐次构造左深二叉 Join；精简 IR 测试只验证两分支，更多分支的消费者兼容性未由相关测试证明。
- `output_schema.get(index)` 在 schema 短于聚合列表时产生空分支 schema而不报错；输入计划结构正确性依赖上游构造。
- 深计划使用递归，代码没有显式深度限制；异常深树可能增加栈使用。

## 并发与资源生命周期

该规则同步、单线程、纯内存执行，不创建线程、异步任务、锁、通道、事务、文件或网络资源。`MaxMinEliminator` 无字段，可重复构造；一次调用中的所有可变状态由该调用独占。

`optimize` 获取计划所有权，`eliminate` 通过独占 `&mut Plan` 后序修改。单聚合移动原子树并按需克隆 schema、keys、统计摘要；多聚合为每个分支深克隆整个原输入，因此时间和内存成本随聚合数乘以输入子树大小增长。分支、旧节点及临时表达式由 Rust 所有权在离开作用域时自动释放，无显式清理协议。

多聚合失败时，已经构造的局部分支随返回路径释放，原 `source` 被重新放回 `plan.children`。该恢复只针对当前聚合节点取走的输入；后序递归已经对该输入内部完成的合法改写会保留。

## 与 Go 版本的对应关系

Rust `MaxMinEliminator`、规则名和公开 `changed = false` 契约对应 Go `rule_max_min_eliminate.go` 的同名类型、`Name` 与 `Optimize`。两者都后序处理子树，要求标量聚合全部为 MAX/MIN，单聚合无条件采用 Limit 改写，多聚合要求每个聚合列能利用索引顺序，并保留聚合外壳处理空输入。

当前精简 Rust 对应关系并不完整：

- Go `eliminateSingleMaxMin` 会根据类型 flag 为 nullable 参数增加 `NOT(IS NULL(arg))` Selection；本文件没有 nullable 元数据和该 Selection。
- Go 会拒绝 ENUM/SET 参数，因为排序算子与聚合对这两类值的排序语义不同；精简 `FieldType` 无法表达这两类。
- Go `checkColCanUseIndex` 会累计 Selection 条件，调用 ranger 判断所有条件是否能下推，并允许等值前缀后的索引列提供顺序；还处理 int handle 和 common handle。Rust 只判断索引第一列是否等于目标列，并忽略谓词。
- Go `cloneSubPlans` 针对 Selection/DataSource 有选择地深浅拷贝 access path、schema 和列元数据；Rust 因 owned `Plan` 模型直接深克隆整个输入。
- Go 拆分聚合后调用真实 `PruneColumns`，失败就放弃；Rust 仅按输出偏移构造单列 schema，没有列裁剪或错误通道。
- Go 用一系列二叉 Inner Join 合并分支并构造真实 join schema；Rust 用一个允许多 children 的 Join 并拼接列 ID。
- Go 明确跳过 CTE；精简 IR 没有相应节点。

完整 Rust 主链中的 `optimizer_runtime.rs::eliminate_single_max_min_descendants` 使用真实 logicalop/表达式类型，支持 CTE 边界、ENUM/SET 拒绝、nullable 过滤、Sort 和 Limit，并可能传播表达式构造错误；但它当前只接受单个聚合函数，不实现 Go 的多聚合拆分。因此本文件是可运行的精简语义模型和测试面，不应被描述为完整生产实现的直接入口。

## 扩展指南

- 修改精简规则时以 `eliminate` 为主要接入点，以 `index_can_produce_order` 为多聚合索引能力接入点；同步更新独立文件 `rule_max_min_eliminate_test.rs`，跨规则边界场景可放入 `rule_aster_unit_test.rs`，不要把测试写回生产源文件。
- 新增计划类型、nullable/type flag、句柄列或 access condition 语义前，应先扩展 `rule_init.rs` 的 `Plan`/`PlanKind`/`Expr`/`FieldType` 数据契约，再逐项对照 Go；不要依据文件前半的注释草稿虚构尚不存在的外部类型接线。
- 若增强 `index_can_produce_order`，至少覆盖 Selection 谓词不可下推、复合索引等值前缀、int/common handle、非首列以及无索引路径。错误或过宽判定会把一次全局聚合复制成多路错误计划，属于正确性风险。
- 若加入 NULL 过滤或 ENUM/SET 拒绝，需保持空输入仍返回一行 NULL、全部 NULL 输入、NOT NULL 列、常量表达式和 DISTINCT 的语义，并与 `optimizer_runtime.rs` 的真实计划实现避免漂移。
- 若让本类型进入完整优化器，必须明确精简 `Plan` 与 `logicalop::LogicalPlanRef` 的桥接或替换关系，同时检查 `optimizer_runtime.rs::LOGICAL_RULES`、`LogicalRule::MaxMinEliminate` 分支和现有 SQL 级测试；仅保留 `default_rule_names` 中的字符串不构成接线。
- 多聚合扩展应验证三路以上的 Join 形状、schema、列裁剪和消费者是否接受 n-ary Join；如需对齐 Go，应改为左深二叉链并补充结构测试。
- 性能风险主要来自每个聚合深克隆输入子树；宽计划或聚合函数较多时，应在不共享可变状态、不破坏分支独立性的前提下评估更轻量的克隆表示。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/planner/core/rule/rule_max_min_eliminate` 未找到目标文件，`query MaxMin --limit 30 --json` 也未命中本规则，只返回其他 Max/Min 符号；因此没有把图缺失解释为“无符号/无调用者”，按技能规则回退到精确源码与文本检索。
- 目标实现：`pkg/planner/core/rule/rule_max_min_eliminate.rs`，确认注释草稿边界、实际 `MaxMinEliminator`、`eliminate` 和 `index_can_produce_order` 的全部可执行分支。
- 数据模型与模块边界：`pkg/planner/core/rule/rule_init.rs`、`pkg/planner/core/rule/lib.rs`、`pkg/planner/core/rule/Cargo.toml`；目标目录无 `doc.go`。
- Rust 调用/测试：`pkg/planner/core/rule/rule_max_min_eliminate_test.rs`、`pkg/planner/core/rule/rule_aster_unit_test.rs`；全仓库 Rust 精确检索确认本类型当前只被这些测试直接构造，`default_rule_names` 只有名称记录。
- 完整 Rust 主链对照：`pkg/planner/core/optimizer_runtime.rs` 的规则枚举、顺序、分派和 `eliminate_single_max_min_descendants`；`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs::max_min_elimination_adds_null_filter_order_and_limit` 验证完整计划上的 nullable 过滤、降序和 Limit 1。
- Go 对照：`pkg/planner/core/rule/rule_max_min_eliminate.go`、`pkg/planner/core/rule/rule_max_min_eliminate_test.go`、`pkg/planner/core/optimizer.go`；Go 独立回归确认空聚合不 panic、无错误、changed 为 false 且保持原节点。
- 人工复核重点：单/多聚合分支、失败恢复、索引判定、固定公开 changed 标志、数据克隆与 schema 构造，以及精简规则和完整运行时实现的边界。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令确认文档存在且恰好包含 11 个固定二级标题。
