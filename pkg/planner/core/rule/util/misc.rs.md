# `pkg/planner/core/rule/util/misc.rs`

## 文件定位

本文件实现 crate `astersql-planner-core-rule-util` 的杂项公共能力，并由同目录的 [`lib.rs`](lib.rs) 以 `pub use misc::*` 全量再导出。crate 边界和依赖由 [`Cargo.toml`](Cargo.toml) 定义：它直接依赖逻辑计划抽象 `base`、表达式与 schema 模型 `expression`、整数集合 `intset`、元数据模型 `model` 和 MySQL 列标志 `mysql`。因此它位于“逻辑算子/优化规则”与“表达式、表索引元数据”之间，不负责选择优化规则或执行 SQL。

当前 Rust 生产调用点分成四组：排序类算子用列替换同步表达式（`logical_sort.rs`、`logical_top_n.rs`）；Selection 和索引扫描用 schema/唯一键信息推导性质（`logical_selection.rs`、`logical_index_scan.rs`、`logical_datasource.rs`）；旧 Cascades 规则用投影替换与外表列归属判断（`pkg/planner/cascades/old/transformation_rules.rs`）；逻辑算子与规则初始化通过三组回调跨 crate 调用谓词简化实现（`logical_datasource.rs`、`rule_init.rs` 等）。截至本次检索，`BuildKeyInfoPortal` 仅有文件内递归调用，`IsColFromInnerTable` 仅有测试调用，不能据 Go 侧用途宣称二者已接入 Rust 生产主链。

## 核心职责

1. 用列的二进制表达式哈希建立 `ColumnReplaceMap`，在列、相关列和标量函数树中替换列身份，同时保留原表达式拥有的类型/`IN` 操作数语义。
2. 将表达式中的 schema 列替换为同下标的投影表达式；两类树改写都采用写时复制，避免修改可能被多个计划节点共享或已经计算过哈希的表达式。
3. 根据 `UniqueID` 判断列全部来自外表或至少一列来自内表，并判断等值条件是否覆盖某个主键/唯一键，从而支持连接消除和“最多一行”性质推导。
4. 把唯一索引分类为全列非空的强键或允许空值的唯一键，供逻辑算子的 schema 键信息构建使用。
5. 通过 `OnceLock` 保存规则包提供的谓词下推 flag、普通谓词简化和 Join 谓词简化函数，解决 Rust crate 之间不能像 Go 包变量那样直接赋值的初始化边界。
6. 提供后序遍历逻辑计划树并调用各节点 `build_key_info` 的门户；该函数已有实现，但当前 Rust 生产代码中未发现入口。

## 主要符号

- `ColumnReplaceMap = HashMap<Vec<u8>, Column>`：键是 `Expression::HashCode(column)` 返回的原始字节，不经文本编码；值是子计划中的目标列。
- `column_hash_key`、`resolve_column_and_replace`：内部查表核心。命中后克隆目标列身份，再把原列的 `RetType` 和 `InOperand` 覆盖回去；返回值中的布尔量供写时复制逻辑判断祖先是否需要克隆。
- `ResolveExprAndReplace` / `resolve_expr_and_replace`：公开门面与递归实现。识别 `Column`、`CorrelatedColumn`、`ScalarFunction`，其他表达式类型原样返回。相关列只替换其内嵌 `column`；标量函数仅在某个后代变化时克隆，并显式保留 charset/collation、coercibility 和 repertoire。
- `ResolveColumnAndReplace`：单列版本，只返回列值而隐藏“是否变化”。
- `ReplaceColumnOfExpr` / `replace_column_of_expr`：按 `schema.ColumnIndex` 找到投影下标并克隆 `exprs[index]`；标量函数递归规则与前述写时复制一致。
- `IsColsAllFromOuterTable`：非空列集且所有 `UniqueID` 都在外表集合中才返回 `true`。空集返回 `false` 是有意契约。
- `IsColFromInnerTable`：任一列的 `UniqueID` 位于内表集合即返回 `true`；空集自然为 `false`。
- `CheckMaxOneRowCond`：检查等值条件列 ID 是否完整覆盖 `Schema.PKOrUK` 或 `Schema.NullableUK` 中的任意一组键；额外条件不影响结果。
- `CheckIndexCanBeKey`：仅处理 `IndexInfo.Unique == true` 的索引，返回 `(nullable_unique_key, non_null_key)`。全部索引列存在且均为 `NOT NULL` 时仅第二项为 `Some`；列均存在但至少一列可空时仅第一项为 `Some`；非唯一或缺列时两项均为 `None`。
- `SetPredicatePushDownFlagHook`、`PredicateSimplificationHook`、`PredicateSimplificationForJoinHook`：三种函数指针 ABI；后两者传递计划上下文、谓词所有权、常量传播开关和可选过滤器，Join 版本还传两侧 schema。
- 三个 `Register*`：将函数指针写入相应 `OnceLock`，只有首次注册返回 `true`；重复初始化安全地返回 `false` 且不会替换既有实现。
- `RuleInitHooksRegistered`：只检查三个槽是否全部安装，供初始化回归测试使用。
- `SetPredicatePushDownFlag`、`ApplyPredicateSimplification`、`ApplyPredicateSimplificationForJoin`：调用注册函数；未注册时以带具体 hook 名称的 `expect` 触发 panic，表示初始化顺序错误。
- `BuildKeyInfoPortal`：先递归可变子节点，再快照自身 schema 和各子 schema，最后调用节点的 `build_key_info`，保证父节点看到已经完成的子节点键信息。

## 执行流程

列映射改写从调用者构造 `ColumnReplaceMap` 开始。`ResolveExprAndReplace` 对根表达式分派：普通列直接查哈希；相关列查其内嵌列；标量函数先递归克隆每个参数以取得“新值 + 是否变化”。若全部参数未变，函数返回原来的 `ExprBox`；首次发现变化后才克隆函数节点，并只写回变化的参数槽。`logical_sort.rs` 和 `logical_top_n.rs` 用该流程在列裁剪/计划重写后同步 `ByItems`。

投影消除路径中，旧 Cascades 的 `MergeAdjacentProjection::on_transform` 调用 `ReplaceColumnOfExpr`，以底层 Projection 的 schema 确定上层列表达式的下标，再替换为底层投影表达式。下标不存在或超出 `exprs` 长度时保持原表达式，避免越界。

性质推导路径中，`LogicalSelection::BuildKeyInfo` 从形如 `列 = 常量/相关列` 的等值条件收集列 ID，再用 `CheckMaxOneRowCond` 判断是否覆盖任一键。`LogicalIndexScan` 与 `LogicalDataSource` 遍历访问路径的索引，将 `CheckIndexCanBeKey` 的第二返回值加入强键 `PKOrUK`，或把第一返回值加入 `NullableUK`。旧 Cascades 的外连接消除规则用 `IsColsAllFromOuterTable` 拒绝引用内侧列或没有引用列的候选。

谓词简化路径先由 `rule_init::init` 调用 `InstallPredicateSimplificationPassthrough` 安装普通/Join 简化，再注册 flag 设置函数。之后 DataSource、Join、Selection、CTE 等算子调用本文件的门面函数，实际执行 logicalop/rule crate 中的实现。`OnceLock` 使多次 `init` 不改变首次安装的函数。

`BuildKeyInfoPortal` 的算法是严格后序：对每个 `logical_children_mut()` 递归，递归返回后重新通过不可变 children 取得 schema 快照，再在当前节点上调用 `build_key_info`。截至本次验证，没有找到 Rust 生产调用它的入口，故该流程是已实现但尚未接线的能力。

## 数据与状态

列替换表和等值列集合均为单次调用者拥有的数据：前者以 `Vec<u8>` 保留完整二进制哈希键，后者以 `HashSet<i64>` 保存列 `UniqueID`。表达式改写接受并返回拥有所有权的 `ExprBox`，未变化分支直接返还原 box，变化分支创建必要的克隆；函数自身不缓存表达式或 schema。

schema 中与本文件相关的持久性质是 `Columns`、`PKOrUK` 和 `NullableUK`。`CheckIndexCanBeKey` 依赖 `columns` 与 `schema.Columns` 的位置一一对应，并按索引列顺序产出 `KeyInfo`；调用者必须保证两者长度和排列来自同一张表的同一 schema。索引列匹配使用小写名 `Name.L`，而非列 ID。

唯一跨调用共享状态是三个进程级 `OnceLock<fn>`。槽位从未注册单向转为已注册，没有清理或替换路径；它们保存的是无捕获函数指针，不持有请求上下文。`RuleInitHooksRegistered` 只观测注册状态，不触发初始化。

## 依赖与调用关系

下游依赖方面，`expression` 提供 `Expression` 的动态降型/克隆/哈希、`Column`、`CorrelatedColumn`、`ScalarFunction`、`Schema` 与 `KeyInfo`；`base::LogicalPlan` 提供 children、schema 和 `build_key_info`；`intset::FastIntSet` 提供列 ID 成员查询；`model::{IndexInfo, ColumnInfo}` 与 `mysql::type::HasNotNullFlag` 决定索引键分类。标准库的 `HashMap`/`HashSet` 承载调用内数据，`OnceLock` 承载注册状态。

已核对的 Rust 上游包括：

- `logical_sort.rs`、`logical_top_n.rs` → `ResolveExprAndReplace`；
- `logical_selection.rs` → `CheckMaxOneRowCond` 与 `ApplyPredicateSimplification`；
- `logical_index_scan.rs`、`logical_datasource.rs` → `CheckIndexCanBeKey`；
- `logical_datasource.rs`、`logical_join.rs`、`logical_selection.rs`、`logical_plans_misc.rs` → 谓词简化门面；`logical_cte.rs` → flag 门面；
- `pkg/planner/cascades/old/transformation_rules.rs` → `ReplaceColumnOfExpr`、`IsColsAllFromOuterTable`；
- `logical_datasource.rs::InstallPredicateSimplificationPassthrough` 与 `rule_init.rs::init` → 三个注册函数。

RustCodeGraph 的文件关系报告 `misc.rs` 被 7 个逻辑算子文件使用，但精确 `callers` 查询未生成跨文件边，因此以上调用点由精确源码检索补齐。全仓库检索未发现 `BuildKeyInfoPortal` 的 Rust 外部调用，也未发现 `IsColFromInnerTable` 的 Rust 生产调用；Go 侧存在相应调用不等于 Rust 已接线。

## 错误处理与边界

本文件没有可恢复的 `Result` 错误。查不到替换列、表达式类型不受支持、schema 中找不到列或投影数组过短时，均保守返回原值；非唯一索引、索引列缺失或无法形成完整键时返回 `(None, None)`。这些分支不会部分提交 key：只要任一索引列缺失，就丢弃此前累积结果。

`CheckMaxOneRowCond` 对空等值集合返回 `false`；`IsColsAllFromOuterTable` 对空列集也返回 `false`，后者同时服务“没有可证明 duplicate-agnostic 的聚合列”和“没有父计划引用列”两类 Go 语义，不能改成数学意义上的空集全称真。`CheckMaxOneRowCond` 检查可空唯一键，是因为调用者只收集普通等号 `=` 与非 NULL 常量/相关列的形式，而非 NULL-safe equality。

三种调用门面在槽未注册时 panic。这是编程/初始化错误边界，而不是查询级错误；新增调用者必须保证 planner 初始化先完成。注册函数忽略重复注册时不会报告冲突详情，调用方若需要验证完整性应使用 `RuleInitHooksRegistered`。

动态表达式仅显式处理列、相关列和标量函数。其他 `Expression` 实现保持不变；若未来新增包含子表达式的复合节点，必须显式扩展递归，否则其内部列不会被改写。`CheckIndexCanBeKey` 直接用表列位置访问 `schema.Columns[position]`，其安全性依赖调用者维持两者位置契约。

## 并发与资源生命周期

表达式改写只操作调用参数和局部集合，没有锁、异步任务、通道、I/O 或事务。写时复制限制了共享表达式树的可变影响范围：未变化树复用原对象，变化路径克隆祖先，目标投影表达式通过 `CloneExpr` 进入新树。它既避免原树被就地改写，也意味着深表达式的成本与访问节点数和实际变化路径数相关。

三个 `OnceLock` 可并发安全地执行首次注册和读取；生命周期与进程一致。注册函数指针没有析构资源，谓词向量和上下文只在一次调用期间传给 hook。调用门面读取槽后同步执行 hook，不创建后台工作。

`BuildKeyInfoPortal` 通过先结束对子节点的可变借用、再创建 schema 快照规避同一计划对象的别名冲突。快照会克隆当前节点与每个直接子节点的 schema，生命周期只持续到当前节点 `build_key_info` 返回；与 Go 版本的 slice pool 不同，Rust 版本没有对象池或显式复用，因此没有池归还/泄漏风险，但会产生按节点的 schema 克隆成本。

## 与 Go 版本的对应关系

直接对照文件是同目录 [`misc.go`](misc.go)，crate 元数据也声明 `go-package = "pkg/planner/core/rule/util"`。Rust 的列/表达式替换、外内表判断、最大一行判断、索引分类和后序构键总体保留 Go 的分支顺序与语义。

主要表示差异如下：Go 的列替换键是由 `HashCode()` 字节转成的 `string`，Rust 直接使用 `Vec<u8>`，避免文本假设；Go 返回指针并用对象身份判断变化，Rust 返回拥有所有权的值并显式携带 `changed`。Go 的投影替换直接返回 `exprs[idx]`，Rust 用 `CloneExpr` 满足所有权与共享约束。Go 的相关列克隆还显式保留 `Data`；Rust 的 `CorrelatedColumn::Clone` 后只覆盖内嵌列，`Data` 是否保留依赖该 Clone 实现，本文件不另行操作。

Go 的三个 hook 是可赋值包变量；Rust 将其拆成 hook 类型、`OnceLock`、注册函数和调用门面，并增加 `RuleInitHooksRegistered` 供显式初始化验证。Go 若未初始化会因调用 nil 函数而 panic，Rust 用具名 `expect` 提供更明确的 panic 信息。Rust 初始化发生在 `rule_init::init` 及 logicalop 的安装函数，不依赖 Go 的包 `init`。

Go 的 `BuildKeyInfoPortal` 使用 `zeropool` 复用子 schema slice，并把计划/schema 指针传给 `BuildKeyInfo`；Rust 为满足借用规则克隆 self/child schema 快照，且当前没有生产调用点。Go 当前在构键规则、Join 和聚合下推路径中调用该门户，Rust 对应接线尚未在本次检索中出现，应记录为迁移状态差异而非已支持行为。

测试对应方面，Rust 的 [`misc_aster_unit_test.rs`](misc_aster_unit_test.rs) 覆盖类型标志保留、基础表达式/投影替换、空列集与唯一键边界、索引可空/缺列分类；[`misc_test.rs`](misc_test.rs) 验证二进制哈希键能区分高位字节。Go 的 `logicalop_test/logical_operator_test.go` 还直接验证两类标量函数改写的写时复制和原树不变；Rust 源码实现了这一分支，但现有同目录 Rust 测试没有等价的嵌套标量函数断言。

## 扩展指南

新增表达式节点支持时，应修改 `resolve_expr_and_replace` 和/或 `replace_column_of_expr` 的动态类型分派，保持“不变则返回原 box、变化才克隆祖先”的约束，并在同目录独立测试文件中增加原树不变、部分参数变化、嵌套节点和节点元数据保留测试；不要把测试嵌入 `misc.rs`。

调整唯一键推导时，应首先确认 `Schema.PKOrUK`/`NullableUK` 的语义及 Go `misc.go` 对应增量，再修改 `CheckMaxOneRowCond` 或 `CheckIndexCanBeKey`。重点回归非唯一索引、复合索引中间列可空、索引列缺失、schema/元数据顺序不一致及普通等号对 nullable key 的约束。任何改变都会影响 Selection 的行数上界和 DataSource/IndexScan 的键信息，可能进一步改变连接消除和代价估算。

增加第四种跨 crate 回调时，应仿照现有模式同时定义函数指针类型、私有 `OnceLock`、注册函数、调用门面和完整性检查，并在真正的 planner 初始化入口安装。若回调需要捕获状态，现有 `fn` 类型不适用，需要审慎选择带线程安全约束的 trait object，并评估全局生命周期与测试隔离。

若要接通 `BuildKeyInfoPortal` 或 `IsColFromInnerTable`，应从对应 Go 调用点的具体提交增量出发，在相应 Rust 规则/算子中做最小接线，并补独立测试证明后序顺序或连接消除条件；不能仅因函数已存在就认为功能完整。`BuildKeyInfoPortal` 的 schema 克隆可能在大计划树上产生额外成本，接线前应同时评估正确性与性能。

## 验证依据

- RustCodeGraph：`status` 报告索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/core/rule/util` 找到 `misc.rs`、`misc.go`、`lib.rs` 及两份 Rust 测试；`node --file .../misc.rs` 读取了完整 374 行并报告其被 7 个文件使用；对主要公开符号执行了 `query`、`callers` 和 `callees`。图能确认文件内递归/辅助调用，但未返回这些符号的跨文件 callers，故用精确源码搜索补证。
- 已阅读全文：`pkg/planner/core/rule/util/misc.rs`、`misc.go`、`Cargo.toml`、`lib.rs`、`misc_test.rs`、`misc_aster_unit_test.rs`；目标目录不存在 `doc.go`。
- 已核对直接调用证据：`logical_sort.rs`、`logical_top_n.rs`、`logical_selection.rs`、`logical_index_scan.rs`、`logical_datasource.rs`、`logical_join.rs`、`logical_cte.rs`、`logical_plans_misc.rs`、`rule_init.rs`、`rule_init_test.rs`、`pkg/planner/cascades/old/transformation_rules.rs`，以及 Go 的 `logicalop_test/logical_operator_test.go` 和生产调用检索结果。
- 行为边界来自独立 Rust 测试：二进制哈希键、列类型标志、schema 下标替换、外/内表空集及成员关系、PK/UK/nullable UK、非空/可空/缺列索引。Go 测试额外证明标量函数改写应保持原共享树不变。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令、路径/链接存在性检查和 `git diff --check`；结果在最终回复中记录。
