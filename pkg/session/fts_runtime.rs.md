# [`pkg/session/fts_runtime.rs`](fts_runtime.rs)

## 文件定位

`fts_runtime.rs` 属于 `astersql-session` crate，由 `pkg/session/lib.rs` 以公开模块 `pub mod fts_runtime` 挂载。它是一个面向全文检索（FTS）的轻量、内存态会话切片：用真实 parser AST、DDL 元数据构建、planner FTS 规划/解析器和 KV 事务 mem-buffer 把 Go 主链上的关键 FTS 语义连在一起。它不是 `pkg/session/session.rs` 中完整 SQL Session 的替代品：只支持文件内 `execute` 明确分派的少量 DDL、事务、INSERT 和 FTS SELECT，直接使用者主要是独立测试 `pkg/session/fts_runtime_test.rs`。

crate 边界由 `pkg/session/Cargo.toml` 确认：直接依赖 `astersql-parser`/`astersql-parser-ast`、`astersql-ddl`、`astersql-expression`、`astersql-planner-core`、`astersql-kv`、`astersql-meta-model`/`metabuild`、`astersql-tablecodec` 和 `astersql-config-deploymode`；`nextgen` feature 会传递给 deploy mode 与 kernel type crate。

## 核心职责

1. 把单条 SQL 解析为规范 AST，并将可支持的语句分派到内存表元数据、显式事务、模拟行写或 FTS 规划路径（`parse_one`、`execute`）。
2. 复用 DDL 规范建模入口创建 `TableInfo` 和 FULLTEXT 索引，并维护 TiFlash replica 可用性元数据（`create_table`、`alter_table`）。
3. 在进入 planner 前校验 `FTS_MATCH_WORD`的部署模式、常量查询串和列参数，然后执行 `BuildFullTextPlan` 与 `ResolveFullTextPlan`（`validate_fts_expression`、`plan_select`）。
4. 把当前 KV 事务中目标表的未提交写传给 resolver，阻止脏事务中使用 FTS（`transaction_has_table_content`、`insert`）。
5. 提供最小 PREPARE/EXECUTE/DEALLOCATE 生命周期，保存 SQL 但每次重建计划（`prepare`、`execute_prepared`、`deallocate`）。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `PreparedFtsStatement { sql, cacheable }` | 私有 | PREPARE 名称映射的值；保留原 SQL 和 planner 的 cacheability 判定。 |
| `FtsExecution { plan, from_plan_cache }` | 公开类型、私有字段 | 返回可选 `PlanNode` 及 cache 标记；通过 `plan()` 和 `from_plan_cache()` 只读访问。 |
| `FtsSessionRuntime<S: kv::Storage>` | 公开 | 核心有状态运行时，拥有 storage、表元数据、可选事务、prepared 映射和表 ID 分配器。 |
| `new` | 公开 | 构造空运行时，`next_table_id` 从 1 开始。 |
| `parse_one` / `table_name` | 私有 | 解析单语句；将 FROM/INSERT 约束为无右侧 join、无子查询的单表。 |
| `validate_fts_expression` / `validate_select_fts` | 私有 | 递归扫描函数参数、二元/一元/括号表达式，并扫描 WHERE、投影和 ORDER BY。 |
| `planner_metadata` | 私有 | 将 `TableInfo` 投影为 planner 所需的 `FtsTableMetadata`。 |
| `plan_select` | 私有 | 连接表查找、dirty 检查、计划构建和 resolver 管线。 |
| `create_table` / `alter_table` / `insert` | 私有 | 实现本切片支持的元数据及事务写入操作。 |
| `execute` | 公开 | 文本 SQL 入口，对 AST 类型进行白名单分派。 |
| `prepare` / `execute_prepared` / `deallocate` | 公开 | 最小 prepared statement API。 |
| `Drop for FtsSessionRuntime` | 内部生命周期 | 对尚未结束的事务尝试回滚。 |

文件只有 `#![allow(non_snake_case)]`，没有条件编译块、模块级常量、trait 声明或类型别名。

## 执行流程

`execute(sql)` 先去掉空白和结尾分号，再按大小写不敏感的前 7 个字节剔除可选 `EXPLAIN`。它把剩余文本交给 `parser::New().ParseOneStmt`，随后按 AST 动态类型顺序分派：

1. `CreateTableStmt` 调用 `BuildTableInfoWithStmt`，分配递增表 ID，再以小写表名登记。CREATE 直接包含 FULLTEXT 时先检查 Starter mode。
2. `AlterTableStmt` 遍历 spec：`ADD FULLTEXT` 调用 `BuildCanonicalFullTextIndex`；`SET TIFLASH REPLICA` 写入 `TiFlashReplicaInfo`，以 `Count > 0` 作为本切片的 available 判定；其他 ALTER 被拒绝。
3. `BEGIN` 从 storage 开启事务，但不允许嵌套开启；`COMMIT`/`ROLLBACK` 先 `take` 出活跃事务，然后调用 KV 事务终结方法。
4. `INSERT` 要求活跃事务，把每行第一列的有符号/可转换无符号整数视为 handle，编码为 table row key，并写入固定占位值 `fts-row`。它用于产生真实 mem-buffer 脏状态，不是完整 DML 执行器。
5. `SelectStmt` 先确认存在合法 FTS 表达式，再限定单表、查找表元数据、检查该表的未提交写，构建计划并按 WHERE、TopN、Projection、残留 FTS 拒绝的顺序解析。只有 SELECT 返回 `Some(PlanNode)`。
6. 其他 AST 类型报 `unsupported FTS runtime statement`。`explain` 布尔值在当前实现中未影响计划类型或返回形式；前缀只用于允许用 EXPLAIN 文本触发同一 SELECT 规划路径。

`prepare` 仅接受 SELECT，且会先完整调用 `plan_select`；成功后保存原 SQL 和 `FullTextPreparedCacheability` 结果。`execute_prepared` 克隆条目后再调用 `execute`，所以每次都重新解析和规划。对含 FTS 的 SELECT，该 cacheability 函数固定返回 false，与 Go 测试要求的不命中 plan cache 一致。

## 数据与状态

- `storage: S` 由运行时拥有，`S: kv::Storage`，仅在 BEGIN 时创建事务。
- `tables: HashMap<String, TableInfo>` 是运行时本地 catalog；key 为规范化小写名。它不与 Domain/InfoSchema 共享，也不做持久化。
- `transaction: Option<Box<dyn kv::Transaction>>` 表示最多一个显式事务。COMMIT/ROLLBACK/Drop 都会取走该 Option，防止二次终结。
- `prepared: HashMap<String, PreparedFtsStatement>` 按名称覆盖插入；代码没有单独拒绝同名 PREPARE。
- `next_table_id` 每次 CREATE 先分配再递增。由于在检查 `tables.insert` 的重名结果之前已递增，重名 CREATE 失败仍会消耗 ID；并且 `insert` 会先替换旧元数据再返回“已存在”错误，这是当前代码的可观察边界，不应视为完整 catalog 语义。
- `planner_metadata` 只导出 Public 且 `FullTextInfo.is_some()` 的索引；索引列按 offset 回查表列，非法 offset 被 `filter_map` 忽略。TiFlash 仅在 replica 存在、count 大于 0 且 available 为 true 时对 planner 可用。

## 依赖与调用关系

直接上游证据是 `pkg/session/fts_runtime_test.rs`：RustCodeGraph 对 `FtsSessionRuntime` 的 trail 显示测试文件导入该类型，并由 `runtime()` 构造后调用公开 API。当前没有证据表明完整 `session.rs` 主执行链会构造此运行时，因此应将它视为可执行的聚焦适配层。

主要下游关系为：

- 解析：`parser::New().ParseOneStmt` → `ast::Node` 动态分派。
- 表元数据：`BuildTableInfoWithStmt` → `model::TableInfo`；ALTER FULLTEXT → `BuildCanonicalFullTextIndex`。DDL 入口内部仍执行 Starter 门禁和全文索引形状/字符串列校验，会话层不能绕过。
- 表达式：`builtin_fts::build_match_word` 执行 Starter mode、字符串常量和列参数门禁。
- 规划：RustCodeGraph 确认 `plan_select` 调用 `BuildFullTextPlan`；该 builder 下调 `one_table`、`RestoreFtsExpression`、`read_limit_value`。之后 `ResolveFullTextPlan` 依次调用 Where/TopN/Projection/RejectRemaining resolver。
- 事务：`transaction_has_table_content` 调用 `crate::runtime::transaction_has_table_prefix`。后者对非 pipelined 事务以 `GenTablePrefix(table_id)..PrefixNext()` 扫描 mem-buffer，查到任意 key 即 dirty；pipelined 事务直接返回 false。
- 行键：`tablecodec::EncodeRowKeyWithHandle` 产生与 dirty 扫描相同表前缀下的 key。

## 错误处理与边界

所有公开操作返回 `SessionResult`。parser、DDL、KV 错误与 planner 字符串错误均被转换为 `SessionError`；转换保留了面向用户的错误文本，但此文件不添加语句类型等额外上下文。

明确拒绝的边界包括：非单表或子查询 FROM；无 FROM 的 FTS SELECT；表不存在；不含 FTS 的 SELECT；`FTS_MATCH_WORD` 参数数不为 2、第一参非字符串常量或第二参非列；Premium mode 下的 FTS；不支持的 ALTER；无事务 INSERT/COMMIT/ROLLBACK；嵌套 BEGIN；缺少/非整数/溢出的行 handle；未准备名称；以及 resolver 定义的索引不匹配、TiFlash 不可用、FTS 表达式形状和 dirty transaction 错误。

`validate_fts_expression` 只显式递归 Function/Binary/Unary/Parentheses 结构，其他 AST 容器默认视为未找到 FTS；扩展 parser AST 形状时必须同步评估此遍历。`EXPLAIN` 检测是文本前缀而非 AST ExplainStmt，且当前不校验 `explain` 后的语法选项；这是轻量切片的范围限制。

## 并发与资源生命周期

`FtsSessionRuntime` 的可变 API 都需要 `&mut self`，文件内没有 `Arc`、锁、channel、async task 或内部并发调度；同一实例的串行化由 Rust 借用规则保证，但类型是否 `Send`/`Sync` 取决于 storage 和 trait object，本文件未声明这一保证。

事务的所有权在 BEGIN 后转入 runtime；正常 COMMIT/ROLLBACK 会在调用 KV 前从 Option 移出。因此 KV 的 Commit/Rollback 若返回错误，runtime 也不会恢复该 transaction。`Drop` 是最后的资源保护：若 Option 仍有事务则调用 Rollback，但为了不在析构期 panic，回滚错误被丢弃。mem-buffer iterator 的下游实现在判定 `Valid()` 后显式 `Close()`。

`pkg/session/fts_runtime_test.rs` 中的 `DEPLOY_MODE_LOCK` 与 `DeployModeGuard` 是测试对全局 deploy mode 的并发/恢复保护，并非此生产文件的内部机制。

## 与 Go 版本的对应关系

仓库中没有 `pkg/session/fts_runtime.go`的逐文件对应物。Rust 文件是为可执行验证组合了多个 Go 子系统的聚焦适配层：

- `pkg/expression/builtin_fts.go` 的 `ftsMatchWordFunctionClass.getFunction` 对应 `validate_fts_expression` 下调的 builtin 门禁：Starter only、against 必须是字符串常量、match 参数必须是列；Go 还会在 statement context 记录 FTS 使用，该轻量 runtime 无完整 session statement context。
- `pkg/ddl/index.go` 的 `checkFullTextSupportedInStarter` 与 FULLTEXT 元数据构建语义由 Rust `BuildTableInfoWithStmt`/`BuildCanonicalFullTextIndex` 复用，而不在 session 文件内复制全部 DDL 逻辑。
- `pkg/planner/core/fts_resolve_index.go` 中 Where/TopN/Projection/RejectRemaining 解析阶段对应 Rust `ResolveFullTextPlan`。
- `pkg/planner/core/fts_resolve_index_test.go` 是直接的 Go 行为基准：`TestFTSRequiresStarterMode`、`TestTiFlashFTSMatchWordPushDown`、`TestTiFlashFTSMatchWordPreparedPlanCache` 和 `TestTiFlashFTSMatchWordDirtyTxn` 与 Rust 独立测试的四类场景对齐。Go 测试通过真实 TestKit/Domain/mock TiFlash 驱动完整主链；Rust 测试通过此 runtime 构造局部元数据和 KV 状态。

因此，这里的“对齐”是关键行为与错误意图对齐，不是 Go Session 架构的逐类型完整移植。

## 扩展指南

- 扩展可接受 SQL/AST 时，从 `execute` 的分派白名单和 `table_name` 的单表不变式开始；不要把未支持的完整 DDL/DML 行为假定为已经存在。
- 新增 FTS 可出现的表达式容器时，同步修改 `validate_fts_expression`/`validate_select_fts`，并确保 planner builder/resolver 接受相同形状。
- 修改索引或 TiFlash 元数据时，优先改真实拥有者（DDL/model/planner），再更新 `planner_metadata`的投影；不要在 session 切片里建立第二套规则。
- 修改 dirty 语义时，同时复核 `insert`的行键编码、`transaction_has_table_content` 和 `runtime/relational_scan.rs::transaction_has_table_prefix`，特别注意 pipelined transaction 当前返回 false 的边界。
- 修改 prepared 行为时，保持 `FullTextPreparedCacheability` 与 `execute_prepared` 一致；“重建计划”与“`from_plan_cache` 报告值”是两个需要分别验证的合同。
- 测试应继续放在独立的 `pkg/session/fts_runtime_test.rs`，不要内嵌到生产文件。涉及 Go 主链行为时还应同步检查 `pkg/planner/core/fts_resolve_index_test.go`。性能风险主要来自每次 prepared EXECUTE 的重新解析/规划以及每次 FTS 规划的 mem-buffer 前缀扫描；兼容风险集中在 Go/Rust 错误文本、resolver 顺序和部署模式门禁的偏移。

## 验证依据

- 生产源码：`pkg/session/fts_runtime.rs`（完整 465 行）；直接边界：`pkg/session/lib.rs`、`pkg/session/Cargo.toml`、`pkg/session/runtime/relational_scan.rs::transaction_has_table_prefix`。`pkg/session` 下无 `doc.go`，因此没有额外包级 Go 契约可读。
- RustCodeGraph：`status` 显示索引包含 11,467 文件；`files --filter pkg/session/fts_runtime.rs` 识别该文件的 22 个符号；`node --file ...` 读取文件全貌；`node FtsSessionRuntime` 定位结构与测试导入；`query/node BuildFullTextPlan`、`ResolveFullTextPlan`、`FullTextPreparedCacheability`、`transaction_has_table_prefix` 核对了调用边、resolver 顺序、FTS 不可缓存结论与 dirty 扫描。
- Rust 独立测试：`pkg/session/fts_runtime_test.rs`，覆盖 Premium 门禁、Starter+TiFlash 下推与非法形状、prepared 不命中缓存、非常量参数、未提交写拒绝以及回滚后恢复。
- Go 对照：`pkg/expression/builtin_fts.go`、`pkg/ddl/index.go`、`pkg/planner/core/fts_resolve_index.go`、`pkg/planner/core/fts_resolve_index_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文档存在且固定的 11 个二级标题各出现一次。
