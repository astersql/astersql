# `pkg/bindinfo/binding.rs`

## 文件定位

`binding.rs` 是 `astersql-bindinfo` crate 的核心模型与匹配层。crate 根模块 `pkg/bindinfo/lib.rs` 以私有模块 `mod binding` 装入它，再通过 `pub use binding::*` 将这里的公开常量、结构体、trait 与函数暴露给会话、计划缓存和绑定管理代码。`pkg/bindinfo/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/bindinfo`，直接依赖 `astersql-parser`、`astersql-util-hint`、`astersql-util-parser` 和 `serde`。

它位于 SQL 进入优化器之前的计划基线链路：`pkg/session/hint_runtime.rs::StartStatementHintsWithBindings` 将解析后的 AST 转换为 `Statement`，同步 fuzzy-binding 与 usage 开关，调用 `MatchSQLBinding`，再把命中绑定的 hint 交给语句 hint 生命周期。`pkg/planner/core/plan_cache_utils.rs` 则调用 `NormalizeStmtForBinding` 和 `CollectTableNames`，在 prepared statement 的 AST 被后续重写前保存 `BindingMatchInfo`。显示绑定时，`pkg/session/runtime/dispatch.rs::show_binding_record_set` 使用 `RestoreDBForBinding` 恢复默认库。

本文件既不是单纯门面也不是桩：它实际实现绑定状态与元数据、SQL/AST 归一化、表名和参数收集、会话优先的绑定匹配、跨库候选选择、hint 准备、缓存版本合并以及使用时间更新。具体的绑定目录、持久化、增删改和 hint 应用生命周期分别由同 crate 的其他模块及 `pkg/session/hint_runtime.rs` 承担。

## 核心职责

1. 定义兼容 Go 的绑定词汇：`StatusEnabled`、`StatusDisabled`、`StatusUsing`、`StatusDeleted`、`StatusBuiltin`，来源 `SourceManual`/`SourceHistory`，以及 `SessionBindingScope`/`GlobalBindingScope`。
2. 用 `Binding` 表示一条“原 SQL → 带 hint SQL”的绑定，并保存摘要、字符集、排序规则、表名、状态及使用时间；`BindingTime` 以 UTC 微秒整数提供可排序、可序列化的时间表示。
3. 通过 `BindingMatchContext` 把会话实现与匹配算法解耦；`MatchSQLBindingWithCache` 负责前置过滤、语句级缓存、自检模式和耗时记录，`matchSQLBindingCore` 负责生成匹配键并执行“会话级优先、全局级次之”的查找。
4. 通过 `NormalizeStmtForBinding`、`noDBDigestFromBinding`、`CollectTableNames` 和 `hasParam` 从真实 parser AST 提取稳定的匹配事实，避免用字符串替换猜测 SQL 结构。
5. 通过 `crossDBMatchBindingsWithFuzzy` 和 `crossDBMatchBindingTableName` 在同摘要候选中选择表名最精确的启用绑定，并受 fuzzy-binding 开关约束。
6. 通过 `prepareHints` 解析、规范化并保存 optimizer hints，同时调用 `BindingValidator` 做运行环境相关的 SQL 合法性检查。
7. 通过 `pickCachedBinding` 合并缓存与存储版本，使较新记录胜出并让最新删除墓碑阻止旧绑定继续生效。

## 主要符号

- `BindingTime(i64)`：UTC 微秒时间戳。`now` 对系统时钟早于 epoch 的情况回退为零，对超过 `i64` 的值饱和到 `i64::MAX`；`is_zero` 判断未设置值。
- `TableName { Schema, Name, Alias }`：匹配所需的库、表、别名三元组。跨库通配符的实际表示是 `Schema == "*"`；私有 `effective_schema` 提供“空 schema 回退当前库”的辅助语义，但当前文件的主匹配路径没有调用它。
- `Statement { SQL, Tables, HasParamMarker }`：把 SQL 原文与调用方已有的 AST 事实封装在一起。`CollectTableNames` 优先复用非空 `Tables`，否则重新解析 `SQL`；`hasParam` 同样先信任 `HasParamMarker`。
- `HintSet`：当前 Rust 模型只持有规范化后的 hint 字符串列表。`Binding::Hint` 与 Go 的完整 `*hint.HintsSet` 并非同一内部表示。
- `Binding`：绑定主记录。`IsBindingEnabled` 仅接受 `enabled` 与兼容状态 `using`；`size` 按 Go 当前统计口径只估算一组固定字符串字段和两个 `BindingTime`，不代表完整堆占用；`UpdateLastUsedAt`、`UpdateLastSavedAt` 修改内部共享的 usage 状态。
- `bindingInfoUsageInfo`：两个 `Arc<Mutex<Option<BindingTime>>>` 分别保存最近使用与最近保存时间；克隆 `Binding` 后这些时间仍共享。
- `BindingMatchInfo`：计划缓存可预计算的 no-db digest 与表名。只有两个字段均非空时，`matchSQLBindingCore` 才认为信息完整，否则重算并回填。
- `BindingCacheItem`：缓存命中的 `Arc<Binding>`、命中标志和作用域；未命中也会缓存，以避免重复解析与目录查询。
- `BindingMatchContext`：会话适配器。必需方法提供开关、当前库、缓存和会话/全局查找；带 fuzzy 的方法、usage 开关、自检模式及耗时回调有兼容默认值。生产实现是 `pkg/session/hint_runtime.rs::SessionBindingCatalog`。
- `MatchSQLBinding` / `MatchSQLBindingWithCache`：公开匹配入口。后者允许计划缓存传入 `BindingMatchInfo`。
- `crossDBMatchBindingsWithFuzzy` / `crossDBMatchBindingTableName`：跨库候选选择与逐表比较函数。
- `BindingValidator` / `prepareHints` / `checkBindingValidation`：绑定 SQL 准备与外部校验边界。
- `pickCachedBinding`：缓存和存储记录的版本仲裁函数。
- `RestoreDBForBinding` / `NormalizeStmtForBinding` / `eraseLastSemicolon`：SQL 恢复、规范化和文本辅助函数。
- `tableNameCollector` / `paramChecker`：保留 visitor 风格的公开适配器；本文件的完整 SQL 路径实际由 `AstFacts` 及 `walk_ast_*` 系列递归遍历 parser AST。

## 执行流程

### 运行时匹配

1. `StartStatementHintsWithBindings` 先依据会话变量和语句 `SET_VAR` hint 得到有效 fuzzy-binding 开关，再将 AST/SQL 包装成 `Statement`，调用 `MatchSQLBinding`。
2. `MatchSQLBindingWithCache` 若计划基线关闭，或 `mayHaveSQLBinding` 判定为无可绑定形态，立即返回未命中。`INSERT/REPLACE ... VALUES` 与 `... SET` 被排除，`INSERT/REPLACE ... SELECT` 保留；`EXPLAIN` 递归检查其底层语句。
3. `statement_cache_key` 将 SQL 长度与原文、参数标记、每个表的 schema/name/alias 长度和值共同编码，避免只有 SQL 文本相同但预收集表事实不同的语句错误共用缓存。
4. 普通模式下缓存命中直接返回；未命中则计时执行 `matchSQLBindingCore`，回填命中或未命中项，再通过 `record_binding_match_duration` 报告真实查找耗时。测试模式下，若没有传入 `BindingMatchInfo`，prepared cache 可直接复用；否则重算，并用 `assertMatchSQLBinding` 校验旧缓存与新结果的一致性。
5. `matchSQLBindingCore` 复用完整的预计算信息，或调用 `NormalizeStmtForBinding(..., noDB = true)` 与 `CollectTableNames` 重建并回填。随后先调用 `match_session_binding_with_fuzzy`；命中即返回 `session`，且不更新 usage 时间。会话未命中才调用全局目录；全局命中且 usage 开启时调用 `UpdateLastUsedAt`，返回 `global`。

### 摘要与 AST 事实

1. `parse_sql` 使用 `astersql_parser::Parser::ParseOneStmt`，把 parser 错误转换成 `BindError`。
2. `NormalizeStmtForBinding` 只接受 Select、SetOpr、Delete、Update、Insert，或包裹这些类型的 Explain；其他语句、空 SQL、解析失败和无底层语句的 Explain 均返回两个空字符串。
3. `normalize_parsed_statement` 在 `noDB = true` 时调用 `RestoreWithoutDB`，否则调用 `RestoreWithDefaultDB`；恢复结果交给 `NormalizeDigestForBinding` 参数化字面量并产生 digest。
4. `collect_ast_facts` 递归遍历 select/set operation/insert/update/delete/explain，并深入 CTE、子查询、join、表达式、窗口、排序、limit、returning 等位置，收集 `TableName` 和参数标记。`CollectTableNames`/`hasParam` 对解析错误采取保守回退：分别返回调用方已有表列表或 `false`。

### Hint 准备与版本合并

1. `prepareHints` 用 `catch_unwind` 包裹 `prepare_hints_inner`，把 panic payload 转成包含绑定 SQL 的 `BindError`。
2. 已同时具备非空 hint/ID 的记录以及 `deleted` 墓碑直接跳过。其余记录先解析 SQL并识别 `*` schema，再以 `*` 或 `binding.Db` 作为 hint 解析默认库。
3. 非跨库且无参数的 SQL 经过 `checkBindingValidation`；该函数在校验器支持时临时关闭 plan baselines，调用外部 `validate_binding_sql`，随后恢复原值。
4. hint 集合恢复出的 ID 为空且 parser 返回 warning 时，第一个 warning 升格为错误；成功时分别恢复 table/index hints，写入 `Hint.Hints`、`ID` 与解析得到的 `TableNames`。
5. `pickCachedBinding` 合并可选缓存项与存储项，求最大 `UpdateTime`，从最大时间的记录中返回第一个非 `deleted` 项。删除墓碑只有在它独占最新时间时才能确保结果为 `None`；若同一最大时间同时存在非删除项，则输入顺序决定所选非删除项。

## 数据与状态

- 不可变匹配数据主要通过 `Arc<Binding>` 共享，缓存命中可复用同一实例；`assertMatchSQLBinding` 使用 `Arc::ptr_eq`，因此测试自检要求对象身份一致，而不只是字段相等。
- `Binding::UsageInfo` 使用内部可变性，使持有 `&Binding` 的全局匹配路径也能更新时间。该字段带 `#[serde(skip)]`，不会进入持久化序列化；反序列化后使用默认的空时间状态。
- `BindingTime`、`Binding`、`TableName`、`Statement`、`HintSet` 支持 serde，供缓存/存储边界使用；时间是进程时区无关的 UTC 微秒数。
- `BindingMatchInfo` 是一次计划构建/匹配之间的临时复用数据，不可只提供 digest 或只提供表名：任一缺失都会触发两者一起重算。
- `BindingCacheItem` 的生命周期由 `BindingMatchContext` 决定。生产 `SessionBindingCatalog` 在全局列表、会话列表或 fuzzy 开关变化时清空缓存；本文件只定义读写协议。
- 候选顺序是可观察状态：跨库匹配只在 wildcard 数严格减少时替换结果，所以相同精确度保留第一个候选；`binding_test.rs::cross_db_matching_uses_star_and_preserves_first_tie` 固定了该行为。

## 依赖与调用关系

- 上游运行时：`pkg/session/hint_runtime.rs::StartStatementHintsWithBindings -> MatchSQLBinding -> MatchSQLBindingWithCache -> matchSQLBindingCore`。命中后由 hint runtime 重新解析绑定 SQL 并进入语句级 hint apply/restore 生命周期。
- 上游计划缓存：`pkg/planner/core/plan_cache_utils.rs::GeneratePlanCacheStmtWithAST -> NormalizeStmtForBinding + CollectTableNames`，在 prepared AST 被别名解析等步骤重写前保存稳定匹配键。
- 上游升级与展示：`pkg/session/upgrade_run.rs::plan_binding_digest_refresh` 调用规范化刷新摘要；`pkg/session/runtime/dispatch.rs` 多处调用 `RestoreDBForBinding` 生成 SHOW/DDL 所需 SQL。
- 下游 parser：`astersql_parser::Parser::ParseOneStmt` 产生 AST，`NormalizeDigestForBinding` 产生规范化 SQL 与摘要。
- 下游 SQL 恢复：`astersql_util_parser::RestoreWithoutDB` 与 `RestoreWithDefaultDB` 控制跨库摘要和默认库补全。
- 下游 hint：`astersql_util_hint::ParseHintsSet`、`RestoreTableOptimizerHint`、`RestoreIndexHint` 提供 hint 解析与稳定文本表示。
- 会话目录实现：`pkg/session/hint_runtime.rs::SessionBindingCatalog::match_bindings` 先用 `noDBDigestFromBinding` 过滤摘要，再调用 `crossDBMatchBindingsWithFuzzy` 比较表名。
- RustCodeGraph 的直接调用证据包括：`StartStatementHintsWithBindings -> MatchSQLBinding`，`MatchSQLBindingWithCache -> matchSQLBindingCore -> NormalizeStmtForBinding`，以及 `CollectTableNames` 被 hint runtime 与计划缓存共同调用。

## 错误处理与边界

- parser、hint 解析和 hint 恢复错误均包装或映射为 `BindError`；`prepareHints` 额外把 panic 转换成错误，避免异常穿过绑定加载边界。
- `NormalizeStmtForBinding`、`RestoreDBForBinding`、`CollectTableNames`、`hasParam` 对解析失败分别返回空结果或已有事实，而不是传播错误；调用方必须把空 digest/SQL 当作“不可匹配/不可恢复”，不能视为合法摘要。
- `noDBDigestFromBinding` 不吞 parser 错误，适合绑定目录在筛选候选时显式排除损坏的 BindSQL。
- `checkBindingValidation` 拒绝纯空白 SQL；若校验器暴露 plan-baseline 状态，它会在调用后恢复原值。不过当前实现不是 RAII guard：若 `validate_binding_sql` 自身 panic，恢复语句不会执行；通常由外层 `prepareHints` 捕获 panic，但校验器状态可能已被留在关闭状态。这是扩展校验器时需关注的风险。
- 跨库比较要求表数量、顺序和表名都一致，名称比较忽略 ASCII 大小写；只有绑定侧 `Schema == "*"` 是通配符，空 schema 不是通配符。语句 schema 为空时可与当前数据库的显式绑定 schema 匹配。
- fuzzy-binding 关闭时，只拒绝使用过 `*` 的候选；完全精确的候选仍可命中。禁用、删除、内建等非 `enabled`/`using` 状态不会进入候选。
- `mayHaveSQLBinding` 在解析失败时返回 `true`，把最终决定留给后续规范化；因此错误 SQL 可能进入匹配流程，但会得到空摘要/表列表而通常无法命中。
- `pickCachedBinding` 的注释提到按“更新时间、创建时间”，实际代码只比较 `UpdateTime`，没有用 `CreateTime` 做二级排序；文档和扩展逻辑应以当前实现为准。

## 并发与资源生命周期

- `Arc<Binding>` 让会话目录、缓存与调用方共享绑定而无需复制；除 usage 字段外，匹配路径只读绑定内容。
- `LastUsedAt` 与 `LastSavedAt` 各自由 `Arc<Mutex<Option<BindingTime>>>` 保护。锁中毒时通过 `PoisonError::into_inner` 继续读写，保持统计更新可用；两个时间字段不是一个原子快照，调用方不能假设跨字段一致性。
- `BindingMatchContext` 接收 `&mut` 仅用于缓存和开关同步；实际目录查找方法只需 `&self`。本文件不启动线程、异步任务或通道，也不拥有存储连接。
- `prepareHints` 解析出的 AST、parser 与 hint 集合均为函数内临时资源；成功后只把字符串化 hint、ID 和表名写回绑定。
- Go 的表名收集器使用 `sync.Pool`，Rust 实现每次创建局部 `AstFacts` 和 `Vec`，没有全局池；这减少共享状态，但热路径分配特征与 Go 不同。
- 校验器资源的打开、执行和关闭由 `BindingValidator::validate_binding_sql` 的实现负责。本 trait 只规定同步调用和 plan-baseline 开关协议。

## 与 Go 版本的对应关系

- 对照文件是 `pkg/bindinfo/binding.go`。状态常量、`Binding` 的主要字段、启用判定、size 统计口径、会话优先于全局的匹配顺序、全局 usage 更新、跨库最少通配符选择、hint warning 升错以及删除墓碑语义均有直接对应。
- Rust 用 `BindingTime(i64)` 替代 Go `types.Time`，用 `Arc<Mutex<Option<BindingTime>>>` 替代 `atomic.Pointer[time.Time]`；持久化表示和同步原语不同，但保留“未使用为 None、命中全局后更新时间”的意图。
- Go 匹配函数直接依赖 `sessionctx.Context`、`SessionBindingHandle`、全局 handle、metrics 和 statement context；Rust 用 `BindingMatchContext` 隔离这些尚未完全同构的会话设施，生产接线落在 `SessionBindingCatalog`。Go 在 domain 初始化、session binding handle 或 global handle 缺失时显式返回；Rust 由适配器返回 `None` 表达目录不可用。
- Go 缓存键依赖 AST 对象身份且在核心函数内写缓存；Rust 生成包含 SQL、参数标记与表事实的字符串键，并在外层统一缓存命中和未命中。Rust 测试 `matching_avoids_cache_aliases_recomputes_partial_info_and_session_usage` 验证相同 SQL、不同表事实不会错误复用。
- Go `CollectTableNames` 依赖通用 `ast.Walk` 与对象池；Rust 显式列举当前 parser AST 变体递归遍历。新增 AST 变体时，Rust 不会自动获得 Go visitor 的覆盖范围，必须同步更新 `walk_ast_expr`/`walk_ast_node` 等分支和独立测试。
- Go `NormalizeStmtForBinding` 对 Explain SetOpr 有额外的字符串切片兼容逻辑，并基于 AST 的原始 Text 是否为空做保护；Rust 对底层 `SetOprStmt` 使用统一恢复路径，输入来自 `Statement.SQL`。这是实现形态差异，不能从 Go 特例推断 Rust 已逐项等价。
- Go `Binding.Hint` 保存完整 `HintsSet`；Rust `HintSet` 保存恢复后的 table/index hint 字符串。因此需要完整 statement-hint 元数据的运行路径会在 `StartStatementHintsWithBindings` 中从 `BindSQL` 重新解析，而不能只依赖 `Binding::Hint`。
- Go `checkBindingValidation` 实际执行 `EXPLAIN FORMAT='hint'` 并关闭结果集；Rust 把该行为委托给 `BindingValidator`。本文件保证临时关闭/恢复 plan baselines，但资源清理与 SQL 执行语义必须由会话适配器保持。
- Rust 单元回归在 `pkg/bindinfo/binding_test.rs`，跨 crate/真实会话回归在 `pkg/bindinfo/tests/bind_test.rs`；Go 对照测试是 `pkg/bindinfo/tests/bind_test.go` 及同目录其他 bindinfo 测试。

## 扩展指南

- 新增绑定状态、来源或字段时，优先修改 `Binding` 和相应常量，并同步检查序列化、`size` 口径、缓存/存储加载、SHOW 输出及 Go `binding.go`。测试应放在独立的 `pkg/bindinfo/binding_test.rs` 或 `pkg/bindinfo/tests/bind_test.rs`，不要内嵌到生产文件。
- 改变匹配优先级或 cache key 时，修改 `MatchSQLBindingWithCache`/`matchSQLBindingCore`/`statement_cache_key`，并覆盖：会话优先、全局 usage、命中与未命中缓存、prepared test mode、部分 `BindingMatchInfo` 重算、相同 SQL 不同表事实。
- 新增可绑定语句类型或 parser AST 变体时，需要同时审查 `may_have_sql_binding_node`、`is_bindable_statement`、`walk_ast_node`、`walk_ast_expr` 及 Go 对照。尤其要为 CTE、子查询、窗口、returning、set operation、INSERT SELECT 与参数标记补独立回归，防止摘要和表列表失配。
- 改变跨库规则时，集中修改 `crossDBMatchBindingTableName` 与 `crossDBMatchBindingsWithFuzzy`，保留表顺序/数量不变量、ASCII 不区分大小写、当前库回退、`*` 计数、fuzzy 开关和同精确度首项胜出的测试。
- 扩展 hint 类型时，检查 `prepare_hints_inner` 当前只收集 `tableHints` 与 `indexHints` 的事实，并与 `StartStatementHintsWithBindings` 的完整重新解析保持一致；不要仅向 `HintSet.Hints` 填字符串却遗漏运行时所需语义。
- 增强校验生命周期时，建议为 plan-baseline 状态引入可析构恢复的 guard，确保错误和 panic 都恢复原状态；同时在独立测试中记录校验调用次数、失败传播和状态恢复。
- 调整版本仲裁时，先明确同一 `UpdateTime` 的冲突规则。若要兑现注释中的 `CreateTime` 次序，应同步 Go 行为或记录明确差异，并补充“同更新时间、一条墓碑一条有效记录”的确定性测试。
- 性能敏感修改应关注 parser 重复解析、AST 遍历分配、每个候选调用 `noDBDigestFromBinding`、cache key 字符串分配和 usage mutex 竞争；计划缓存传入完整 `BindingMatchInfo` 是当前避免重复归一化的主要接口。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `pkg/bindinfo/binding.rs`（1087 行、98 个符号）；文件被 `pkg/session/hint_runtime.rs`、`pkg/planner/core/plan_cache_utils.rs`、`pkg/session/runtime/dispatch.rs` 和绑定测试等直接使用。
- RustCodeGraph 调用证据：`MatchSQLBindingWithCache -> matchSQLBindingCore -> NormalizeStmtForBinding`；`StartStatementHintsWithBindings -> MatchSQLBinding`；`CollectTableNames` 的调用方包括 `BindingStatementFromAST`、计划缓存与核心匹配；`prepareHints`、跨库函数及 `pickCachedBinding` 均被独立 Rust 测试覆盖。
- 已阅读生产与边界文件：`pkg/bindinfo/binding.rs`、`pkg/bindinfo/lib.rs`、`pkg/bindinfo/Cargo.toml`、`pkg/session/hint_runtime.rs`、`pkg/planner/core/plan_cache_utils.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/upgrade_run.rs`。
- 已阅读 Go 对照：`pkg/bindinfo/binding.go`，重点核对匹配缓存、会话/全局顺序、跨库匹配、AST visitor、hint 准备、版本仲裁、SQL 规范化与校验生命周期。
- 已阅读 Rust 测试：`pkg/bindinfo/binding_test.rs` 和 `pkg/bindinfo/tests/bind_test.rs`；补充证据来自 `pkg/bindinfo/binding_operator_test.rs::canonical_cached_binding_prefers_newer_and_honors_delete_tombstone`。测试固定了 size 口径、摘要来源、跨库 tie、墓碑、参数识别、嵌套表收集、hint 快捷路径、规范化、缓存隔离、INSERT/REPLACE 过滤和真实会话命中等边界。
- 本任务是只读行为分析和文档新增，按计划不运行 Cargo。结构验收以任务文件指定命令确认目标文件存在且恰有 11 个固定二级章节；结论中的“当前实现”均来自上述源码、调用图或测试，没有把 Go 特例写成 Rust 已支持事实。
