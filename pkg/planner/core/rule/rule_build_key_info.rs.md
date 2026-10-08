# `pkg/planner/core/rule/rule_build_key_info.rs`

## 文件定位

本文件实现逻辑优化规则 `BuildKeySolver`，在 `astersql-planner-core-rule` crate 内为精简逻辑计划 IR 推导候选键信息。模块由 [`lib.rs`](lib.rs) 公开为 `rule_build_key_info`；规则操作的数据模型和统一接口来自 [`rule_init.rs`](rule_init.rs) 的 `Plan`、`PlanKind`、`JoinType`、`Expr` 与 `LogicalRule`。

这里的“key”存放在每个 `Plan.keys: Vec<Vec<i64>>` 中：外层向量表示多个候选键，内层列 ID 向量表示一个候选键的组成列。该信息是优化期元数据，不改变计划树结构或运行期行数据。本文件只负责精简 Rust IR；Go 主优化流水线中同名规则的完整实现仍通过各逻辑算子的 `BuildKeyInfo` 完成，二者覆盖面不能等同。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-planner-core-rule`，库入口为 `lib.rs`。本文件自身只直接使用 crate 内的 `rule_init`，没有直接调用 Cargo 清单中的 operator、rule-util 或其他外部 crate。

## 核心职责

- `BuildKeySolver::name` 提供稳定规则名 `"build_keys"`，与 Go `BuildKeySolver.Name` 一致。
- `BuildKeySolver::optimize` 接收一棵拥有所有权的 `Plan`，调用私有函数 `build_keys` 原地填充整棵树的 `keys`，随后返回原树和 `changed = false`。
- `build_keys` 按后序遍历处理计划树，保证父节点推导前子节点的 `keys` 已更新。
- 当前只为 `DataSource`、`Projection`、`Selection`、`Sort`、`Limit`、`Semi Join` 和 `AntiSemi Join` 定义推导规则；其余 `PlanKind` 明确得到空键集合。这是当前 Rust 代码的保守覆盖范围，不代表 Go 版本只支持这些算子。

该规则的结果可供依赖唯一性/候选键信息的后续优化使用。Go 侧的直接证据包括 [`optimizer.go`](../optimizer.go) 中 `BuildKeySolver` 位于列裁剪之后，以及 `rule_join_elimination.go`、聚合消除等逻辑读取 schema key 信息；Rust 精简流水线当前没有在本文件中声明这些消费者。

## 主要符号

- `pub struct BuildKeySolver`：无字段、无内部状态的规则对象；公开类型，可由 crate 使用者构造。
- `impl LogicalRule for BuildKeySolver`：实现 [`rule_init.rs`](rule_init.rs) 定义的规则协议。
  - `fn name(&self) -> &'static str`：返回静态规则标识 `"build_keys"`。
  - `fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String>`：取得计划所有权，填充 key 信息，固定返回 `Ok((plan, false))`。
- `fn build_keys(plan: &mut Plan)`：文件私有递归函数，也是全部推导逻辑所在。它先递归 `plan.children`，再覆盖当前节点的 `plan.keys`。

本文件没有模块级常量、条件编译项、异步函数或自定义错误类型。`BuildKeySolver` 是公开 API；`build_keys` 只允许本模块调用。

## 执行流程

1. 调用者通过 `LogicalRule::optimize` 传入计划；`optimize` 将其作为可变局部变量。
2. `build_keys` 依次递归所有 `children`。因此即使父节点最终不保留 key，所有子树仍会完成推导。
3. 根据当前 `PlanKind` 重新计算并覆盖 `plan.keys`：
   - `DataSource { indexes, .. }`：遍历 `BTreeMap` 中的索引列向量，丢弃空向量，克隆其余向量作为候选键。顺序由 `BTreeMap` 的索引 ID 排序决定。
   - `Projection { expressions }`：先要求表达式数量等于输出 `schema` 长度，否则直接清空 key。存在第一个子节点时，逐个检查其候选键；一个子键的每一列都必须能在投影表达式中找到恰好只引用该列的表达式，成功时以对应输出位置的 `plan.schema[offset]` 组成新键。任一组成列无法映射，则整个候选键被丢弃。
   - `Selection`、`Sort`、`Limit`：克隆第一个子节点的全部 key；无子节点时为空。
   - `Join { join_type: Semi | AntiSemi, .. }`：仅继承第一个（左）子节点的 key；无左子节点时为空。
   - 其他算子，包括 Inner/Outer Join、Aggregation、UnionAll、PartitionUnion、TableDual 和 `Other`：清空当前节点 key。
4. 递归回到根节点后，`optimize` 返回更新后的计划，`changed` 固定为 `false`。该布尔值表示没有结构性计划改写，不表示 key 元数据没有变化。

投影匹配的实际判定是 `expression.columns() == BTreeSet::from([column])`。它验证“表达式只依赖这一列”，并不验证表达式一定是裸列恒等映射；例如只引用一个列的标量函数或 cast 在当前实现中也可能匹配。扩展或依赖该行为时应以代码判定为准，不应仅依据源码注释中的“单列恒等”措辞。

## 数据与状态

核心输入/输出状态均在 [`rule_init.rs`](rule_init.rs) 的 `Plan` 中：

- `children: Vec<Plan>` 决定递归拓扑；算法假定它是一棵有限、由所有权保证无环的树。
- `schema: Vec<i64>` 提供当前节点输出位置到列 ID 的映射，Projection 分支用表达式下标索引它。
- `keys: Vec<Vec<i64>>` 是本规则覆盖写入的派生状态；旧值不会合并保留。DataSource 的来源是 `PlanKind::DataSource.indexes: BTreeMap<i64, Vec<i64>>`，其余受支持节点从首个子节点派生。
- `Expr::columns()` 返回表达式引用的列 ID 集合。集合会去重，因此当前 Projection 逻辑只比较依赖列集合，不检查引用次数、表达式类型、确定性或是否保持一一映射。

关键不变量是先子后父：Projection 和透传算子读取子节点 `keys` 时，它们已经由本轮执行重新计算。规则不会读取或改写 `predicates`、`estimated_rows`、`used_stats`、索引 ID、分区信息或 Join 条件。

Projection 使用 `expressions.iter().enumerate().find_map(...)`，同一输入列若对应多个输出表达式，只选择最靠前的输出位置；一个复合子键中不同列也没有显式禁止映射到同一输出列。DataSource 则把每个非空 `indexes` 条目视为候选键，当前精简 IR 没有单独的 unique/primary 标记可供过滤。

## 依赖与调用关系

上游关系：

- [`lib.rs`](lib.rs) 公开模块，并在测试配置下装配 `rule_build_key_info_test` 与综合边界测试 `rule_aster_unit_test`。
- RustCodeGraph 将 `BuildKeySolver` 的直接代码引用定位到 [`rule_build_key_info_test.rs`](rule_build_key_info_test.rs) 和 [`rule_aster_unit_test.rs`](rule_aster_unit_test.rs)；后者直接调用 `BuildKeySolver.optimize(plan)`。
- `LogicalRule::optimize` 是统一入口。当前 [`rule_init.rs`](rule_init.rs) 的 `default_rule_names()` 使用字符串描述默认顺序，但其中写的是 `"build_key_solver"`，与本规则 `name()` 的 `"build_keys"` 不同；本文件没有把 `BuildKeySolver` 实例装入该函数。

下游关系：

- `optimize` 唯一直接调用本文件私有的 `build_keys`。
- `build_keys` 递归调用自身，并使用 `Plan.children`、`Plan.schema`、`Plan.keys`、`PlanKind`、`JoinType` 以及 `Expr::columns()`。
- 标准库依赖仅是通过全限定路径构造 `std::collections::BTreeSet`。

Go 应用主链的对应接线位于 [`optimizer.go`](../optimizer.go)：`optRuleList` 注册 `&rule.BuildKeySolver{}`，对应 flag 为 `FlagBuildKeyInfo`，逻辑计划构建器会在多个入口设置该 flag。Go `BuildKeySolver.Optimize` 下游调用 [`util/misc.go`](util/misc.go) 的 `BuildKeyInfoPortal`，后者同样后序递归，但把实际算子语义分派给每个 `LogicalPlan.BuildKeyInfo`。这些 Go 调用边是语义参照，不是本 Rust 文件当前真实调用边。

## 错误处理与边界

`optimize` 的签名允许返回 `String` 错误，但当前路径不生成错误，完成后总是 `Ok`。边界通过空结果保守处理：

- Projection 的表达式数与 schema 长度不同会立即清空当前 key，避免以表达式下标越界访问 schema；[`rule_aster_unit_test.rs`](rule_aster_unit_test.rs) 的 `projection_with_pruned_schema_does_not_panic_while_building_keys` 覆盖了该回归，并确认 `changed == false`。
- 缺少第一个子节点的 Projection、透传一元算子、Semi/AntiSemi Join 都返回空 key，不 panic。
- 空索引列向量不会成为 DataSource key。
- Projection 的任一子键列无法映射时，只丢弃该候选键，其他候选键仍可保留。
- 所有未专门处理的 `PlanKind` 都清空 key，避免传播未经证明的唯一性。

当前测试缺口也构成使用边界：独立测试 [`rule_build_key_info_test.rs`](rule_build_key_info_test.rs) 只验证规则名；综合测试只直接覆盖 schema/表达式长度不匹配。DataSource 推导、合法 Projection 重映射、一元算子继承、Semi/AntiSemi 左侧继承、无子节点和不支持算子清空，尚未在该独立测试文件中得到逐项验证。

## 并发与资源生命周期

规则是同步、单线程、纯内存树遍历。`BuildKeySolver` 无字段，因此不存在共享可变状态、锁、原子变量、通道或后台任务。`optimize` 取得整棵 `Plan` 的所有权；递归期间通过独占 `&mut Plan` 借用逐节点更新，Rust 借用规则阻止同时读写同一计划节点。

资源生命周期由栈递归与 owned `Vec` 管理：DataSource 和透传分支会克隆 key 向量，Projection 构造新的向量，旧 `plan.keys` 在赋值时被释放。算法额外空间包括递归栈、克隆/新建的 key 集合，以及每次 `Expr::columns()` 构造的 `BTreeSet`。时间成本受节点数、候选键数量、每键列数和 Projection 表达式扫描影响；Projection 对每个 key 列线性扫描所有表达式并重复构造列集合，最坏可近似为 `候选键列总数 × 表达式数 × 表达式遍历成本`。

计划树极深时递归栈可能增长；代码没有显式深度限制或迭代回退。由于没有外部 I/O、事务或句柄，本文件不存在需要显式关闭、回滚或取消的资源。

## 与 Go 版本的对应关系

直接 Go 对照是 [`rule_build_key_info.go`](rule_build_key_info.go)：两端都有无状态 `BuildKeySolver`，规则名均为 `"build_keys"`，优化都返回未改写计划、`changed = false` 且正常路径无错误；两端也都采用子节点先于父节点的后序推导。

主要差异如下：

- Go 接口接收 `context.Context` 和 `base.LogicalPlan`；context 在实现中未使用。Rust 接收 owned 精简 `Plan`，没有 context。
- Go 规则调用 `ruleutil.BuildKeyInfoPortal(p)`；portal 收集所有子 schema 后调用各具体逻辑算子的 `BuildKeyInfo`，因此算子覆盖与唯一键语义由 operator 层负责。
- Rust 本文件内联一个仅面向 `rule_init::PlanKind` 的子集实现。DataSource 直接把所有非空 index 列表当 key，Projection 只依据引用列集合映射，Semi/AntiSemi 只继承左侧，其他多数算子清空。因此它不能作为 Go 完整 key 推导的等价替代证据。
- Go key 位于 `expression.Schema` 的 `PKOrUK`/`NullableUK` 等结构，能够区分更多 key 属性；Rust `Vec<Vec<i64>>` 不表达 nullable、主键/唯一键类别或列对象语义。
- Go 的 [`logical_plans_test.go`](../logical_plans_test.go) 中 `TestUniqueKeyInfo` 通过真实 SQL、完整逻辑优化和 golden 数据递归核对各节点唯一键。Rust 当前测试使用手工精简 IR，覆盖范围显著更窄。

因此，移植扩展应以 Go `BuildKeyInfoPortal`、各 `Logical*.BuildKeyInfo` 和 `TestUniqueKeyInfo` 为语义来源，但必须针对 Rust `Plan` 能表达的信息明确降级策略，不能假定当前结构已经承载 Go 的全部语义。

## 扩展指南

扩展时优先修改私有 `build_keys` 的对应 `PlanKind` 分支；若新语义需要 nullable key、unique 标记、表达式等价性或多子节点映射，则先评估并扩展 [`rule_init.rs`](rule_init.rs) 的 `Plan.keys`/`PlanKind` 数据模型，而不是在本文件中用列 ID 猜测。

具体注意点：

- 新增算子分支前，对照 Go 对应 `Logical*.BuildKeyInfo`，确认 key 保留/失效条件、左右输入方向和 NULL 语义。
- 收紧 Projection 为真正的恒等列映射时，不能只用 `Expr::columns()`；应显式匹配 `Expr::Column`，并决定 cast、确定性单列表达式及重复输出列如何处理。该变化可能影响依赖现状的优化规则。
- DataSource 若要对齐 Go，需区分唯一索引/主键、nullable unique key、不可见或无效索引，而不是接受所有非空 `indexes`。
- 加入 Inner/Outer Join 或 Aggregation 推导时，要从等值条件、连接类型、分组列和 NULL 填充规则证明唯一性；错误传播 key 会导致后续消除类优化产生错误结果，宁可在前置条件不足时清空。
- 若改变计划结构，`changed` 契约也要同步调整；仅更新派生 key 元数据时应继续保持 Go 的 `false`。

测试应放在独立的 [`rule_build_key_info_test.rs`](rule_build_key_info_test.rs)，不要内嵌进生产文件。建议至少覆盖每个受支持分支、复合键、多个候选键、重复投影、单列非恒等表达式、缺失子节点和所有新加入的拒绝路径；涉及跨规则安全性的场景可补充到 [`rule_aster_unit_test.rs`](rule_aster_unit_test.rs)。与 Go 对齐的完整语义应参考 `logical_plans_test.go::TestUniqueKeyInfo` 及其 testdata。

## 验证依据

本说明依据以下本地事实形成：

- 目标实现：[`rule_build_key_info.rs`](rule_build_key_info.rs) 的 `BuildKeySolver`、`LogicalRule` 实现和 `build_keys` 全部分支。
- 数据模型与接口：[`rule_init.rs`](rule_init.rs) 的 `Expr::columns`、`JoinType`、`PlanKind`、`Plan`、`LogicalRule` 和 `default_rule_names`。
- crate 装配与边界：[`lib.rs`](lib.rs) 和 [`Cargo.toml`](Cargo.toml)。当前目录没有 `doc.go`，因此无额外包级 Go 契约可读。
- Rust 测试：[`rule_build_key_info_test.rs`](rule_build_key_info_test.rs) 验证规则名；[`rule_aster_unit_test.rs`](rule_aster_unit_test.rs) 验证裁剪后 Projection 安全清空且不报告结构变化。
- Go 对照：[`rule_build_key_info.go`](rule_build_key_info.go)、[`util/misc.go`](util/misc.go) 的 `BuildKeyInfoPortal`、[`optimizer.go`](../optimizer.go) 的规则注册/顺序，以及 [`logical_plans_test.go`](../logical_plans_test.go) 的 `TestUniqueKeyInfo`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`query BuildKeySolver` 定位 Rust/Go 同名类型和 Rust 测试引用，`node build_keys` 确认源码及 `optimize -> build_keys` 调用边，`callers`/`explore` 也确认私有函数由本文件 `optimize` 调用。图对常见方法名 `optimize` 的全局查询存在大量同名噪声，因此应用主链接线另以精确文件和 `rg` 结果核验。
- 人工复核重点：后序遍历、每个 `PlanKind` 的保留/清空规则、Projection 的实际列集合判定、固定 `changed = false`、Go/Rust 覆盖差异，以及独立测试位置。

任务要求的结构检查应确认本文恰含上述十一个固定二级标题；本任务是纯文档分析，不运行 Cargo。
