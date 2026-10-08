# [`pkg/session/hint_runtime.rs`](hint_runtime.rs)

## 文件定位

`hint_runtime.rs` 属于 `astersql-session` crate，由 `pkg/session/lib.rs` 以公开模块 `pub mod hint_runtime` 挂载。它位于会话执行入口与 `astersql-util-hint`、`astersql-bindinfo`、`astersql-sessionctx-variable` 之间：在一条文本 SQL、预处理语句或 EXPLAIN 进入实际执行前，选择用于匹配的 AST/SQL，解析查询自身的语句级 Hint，匹配会话级或全局 SQL Binding，再把最终 Hint 及 `SET_VAR` 效果放入本会话的 statement context；语句结束时负责恢复临时系统变量。

crate 边界由 `pkg/session/Cargo.toml` 确认。该文件直接使用 `astersql-parser`/`astersql-parser-ast`、`astersql-util-hint`、`astersql-util-parser`、`astersql-bindinfo`、`astersql-sessionctx-variable`/`vardef`、`astersql-util-sem-v2`、`astersql-errors` 和 planner error 定义。`pkg/session` 没有 `doc.go`；模块契约来自 `pkg/session/lib.rs`、运行时调用点和独立测试。

## 核心职责

1. 通过 `InitializeHintRuntime` 为 hint crate 注册进程级 SEM 受限 Hint 检查器，并保证只注册一次。
2. 通过 `parse_statement_hints` 解析 AST 中的 statement Hint，验证 `SET_VAR` 是否对应存在且允许 Hint 更新的系统变量，并把解析问题统一收集为共享错误。
3. 通过 `StartStatementHints` 建立语句生命周期：清理上一轮 binding 标志、解析并应用查询 Hint、让命中的 Binding Hint 覆盖有效 Hint、更新 `StmtCtx.StmtHints` 和警告。
4. 通过 `StatementHintGuard` 将 Hint 生命周期绑定到 Rust 所有权边界：显式 `Finish` 正常还原，遗漏显式收尾时由 `Drop` 尝试兜底还原。
5. 通过 `BindingStatementFromAST`、`SessionBindingCatalog` 和 `BindingMatchContext` 实现会话所需的 binding 描述、会话/全局 binding 快照、digest 与跨库表名匹配及匹配缓存。
6. 通过 `StartStatementHintsWithBindings` 组合 binding 匹配和 Hint 生命周期，使调用方无需自行挑选 Binding 或构造 `HintsSet`。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `INITIALIZE_HINT_RUNTIME: Once` | 私有静态量 | 将受限 Hint checker 的全局注册限制为进程内一次。 |
| `restricted_hint_checker` | 私有函数 | 把 `sem_v2::IsRestrictedHint` 的错误转换为 hint crate 使用的无栈错误。 |
| `InitializeHintRuntime` | 公开函数 | 幂等注册受限 Hint checker。 |
| `parse_statement_hints` | 私有函数 | 调用 `hint::ParseStmtHints`，提供 `SET_VAR` 与 `HYPO_INDEX` 检查器并归一化警告类型。 |
| `apply_set_vars` | 私有函数 | 应用 `StmtHints.SetVars`，登记首次旧值；失败只追加警告。 |
| `StatementHintGuard<'a>` | 公开结构 | 借用本次会话变量，保存查询 Hint、最终 Hint、警告、命中的 binding SQL 和收尾状态。 |
| `BindingSQL` / `QueryHints` / `EffectiveHints` / `Warnings` | 公开方法 | 暴露生命周期内的只读结果。 |
| `ApplySuccessfulOptimizeEffects` | crate 内方法 | 为运行时直接求值且成功优化的 SELECT 再应用最终 `SET_VAR` 效果，并将失败写入 statement warning。 |
| `Finish` / `Drop` | 公开收尾、析构兜底 | 调用 `SessionVars::FinishHintStatement`；`finished` 防止成功路径重复收尾。 |
| `StartStatementHints` | 公开函数 | 不做 binding 查找，只接受可选的已选 `HintsSet` 并建立完整 Hint 生命周期。 |
| `collect_binding_tables` | 私有函数 | 从 `SelectStmt` 的 FROM、JOIN 和 `TableSource.QuerySource` 递归收集表名、schema 和别名。 |
| `BindingStatementFromAST` | 公开函数 | 把 SQL 与 AST 转为 bindinfo `Statement`，并判定参数标记。 |
| `binding_sql_for_warning` | 私有函数 | 尽量规范恢复命中的 binding SQL，供“查询 Hint 被 Binding 覆盖”警告展示。 |
| `SessionBindingCatalog` | 公开结构 | 会话持有的 session/global binding 集合与匹配缓存；公开当前库及三个功能开关。 |
| `AddSessionBinding` / `AddGlobalBinding` / `ReplaceGlobalBindings` / `Bindings` / `DropSessionBindings` | 公开方法 | 管理 binding 快照、稳定展示顺序与缓存失效。 |
| `BindingMatchContext for SessionBindingCatalog` | trait 实现 | 向 bindinfo 提供开关、缓存及 session/global/fuzzy 匹配能力。 |
| `StartStatementHintsWithBindings` | 公开函数 | 读取 fuzzy/usage 开关，调用真实 `MatchSQLBinding`，解析命中的 BindSQL，再进入 `StartStatementHints`。 |

文件没有条件编译块；命名延续 Go API 风格，因此使用 `#![allow(non_snake_case)]`。

## 执行流程

生产主链首先由 `pkg/session/runtime/dispatch.rs` 选择 Hint 的输入。普通 SQL 使用当前 AST/SQL；`EXECUTE` 使用已保存的预处理 SQL 及重新解析出的 AST；`EXPLAIN` 使用内部语句和去除 EXPLAIN 前缀后的 SQL。随后调用 `StartStatementHintsWithBindings`：

1. 从真实会话变量读取 `tidb_opt_enable_fuzzy_binding`。为对齐 Go 在匹配 universal binding 前应用语句 `SET_VAR` 的顺序，它先预解析一次查询 Hint，仅用其中同名 `SET_VAR` 覆盖本次 fuzzy-binding 开关；这一步不负责真实的应用/恢复。
2. 将 fuzzy-binding 开关与全局 `EnableBindingUsage` 同步到 `BindingMatchContext`，再用 `BindingStatementFromAST` 构造匹配描述。
3. `BindingStatementFromAST` 先从当前 `SelectStmt` 的 FROM/JOIN/派生查询收集表；随后调用 bindinfo 的 parser 型 `CollectTableNames`。只要 parser 返回非空集合就以它为准，因此表达式子查询中的表也能进入匹配事实；最后由 `hasParam` 判断真实参数标记，而不是搜索 SQL 字符串中的 `?`。
4. `MatchSQLBinding` 先查 session binding，再查 global binding；`SessionBindingCatalog::match_bindings` 以无数据库 digest 筛候选，再调用 `crossDBMatchBindingsWithFuzzy` 按当前库、表序列和通配 schema 选择结果。trait 层缓存 statement key 到 `BindingCacheItem`。
5. 若找到 binding，则用其 charset、collation 和数据库调用 `ParseHintsSet`；包含 `*` schema 的 universal binding 以 `*` 作为解析数据库。解析失败会留下警告并按无有效 binding Hint 继续，而不是让整条 SQL 失败。
6. `StartStatementHints` 初始化 Hint 运行时和内建 sysvar，调用 `BeginHintStatement`，重置 `ForceNthPlan=-1`、`WriteSlowLog=false`；之后提取查询 Hint、解析并应用其 `SET_VAR`。
7. 若传入有效 binding Hint，则再次解析并应用其 `SET_VAR`，以 binding 的 `StmtHints` 替换 `effective_hints`，并调用 `MarkHintStatementFromBinding`；否则克隆查询 Hint 为最终 Hint。由于恢复表对同名变量只保留首次旧值，binding 覆盖查询值后最终仍恢复到语句开始前的值。
8. 将最终 `ForceNthPlan`、`WriteSlowLog` 与所有警告写入 `StmtCtx`，返回 guard。dispatch 随即读取最终内存额度、最长执行时间、慢日志与 BindingSQL；成功执行无 FROM SELECT 时调用 `ApplySuccessfulOptimizeEffects`；最后显式 `Finish`。

`pkg/session/runtime/planning.rs::ExecutePlannedKVSelect` 是另一条直接入口：它在建立 planner context 之前启动同一 guard，保证 `SET_VAR` 在规划期可见，并在调用返回前还原。`runtime/explain_select.rs`、`runtime/explain_query.rs` 以及 binding 管理语句则直接复用 `BindingStatementFromAST`。

## 数据与状态

- `StatementHintGuard` 借用 `SessionVars`，不会延长会话所有权；`query_hints` 保留 SQL 原文的解析结果，`effective_hints` 表示 binding 覆盖后的消费者视图。两者分开是产生覆盖警告、测试优先级和执行最终配置的基础。
- `warnings: Vec<String>` 是 guard 对解析/应用问题的展示快照；同一问题同时写入 `SessionVars.StmtCtx`，供会话执行结束后发布。错误不会因转成字符串而中止语句。
- `binding_sql` 只有“匹配成功且 BindSQL 成功解析为 `HintsSet`”时设置。仅 digest/表名命中但 BindSQL 解析失败时仍为 `None`。
- `SessionBindingCatalog.session` 与 `global` 保存 `Arc<Binding>`。相同 `SQLDigest` 的新增项会先移除旧项再追加；`Bindings` 只返回 enabled binding，并按 `OriginalSQL`、`BindSQL`、`SQLDigest` 排序，因此 SHOW BINDINGS 得到稳定快照。
- `cache: HashMap<String, BindingCacheItem>` 属于单个会话 catalog。新增、删除或替换 binding 以及 fuzzy 开关变化都会清空缓存；global 快照仅在长度或对应 `Arc` 指针发生变化时替换并失效缓存。
- `CurrentDB` 参与跨库匹配；`UsePlanBaselines` 在 `New` 中默认为 true；`EnableBindingUsage` 也默认为 true，而 `EnableFuzzyBinding` 沿用默认 false，并会逐语句从系统变量/Hint 同步。
- `SessionVars` 内部另有互斥保护的 Hint sysvar 状态与 restore map。`AddHintSystemVarRestore` 对同名变量使用 `or_insert`，这是查询 Hint 后再应用 binding Hint仍能恢复语句前值的不变量。

## 依赖与调用关系

主要上游关系经 RustCodeGraph 的文件使用摘要与直接调用点共同确认：

- `pkg/session/runtime/dispatch.rs` 在通用执行入口调用 `StartStatementHintsWithBindings`，消费 `QueryHints`、`BindingSQL`、`EffectiveHints`，并在执行结束调用 `Finish`。
- `pkg/session/runtime/planning.rs::ExecutePlannedKVSelect` 在聚焦的 KV SELECT 规划/执行路径调用相同入口；同文件另一条路径会直接调用 `StartStatementHints(..., None)`。
- `pkg/session/runtime/session.rs::ConcreteSession` 以 `RefCell<SessionBindingCatalog>` 持有会话 binding 状态；`runtime/dispatch.rs` 从 Domain 同步 global binding，处理 SHOW/CREATE/DROP BINDING；`runtime/control.rs` 提供添加 session binding 的控制入口。
- `pkg/session/runtime/explain_select.rs`、`runtime/explain_query.rs` 和 `runtime/dispatch.rs` 的 binding DDL 路径调用 `BindingStatementFromAST`。

主要下游关系为：

- Hint：`ExtractTableHintsFromStmtNode` → `ParseStmtHints`/`ParseHintsSet` → `StmtHints`；`RegisterRestrictedHintChecker` 把 SEM 过滤策略注入 hint crate。
- 系统变量：`GetSysVar` 与 `IsHintUpdatableVerified` 做资格检查；`SetHintSystemVarWithOldState` 执行规范化和 setter hook；`AddHintSystemVarRestore`/`FinishHintStatement` 完成恢复。
- Binding：`CollectTableNames`、`hasParam`、`MatchSQLBinding`、`noDBDigestFromBinding`、`crossDBMatchBindingsWithFuzzy` 完成描述、digest、优先级与 fuzzy 匹配。
- SQL 展示：`Parser::ParseOneStmt`、`RestoreWithDefaultDB`、`RestoreOptimizerHints` 尽可能产生带规范 Hint 注释的 SELECT binding SQL。

## 错误处理与边界

该文件刻意把大多数 Hint 问题降级为 statement warning。未知 `SET_VAR` 返回 `ok=false` 并报告 unresolved warning；存在但 `IsHintUpdatableVerified=false` 的变量返回 planner 的 `ErrNotHintUpdatable`；setter 验证或 hook 失败也只追加警告。SEM 限制通过注册 checker 被 hint parser 过滤并告警。

当前 `parse_statement_hints` 没有 catalog-backed index checker，所以任何 `HYPO_INDEX` 都以 `-1` offset 和明确 warning 拒绝。`ParseStmtHints` 的 replica 参数固定为 `1`；这是当前会话切片传递给 hint parser 的 follower 编码，扩展 replica 语义时不能假设它来自当前 session 配置。

`collect_binding_tables` 自身只对 `SelectStmt`、FROM/JOIN、`TableSource.QuerySource` 递归，不遍历 WHERE 表达式；完整表集合依赖随后 `bindinfo::CollectTableNames` 的 parser 结果覆盖。它也只在 parser 返回非空时覆盖 AST 收集结果，因此修改 bindinfo parser 时要保留这种回退语义。

Binding SQL 无法解析时不会报致命错误：它记录错误字符串、不给 `StartStatementHints` 传 binding Hint，并且不设置 `binding_sql`。`binding_sql_for_warning` 对 SELECT 可重新插入规范 Hint 注释；其他语句若通用 restorer 丢失 optimizer comment，则保留原 `BindSQL`。

`Finish` 是文件中唯一把恢复失败作为 `Result<_, VariableError>` 返回的公开路径。底层 `FinishHintStatement` 即使某个 restore hook 失败也继续恢复其余变量，只返回首个错误；析构兜底路径无法向调用方返回错误，因而显式忽略该结果。调用方若需要可观察的恢复错误，必须显式调用 `Finish`。

## 并发与资源生命周期

`INITIALIZE_HINT_RUNTIME: Once` 保证跨线程调用 `InitializeHintRuntime` 时全局 checker 只注册一次。`SessionVars` 的 Hint sysvar 和 restore map 在下游使用 mutex；锁中毒通过 `PoisonError::into_inner` 继续访问。此文件不创建线程、异步任务或 channel。

`StatementHintGuard<'a>` 是语句级 RAII 资源边界。正常执行把 `self` 移入 `Finish`，先置 `finished=true` 再恢复；因此即使 `FinishHintStatement` 返回错误，随后的 `Drop` 也不会重复执行。异常提前返回、panic unwind 或调用方忘记 `Finish` 时，`Drop` 尝试恢复，防止 `SET_VAR` 泄漏到下一条语句，但恢复错误不可见。

`SessionBindingCatalog` 自身没有内部锁，生产代码把它放在 `ConcreteSession.bindings: RefCell<_>` 中，依赖会话串行借用而非跨线程共享可变访问。binding 值用 `Arc` 共享；`ReplaceGlobalBindings` 通过 `Arc::ptr_eq` 检测 Domain 快照是否真的改变，避免无谓清空缓存。警告 SQL 的 parser、临时 `HintsSet`、guard 与匹配结果都限于当前调用栈。

## 与 Go 版本的对应关系

仓库不存在同名 `pkg/session/hint_runtime.go`；Rust 文件把分散在 Go planner、bindinfo、hint 和 session variable 中的行为接到 Rust session 主链：

- `pkg/planner/optimize.go::optimizeNoCache` 先 `ExtractTableHintsFromStmtNode`/`ParseStmtHints`，应用查询 `SET_VAR`，再 `MatchSQLBinding`；命中后绑定 Hint 替换 `StmtCtx.StmtHints` 并再次应用 `SET_VAR`。Rust 的 `StartStatementHints` 保留这一“查询先应用、binding 后覆盖”的顺序。
- Go 的 `pkg/bindinfo/binding.go::MatchSQLBindingWithCache`/`matchSQLBindingCore` 先检查 plan baseline 和缓存，再按 session 优先、global 次之匹配，并用 digest 与表名支持跨库 binding。Rust 通过 `BindingMatchContext` 与 `SessionBindingCatalog` 提供同类顺序和数据，但 catalog 是 Rust 会话持有的快照。
- Go 的 `crossDBMatchBindings` 在 fuzzy 开关关闭时排除含通配 schema 的候选，并在匹配者中选择通配符最少者；Rust 下调 `crossDBMatchBindingsWithFuzzy`，且允许本条 SQL 的 `SET_VAR(tidb_opt_enable_fuzzy_binding=...)` 影响匹配资格。
- Go 的 `StmtCtx.AddSetVarHintRestore` 保存需恢复的旧值；Rust 的 restore map 保留同名变量首次旧值。Go 在执行收尾把 `FoundInBinding` 推进到 `PrevFoundInBinding`（例如 `pkg/executor/select.go`），Rust 由 `FinishHintStatement` 同时完成恢复和标志推进。
- Go `pkg/util/hint/hint.go::ParseStmtHints` 定义重复 Hint、SET_VAR 冲突、SEM 过滤、资源组、慢日志、NTH_PLAN 等解析语义；Rust 复用对应的 Rust hint crate，而本文件只提供 session 所需 checker 和生命周期。

因此本文件不是 Go optimizer 的完整复制：例如 `HYPO_INDEX` 缺少 InfoSchema checker；Go optimizer 还处理 plan cache、严格 SQL mode、fast plan、绑定计划失败回退等规划职责，它们不属于本文件。

## 扩展指南

- 新增 statement Hint 时，解析规则应放在 `astersql-util-hint` 的 `StmtHints`/`ParseStmtHints` 拥有者中；本文件只在该 Hint 需要 session 资格检查、生命周期应用或运行时消费时接线。同步检查 `StartStatementHints` 对 `StmtCtx.StmtHints` 的显式字段写入是否需要扩展。
- 新增可由 `SET_VAR` 修改的系统变量时，应在 sysvar 定义中正确设置 `IsHintUpdatableVerified` 并实现规范化/setter hook；不要绕过 `SetHintSystemVarWithOldState`。必须验证查询 Hint、binding 覆盖和失败恢复三个阶段。
- 若要支持 `HYPO_INDEX`，最可能修改的是 `parse_statement_hints` 的 `hypo_checker` 依赖边界；需要传入真实 catalog/InfoSchema，而不是继续返回占位 offset。此改动会扩大公开签名和调用链，应另行设计。
- 扩展 Binding 支持的 AST 形状时，同时检查 `BindingStatementFromAST`、bindinfo `CollectTableNames`/`hasParam` 与 Go `NormalizeStmtForBinding`。表顺序、schema/alias、表达式子查询和字符串中的 `?` 都是兼容风险。
- 修改 binding catalog 时保持 session 优先 global、disabled 过滤、最少通配、缓存失效和稳定 SHOW 排序；全局快照更新还要复核 `runtime/dispatch.rs::sync_global_bindings`。
- 所有 Rust 回归测试应继续放在独立的 `pkg/session/hint_runtime_test.rs` 或对应集成测试 crate，不应嵌入生产文件。直接测试至少覆盖 binding 优先与还原、fuzzy 开关导致的缓存失效、AST 表/参数事实、SEM warning；底层恢复语义还应同步 `pkg/sessionctx/variable/session_hint_bridge_aster_unit_test.rs`。性能风险集中在每条语句预解析查询 Hint、逐 binding 重算无库 digest、BindSQL 重解析和缓存频繁失效；兼容风险集中在 Go/Rust warning 文本、匹配优先级和收尾时点。

## 验证依据

- 生产源码：`pkg/session/hint_runtime.rs`（完整 594 行）；crate/module 边界：`pkg/session/Cargo.toml`、`pkg/session/lib.rs`。`pkg/session` 下未找到 `doc.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/session/hint_runtime.rs` 读取全部源码并报告该文件被 `runtime/dispatch.rs`、`runtime/planning.rs`、`runtime/session.rs`、`runtime/system_session.rs` 等使用；精确 `callers/callees` 未返回明细，故用直接调用点搜索补齐证据。
- Rust 运行时调用点：`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/explain_select.rs`、`pkg/session/runtime/explain_query.rs`。
- Rust 独立测试：`pkg/session/hint_runtime_test.rs` 覆盖 binding 覆盖查询 Hint并恢复、fuzzy binding sysvar 开关、表达式子查询表收集与字符串问号、SEM 限制；`pkg/sessionctx/variable/session_hint_bridge_aster_unit_test.rs` 覆盖首次旧值恢复及 binding 标志在 Finish 时推进；`pkg/session/test/resourcegrouptest/resource_group_test.rs` 覆盖 SELECT 的 RESOURCE_GROUP 可见性。
- Go 对照：`pkg/planner/optimize.go::optimizeNoCache`、`pkg/bindinfo/binding.go::MatchSQLBindingWithCache`/`matchSQLBindingCore`/`crossDBMatchBindings`、`pkg/util/hint/hint.go::ParseStmtHints`、`pkg/sessionctx/stmtctx/stmtctx.go::AddSetVarHintRestore`、`pkg/executor/select.go`、`pkg/sessionctx/variable/session.go`。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验证用于确认目标文件存在且固定的 11 个二级标题各出现一次；人工复核用于确认定位、主流程、边界、生命周期、Go 差异和安全扩展入口均有上述直接证据。
