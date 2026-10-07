# `pkg/parser/parser_actions/mview.rs`

## 文件定位

`mview.rs` 是 `astersql-parser` crate 内物化视图（materialized view）及物化视图日志（materialized view log）语法的语义动作分片。它不负责分词、LR 状态迁移或真正执行 DDL；`pkg/parser/parser_runtime.rs` 在 grammar reduction 时取得稳定 `RuleId`、构造 `Rhs` 与 `parser_actions::Context`，`pkg/parser/parser_actions/mod.rs::apply` 再通过本文件的 `owns`/`apply` 将规约结果组装为 `parser-ast` 中的 statement 或中间语义值。

crate 边界由 `pkg/parser/Cargo.toml` 确认：库名是 `astersql-parser`，AST 来自本地依赖 `parser-ast`。目标文件只使用解析器内部的 `Context`、`Rhs`、`RuleId`、生成规则表以及 `mviewCreateOptions`/`mlogCreateOptions`，所有入口均为 `pub(super)` 或私有函数，不是 crate 的公开 API。

## 核心职责

本文件承担四类职责：

1. `owns` 从稳定规则名筛选本分片拥有的物化视图规则，并结合 `RULE_IDS_BY_REDUCTION` 与 `GENERATED_MAIN_ACTION_REQUIRED` 排除没有主语义动作的同名前缀规约。
2. `apply` 将 CREATE、ALTER、DROP、PURGE、CANCEL、REFRESH 物化视图/日志产生式的 RHS 语义值转换为对应 AST statement。
3. 为可选 clause、刷新/清理模式、action 列表和建表选项生成供上层产生式继续消费的动态中间值。
4. 在合并 CREATE 选项列表时检查重复的 `COMMENT`、`SHARD_ROW_ID_BITS` 和 `PRE_SPLIT_REGIONS`，通过 lexer 累计与 Go 版本一致的解析错误。

该文件只表达语法与 AST 字段映射；它不检查表是否存在、不调度刷新或清理任务、不访问 catalog，也不实现物化视图数据维护。

## 主要符号

- `item<T: Any + Clone>(rhs, position) -> Option<T>`：从指定的一基 RHS 位置取得 `yySymType.item`，按目标类型 downcast 并克隆。它用于数值、列名列表、刷新 clause、action 列表及其他可克隆中间值。
- `expression(rhs, position) -> Option<ast::ExprNode>`：读取 RHS 槽的 `expr`，用于 `START WITH`、`NEXT` 等表达式。
- `owns(rule_id) -> bool`：识别 `mview*`、`mlog*`、`create/alter/drop/refresh...materializedview*` 等前缀，并通过生成表确认该规约确实需要主 action。`pkg/parser/parser_actions/remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 固定验证本模块拥有 53 条规则。
- `apply(rule_id, rhs, context) -> Result<bool, isize>`：唯一语义动作入口。它按稳定规则名分支，向 `context.output.statement`、`item`、`expr` 或 `ident` 写入规约结果；正常完成返回 `Ok(true)`，两类 option-list 合并遇到动态值缺失或类型不符时返回 `Ok(false)`。
- 局部闭包 `table(position)` 与 `value(position)`：分别读取克隆后的 `ast::TableName` 和只借用的动态 `Any` 值，减少各 statement 分支的重复 downcast。

主要输出 AST 包括 `CreateMaterializedViewStmt`、`AlterMaterializedViewStmt`、`DropMaterializedViewStmt`、`RefreshMaterializedViewStmt`、`CreateMaterializedViewLogStmt`、`AlterMaterializedViewLogStmt`、`DropMaterializedViewLogStmt`、`PurgeMaterializedViewLogStmt` 和 `CancelMaterializedViewJobStmt`。主要中间类型包括 `MViewRefreshClause`、`MLogPurgeClause`、`MLogAccumulationAlertClause`、两类 alter action、刷新/观察枚举，以及定义在 `pkg/parser/mview_stmt_options.rs` 的两个私有 option accumulator。

## 执行流程

1. `parser_runtime.rs` 规约一个产生式，从生成表取得 `RuleId`，以语义栈切片建立 `Rhs`，并把输出槽、parser 和 lexer 放入 `Context`。
2. `parser_actions/mod.rs::apply` 首先调用 `mview::owns`；命中后直接进入 `mview::apply`。`has_semantic_action` 也复用 `owns`，防止应由本文件处理的规则在动作失败后被误当成普通的 goyacc 默认 `$$ = $1`。
3. 叶子或可选产生式先生成中间值。例如刷新观察选项产生 `None`/`DryRun`/`Profile`，完整刷新模式产生 `InPlace`/`OutOfPlace`/`DeltaApply`，空 option-list 产生默认 accumulator，空 clause 则写 `None` 或默认标量。
4. 递归列表产生式取走左、右 accumulator 或读取已有 action `Vec`，按输入顺序合并。CREATE option 合并同时记录重复项错误，但仍把后一个 comment/标志和所有 table option 合并到输出，以与 Go action 的行为保持一致。
5. 顶层 statement 产生式从固定 RHS 位置读取表名、列、选项、clause、SELECT 或 action 列表并构造 AST。例如 `RefreshMaterializedViewStmt` 根据产生式稳定 ID 的完整刷新后缀区分 COMPLETE/FAST；COMPLETE 读取 complete mode 且没有 `AsOf`，FAST 固定 `CompleteType::InPlace` 并读取可选 `AsOfClause`。
6. `apply` 返回后，运行时把规约输出压回语义栈，后续高层规约继续消费，最终由公开 parser 返回 statement AST。

## 数据与状态

RHS 语义值由 `yySymType` 分字段承载：完整 statement 在 `statement`，表达式在 `expr`，字符串类 grammar 值在 `ident`，其余异构值在 `item: Option<Box<dyn Any...>>`。本文件对可复用值使用 `downcast_ref` 后克隆；对 accumulator 和 SELECT statement 使用 `take()`/`downcast()` 转移所有权，避免复制动态 AST。

`mviewCreateOptions` 累积 comment、重复检测标志和 `Vec<ast::TableOption>`；`mlogCreateOptions` 累积日志建表选项。列表保持 SQL 输入顺序。alter action 列表同样按规约顺序追加，因此 AST 中 action 顺序与用户语句一致。

空产生式的表示需要逐项区分：可选 refresh/purge/alert clause 通常为 `None`，空 attributes 为 `String::new()`，异步模式为 `false`，observe mode 与 complete mode采用各自默认枚举。大量读取使用 `unwrap_or_default()`；这些默认值建立在 grammar 已保证 RHS 类型正确的前提上，不是对任意损坏语义栈的完整校验。

## 依赖与调用关系

上游主链是 `pkg/parser/parser_runtime.rs` 的 reduction → `pkg/parser/parser_actions/mod.rs::apply` → `mview::owns` → `mview::apply`。`parser_actions/mod.rs::has_semantic_action` 也调用 `owns`；`remaining_aster_unit_test.rs` 遍历全部生成规则，验证每条需要 action 的规则只有一个 owner，并确认 `mview_count == 53`。

下游依赖主要是：

- `crate::ast` / Cargo 中的 `parser-ast`：提供所有目标 statement、clause、action、枚举、表达式、表名与 table option 类型。
- `Rhs` 与 `yySymType`：访问规约栈中的一基语义位置；位置必须与 `pkg/parser/parser.y` 产生式严格同步。
- `Context.output`：保存当前规约结果；`Context.lexer`：通过 `Errorf`/`AppendError` 累积重复 option 错误。`Context.parser_state` 在本文件中没有被使用。
- `RULE_IDS_BY_REDUCTION` 与 `GENERATED_MAIN_ACTION_REQUIRED`：限定稳定命名规则的真实 action 所有权，避免仅凭字符串前缀接管无动作规约。
- `mviewCreateOptions`、`mlogCreateOptions`：定义于 `pkg/parser/mview_stmt_options.rs`，是 CREATE 产生式内部使用的私有聚合状态。

RustCodeGraph 的文件节点确认目标共 470 行，并给出了 `item`、`expression`、`owns`、`apply` 的完整源码；精确调用入口由模块分派器和运行时源码交叉核验。

## 错误处理与边界

显式语义错误仅出现在 CREATE option-list 合并：重复 `COMMENT`、`SHARD_ROW_ID_BITS`、`PRE_SPLIT_REGIONS` 会调用 `context.lexer.Errorf` 后 `AppendError`。错误文本与 `pkg/parser/parser.y` 相同；对应 Rust 与 Go 测试都要求公开解析最终返回错误。

语法形状错误主要由生成的 LR 状态机拒绝，而非本文件手工判断。Go 测试 `TestRefreshMaterializedViewStatements` 验证 COMPLETE 必须带具体 mode、FAST 不能带 OUT OF PLACE、CANCEL 必须有 job ID；`TestMaterializedViewCreateOptionOrder` 与 `TestMaterializedViewLogCreatePurgeClauseSyntax` 验证 clause 次序和残缺 PURGE 被拒绝。Rust 的 `go_merge_33_test.rs`/`go_merge_35_test.rs`覆盖相同的成功字段、错误文本与无效输入意图。

类型边界依赖动态 downcast。option-list 合并在左右 accumulator 不存在或类型错误时返回 `Ok(false)`；其他许多字段采用默认值，因此 grammar、稳定 RuleId 和 RHS 位置必须一起更新，否则可能得到默认字段而非显式错误。固定位置通过 `rhs.borrow(position)` 安全返回 `None`，但错误的位置仍可能造成语义错填；新增产生式不能只匹配名称前缀而不逐项核对位置。

`apply` 最后的兜底仍返回 `Ok(true)`，安全性依赖 `owns` 只接管已生成且要求 action 的相关规则。新增相关前缀规则后，必须确保该规则确有对应分支，否则会出现“已处理但未写输出”的风险，并应由 owner/端到端测试捕获。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。每次调用只短暂独占当前 parser reduction 的 `Rhs` 切片、输出槽和 lexer；借用生命周期由 `Rhs<'_>` 与 `Context<'_>` 限定，不会逸出 `apply`。

动态中间值由 `Box` 和 `Vec` 拥有。`take()` 会清空被转移的 RHS 槽，随后所有权进入新的 accumulator 或 statement；只读路径克隆表名、表达式引用型节点或小型值。解析实例之间不存在由本文件引入的共享可变状态，因此并发解析的隔离依赖每个 parser/lexer 实例各自持有状态。

资源生命周期只覆盖一次语法规约及最终 AST 所有权转移；真正的物化视图刷新、日志清理和 DDL job 生命周期位于 parser 之外，不应从本文件推断。

## 与 Go 版本的对应关系

直接 Go 对照不是同名 `.go` action 文件，而是 `pkg/parser/parser.y` 的 Materialized View Statements 区段；辅助 accumulator 的 Go 定义位于 `pkg/parser/mview_stmt_options.go`，Rust 对应定义位于 `pkg/parser/mview_stmt_options.rs`。Rust `apply` 把 Go yacc action 中的 `$N` 类型断言、slice append 和 AST 指针构造改写为一基 `Rhs` 读取、`Any` downcast、`Vec` 追加和 Rust AST 构造。

关键对应关系如下：

- CREATE VIEW/LOG 的表名、列、选项、refresh/purge/alert、attributes 与 SELECT 均保持 `parser.y` 的字段位置和空值语义；两个 option accumulator 保留同样的重复检测标志。
- ALTER VIEW/LOG 的 action 以输入顺序聚合；空 `ALTER ... REFRESH` 构造 FAST refresh，裸 `ALTER ... LOG ... PURGE` 构造默认 purge clause。
- DROP、PURGE、CANCEL 保留 `IF EXISTS`、refresh/log-purge job 类型及 `i64` job ID。
- REFRESH 保留 FAST/COMPLETE、WITH ASYNC MODE、三种 COMPLETE mode、DRY RUN/WITH PROFILE，以及仅 FAST 可携带的 `AsOfClause`。
- Go 的 `MViewStartWithOrNextOpt` 特意避免 typed-nil；Rust 直接用 `Option<Box<dyn Any>>`/`Option<MViewRefreshClause>` 表示缺失，不存在 Go interface typed-nil 陷阱。

`pkg/parser/parser_test.go` 是 Go 语法与 restore 意图的主要证据；`pkg/parser/go_merge_33_test.rs` 和 `go_merge_35_test.rs` 验证 Rust 公开 parser 的 AST 字段、合法/非法语法与重复错误消息。两边当前证据一致，但后续修改仍需逐项同步，不能以共同 grammar 名称替代行为验证。

## 扩展指南

新增或修改物化视图语法时，先在 `pkg/parser/parser.y` 明确产生式与 Go action，再同步生成的稳定规则元数据和本文件分支。若新增规则名前缀不在 `owns` 列表中，应扩展前缀判定；若属于已有前缀，也必须确认 `GENERATED_MAIN_ACTION_REQUIRED` 命中且 `apply` 有实际输出分支。任何 owner 数量变化都要同步审视 `remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 的 53 条断言。

调整 RHS 时必须逐字段核对一基位置，尤其是 CREATE 的 8/9/10/12、CREATE LOG 的 10/11/12、REFRESH 的 5/7/8，以及变长 DROP/CANCEL 规则。新增必需字段不应无条件 `unwrap_or_default()`；应根据 grammar 不变量选择显式失败或安全的可选值。新增 option 时应扩展相应 accumulator、重复标志、合并逻辑及 AST option，并保持输入顺序。

测试必须放在独立文件，不能嵌入 `mview.rs`：端到端解析和 AST 字段优先扩展 `pkg/parser/go_merge_33_test.rs` 或 `pkg/parser/go_merge_35_test.rs`；owner/数字回退约束扩展 `pkg/parser/parser_actions/remaining_aster_unit_test.rs`；Go 对照同步 `pkg/parser/parser_test.go`；AST restore、visitor 或语义命令行为应放入 `pkg/parser/ast/*_test.rs` 的相应独立测试。

主要兼容风险是 Rust/Go 的 nil/default 语义漂移、规则后缀或 RHS 位置变化导致字段错填、漏接规则被兜底为“已处理”，以及重复选项错误文本变化。当前动作只在规约时线性合并短 option/action 列表；若未来支持大列表，应注意反复取出与追加的分配成本，但本文件目前没有执行期性能或并发热点。

## 验证依据

- 目标源码：`pkg/parser/parser_actions/mview.rs`；RustCodeGraph `node --file` 完整核对 470 行，确认模块级符号仅为 `item`、`expression`、`owns`、`apply`，无类型、trait、impl 或条件编译项。
- 主链：`pkg/parser/parser_runtime.rs` 的 reduction、稳定 `RuleId`、`Rhs`/`Context` 调用和默认 `$1` 移动；`pkg/parser/parser_actions/mod.rs` 的优先分派与 `has_semantic_action`。
- crate 与辅助状态：`pkg/parser/Cargo.toml`、`pkg/parser/mview_stmt_options.rs`；Go 辅助对照为 `pkg/parser/mview_stmt_options.go`。
- Go 行为：`pkg/parser/parser.y` 的 Materialized View Statements 全段；`pkg/parser/parser_test.go::TestMaterializedViewDDLStatements`、`TestRefreshMaterializedViewStatements`、`TestMaterializedViewDuplicateOptionsErrMsg`、`TestMaterializedViewCreateOptionOrder`、`TestMaterializedViewLogCreatePurgeClauseSyntax`。
- Rust 测试：`pkg/parser/go_merge_33_test.rs` 的 create/alter/drop/purge/cancel/refresh、option、log 与变体字段用例；`pkg/parser/go_merge_35_test.rs` 的 restore、重复错误和无效顺序用例；`pkg/parser/parser_actions/remaining_aster_unit_test.rs` 的唯一 owner、`mview_count == 53` 与禁止数字回退；`pkg/parser/parser_3_aster_unit_test.rs` 的全部生成 action 已接线断言。
- RustCodeGraph：检查了本地索引状态，使用 `files --filter pkg/parser/parser_actions`、`explore` 和目标文件 `node --file`；宽泛 `explore` 混入其他 materialized-view 模块，因此关键调用边又以 `parser_runtime.rs` 和 `parser_actions/mod.rs` 的直接引用核验。
- 本任务为纯文档分析，依计划未运行 Cargo。交付前使用任务指定命令验证文件存在且恰有 11 个固定二级章节，并人工复查没有把 parser 之外的执行行为写成本文件职责。
