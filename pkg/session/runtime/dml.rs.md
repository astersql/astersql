# `pkg/session/runtime/dml.rs`

## 文件定位

`dml.rs` 是 `astersql-session` crate 中 `runtime` 私有模块的关系型 DML 执行层。模块由 [`pkg/session/runtime.rs`](../runtime.rs) 以 `mod dml;` 挂入，源码中的 `impl ConcreteSession` 因 `use super::*` 而直接扩展具体会话运行时；它不是通用 planner/executor ABI，也不负责 SQL 文本解析。

上游在 [`control.rs`](control.rs) 中接收规范 AST：`execute_insert`、`execute_update`、`execute_delete` 先处理会话 KV 表、系统表和多表形态，再分别调用本文件的 `execute_relational_insert`、`execute_relational_update`、`execute_relational_delete` 或 join 版本。计划结构和表达式辅助函数来自 [`pkg/session/dml_runtime.rs`](../dml_runtime.rs) 的 `InsertPlan`、`UpdatePlan`、`DeletePlan`、`EvalExpr` 等。本文件把这些计划落实为 record/index/MLog KV mutation，并接入约束、事务、统计和协议状态。

[`pkg/session/Cargo.toml`](../Cargo.toml) 声明 crate 名为 `astersql-session`、库入口为 `lib.rs`，Go 对照包元数据为 `pkg/session`，唯一 crate feature `nextgen` 不对本文件做条件编译。本文件自身没有 `#[cfg]` 项、类型、trait 或模块级常量；主要由 7 个模块级辅助函数和一个大型 `impl ConcreteSession` 组成。

## 核心职责

1. 把 INSERT/REPLACE、UPDATE、DELETE（含 join 变体）转换为行键、行值、二级索引键和物化视图日志 mutation；最终交给真实 `kv::Transaction`，而不是内存桩。
2. 在写入前后维持 MySQL/TiDB 语义：默认值与 NOT NULL、类型转换、生成列、AUTO_INCREMENT、唯一键、`IGNORE`、`REPLACE`、`ON DUPLICATE KEY UPDATE`、`ORDER BY ... LIMIT`、`CLIENT_FOUND_ROWS` 和 statement message。
3. 执行外键父行检查、锁定、RESTRICT 与 CASCADE；区分悲观事务、乐观事务和 autocommit 的冲突保护方式。
4. 统一显式事务与 autocommit 的 mutation 生命周期，包括行锁、schema-change/read-only commit fence、失败回滚、提交报告和锁释放。
5. 维护与写路径耦合的派生状态：分区物理 ID、统计 delta、TTL 插入计数、MLog 统计、自增水位和 `DmlExecutionReport`。
6. 为 DDL/后台路径提供局部复用入口，例如索引回填、分区重组、临时表数据清理和待执行外键删除级联。

## 主要符号

- `ignored_integer_overflow_value`：仅对整数列解析 `i128`，按列类型和 unsigned 标志把越界值钳制到 MySQL 类型范围；供 `INSERT IGNORE` 的降级路径使用。
- `ignored_not_null_value`：为被 `IGNORE` 降级的 NULL 提供类型化零值；日期、日期时间、时长、字符串/Blob 和数值采用不同表示。
- `relational_dml_limit_window`：从 AST `Limit` 提取 `usize` 的 offset/count；无 count 视为 `usize::MAX`，解析失败包装为 `SessionError`。
- `runtime_unique_lock_key`：按固定前缀、table ID、index ID、每个值的长度和值编码运行时唯一锁键，避免简单拼接歧义。
- `unique_lock_keys_for_rows` / `primary_lock_keys_for_rows` / `unique_lock_keys_for_predicate`：分别从完整行、主键列和单列等值/IN/AND 谓词推导锁键；只纳入 public unique index，缺列或 NULL 时不产生完整唯一键。
- `RegisterDmlTable`、`ReadDmlRows`、`LastDmlReport`：对外提供关系表元数据注册、行扫描和最近 DML 报告；`InjectNextDmlCommitError`、`SetRetryAutoIncrementIDsForTest`、`InjectAutocommitRetryForTest` 是独立测试使用的注入接口。
- `backfill_relational_index`、`reorganize_partition_rows`、`clear_temporary_table_data`、`clear_local_temporary_table_data`：为 DDL/维护流程复用行编码和统一 mutation 提交路径。
- `validate_and_lock_foreign_keys`：检查子行引用；显式悲观事务根据共享锁配置取共享或排他锁，显式乐观事务记录已检查父键供 prewrite 冲突检测，autocommit/default 路径取排他运行时锁。
- `run_autocommit_commit_retry`：实现测试可观察的 commit/TSO 重试次数，并在每轮检查 SQL killer；真实重试行为受 TiDB 对应 failpoint 控制。
- `apply_relational_mutations` / `apply_relational_mutations_with_autocommit`：所有关系型写入的提交汇聚点，负责 entry-size 检查、加锁、显式事务缓冲或 autocommit transaction、schema/read-only fence、回滚/提交、报告和资源清理。
- `row_physical_id`、`record_relational_stats`、`record_stats_delta`、`flush_pending_stats_deltas`：计算分区物理 ID，并以写前/写后行差异生成 row/modified delta；事务内暂存，适当时统一刷新。
- `evaluate_generated_columns` / `evaluate_ordinary_generated_columns` / `fill_embedding_generated_rows`：求值普通和 `embed_text` 生成列；向量结果经目标列类型转换，并保留隐藏向量距离列的特殊处理。
- `validate_unique_indexes`、`relational_unique_conflict`、`relational_unique_kv_conflict`、`defer_optimistic_insert_constraint`：覆盖语句内重复、现存 record/index 冲突、部分索引条件、NULL 唯一值以及乐观事务延迟约束检查。
- `cascade_foreign_key_updates_at_depth`、`validate_restrict_foreign_key_updates`、`cascade_foreign_key_deletes_at_depth`、`execute_pending_fk_delete_cascade`：实现更新/删除的 RESTRICT、CASCADE、SET NULL/SET DEFAULT 及递归深度保护。
- `execute_relational_insert_with_load_counts`：INSERT 主入口；返回实际 copied/deleted 行数以供 LOAD DATA 路径复用。`execute_relational_insert` 是丢弃计数的薄包装。
- `execute_relational_update` / `execute_relational_join_update`：执行单表或 join UPDATE，去重目标行，重算生成列和索引，并按 capability 计算报告行数。
- `execute_relational_delete` / `execute_relational_join_delete`：执行单表或多表 DELETE，删除 record/index/MLog，触发 FK cascade，并记录统计变化。

## 执行流程

INSERT 的主流程位于 `execute_relational_insert_with_load_counts`：

1. 从会话 SQL mode、client capability 和 AST/plan 得到类型转换标志、严格模式、`NO_AUTO_VALUE_ON_ZERO`、目标 schema/table 及 public 输入列；`IGNORE` 会把截断转换成 warning。
2. 将 `VALUES` 或 INSERT SELECT 的输入映射到表列，补默认值和自增 ID；普通生成列先求值，embedding 生成列可批量补齐。每一行都经过类型转换、NOT NULL/overflow 处理和 record 编码。
3. 读取语句开始时的表快照并逐行探测主键/唯一索引冲突。普通 INSERT 报错；`IGNORE` 记录 warning 并跳过；`REPLACE` 删除冲突旧行；upsert 在旧行上求值 `ON DUPLICATE` 赋值并重新验证。自引用多行语句会看到同一语句已暂存的行。
4. 对最终行集验证语句内唯一性和外键，生成 record、index 与可选 MLog mutation；必要时执行外键 cascade。整批 mutation 只在检查成功后进入 `apply_relational_mutations`，因此前一 tuple 不会提前发布。
5. 更新 `DmlExecutionReport` 的 INSERT 阶段耗时、自增 alloc/rebase、首个实际分配的 `LastInsertID`、affected rows 和 message；最后比较写前/写后行集记录统计 delta，并为 TTL 表记录插入行数。

单表 UPDATE 在 `execute_relational_update` 中先扫描候选行，应用 AST predicate、排序和 `relational_dml_limit_window`；悲观/autocommit 删除或更新在取锁后会用最新快照与事务 overlay 再确认，避免等待锁期间并发写入造成陈旧 mutation。每个匹配行求值 assignment、重算生成列、重新编码主键/索引/MLog；未改变行不计 changed rows。之后依次验证唯一键、外键、RESTRICT/CASCADE，提交 mutation，并根据 `CLIENT_FOUND_ROWS` 在 matched 与 changed 间选择协议 affected rows。

单表 DELETE 在 `execute_relational_delete` 中同样先筛选、排序和截取目标键；拿到删除锁后重新扫描最新视图并重验谓词，随后删除 record 与索引、追加 MLog、递归处理 FK delete cascade，再统一提交和记统计。join UPDATE/DELETE 由 `source.rs` 的 join 执行结果构造目标行，使用 record key 集合避免同一目标行被 join 多次修改。

## 数据与状态

- 行在本文件内主要表示为 `HashMap<String, Option<String>>`；`None` 表示 SQL NULL。持久化边界通过 `encode_relational_row`、`relational_index_mutations` 和 tablecodec 转成 `kv::Key`/bytes。
- `TableInfo` 提供列、索引、外键、分区、TTL、生成列表达式和 AUTO_INCREMENT 元数据；public 状态决定索引/列是否参与写路径，pending write index 还会从运行时注册表补入。
- `ConcreteSession.state` 保存 transaction、悲观模式、warning/message、protocol affected rows/insert ID、最近报告、自增重试 ID、事务写键、乐观 FK 检查键、统计 delta、savepoint 后待提交 TTL 行数等。访问通过 `borrow`/`borrow_mut` 严格缩小借用范围；跨存储调用前常显式 `drop(state)`。
- mutation 使用 `Vec<(kv::Key, Option<Vec<u8>>)>`：`Some(value)` 是 set，`None` 是 delete。record、secondary index、临时索引历史和 MLog mutation 在同一事务中应用。
- `DmlExecutionReport` 记录算子、表、affected rows、last insert ID、write/prewrite keys、是否已提交、commit wait、自增计数和 INSERT/FK 阶段耗时；显式事务内写入时 `Committed` 为 false，真正提交由事务生命周期负责。
- 统计按 `StatsTableKey` 与物理 table/partition ID 聚合 `(row_delta, modified_rows)`；显式事务暂存，autocommit 可在写后立即记录。TTL savepoint 后的插入计数也延迟到可确认的事务边界。

## 依赖与调用关系

上游调用链由 RustCodeGraph 和源码共同确认：

`ConcreteSession::execute`/typed DML executor → [`control.rs`](control.rs) 的 `execute_insert`、`execute_update`、`execute_delete` → 本文件的 `execute_relational_*` → `apply_relational_mutations` → `kv::Transaction::{Set,Delete,Commit/Rollback}`。

重要的同级下游包括：

- [`dml_runtime.rs`](../dml_runtime.rs)：AST plan、字面量/表达式求值、谓词、排序与 limit 数据结构。
- [`row_codec.rs`](row_codec.rs) 及 tablecodec：record/index 编解码、类型转换和 handle 构造。
- [`source.rs`](source.rs) / [`query.rs`](query.rs)：INSERT SELECT、join UPDATE/DELETE 的行源与相关子查询表达式。
- [`mlog.rs`](mlog.rs)：`RuntimeMLog::for_table`、tracked column 判定和日志 mutation。
- [`transaction.rs`](transaction.rs)：schema-change 检查、事务 overlay、行锁释放以及提交阶段共享状态。
- Domain/info schema/KV：解析运行时表、读取 schema 快照、开始真实事务、读取最新键值。
- statistics handle：按逻辑表或分区物理 ID 记录 delta。
- test failpoint 与 SQL killer：模拟 commit/TSO 故障，并允许重试过程被 kill。

`Cargo.toml` 中可直接对应到本文件引用的依赖包括 `astersql-kv`、`astersql-meta-model`、`astersql-parser-ast/mysql/types`、`astersql-expression`、`astersql-infoschema`、`astersql-statistics-handle`、`astersql-table`、`astersql-tablecodec`、`astersql-types` 和 `astersql-testkit-testfailpoint`；多数名字通过 `runtime.rs` 的父模块导入后由 `use super::*` 获得。

## 错误处理与边界

- 所有运行时失败统一返回 `SessionResult`；外部 crate 错误用 `session_error` 或 `session_kv_error` 添加“解析 LIMIT”“编码唯一索引”“应用/提交事务”等阶段上下文。
- 目标表不存在、注册元数据缺 ID/名称/列、LIMIT/OFFSET 不是 `usize`、表达式或类型转换失败都会立即返回；文档不应把本文件描述成支持任意 planner 表达式，实际能力受 `dml_runtime` 和关系查询求值器覆盖范围限制。
- 写入前拒绝超过 `txn_entry_size_limit` 的单项 mutation，错误码形态为 `[kv:8025]`。唯一键错误使用 `[kv:1062]Duplicate entry ...`；`IGNORE` 可将特定错误变成 warning/钳制值，但不会吞掉任意基础设施错误。
- autocommit 写入任一 `Set/Delete` 失败、注入 commit 错误、schema 在执行期间改变、只读模式在 commit 前开启或 `Commit` 失败时，都回滚（可执行处）、释放行锁并返回错误。成功提交前不会发布整批行，独立测试验证了多行语句原子性。
- 外键检查受会话 `foreign_key_checks` 等状态影响；级联递归存在深度上限。测试证明超过 15 层会以 `cascade depth exceeded` 原子失败，而 15 层可成功。
- 唯一索引检查跳过含 NULL 的索引值，并尊重 public 状态和部分索引条件；不能把 `unique_lock_keys_for_predicate` 当成通用谓词分析器，它只识别单列 `=`/`==`、非 NOT 的 `IN` 和 `AND` 组合。
- join 写路径按编码 record key 去重目标行；单表 UPDATE/DELETE 的锁后重读和谓词重验是并发正确性的组成部分，扩展时不能省略。

## 并发与资源生命周期

- 单个 `ConcreteSession` 的可变状态通过内部可变性管理；写入跨越 `borrow_mut`、KV 调用和锁操作时，代码主动释放 `RefCell` 借用，避免长时间持有会话状态借用。
- 运行时唯一/主键锁以 domain 指针标识、table/index ID 和值构造；autocommit 总是走运行时锁，显式悲观事务按约束检查配置加锁，显式乐观事务把父键和延迟唯一约束保留到 prewrite 冲突检查。
- autocommit transaction 从 Domain storage 开始，设置 async commit、1PC 和 pessimistic 选项，注册 `RUNTIME_TXN_INFOS` 的 `Committing` 状态；`RuntimeTxnInfoGuard` 在作用域结束时清理注册信息。成功或错误结束后释放当前语句行锁。
- schema 快照在 mutation 应用前固定；commit 前用实际写键再次检查相关 table schema，并在只读模式边界再次检查写权限。这两个 fence 都发生在真实提交之前。
- 显式事务只把 mutation 写入 transaction buffer，累计内存键/字节、写键和冲突上下文；锁、统计和 TTL 的最终释放/发布服从外层 commit、rollback 与 savepoint 生命周期。
- 全局 commit epoch、key epoch、pending write index 等注册表用 `Mutex`/原子量保护；毒化 mutex 通过 `PoisonError::into_inner` 恢复。新增共享状态必须沿用明确的 owner/domain 作用域，不能把会话局部数据无界提升为全局数据。

## 与 Go 版本的对应关系

Cargo 的 `package.metadata.porting.go-package = "pkg/session"` 表明 crate 的总体迁移来源是 Go `pkg/session`，但本文件的具体行为横跨 Go 会话事务层与执行器写路径，不存在一个同路径 `pkg/session/runtime/dml.go`。

- Rust `execute_relational_insert_with_load_counts` 对应 Go [`pkg/executor/insert.go`](../../executor/insert.go) 的 `InsertExec`：两者都区分普通插入、`IGNORE`、upsert 和 `REPLACE`，执行重复键预取/检查，维护 record/copied/touched/updated 与 `CLIENT_FOUND_ROWS` 口径，并暴露外键检查/级联统计。
- Rust UPDATE/DELETE 对应 Go [`pkg/executor/update.go`](../../executor/update.go)、[`pkg/executor/delete.go`](../../executor/delete.go) 和 [`pkg/executor/write.go`](../../executor/write.go) 的执行/`updateRecord` 路径：都必须重算生成列和索引、更新 affected-row message、删除正确的物理 record，并把 FK check/cascade 纳入写操作。
- Rust 显式事务缓冲和 autocommit commit 边界对应 Go [`pkg/session/txn.go`](../txn.go) 的 statement/transaction 生命周期以及 `txnmanager.go` 的 `OnStmtCommit`。当前 Rust 是面向 `ConcreteSession` 的窄运行时实现，不等价于完整 Go planner/executor 的全部优化和算子框架。
- 源码中的多处注释明确以 Go 为兼容基准：`INSERT IGNORE` 严格模式截断降 warning、自增 ID 重试复用、`CLIENT_FOUND_ROWS`、共享 FK 锁开关、schema/read-only commit fence、TTL savepoint 计数和级联深度。
- Rust 独立测试 `dml_runtime_test.rs` 与 `mysql_dml_compat_test.rs` 用真实 SQL/KV 验证这些可观察语义；因此扩展时应优先复刻对应 Go 行为，而不是为了通过局部用例简化错误、锁或计数规则。

## 扩展指南

- 新增 INSERT 冲突语义或列转换规则时，优先修改 `execute_relational_insert_with_load_counts` 的行准备/冲突分支，并同步检查 `ignored_integer_overflow_value`、`ignored_not_null_value`、生成列和 `DmlExecutionReport`；测试应加在独立的 [`dml_runtime_test.rs`](../dml_runtime_test.rs) 或 [`mysql_dml_compat_test.rs`](../mysql_dml_compat_test.rs)，不要内嵌到生产文件。
- 新增 UPDATE/DELETE 谓词、排序或 LIMIT 行为时，需要同时检查 `dml_runtime` plan、`relational_dml_limit_window`、锁后重读/重验，以及 join 版本的目标行去重；用户可见兼容行为应在 `mysql_dml_compat_test.rs` 做端到端断言。
- 新索引类型、部分索引或编码变化必须同步 `validate_unique_indexes`、`relational_unique_*`、`unique_lock_keys_*`、`relational_index_mutations` 和 DDL backfill/reorg 入口；风险包括锁键碰撞、NULL 语义变化、陈旧索引以及额外 KV/内存开销。
- 新外键 action 必须同时更新父键检查、update/delete cascade、RESTRICT、乐观/悲观冲突记录及深度/自引用保护；参考 `dml_runtime_test.rs` 的 on-duplicate FK、自引用、禁用检查和 15 层边界用例。
- 新的 commit 期检查应放在 `apply_relational_mutations_with_autocommit` 的真实 `Commit` 前，并保证每个错误分支回滚和释放锁；若它也适用于显式事务，还必须接入外层 transaction commit，而不能只改 autocommit。
- 新的派生写（例如日志或统计）应尽量与 base row mutation 保持同一事务，并明确失败是否必须回滚主写。性能评审应关注全表扫描、写前/写后行集合复制、唯一/FK 预取次数、全局 mutex 临界区和 mutation 数量。
- 修改 Rust 逻辑时须遵守仓库规则：同步修改独立测试、保持 Go 语义、先做失败回归再修复，完成后运行 `cargo fmt --all`；本说明任务本身未修改 Rust，因此未运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/dml.rs` 确认目标文件已索引且含 759 个符号。`explore`、`node`、`callers`、`callees` 查询覆盖 `execute_relational_insert`、`execute_relational_update`、`execute_relational_delete`、`apply_relational_mutations`、`validate_and_lock_foreign_keys`、`evaluate_generated_columns_inner`、`validate_unique_indexes`、FK cascade 和 `record_relational_stats`。
- 关键调用边：RustCodeGraph 明确给出 `control.rs::execute_update → dml.rs::execute_relational_update`；综合 `control.rs` 源码确认 insert/delete 的同类分派；本文件多处 `execute_relational_* → apply_relational_mutations`，而 DDL 清理/重组也复用该提交汇聚点。
- 已读生产路径：[`dml.rs`](dml.rs)、[`runtime.rs`](../runtime.rs)、[`control.rs`](control.rs)、[`dml_runtime.rs`](../dml_runtime.rs)、[`pkg/session/Cargo.toml`](../Cargo.toml)，以及调用关系涉及的 `row_codec.rs`、`source.rs`、`transaction.rs`、`mlog.rs` 的符号/引用。
- 已读 Go 对照：[`pkg/executor/insert.go`](../../executor/insert.go)、[`pkg/executor/update.go`](../../executor/update.go)、[`pkg/executor/delete.go`](../../executor/delete.go)、[`pkg/executor/write.go`](../../executor/write.go)、[`pkg/session/txn.go`](../txn.go) 与 [`pkg/session/txnmanager.go`](../txnmanager.go)。
- 已读独立 Rust 测试：[`pkg/session/dml_runtime_test.rs`](../dml_runtime_test.rs) 覆盖真实 KV DML、生成列、commit 失败、auto-ID、外键、MLog、分区和级联深度；[`pkg/session/mysql_dml_compat_test.rs`](../mysql_dml_compat_test.rs) 覆盖默认值/NULL、多行原子性、IGNORE/upsert/REPLACE、affected rows、ORDER/LIMIT 与自增协议状态。未发现同目录 `runtime/dml_test.rs`。
- 结构验证要求：目标文件必须存在，且固定二级标题必须恰好出现 11 个；交付时另执行 `git diff --check` 和范围检查，确保唯一新增生产物是本说明文档，未修改 Rust、Go、Cargo 或只读 `plan.md`。
