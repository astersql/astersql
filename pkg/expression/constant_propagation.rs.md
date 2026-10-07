# `pkg/expression/constant_propagation.rs`

## 文件定位

该文件属于 `astersql-expression` crate：`pkg/expression/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `pkg/expression/lib.rs` 通过 `#[path = "constant_propagation.rs"] mod constant_propagation_kernel;` 装载实现。它位于逻辑优化与表达式基础设施之间，接收已构造的 `Expression` 条件列表，在不执行查询的前提下推导等价条件、代入常量并压缩恒真/恒假谓词。

crate 根目前公开再导出借用型入口 `PropagateConstantRef` 和 `PropagateConstantForJoinRef`。真实上游包括 `pkg/planner/core/operator/logicalop/logical_datasource.rs` 中安装的普通谓词与 join 谓词简化回调，以及 `pkg/planner/cascades/old/transformation_rules.rs::PropagateConstant`。拥有型入口和外连接入口仍是本模块的公开函数，但未由 `lib.rs` 对 crate 外再导出。

## 核心职责

- 对 CNF 条件反复发现 `列 = 常量`，通过 `ColumnSubstitute` 把常量代入其余谓词，直到没有新等式或达到 `MaxPropagateColsCnt` 次迭代。
- 用 `SimpleIntSet` 维护 `列 = 列` 的等价类，从已有谓词双向派生等价列上的新谓词，同时跳过不确定、有副作用、NULL 敏感或排序规则不兼容的表达式。
- 对顶层 OR 的每个 DNF 分支独立执行 CNF 常量传播，再用 `ComposeCNFCondition` / `ComposeDNFCondition` 组合结果。
- 为 inner join 可选地保留跨两侧 schema 的等值 join key，避免传播与去重后损失 join reorder、index join 等后续优化所需信息。
- 为 outer join 单独限制传播方向：只抽取 outer/preserved 侧的 `列 = 常量`，仅把安全派生条件加入 join 条件，并在 `null_sensitive` 模式下禁用列等价派生。
- 在矛盾、恒假或求值异常场景中采取保守策略，并维护 plan-cache 跳过标记与优化期 warning 边界。

## 主要符号

- `MaxPropagateColsCnt`：公开的传播列数与迭代上限，默认 100。它是 `pub static mut`，读取需要 `unsafe`；调用方若修改它，必须自行保证不存在并发数据竞争。
- `VaildConstantPropagationExpressionFuncType`：派生谓词过滤回调；名称保留 Go 版本的 `Vaild` 拼写。常量结果不受该回调拒绝，非恒定派生式才受过滤。
- `ValidCompareConstantPredicate` / `ValidCompareConstantPredicateHelper`：识别 GT、GE、LT、LE、EQ 中“一侧 Column、另一侧 Constant”且双方 collation 相同的比较式。
- `SimpleIntSet`：本地并查集。`FindRoot` 执行路径压缩，`Union` 合并列等价类，`GrowNewIntSet` 为一次求解重建父节点数组。
- `BuildContextProxy`、`BorrowedExprContext`、`ConstantPropagateContext`：上下文适配层。前者把共享 `BuildContext` 暴露为替换函数要求的独占引用；后两者完整转发会话语义，并让 `IsConstantPropagateCheck` 固定返回 true。
- `BasePropConstSolver`：共享求解状态，包括 `UniqueID -> 索引` 的 `col_mapper`、列索引到常量的 `eq_mapper`、并查集、列快照和表达式上下文。
- `PropConstSolver`：普通 CNF/inner-join 求解器；核心方法为 `pickNewEQConds`、`propagateConstantEQ`、`propagateColumnEQ` 和 `solve`。
- `PropOuterJoinConstSolver`：outer-join 求解器；分别保存 `join_conds`、`filter_conds`、outer/inner schema、过滤回调和 `null_sensitive` 标记。
- `PropagateConstant` / `PropagateConstantRef`：普通条件列表的拥有型/借用型入口。
- `PropagateConstantForJoin` / `PropagateConstantForJoinRef`：带两侧 schema 和 `keep_join_key` 的 inner-join 入口。
- `PropConstForOuterJoin`：outer join/anti semi join 的入口，返回分别处理后的 join 与 filter 条件。
- `PropagateConstantSolver`：与 Go 接口对齐的 trait；当前本文件的实际调用直接使用 `PropConstSolver` 的同名固有方法。
- `cloneJoinKeys` / `isJoinKey`：识别并克隆两侧 schema 各取一列的 EQ 条件。

## 执行流程

普通条件路径由 `PropagateConstantRef` 或 `PropagateConstant` 创建 `PropConstSolver`，包装上下文后进入 `PropConstSolver::solve`：

1. `extractColumnsInternal` 把每个输入拆成 CNF 项，收集所有列并以 `UniqueID` 去重。
2. 若列数超过 `MaxPropagateColsCnt`，立即返回原条件，避免并查集后的两层列遍历造成优化时间爆炸。
3. `propagateConstantEQ` 反复调用 `pickNewEQConds`。后者识别 EQ、检测恒假与冲突常量、按列类型构造 cast，并截断 cast 探测产生的 warning；随后 `ColumnSubstitute` 只改写尚未访问的条件。
4. `propagateColumnEQ` 收集 collation 相同且非 hybrid 的列等式，构造等价类。冗余等式可替换为 true；每个等价列对会尝试两个替换方向，派生条件经可选过滤器筛选后追加。
5. `propagateConstantDNF` 对顶层 OR 分支递归运行普通求解器；随后补回要求保留的 join key，并用 `RemoveDupExprs` 去重。
6. 入口调用 `Clear` 释放本次求解持有的列、常量、schema 和上下文引用。

outer-join 路径由 `PropConstForOuterJoin` 配置两侧 schema 后进入 `PropOuterJoinConstSolver::solve`。它先只从 outer 侧收集 `列 = 常量` 并仅代入 join 条件；再从 join 条件识别 `outerCol = innerCol`，必要时为 inner 列增加 `IS NOT NULL`，将 outer 侧安全谓词派生为新的 join 条件。来自 filter 的谓词必须完全属于 outer schema 才可向 inner 侧派生。`null_sensitive` 为 true 时整个列等价派生阶段跳过。最后 join/filter 分别做 DNF 传播，只有 join 条件补回 join key。

## 数据与状态

列身份以 `Column::UniqueID` 为准，`col_mapper` 将其压缩为连续索引；`columns` 保存用于重建 schema 和派生条件的列副本。`eq_mapper` 记录当前轮已知的“列索引 = 常量”，同一索引出现不可比较或不相等的常量即视为矛盾。`union_set` 仅表达列等价关系，不承载常量值。

条件、列、常量和 schema 均按 Rust 所有权克隆到求解器或返回值中；`Rc<dyn ExprContext>` 让 DNF 子求解器共享同一会话上下文。`ConstantPropagateContext` 与 `BorrowedExprContext` 不复制会话状态，只转发 eval context、字符集、随机源、plan-cache 决策、连接 ID 等接口。

`visited` 数组保证已用于提取等式的条件不被重复代入。普通求解器只在求解期间追加派生条件，并以进入列传播前的 `original_len` 限制派生源，避免新条件递归膨胀。outer-join 求解器对 join/filter 使用带 offset 的访问位图。

## 依赖与调用关系

上游接线如下：

- `pkg/expression/lib.rs` 装载本文件并再导出两个借用型入口。
- `pkg/planner/core/operator/logicalop/logical_datasource.rs::InstallPredicateSimplificationPassthrough` 在普通谓词简化时调用 `PropagateConstantRef`，在 join 谓词简化时读取 `TiDBOptAlwaysKeepJoinKey` 后调用 `PropagateConstantForJoinRef`。
- `pkg/planner/cascades/old/transformation_rules.rs::PropagateConstant` 同样委托给 `astersql_expression::PropagateConstantRef`。

主要下游依赖都来自 expression crate 内部：`SplitCNFItems`/`SplitDNFItems` 与组合函数维护布尔范式，`ExtractColumns`/`Schema` 识别列归属，`ColumnSubstitute` 执行代入，`NewFunctionInternal` 重建标量函数，`EvalBool` 判断常量真值，`BuildCastFunction` 和 `BuildNotNullExpr` 构造派生表达式，`RemoveDupExprs` 清理重复项。类型、Datum、collation、chunk row、AST 函数名及 plan-cache 判断分别由 `types`、`collate`、`chunk`、`ast` 和 `util_kernel` 提供；对应依赖可在 `pkg/expression/Cargo.toml` 的本地 crate 依赖中核验。

RustCodeGraph 将目标文件标为被 `pkg/expression/constant_propagation_test.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs` 和若干 planner 测试引用；其 `callers` 对再导出入口没有生成边，因此上述上游位置又以精确源码搜索核实。

## 错误处理与边界

- 输入为空时四个普通/连接入口直接返回；超过列数上限时返回未经传播的原条件。
- 常量条件由 `EvalBool` 求值。恒假会把整个 CNF 压缩成 false；求值错误通过 `terror::Log` 记录并停止当前等式提取，不把错误向函数签名外传播。outer-join 测试确认参数标记求值失败时原 filter 条件仍被保留。
- 同一列对应冲突常量、常量比较失败，或 plan-cache 相关 NULL 常量会触发矛盾处理；若可能覆盖可变参数，`SetSkipPlanCache` 记录原因。
- hybrid 类型在普通传播中被跳过；outer join 只专门转换 ENUM 的整数、字符串、ENUM/SET Datum，其余种类或解析失败均不传播。
- 列替换要求静态类型相同，并逐层检查目标列与所在表达式的 collation。字符串 IN 不执行“等价列命中即 true”的改写。
- `unFoldableFunctions`、`IsNull`，以及 null-aware 模式下的 IFNULL、IF、CASE、NullEQ 会使整棵候选子树放弃替换，防止不确定性、副作用或 NULL 语义改变。
- outer join 只从 preserved 侧提取常量；filter 中混合 outer/inner 列的谓词不会派生。`null_sensitive` join 不生成 inner `IS NOT NULL` 或其它列等价派生。
- `NewFunctionInternal`、CNF/DNF 组合失败时保留原条件或回退为 false 常量；这些路径是保守回退，不对外返回 `Result`。

## 并发与资源生命周期

求解过程本身同步执行，不创建线程、异步任务、通道、锁、事务或 I/O 资源。上下文以 `Rc` 共享，因此这些求解器不是 `Send`/`Sync` 设计；其生命周期局限于一次 planner 优化调用。

每个公开入口创建局部 solver，在返回前显式 `Clear`：清空映射、条件、schema、并查集并释放 `Rc` 上下文。与 Go 的 `sync.Pool` 不同，Rust 当前没有跨调用对象池；`Clear` 保留了 Go 的资源收尾语义，但局部 solver 随后仍会正常 drop。`GetUniqueIDToColumnMap` 获取的复用 map 在提取结束后由 `PutUniqueIDToColumnMap` 归还。

并发风险集中在 `pub static mut MaxPropagateColsCnt`：文件内读取都位于 `unsafe` 块，若外部在并行优化期间写入会产生 Rust 层面的数据竞争风险。扩展时优先考虑不可变常量、原子值或受控配置，而不是新增无同步写入。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/constant_propagation.go`。Rust 保留了 Go 的 `basePropConstSolver`、普通与 outer-join solver、两阶段传播、DNF 分支处理、join-key 保留、100 列上限、过滤回调、plan-cache 标记和 outer-join NULL 安全限制；主要函数可以按同名符号逐一映射。

实现层差异包括：Go 使用 `disjointset.SimpleIntSet`，Rust 在文件内实现 `SimpleIntSet`；Go 的两个 solver 由 `sync.Pool` 复用，Rust 每次构造局部值并用 `Clear` 收尾；Go 超限时写背景 warning 日志，Rust 当前只原样返回；Rust 增加拥有型与借用型上下文适配器，以满足 trait object 生命周期并保持 `WithConstantPropagateCheck` 语义。Go 接口通过动态接口值使用，Rust 虽声明 `PropagateConstantSolver` trait，当前核心路径使用具体类型的固有方法。

测试证据并非完全一一复制。Rust 独立测试 `pkg/expression/constant_propagation_test.rs` 覆盖借用/拥有 join 入口结果一致且可保留 join key、NE 不应建立等价类、outer-join 常量求值错误保留原条件。Go `pkg/expression/constant_test.go::TestConstantPropagation` 提供更广的基准语义，包括等价链、恒假、双向谓词派生、IN、嵌套表达式和 RAND 不确定性；扩展 Rust 行为时应同时以这些 Go 用例核对移植完整性。

## 扩展指南

- 新增可传播比较形式时，从 `ValidCompareConstantPredicateHelper`、`validEqualCond` 和 `pickNewEQConds` 入手，先确认类型转换、collation、NULL 与 plan-cache 语义，再同步更新独立 Rust 测试；不要把测试嵌入本源文件。
- 修改列等价传播规则时，同时审查 `tryToReplaceCond`、`replaceEqCondtionWithTrue`、普通/outer 两个 `propagateColumnEQ` 以及派生过滤回调。任何放宽都要覆盖不确定函数、副作用、字符串 IN 和两个替换方向。
- 修改 outer join 行为时，必须分别验证 preserved/inner 方向、WHERE 条件的 schema 归属、null-sensitive semi/anti join 以及 inner `IS NOT NULL` 的生成条件；最接近的实现入口是 `PropOuterJoinConstSolver::{pickEQCondsOnOuterCol,deriveConds,propagateColumnEQ}`。
- 修改性能限制时注意列等价枚举为二次复杂度，且每个等价列对还会扫描原条件。若替换 `MaxPropagateColsCnt`，应消除或封装其 `static mut` 并保持“超限返回原条件”的兼容行为。
- 若公开新的拥有型或 outer-join API，需要同步调整 `pkg/expression/lib.rs` 的再导出，并检查 planner 接线；仅把函数标记 `pub` 不代表 crate 外可达。
- 测试应优先扩展 `pkg/expression/constant_propagation_test.rs`，并用 `pkg/expression/constant_test.go::TestConstantPropagation` 及 Go 同名实现作语义基线。高风险项是 outer-join NULL 语义、collation、hybrid ENUM、warning 截断、plan-cache 可变参数及 join key 保留。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；通过 `node --file pkg/expression/constant_propagation.rs` 分段读取 1–1306 行，并用 `query` 定位 `PropagateConstant`、`PropConstForOuterJoin`、`ValidCompareConstantPredicate`；`explore` 确认 `solve -> propagateColumnEQ -> true_constant` 等内部流及目标文件的引用者。公开入口的 `callers` 查询为空，已用精确源码搜索补证，不据此宣称“无调用者”。
- Rust 源与装配：`pkg/expression/constant_propagation.rs`、`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`。
- Rust 上游：`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`pkg/planner/cascades/old/transformation_rules.rs`。
- Rust 独立测试：`pkg/expression/constant_propagation_test.rs`；测试与源文件分离，覆盖三个关键回归场景。
- Go 对照：`pkg/expression/constant_propagation.go` 全文件，以及 `pkg/expression/constant_test.go::TestConstantPropagation`。
- 本任务只新增说明文档，没有运行 Cargo 或代码测试；验收以任务指定的 11 章节结构命令和人工事实复核为准。
