# `pkg/planner/util/null_misc.rs`

## 文件定位

本文件属于 `astersql-planner-util` crate。`pkg/planner/util/Cargo.toml` 将该 crate 的入口设为 `lib.rs`，并直接依赖表达式层 `astersql-expression`、表达式上下文、Parser AST、Planner `PlanContext`、错误上下文、MySQL 类型标志和 chunk 行等 crate。`pkg/planner/util/lib.rs` 以私有模块 `null_misc` 装载本文件，再通过 `pub use null_misc::*` 导出两个公开函数：`IsNullRejected` 和 `ResetNotNullFlag`。

它位于逻辑规划优化与表达式求值之间：一部分代码证明谓词在外连接内侧列被补成 SQL `NULL` 后不可能为真，供外连接简化、谓词下推和函数依赖推导使用；另一部分代码在外连接输出 Schema 上清除不再成立的 `NotNull` 类型标志。它不执行查询，也不持有计划节点。

## 核心职责

1. `IsNullRejected` 对表达式进行 NULL 拒绝证明。证明结论是保守的：返回 `true` 表示已经证明谓词不可能为 `TRUE`；返回 `false` 既可能表示谓词能为真，也可能只是现有规则无法证明。
2. `proveNullRejected` 及其辅助函数同时维护“恒非真”和“必为 NULL”两个不同事实，以正确描述 SQL 三值逻辑，避免把 `FALSE` 与 `NULL` 混为一谈。
3. `tryFoldNullifiedConstant` 将属于 `inner_schema` 的列替换为保留原类型但允许空的常量 `NULL`，再尝试常量折叠，补足仅靠函数属性无法证明的 `COALESCE`、`IFNULL`、`IF` 等场景。
4. `proveNullRejectedScalarFunc` 用专门真值规则处理 `AND`、`OR`、`NOT`、`IN`、`IS NULL`、`WEEK`/`YEARWEEK`，并将其余 builtin 委托给 `null_misc_builtins.rs` 中的 NULL 传递表和 NULL 测试表。
5. `ResetNotNullFlag` 对 Schema 的指定半开区间执行写时克隆，清除字段类型的 `NotNullFlag`，使外连接产生的可空列在类型元数据中得到反映。

## 主要符号

- `NullRejectProof { non_true, must_null }`：文件私有的两位证明值。`non_true` 表示结果只能为 `FALSE` 或 `NULL`；`must_null` 是更强的“结果必为 `NULL`”。默认值两个字段均为 `false`，代表没有得到证明。
- `allConstants(context, expr) -> bool`：递归确认表达式树只含无参数标记、无延迟表达式的常量和标量函数，并先用 `MaybeOverOptimized4PlanCache` 排除可能导致计划缓存过度优化的表达式。
- `IsNullRejected(context, inner_schema, predicate) -> bool`：公开入口。先在 NULL 拒绝专用上下文中调用 `PushDownNot`，再取递归证明的 `non_true` 位。
- `proveNullRejected(...) -> NullRejectProof`：私有递归分派器。依次尝试置 NULL 折叠、内外侧列识别、常量/延迟常量处理和标量函数规则；未知表达式返回默认值。
- `proveNullRejectedScalarFunc(...)`：合并子表达式证明，并查询 `null_reject_test_mode`、`is_null_reject_null_preserving` 两个 builtin 属性接口。
- `proveNullRejectedIn(...)`：当 `IN` 的被查值必为 NULL，或候选列表每项都必为 NULL 时，证明整个 `IN` 必为 NULL。
- `tryFoldNullifiedConstant`、`tryFoldStaticConstant`、`tryFoldNullifiedScalarFunc`：组成“置 NULL 后折叠”的主链；后者对普通函数、NULL 传递函数和特殊控制函数分流。
- `tryFoldNullifiedCoalesceLike`：按参数顺序返回首个可折叠的非 NULL 常量；全部为 NULL 时返回 `NewNull()`。
- `tryFoldNullifiedIf`：要求至少三个参数，先把条件折叠为整数真值，再只折叠实际选择的分支；条件为 NULL 或零时选择第三个参数。
- `foldNullifiedFunction`：用原函数名、返回类型和已折叠参数重建表达式，再调用 `FoldConstant`；只接受没有 `ParamMarker`/`DeferredExpr` 的最终常量。
- `proofFromConstant`：SQL NULL 产生 `(true, true)`，可成功转布尔且等于零的常量产生 `(true, false)`，真值、转换失败和动态常量均不提供证明。
- `nullRejectFoldCtx`：从 `PlanContext::GetNullRejectCheckExprCtx` 取得专用表达式上下文，并把截断错误级别包装为 `LevelIgnore`。
- `ResetNotNullFlag(schema, start, end)`：公开 Schema 元数据修正入口；区间语义是 Rust 切片的 `[start, end)`。

## 执行流程

`IsNullRejected` 的主流程如下：

1. 用 `nullRejectFoldCtx` 创建 NULL 拒绝检查上下文；该上下文既标识当前处于 NULL 拒绝检查，又忽略常量转布尔或折叠过程中的截断错误。
2. `PushDownNot` 将否定尽量下推，减少后续需要直接处理的逻辑形态。
3. `proveNullRejected(..., allow_nullified_fold = true)` 首先尝试把内侧列具体化为带原类型的 SQL NULL 并折叠。若得到静态常量，`proofFromConstant` 直接按 NULL/假/真分类。
4. 无法折叠时进入符号证明：内侧列得到 `(non_true=true, must_null=true)`，外侧列不产生证明；普通静态常量按其值分类；含 `DeferredExpr` 且无参数标记的常量只递归分析延迟表达式，并禁止再次做置 NULL 折叠。
5. 标量函数根据 SQL 三值逻辑合并：`AND` 任一侧 `non_true` 即恒非真，但两侧都 `must_null` 才必为 NULL；`OR` 两侧都 `non_true` 才恒非真，且两侧都 `must_null` 才必为 NULL；普通 `NOT` 只有在子式 `must_null` 时才能证明，而 `NOT(IS NULL(x))` 在 `x` 必为 NULL 时得到确定的假而非 NULL。
6. `IS NULL` 明确返回未知证明，因为它会接受 NULL；`IN`、`WEEK`/`YEARWEEK` 使用各自规则；登记的测试函数和 NULL 传递函数则由 `null_misc_builtins.rs` 驱动。没有规则的函数返回默认值，保持保守。
7. 公开入口只返回最终 `non_true`，供调用者决定是否可以应用优化。

置 NULL 折叠中，`COALESCE`/`IFNULL` 按首个非 NULL 参数选值，`IF` 只分析选中的分支。普通函数要求参数都能折叠；若函数登记为 NULL 传递且已得到 NULL 参数，可以直接得到 NULL，否则重建函数并调用表达式层常量折叠。

`ResetNotNullFlag` 的流程独立于证明链：遍历 `schema.Columns[start..end]`，克隆每个列对象及其可选返回类型，在克隆类型上删除 `mysql::type::NotNullFlag`，再替换原列，避免直接修改可能共享的列对象。

## 数据与状态

证明过程只读取传入的 `PlanContext`、`Schema` 和表达式树；递归函数返回按值复制的 `NullRejectProof`，没有模块级可变状态。核心不变量是 `must_null => non_true`：本文件所有构造 `must_null=true` 的分支也同时设置 `non_true=true`。默认值表示“未知”，而不是“已证明可为真”。

内侧列的识别完全由 `Schema::Contains` 决定。替换列时复制 `RetType` 并删除 `NotNullFlag`，因此常量折叠仍使用原来的 SQL 类型分派，但不会携带与 NULL 值矛盾的非空标志。若列没有 `RetType`，该折叠路径返回 `None`。

动态信息通过 `ParamMarker` 与 `DeferredExpr` 表示。`ParamMarker` 从不被当作编译期真值；`DeferredExpr` 允许符号递归，但令 `allow_nullified_fold=false`，防止把执行期值固化进计划缓存。builtin 分类数据位于相邻的 `null_misc_builtins.rs`：当前 Rust 测试固定检查 174 个 NULL 传递函数与 3 个 NULL 测试函数，并检查名称去重。

唯一会修改调用方数据的 API 是 `ResetNotNullFlag`。其修改范围严格由 `[start, end)` 指定；本文件本身不保存 Schema、表达式、缓存、锁或事务状态。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 与源码搜索共同确认：

- `pkg/planner/core/operator/logicalop/logical_join.rs::PredicatePushDown` 分别以左右子树 Schema 调用 `planner_util::IsNullRejected`，判断外连接是否能被谓词拒绝 NULL，从而参与连接类型简化及后续谓词下推。
- `pkg/planner/core/operator/logicalop/logical_projection.rs::ExtractFD` 用投影 Schema 调用 `IsNullRejected`，把可证明非空的表达式信息纳入函数依赖集合。
- `pkg/planner/util/funcdep_misc.rs::ExtractNotNullFromConds` 为条件中每一列构造单列 Schema，再调用 `IsNullRejected`，收集满足条件时必非空的列 ID。
- `pkg/planner/core/operator/logicalop/logical_join.rs` 在合并外连接 Schema 时调用 `ResetNotNullFlag`；`logical_max_one_row.rs::Schema` 也对其输出区间调用该函数。
- `pkg/planner/util/null_misc_test.rs::test_is_null_rejected_proof_modes` 是本文件的独立 Rust 单元测试入口。

下游依赖包括：表达式对象的动态类型识别与克隆、`PushDownNot`、`FoldConstant`、`NewFunction`、常量 `EvalInt`/`ToBool`；`PlanContext` 的 NULL 拒绝表达式上下文；`parser_ast::functions` 的函数名；`chunk::Row::default()`；`errctx::LevelIgnore`；以及相邻 `null_misc_builtins.rs` 的两个分类查询。Cargo 元数据表明这些均通过工作区 path dependency 接入，没有本文件专属 feature 或条件编译分支。

## 错误处理与边界

该证明器有意采用“失败即未知”的安全策略。函数构造失败、常量折叠未得到静态常量、`EvalInt` 失败、`ToBool` 失败、返回类型缺失、表达式类型未知或 builtin 未登记时，分别通过 `Option::None`、`.ok()?` 或默认 `NullRejectProof` 放弃优化，不向上抛错。这样可能损失优化机会，但不会凭不足证据把外连接错误地简化为内连接。

关键边界包括：空参数 `IN` 返回未知；`IF` 少于三个参数返回 `None`；`IS NULL(inner_col)` 不拒绝 NULL；`NOT(IS NULL(inner_col))` 得到确定的假；`IN` 只在被查值必为 NULL或全部候选必为 NULL时成立；`WEEK`/`YEARWEEK` 只有日期参数按 NULL 传递处理，因为 NULL mode 在 MySQL/TiDB 中等价于 mode 0。参数标记和延迟表达式不会作为静态常量分类。

`proveNullRejectedScalarFunc` 对 `AND`、`OR`、`NOT` 等已知函数直接按固定参数位置索引，依赖表达式构造层保证合法元数；本文件只对 `IN` 空参数和 `IF` 参数不足做显式保护。`ResetNotNullFlag` 直接切片 `schema.Columns[start..end]`，调用者必须保证 `start <= end <= Columns.len()`，否则会触发 Rust 越界 panic；这一点与 Go 循环依赖合法区间的前置条件相同。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部资源。NULL 拒绝证明使用函数栈递归和局部 `Vec`；临时折叠上下文只借用 `PlanContext`，其生命周期不逃逸。输入谓词在公开入口按所有权传入，`PushDownNot` 可返回改写后的表达式；内部观察表达式时使用共享借用，需要重建或折叠时显式 `CloneExpr`/克隆常量。

`ResetNotNullFlag` 独占借用 `&mut Schema`，因此 Rust 类型系统阻止同一 Schema 在修改期间被并发读写；逐列克隆后替换还避免意外修改可能被其他计划结构共享的 `Column`/`RetType` 对象。性能成本主要来自表达式递归、克隆和可折叠函数重建，深度与表达式树规模相关；没有跨调用缓存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/null_misc.go`，Go 测试是 `pkg/planner/util/null_misc_test.go`。Rust 保留了 Go 的整体算法与符号分层：`nullRejectProof`/`NullRejectProof`、`allConstants`、`IsNullRejected`、递归符号证明、`IN` 规则、置 NULL 折叠、`COALESCE`/`IFNULL`/`IF` 特判、常量分类、截断错误忽略上下文和 `ResetNotNullFlag` 均一一对应。

两版的语义重点一致：区分 `nonTrue` 与 `mustNull`，对未知 builtin 保守返回无证明，避免计划缓存相关动态常量被提前折叠，并保留列类型后删除非空标志。Go 的函数属性使用 map，Rust 将其移到 `null_misc_builtins.rs` 的静态切片并通过查询函数访问；Rust 中少数尚无 AST 常量的名称以字符串保留，这属于登记表适配而非本文件逻辑变化。

实现细节上，Go 的普通函数置 NULL 折叠会遍历全部参数、记录 `allConstantArgs` 和 `hasNullArg`；Rust 在循环中用 `?`，遇到首个不可折叠参数即退出该折叠路径。对于 NULL 传递函数，后续符号证明仍会依据任一 `must_null` 子式得出结论，因此整体设计保持保守，但若调整折叠顺序或加入新特殊函数，应同时对照两版并增加等价测试。Rust 当前独立测试覆盖核心逻辑组合与 `IN`，Go 测试覆盖更广的 builtin、`COALESCE`/`IF`、延迟表达式、JSON/AES/WEEK 特例；不能把 Go 的广覆盖误认为这些边界已经全部由 Rust 测试直接验证。

## 扩展指南

- 新增逻辑运算或特殊真值规则时，优先修改 `proveNullRejectedScalarFunc`；必须写出 SQL NULL、FALSE、TRUE 三种输入下的规则，并维护 `must_null => non_true`。
- 新增 `IN` 类语义应修改或仿照 `proveNullRejectedIn`，特别检查空参数、被查值为 NULL、部分/全部候选为 NULL。
- 新增会隐藏 NULL 但可在置 NULL 后确定分支的函数，应接入 `tryFoldNullifiedScalarFunc` 并增加专用折叠函数；不要仅凭“通常返回 NULL”将它加入 NULL 传递表。
- 新增或修正 builtin 属性时应修改 `pkg/planner/util/null_misc_builtins.rs`，同步 Go 对照表，并更新 `pkg/planner/util/null_misc_test.rs::test_null_reject_builtin_registry_snapshot` 的数量、去重和代表性查表断言。
- 修改参数标记、延迟表达式或计划缓存处理时，应重点复核 `allConstants`、`proveNullRejected` 的 `allow_nullified_fold` 传播、`tryFoldStaticConstant` 和 `proofFromConstant`；错误的静态化可能导致不安全的计划复用。
- 修改 Schema 可空性处理时应在 `ResetNotNullFlag` 保持克隆后替换，并检查 `logical_join.rs` 与 `logical_max_one_row.rs` 的区间边界。
- Rust 测试逻辑必须继续放在独立的 `pkg/planner/util/null_misc_test.rs`，不要内嵌到生产源文件。至少同步验证正反例；若移植 Go 的广泛回归场景，应保持 Go 测试的实际分支与预期，而不是用简化桩代替。
- 兼容风险主要是 SQL 三值逻辑或 MySQL builtin NULL 语义漂移；正确性风险高于性能收益。性能改动需避免重复克隆/折叠，但不能用缓存绕过 `PlanContext`、SQL mode、类型上下文或计划缓存保护。

## 验证依据

- 源码：`pkg/planner/util/null_misc.rs`，完整检查了 453 行及其中所有结构体、公开函数和私有辅助函数；文件没有 trait、impl、模块级常量或条件编译项。
- crate 与模块接线：`pkg/planner/util/Cargo.toml`、`pkg/planner/util/lib.rs`；确认 crate 名、依赖、`mod null_misc`、公开再导出和独立测试模块声明。目录内不存在更近的 `doc.go`。
- builtin 直接依赖：`pkg/planner/util/null_misc_builtins.rs`；确认两类登记表、`NullRejectTestMode` 和查询函数。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`explore "pkg/planner/util/null_misc.rs IsNullRejected proveNullRejected"` 确认 `IsNullRejected -> proveNullRejected`、递归证明/折叠辅助链，以及来自 `logical_join.rs`、`logical_projection.rs`、`funcdep_misc.rs` 和 `null_misc_test.rs` 的调用；按文件读取节点确认目标源和调用点。
- Rust 调用者：`pkg/planner/core/operator/logicalop/logical_join.rs`、`logical_projection.rs`、`logical_max_one_row.rs`、`pkg/planner/util/funcdep_misc.rs`；另用精确源码搜索复核 `IsNullRejected` 与 `ResetNotNullFlag` 的生产调用位置。
- Rust 测试：`pkg/planner/util/null_misc_test.rs`；`test_is_null_rejected_proof_modes` 覆盖内/外侧列、真/假常量、`IS NULL`、NULL 测试函数、`NOT`、`AND`、`OR` 和 `IN`，登记表快照测试覆盖数量、去重与代表函数。
- Go 对照：`pkg/planner/util/null_misc.go` 与 `pkg/planner/util/null_misc_test.go`；逐段核对算法分层、保守失败策略、动态常量限制、特殊 builtin 和更广回归矩阵。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务规定的 11 章节结构检查，并人工复核上述路径和符号均可从当前仓库定位。
