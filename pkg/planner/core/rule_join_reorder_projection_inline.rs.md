# `pkg/planner/core/rule_join_reorder_projection_inline.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 以 `lib.rs` 为库入口，`pkg/planner/core/lib.rs` 通过 `pub mod rule_join_reorder_projection_inline` 公开本模块，并在 `#[cfg(test)]` 下把独立测试 `rule_join_reorder_projection_inline_test.rs` 接入测试构建。目标文件没有条件编译项、模块级常量、自定义类型或 trait，只提供五个公开函数。

它是 Join Reorder 的 Projection 穿透辅助模块：输入和输出都使用相邻 `rule_join_reorder.rs` 定义的简化 `JoinPlan`、`JoinNode` 与 `joinGroupResult`。需要特别区分“公开模块”和“已接入优化主链”：RustCodeGraph 的 caller 结果与 Rust 源码搜索只发现独立 Rust 测试调用 `tryInlineProjectionForJoinGroup`；`JoinReOrderSolver::optimizeRecursive` 当前仍直接调用 `extractJoinGroup`，而 `extractJoinGroupImpl` 不穿透 Projection。因此本文件当前是可独立调用和测试的移植实现，尚未接入 Rust Join Reorder 的生产递归入口。Go 同名文件则已由 Go `rule_join_reorder.go::extractJoinGroupImpl` 在会话开关允许时调用。

## 核心职责

本文件围绕“Projection 是否能安全地从 Join 组边界上被消去”承担四项职责：

1. `tryInlineProjectionForJoinGroup` 识别直接包裹 Join 的 Projection，并区分未处理、成功穿透和安全回退三种结果。
2. `canInlineProjectionBasic` 与 `isInlineableProjectionExpr` 执行不依赖 Join 组形状的基础检查：表达式数量必须匹配输出 schema，每个表达式必须有可追踪列，并拒绝由名称前缀标记的非确定性、可变、副查询或相关表达式。
3. `canInlineProjection` 根据 `extractJoinGroup` 得到的叶子 schema 验证每个引用列是否唯一归属于组内某个叶子，并要求组内至少两个叶子。
4. `buildColExprMapForProjection` 建立“Projection 输出列号到表达式”的映射；成功路径随后仅用其中的单列引用改写 `eqEdges` 两端列号，并把 Projection 的输出 schema 保存到 `originalSchema`。

这里的“内联”并不执行通用表达式树替换。Rust `Expression` 只有一个 `column: Option<usize>` 可供本模块追踪，所以实现只能处理单个列号层面的映射；`name` 前缀承担部分安全标签的作用。文档与扩展代码不能把它解释为已经具备 Go 版本的完整表达式替换能力。

## 主要符号

- `tryInlineProjectionForJoinGroup(plan: &JoinPlan) -> (Option<joinGroupResult>, bool)`：总入口。第二个返回值表示当前 Projection 是否已被本函数最终处理；`(None, false)` 表示调用者应走普通抽取逻辑，`(Some(result), true)` 表示成功穿透或已将整个 Projection 作为一个原子叶子回退。当前函数不会返回 `(None, true)`。
- `buildColExprMapForProjection(schema: &[usize], expressions: &[Expression]) -> HashMap<usize, Expression>`：按位置 `zip` 输出 schema 和表达式并克隆表达式。若表达式直接引用的列号与输出列号相同，则过滤该项，避免 `output -> output` 自引用。若两个切片长度不同，`zip` 会静默截到较短一侧；正常入口依赖 `canInlineProjectionBasic` 事先保证长度相等。
- `isInlineableProjectionExpr(expression: &Expression) -> bool`：要求 `column.is_some()`，同时拒绝名称以 `nondeterministic:`、`mutable:`、`subquery:` 或 `correlated:` 开头的表达式。这是简化模型的协议检查，不会递归遍历真实表达式树，也不会检查 `function_count`、`virtual_column` 或 `return_type`。
- `canInlineProjectionBasic(plan: &JoinPlan) -> bool`：仅对 `JoinNode::Projection` 返回可能的真值；要求所有表达式通过上述检查，且 `expressions.len() == plan.schema.len()`。
- `canInlineProjection(plan: &JoinPlan, childResult: &joinGroupResult) -> bool`：重复基础检查，要求 Join 组有多于一个叶子，然后建立 `column -> leaf_index` 的唯一归属表。任一列在多个叶子 schema 中重复，或任一表达式列不属于任何叶子，都会返回 `false`。多个输出表达式引用同一个、且唯一归属的叶子列是允许的。

所有函数目前都是 `pub`，没有私有函数。名称沿用 Go 风格的驼峰写法，是当前 API 事实。

## 执行流程

`tryInlineProjectionForJoinGroup` 的实际控制流如下：

1. 用模式匹配检查根节点。根不是 `JoinNode::Projection` 时直接返回 `(None, false)`。
2. 要求 Projection 的直接子节点是 `JoinNode::Join`，且 `canInlineProjectionBasic(plan)` 为真；任一条件不满足都返回 `(None, false)`。这意味着 Projection→Projection→Join、Projection→Selection→Join 等堆叠一元算子不会被穿透。
3. 对直接 Join 子树调用 `extractJoinGroup(child)`。该函数展开连续 Inner Join，收集叶子、等值边、其它条件、Join 类型和原始 schema；非 Inner Join 边界会整体成为叶子。
4. 调用 `canInlineProjection(plan, &child_result)`。若失败，构造新的 `joinGroupResult`：`joinNodePlans` 只含完整的原 Projection 计划，`originalSchema` 取 Projection schema，其余字段使用默认值，然后返回 `(Some(result), true)`。该回退防止调用者继续向下重排不安全表达式。
5. 若完整检查通过，调用 `buildColExprMapForProjection`。遍历子 Join 组的每条 `eqEdges`：当边的左/右列号恰好是映射键，且对应表达式含 `Some(column)` 时，将边端点替换为该列号。
6. 把结果的 `originalSchema` 改为 Projection 的输出 schema，并返回 `(Some(result), true)`。原 Projection 节点本身不会保留在叶子列表中，后续重排方应依据 `originalSchema` 恢复外部输出。

第 5 步是按当前源码描述的精确行为：映射键来自 Projection 输出 schema，而被扫描的是子 Join 抽取出的等值边。只有边端点确实使用这些输出列号时才会发生替换；本文件没有像 Go 版那样维护并向上传播通用 `colExprMap`。

## 数据与状态

- 输入计划通过不可变借用传入。成功或回退结果均拥有克隆后的计划、表达式、边和 schema，不修改调用者持有的 `JoinPlan`。
- `JoinPlan.schema: Vec<usize>` 同时承担输出列顺序和列身份。本文件按位置把它与 Projection 表达式配对；长度相等是成功入口的不变量。
- `Expression.column: Option<usize>` 是唯一参与内联判定与边改写的表达式数据。`None` 被视为 constant-only 或不可追踪表达式并拒绝；`Some(id)` 只能表达单列归属，不能表达跨两叶的多列标量函数。
- `leaf_by_column: HashMap<usize, usize>` 只在 `canInlineProjection` 调用期间存在。插入相同列号两次即失败，即使重复发生在同一叶 schema 内也会失败；这比注释中“出现在多个叶 schema”更保守，是源码的实际行为。
- `mapping: HashMap<usize, Expression>` 只在一次成功入口中存在。重复输出 schema 列号会以后插入的表达式覆盖先前表达式；基础检查只校验长度，不校验输出列号唯一性。
- 安全回退创建的 `joinGroupResult` 会丢弃刚抽取的子组边、条件和 Join 类型，把整个 Projection 封装成唯一原子叶子；这是防止错误重排的有意边界。

模块没有全局变量、缓存、会话状态或跨调用的可变状态。

## 依赖与调用关系

直接 Rust 依赖只有三组：

- `crate::rule_join_reorder::{JoinNode, JoinPlan, extractJoinGroup, joinGroupResult}` 提供简化计划树、Join 组抽取入口和返回结构；安全回退还直接构造同模块的 `basicJoinGroupInfo`。
- `crate::task::Expression` 提供表达式占位结构。其定义位于 `pkg/planner/core/task.rs`，字段包括 `name`、`column`、`function_count`、`virtual_column` 和 `return_type`，本文件只读取前两项。
- `std::collections::HashMap` 用于输出列映射和叶子归属表。

模块装配由 `pkg/planner/core/lib.rs` 完成，crate 边界由 `pkg/planner/core/Cargo.toml` 确认；本文件没有直接使用 Cargo 清单中的外部 crate 或 feature。

RustCodeGraph 的 callee 边确认 `tryInlineProjectionForJoinGroup` 调用 `extractJoinGroup`、`buildColExprMapForProjection`、`canInlineProjectionBasic`、`canInlineProjection`，并实例化 `basicJoinGroupInfo` 与 `joinGroupResult`。caller 边中，Rust 侧只有 `rule_join_reorder_projection_inline_test.rs::unknown_columns_are_unsafe_and_fall_back_to_an_atomic_leaf`；图里显示的另一个生产 caller 是同名 Go 函数进入 Go helper，并非 Rust 主链。源码搜索同样没有发现 Rust 生产调用。若将来接线，最接近的入口是 `rule_join_reorder.rs::extractJoinGroupImpl` 或 `JoinReOrderSolver::optimizeRecursive`，但必须先解决 Go/Rust 语义差异，不能只机械插入一次调用。

## 错误处理与边界

本模块不返回 `Result`，所有拒绝都以布尔值或安全回退表达，不产生错误文本：

- 非 Projection、子节点非直接 Join、基础表达式检查失败：返回 `(None, false)`，由调用方决定普通处理方式。
- Join 组只有一个叶子、叶 schema 列号冲突、表达式引用未知列：返回原子 Projection 组并标记 `handled = true`。
- constant-only 表达式以及名称带四种危险前缀的表达式会在递归抽取前被拒绝。
- `canInlineProjection` 的 `let ... else` 可处理被直接调用时传入非 Projection 的情况，返回 `false`；正常总入口已先完成 Projection 匹配。

重要的未实现边界包括：没有真实的可变副作用/非确定性检测，没有递归表达式节点白名单，没有相关列对象，没有 Go 版 `nullExtendedCols` 保护，没有子 `colExprMap` 的替换或合并，没有会话变量开关与优化变量记录，也没有 Projection-for-Expand 的专门标志。因此安全性依赖简化 `Expression.name` 协议和单列模型；新增表达式能力时必须同步收紧判定，不能默认现有检查仍充分。

## 并发与资源生命周期

本文件没有异步任务、线程、锁、通道、事务、文件句柄或网络资源。函数调用期间只创建局部 `HashMap`，返回值通过所有权离开；对子计划与表达式的保留依靠 `Clone`。没有清理钩子或跨请求生命周期。

资源成本主要来自克隆与遍历：`extractJoinGroup` 会克隆叶子、边和条件；安全回退再次克隆完整 Projection 计划；映射构建克隆每个表达式；边改写是 `O(E)` 次 HashMap 查询，叶归属与表达式检查约为 `O(C + P)`，其中 `E` 为等值边数、`C` 为所有叶 schema 列数、`P` 为投影表达式数。真正接入主链后，大计划上的克隆量是需要评估的性能风险。

## 与 Go 版本的对应关系

`pkg/planner/core/rule_join_reorder_projection_inline.go` 是直接语义对照。两版保留了同名的四层结构：总入口、映射构建、基础表达式检查、Join 组归属检查；也都只尝试直接包在 Join 上的 Projection，失败时把 Projection 作为原子叶子，并避免 pass-through 自引用映射。

Rust 已对齐的意图包括：拒绝 constant-only/危险表达式、要求列能归属 Join 组叶子、拒绝跨叶或未知归属、成功后保留 Projection 输出契约、失败时不破坏原计划，以及允许多个输出引用同一安全叶列。独立 Rust 测试实际覆盖 constant-only 拒绝、同一叶列被多次投影、未知列拒绝与原子回退。

两版尚未等价，关键差异如下：

- Go `extractJoinGroupImpl` 受 `TiDBOptJoinReorderThroughProj` 控制并实际调用 helper；Rust 主链尚未调用本模块，也没有会话开关。
- Go 表达式是完整接口树，递归允许 Column/ScalarFunction/无 DeferredExpr 的 Constant，并调用 `ExtractColumns`、`IsMutableEffectsExpr`、`CheckNonDeterministic`、`IsCorrelated`；Rust 只看单个 `column` 和字符串前缀，且一律拒绝无列表达式。
- Go 在判定和建图时先用子 `colExprMap` 做 `SubstituteColsInExpr`，随后合并当前映射与子映射；Rust 没有 `colExprMap` 字段，只尝试把映射应用到 `eqEdges` 的整数端点。
- Go 验证一个表达式的所有列来自同一叶，并专门拒绝引用 Outer Join 空值扩展侧的表达式；Rust 单个 `Expression` 最多记录一个列，无法表达这两类完整检查，也没有 `nullExtendedCols`。
- Go 成功时记录 `TiDBOptJoinReorderThroughProj` 相关优化变量；Rust 没有会话上下文。
- Go 的重复列归属仅在同一列出现在不同叶子时拒绝；Rust 当前对任何第二次出现都拒绝，包括同一叶内重复 schema id。
- Go 安全回退保留真实逻辑计划对象；Rust 回退使用简化 `JoinPlan` 克隆并把 `originalSchema` 显式设置为 Projection schema。

因此本文件是 Go 行为的局部、保守移植，而不是可互换实现。后续补齐必须逐项移植真实约束和测试，不应为了接线而删减 Go 语义。

## 扩展指南

- 接入 Rust Join Reorder 主链时，优先修改 `rule_join_reorder.rs` 的抽取入口，并先定义开关、成功后的 schema 恢复、表达式映射传播和回退契约；同步扩展独立 `rule_join_reorder_projection_inline_test.rs` 与 `rule_join_reorder_test.rs`。接线前应新增一个证明主入口确实穿透 Projection 的回归测试。
- 扩展 `Expression` 为多列或树形表达式后，修改 `isInlineableProjectionExpr` 做递归节点白名单与真实副作用检查，修改 `canInlineProjection` 检查全部引用列属于同一叶，并加入 constant、标量函数、跨叶、相关表达式、非确定性与 mutable-effect 测试。
- 补齐嵌套 Projection 时，修改 `tryInlineProjectionForJoinGroup` 的直接子节点限制，同时为子映射替换、映射合并、pass-through 冲突和 Projection→Projection→Join 新增独立测试；不能只放宽 `matches!(child.node, JoinNode::Join { .. })`。
- 补齐 Outer Join 支持时，必须同时把 null-extended 列信息加入 `joinGroupResult`，在判定中设置正确性栅栏，并覆盖 `IFNULL` 等依赖空值扩展时机的表达式。
- 修改 `buildColExprMapForProjection` 或边改写时，应覆盖映射命中/不命中、pass-through、自重复输出 id、schema/表达式长度不一致和链式派生列。若目标是对齐 Go，应引入并传播表达式映射，而不是继续把复杂表达式压缩成单列号。
- 修改列归属检查时，要明确同一叶内重复列 id 的策略；若改为 Go 行为，应只拒绝同一 id 被不同叶拥有，并增加相应回归测试。
- Rust 测试继续放在同目录独立 `*_test.rs` 文件，通过 `lib.rs` 的 `#[cfg(test)] mod ...` 接入，不应把测试内嵌进生产源文件。

兼容性风险集中在输出 schema 恢复与危险表达式移动，正确性风险集中在跨叶表达式、Outer Join 空值扩展和非确定性求值次数，性能风险集中在计划/表达式克隆与未来通用替换遍历。

## 验证依据

- RustCodeGraph 索引状态：项目索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；通过 `node --file pkg/planner/core/rule_join_reorder_projection_inline.rs --offset 1 --limit 400` 读取了目标文件全部 107 行。
- RustCodeGraph 符号与边：查询了 `tryInlineProjectionForJoinGroup`、`buildColExprMapForProjection`、`isInlineableProjectionExpr`、`canInlineProjectionBasic`、`canInlineProjection`、`extractJoinGroup`；callee 结果确认总入口的内部调用和结果类型实例化，caller JSON 与源码搜索确认 Rust 侧仅有独立测试 caller、没有生产主链 caller。
- crate 与模块证据：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。目标包根目录没有 `doc.go`；最近的 `pkg/planner/core/base/doc.go` 属于 `base` 子包，不作为当前 crate 行为定义。
- Rust 上下游证据：`pkg/planner/core/rule_join_reorder.rs` 的 `JoinNode`、`JoinPlan`、`joinGroupResult`、`extractJoinGroupImpl` 和 `JoinReOrderSolver::optimizeRecursive`；`pkg/planner/core/task.rs` 的 `Expression`。
- 独立 Rust 测试：`pkg/planner/core/rule_join_reorder_projection_inline_test.rs`。测试覆盖基础 constant-only 栅栏、同一叶列多次投影、未知列拒绝和原子回退；它没有覆盖成功路径的 `eqEdges` 改写、危险名称前缀或生产入口接线。
- Go 对照与测试：`pkg/planner/core/rule_join_reorder_projection_inline.go`、`pkg/planner/core/rule_join_reorder.go`、`pkg/planner/core/rule_join_reorder_dp_test.go`。Go 测试额外证明非确定性、跨叶、null-extended 表达式拒绝，以及安全派生列映射和原子回退。
- 本任务只新增说明文档，按计划未运行 Cargo，也未把历史测试结果当作本次运行证据；最终仅执行任务指定的 11 章节结构检查，并人工复核重要结论均能回指上述符号和文件。
