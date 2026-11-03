# `pkg/session/runtime/dml.rs`

## 文件定位

在写入前后维持 MySQL/TiDB 语义：默认值与 NOT NULL、类型转换、生成列、AUTO_INCREMENT、唯一键、`IGNORE`、`REPLACE`、`ON DUPLICATE KEY UPDATE`、`ORDER BY ... LIMIT`、`CLIENT_FOUND_ROWS` 和 statement message。

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

MySQL autocommit 的普通 `ON DUPLICATE KEY UPDATE` 若有 public 二级唯一索引，先准备 VALUES，再对每个输入行的 record key 和非 NULL 唯一 index key 调用当前 statement transaction 的 `BatchGet`。保留 VALUES 中重复的索引键，使真实 TiKV 返回的重复锁进入 client-rust 的解析器；解析成功后，仍执行已有完整冲突扫描，避免 UPDATE 表达式改动唯一值时漏检其他行。读取失败直接返回带 `batch read UPSERT duplicate keys` 的错误；扫描和批量读耗时都累计在 prefetch 时间。PostgreSQL、显式事务、REPLACE、IGNORE 及没有二级唯一索引的路径保持其既有分支。

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

内部 MV 会话绑定真实 stats collector 时，事务和 autocommit 变更进入 Domain 既有 pending stats 队列；普通会话保留原记录路径。

commit 刷出 pending_stats_deltas 时，已附着内部 collector 的会话先累积到真实 SessionStatsItem，再由 Domain sweep 转交原统计增量队列；rollback 仍丢弃 pending delta，避免重复记账。

- Rust 显式事务缓冲和 autocommit commit 边界对应 Go [`pkg/session/txn.go`](../txn.go) 的 statement/transaction 生命周期以及 `txnmanager.go` 的 `OnStmtCommit`。当前 Rust 是面向 `ConcreteSession` 的窄运行时实现，不等价于完整 Go planner/executor 的全部优化和算子框架。
- Rust 独立测试 `dml_runtime_test.rs` 与 `mysql_dml_compat_test.rs` 用真实 SQL/KV 验证这些可观察语义；因此扩展时应优先复刻对应 Go 行为，而不是为了通过局部用例简化错误、锁或计数规则。

来源 `0073665a657ecc5b31f88b410504647395772ebb` 的生产变化位于 client-go 依赖，新增 TiKV 回归要求 INSERT 本身遇到重复 BatchGet 锁并物理解析。Rust 复用已发布 client-rust `v0.4.2-aster.13` 的事务状态缓存与按事务/region 清理去重，本文件只补齐进入该真实客户端路径的最小读取次序接线；不移植 client-go 源码、不改 PostgreSQL 方言。独立 `tests/realtikvtest/txntest/txn_test.rs` 的 `port_0073665a65_` 用例观察真实请求/响应，覆盖已提交二级锁和过期事务回滚，最终检查 SQL 数据及索引。

## 扩展指南

- 新增 INSERT 冲突语义或列转换规则时，优先修改 `execute_relational_insert_with_load_counts` 的行准备/冲突分支，并同步检查 `ignored_integer_overflow_value`、`ignored_not_null_value`、生成列和 `DmlExecutionReport`；测试应加在独立的 [`dml_runtime_test.rs`](../dml_runtime_test.rs) 或 [`mysql_dml_compat_test.rs`](../mysql_dml_compat_test.rs)，不要内嵌到生产文件。
- 新增 UPDATE/DELETE 谓词、排序或 LIMIT 行为时，需要同时检查 `dml_runtime` plan、`relational_dml_limit_window`、锁后重读/重验，以及 join 版本的目标行去重；用户可见兼容行为应在 `mysql_dml_compat_test.rs` 做端到端断言。
- 新索引类型、部分索引或编码变化必须同步 `validate_unique_indexes`、`relational_unique_*`、`unique_lock_keys_*`、`relational_index_mutations` 和 DDL backfill/reorg 入口；风险包括锁键碰撞、NULL 语义变化、陈旧索引以及额外 KV/内存开销。
- 新外键 action 必须同时更新父键检查、update/delete cascade、RESTRICT、乐观/悲观冲突记录及深度/自引用保护；参考 `dml_runtime_test.rs` 的 on-duplicate FK、自引用、禁用检查和 15 层边界用例。
- 新的 commit 期检查应放在 `apply_relational_mutations_with_autocommit` 的真实 `Commit` 前，并保证每个错误分支回滚和释放锁；若它也适用于显式事务，还必须接入外层 transaction commit，而不能只改 autocommit。
- 新的派生写（例如日志或统计）应尽量与 base row mutation 保持同一事务，并明确失败是否必须回滚主写。性能评审应关注全表扫描、写前/写后行集合复制、唯一/FK 预取次数、全局 mutex 临界区和 mutation 数量。
- 修改 Rust 逻辑时须遵守仓库规则：同步修改独立测试、保持 Go 语义、先做失败回归再修复，完成后运行 `cargo fmt --all`；本说明任务本身未修改 Rust，因此未运行 Cargo。

### PostgreSQL限定布尔显式转换（任务3）

PG INSERT中的CAST表达式局部接入现有关系求值器及共享布尔转换核；按目标表保存类型上下文，写入前完成归一化，避免DML字符串转换绕过int4→bool规范化。MySQL继续现有DML求值。

## PostgreSQL 无符号反向目标（任务13）

INSERT 在已声明的共享 unsigned 整数目标列处精确校验来源宽度：拒绝负值、上界之外和非整数字符串，携带 PgNumericError::OutOfRange 到 PG 协议。此分支只在 PostgreSQL 方言启用，不从 PG OID 反推 unsigned；MySQL 既有转换与 warning 分支保留。NULL 继续沿列元数据保存目标身份；验证在发布 mutation 前进行。覆盖见 pg_extended_test::pg_scalar_13 和 relational_value_test::pg_scalar_13_unsigned_target。

批次15：新增表的PostgreSQLNumericText标记明确物理编码，缺省false保留既有表解码；声明numeric列的INSERT目标转换复用PgNumeric，物理行复用字符串Datum保留大精度/scale/特殊值，读取仅覆盖物理解码字段类型，逻辑目录numeric及typmod不变。MySQL创建表继续原MyDecimal路径；numeric索引/算术及DDL类型重建不在此接线范围。

### 2026-10-26 varchar 字符长度（任务22）

共享 `pg_varchar` 按 Unicode 标量计数；显式 CAST 裁剪，赋值仅允许额外 U+0020，类型长度域为 1..10485760。沿已有 FieldType 文本身份及错误源传递，保留 MySQL 路径。回归见 `pg_extended_test::pg_scalar_22`、`builtin_cast_test::pg_scalar_22_formal_varchar` 和 `relational_value_test::pg_scalar_22_varchar_relational`。

### 2026-10-26 定长文本（任务23）

INSERT/UPDATE 文本列赋值覆盖 bpchar，利用源 AST/绑定类型去尾后执行目标字符约束；拒绝超长非空格值，保留既有写入原子性。

### 无目标冲突忽略与 CHECK 候选行局部接线

PostgreSQL `ON CONFLICT DO NOTHING` 的 Any 目标复用列目标仲裁路径，选择真实公开、完整且非部分/MV 的唯一索引及物理主键；候选与本语句先前插入行同样参与冲突检查，跳过候选不覆盖旧行、不增加返回行或影响计数。无唯一索引时不要求目标推断成功。MySQL IGNORE 行为不用于实现此语义。

PG INSERT 在普通生成列求值后、仲裁与发布写入前，读取既有 ConstraintInfo 的 Enforced/StatePublic/ExprString，复用 ParseGeneratedExpr/EvalExpr 检查 CHECK。布尔 FALSE 拒绝，UNKNOWN 通过，求值错误传播；非布尔结果显式报错。此接线限 PG INSERT，不代表 UPDATE/ALTER/回填或完整 CHECK 表达式已验收，也不改变 MySQL 策略。测试见 pg10_07_behavior、pg10_07_all_unique_arbiters、pg10_07_check_prerequisite、pg10_07_check_candidate_semantics。


## 2026-11-02 命名约束仲裁

ON CONFLICT ON CONSTRAINT 从当前目标表的真实声明元数据绑定支撑索引/主键，在候选写入之前验证。不存在名、他表名称和普通唯一索引不能作为约束；仍复用 DO NOTHING 的现有冲突过滤、计数及原子性路径。

PG 冲突推断从完整有序 Elements 验证仲裁形态，再提取基本列绑定真实索引。任何表达式（含括号列）、缺列元素或长度/排序选项均在写入前拒绝，混合目标不能因列投影非空绕过门禁。

PG10-10：普通列推断目标无匹配唯一索引时复用结构化 PG 诊断通道返回42P10，保持写前绑定失败；命名约束路径不改变。

## 2026-11-02 冲突更新候选行绑定

共享 INSERT 支持无动作 WHERE 的基本唯一目标 DO UPDATE：旧行与默认值求值后的 excluded 分别绑定，SET 右侧同时读取旧行；保留未赋值列，按实际新行投影 RETURNING 并按 PG 计数。supported_conflict_action 同时用于简单和扩展执行门禁，扩展 prepared handle 保留原始冲突节点。目标谓词、动作 WHERE、重复命中、生成列及物化视图日志表仍保留独立能力门禁。测试入口为 pg10_11_behavior 和共享层 pg10_11_；运行证据见本次交接。


## 2026-11-02 冲突更新动作条件

PG10-12：动作 WHERE 在共享冲突路径按旧行与 excluded 绑定，受限 `<` 比较和布尔字面量/转换复用共享表达式求值；仅 TRUE 更新，FALSE/NULL 不求值 SET、不返回旧行且不计数。不变值更新仍计1，目标 WHERE 与部分索引推断继续拒绝。扩展协议保留支持的冲突子句参数映射，typed NULL 和新 Bind 条件值进入同一执行链。测试入口 pg10_12_behavior、pg10_12_action_condition；独立参考端、真实并发与性能不在本次验收范围。


### PG10-12 验证证据（2026-11-02）

Ready 按根协议执行；verify-profile 技能缺失，不声称运行该技能。动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，阶段结束释放锁并保留缓存。全部命令从仓库根目录执行，继承该绝对路径：

```bash
cargo fmt --all
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_12_behavior -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_12_ -- --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_11_behavior -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_10_behavior -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_10_expression_gate -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::prepared_crud_roundtrip -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_11_ -- --nocapture --test-threads=1
make lint
git diff --check
```

最终全部退出0；7条测试命令各运行1个有效测试，合计7 passed，无ignored或零测试。TCP新测试的48个成功组合覆盖simple/extended、RETURNING有/无、COMMIT/ROLLBACK，另含两组新Bind及16个错误/恢复组合。W/O完整回读证明提交及回滚；实际隔离REPEATABLE-READ。参数typed NULL不触发更新，参数换值true/false分别更新/跳过。动作表达式除零在Execute报22012，Sync后E、SELECT报25P02，ROLLBACK恢复I并撤销pending写入；服务关闭后资源计数(0,0,0)，端口拒连。共享层验证跳过不求值SET、更新加插入计2，以及目标WHERE和非布尔动作条件写前拒绝。

初始红灯 /tmp/pg10-12-red.log：1 failed，0A000动作WHERE门禁，退出101。最终日志 /tmp/pg10-12-green-final.log、/tmp/pg10-12-session-final.log；相邻日志 /tmp/pg10-12-adjacent-{pg10_11_behavior,pg10_10_behavior,pg10_10_expression_gate,prepared_crud_roundtrip,session}.log；格式化和lint日志 /tmp/pg10-12-fmt.log、/tmp/pg10-12-lint.log。中间验证发现并修复冲突参数映射与typed NULL整数CAST绑定，未把编译或Parse拒绝冒充动作执行成功。

未修改Go/Bazel/依赖，不触发bazel_prepare。未验证部分索引推断（独立门禁）、真实TiKV竞争、持久重启、独立PG18/MySQL客户端差分或性能；后续由各对应任务取证。此证据仅覆盖动作条件及必要局部接线，不外推完整冲突更新能力。

## 2026-11-02 冲突更新二次错误（PG10-13）

复用共享冲突更新、约束校验及整句写集，不增加执行门禁或第二条事务路径。唯一生产修订是 `pg_conflict_check_constraints` 在 CHECK 为 FALSE 时附带结构化 SQLSTATE 23514，保留原约束消息。该 helper 仅用于 PG INSERT 候选及冲突更新新行，不改变 MySQL 约束策略。

真实 TCP 测试 `pg_dml_test::pg10_13_behavior` 覆盖二次 email 唯一冲突（23505）、SET v=NULL（23502）、SET v=0 / v=-excluded.v（23514）及运行期整数除零（22012），另测新插入后错误更新的整句撤销。9个输入各运行 simple/extended 与 idle/transaction，共36组合。扩展路径要求 Parse/Bind/Describe 成功后 Execute 才报错，没有成功行或命令标签；Sync 后显式事务保持 E，SELECT 1 返回25P02，ROLLBACK 撤销 pending 与部分写入并回 I。W/O 完整旧值回读、恢复查询、表清理、连接/listener关闭及资源归零均有断言。实际隔离为 REPEATABLE-READ。

共享层独立测试 `pg10_13_secondary_errors_are_atomic` 覆盖10个混合失败/事务组合，并验证恢复后插入可复用候选主键及 email 唯一键。测试与生产文件分离。红灯 `/tmp/pg10-13-red.log` 实跑1测试、退出101：SET v=0返回XX000；修复后相同测试1 passed（36组合），日志 `/tmp/pg10-13-green.log`。共享层聚焦1 passed，日志 `/tmp/pg10-13-session.log`；TCP相邻14 passed，日志 `/tmp/pg10-13-adjacent.log`。

Ready 按根AGENTS执行（verify-profile技能缺失）。动态领取共享槽位1，绝对 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；全部命令从仓库根目录执行并继承该路径：

```bash
cargo fmt --all
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_13_behavior -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_13_ -- --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_ -- --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_1 -- --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::prepared_crud_roundtrip -- --exact --nocapture --test-threads=1
make lint
git diff --check
git diff --cached --check
```

格式化与lint分别见 `/tmp/pg10-13-fmt.log`、`/tmp/pg10-13-lint.log`，均退出0。未改Go/Bazel/依赖，不触发bazel_prepare。未验证独立PG18/MySQL参考端诊断全文/位置、RealTiKV、持久重启、并发或性能；这些继续交各专项任务，不外推完整冲突更新支持。

最终共享层相邻命令7 passed，日志 `/tmp/pg10-13-session-adjacent.log`；MySQL预处理命令1 passed，日志 `/tmp/pg10-13-mysql.log`。所有最终验证命令退出0，共22个不同测试有通过证据，无ignored或零测试。槽位使用产物的所有子进程结束后释放自己的锁，保留编译缓存。


## PostgreSQL 冲突更新重复命中（任务14）

`execute_relational_insert_with_output` 的 `pg_updated_keys` 仅跟踪本语句实际插入或更新的目标行。冲突后先检查已写入身份，返回 SQLSTATE 21000（cardinality violation）；随后才计算动作 WHERE。FALSE/UNKNOWN 跳过时不留标记，因此先跳过再更新允许成功；先更新再跳过、新插入后再冲突及无值变化的重复更新均失败。沿用现有 mutations 批次提交与事务恢复，不改变 MySQL 分支。

独立测试 `pg_dml_test::pg10_14_behavior` 覆盖 simple/extended、idle/显式事务、带/不带 RETURNING 的重复旧行、新行、前缀插入后重复、先更新后跳过、无值变化；成功例覆盖两次 FALSE/NULL 和先跳过后更新，分别 COMMIT/ROLLBACK，核对返回描述、命令标签、W/O 完整回读、25P02/E、ROLLBACK/I、资源清理。扩展参数用 typed NULL 和新 Bind 换值。共享测试 `pg10_14_repeated_rows_and_skipped_actions` 使用真实共享会话与存储，并验证错误恢复后唯一键仍可用。

本机 PostgreSQL 18.4 独立临时实例采集7种边界（`/tmp/pg10-14-reference.log`），不将18.4视为18.6的实测。正确性关注语句原子性与身份标记，兼容性关注WHERE顺序和21000，性能未测。精确 S/V/C/M/D/H/P 全字段差分及18.6补丁差异未验证；本次验证SQLSTATE和执行阶段。


2026-11-02 Ready 验证（默认 features、无 nextgen）：动态领取槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。命令均从仓库根目录执行并继承该环境变量：

```sh
cargo fmt --all
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_14_behavior -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_14_ -- --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_ -- --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_ -- --nocapture --test-threads=1
make lint
git diff --check
```

格式化、lint、diff检查退出0；聚焦协议1 passed（72组合、6次参数Bind），共享层1 passed（18组合），相邻协议15 passed，共享层19 passed（含MySQL Query/prepared共存），均退出0。红灯 `/tmp/pg10-14-red.log` 有效1测试、0A000与21000断言失败（Cargo退出101）；最终绿灯 `/tmp/pg10-14-green.log`、共享层 `/tmp/pg10-14-session.log`、相邻 `/tmp/pg10-14-adjacent.log` 和 `/tmp/pg10-14-session-adjacent.log`、lint `/tmp/pg10-14-lint.log`。真实隔离 REPEATABLE-READ。临时参考端已停止并删除其数据；槽位完成后释放锁、保留缓存。未修改Go/Bazel/Cargo依赖，不触发bazel_prepare；未运行昂贵全包/RealTiKV测试。

## PostgreSQL DO NOTHING 唯一键竞争

显式悲观事务的 DO NOTHING 在仲裁前取得与普通写入相同的唯一键锁；等待持有者提交或回滚后，以最新提交行及事务内写入重建仲裁视图。这样次级唯一键也能在持有者提交时跳过、回滚时插入，而不是在语句末尾等待后继续使用旧判断。同一 VALUES 已插入但尚未应用的候选由 `lock_rows` 补回视图，保留语句内部重复仲裁。该批次接线仅覆盖 DO NOTHING；后续 DO UPDATE 的扩展见下节，MySQL 重复键策略不变。

真实存储回归入口是 `pg_client_integration_test::pg10_15_competition`，使用外部真实 TiKV 目标与 PG18.6 参考端，以锁等待诊断作为释放写入者的屏障，记录返回值、命令标签、完整回读及清理；测试存储不能替代该竞争验收。

该聚焦测试沿用 `PG_LIBPQ_LIBRARY` 与 `PG18_REFERENCE_DSN`；`PG10_TARGET_DSN` 指向已按真实 TiKV runbook 启动的隔离目标（不得回退测试存储），`PG10_MYSQL_PORT` 为同一服务器原生端口，仅用于创建独占数据库及读取真实 `information_schema.data_lock_waits` 屏障。`PG10_STORAGE_EVIDENCE` 指向启动者保存的脱敏 JSON：目标二进制路径/SHA256、源码 HEAD、PD 版本及 stores 的 TiKV 版本/构建信息；包装测试将其复制到每次运行证据目录。每侧独立执行64个竞争场景，共128个，覆盖双方提交/回滚、主键/次级唯一键/省略目标、NULL、返回/无返回、simple/extended；extended 用明确OID的参数、statement/portal Describe以及同statement的新Bind换值，并在ROLLBACK后回读。

## PostgreSQL DO UPDATE 等待后的仲裁

DO UPDATE 同样在仲裁前取得候选唯一键锁，等待后复用最新行与事务内写入视图，SET 与动作 WHERE 据此读取锁持有者提交的值。刷新时先移除本语句暂存的删除键，再覆盖尚未应用的候选，避免前一候选移动主键后旧行复活。MySQL 路径保持原有策略。

`pg10_16_competition` 使用任务15相同的真实TiKV/libpq入口与锁等待屏障，覆盖提交后最新值更新、回滚后插入、动作WHERE真/假/NULL、主键/次级唯一键、simple/extended、RETURNING、双方COMMIT/ROLLBACK和重绑定恢复。共享层 `pg10_16_refresh_keeps_updated_candidates` 验证同语句主键移动后的候选与回滚。

任务16真实验收（2026-11-02）：参考端PostgreSQL18.6/libpq180006，目标PD/TiKV8.5.8，实际READ COMMITTED；每侧128组合，共256，`pg_client_integration_test::pg10_16_competition` 1 passed。证据 `target/pg10-16-evidence/31147-1793630051099981000/` 保存逐SQL、等待屏障、值/描述/标签、版本与构建、清理结果；日志 `/tmp/pg10-16-delivery.log`。初次真实红灯 `/tmp/pg10-16-red.log` 为XX000/冲突行未解码，非编译失败。中间重跑因既有WHERE仅开放`<`而拒绝`>`；改用等价的`15 < v`复用已开放能力，未扩大谓词支持。

验证使用动态槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`，各阶段结束释放锁并保留缓存。确切Cargo命令：

```sh
cargo fmt --all
cargo build --locked -p astersql-cmd-tidb-server --bin astersql-cmd-tidb-server
cargo test --locked -p astersql-server --lib pg_client_integration_test::pg10_16_competition -- --exact --nocapture --test-threads=1
cargo test --locked -p astersql-session --lib pg10_ -- --nocapture --test-threads=1
cargo test --locked -p astersql-server --lib pg_dml_test::pg10_ -- --nocapture --test-threads=1
make lint
git diff --check
```

Ready采用上述聚焦测试、格式化、lint及差异检查；`make lint`退出0，日志`/tmp/pg10-16-lint-delivery.log`。未验证高隔离、性能或完整PG兼容；测试中动作WHERE只消费既有开放表达式。未改变Go/Bazel或外部依赖。

共享层新增回归还验证本语句移动主键后，已暂存删除的旧键不再被原始已提交键存在性误判为冲突；此排除只适用于PG DO UPDATE候选。该回归先在第一次相邻运行失败（20 passed/1 failed，`/tmp/pg10-16-adjacent.log`），随后局部修复。

最终生产代码复验退出0：真实竞争1 passed/256场景（233.65秒）；共享层21 passed、相邻协议15 passed，退出0，日志`/tmp/pg10-16-adjacent2.log`。cargo fmt、构建、lint、diff检查均退出0。参考临时实例停止并删除数据；独占TiUP tag清理、PD端点不可达；槽位锁已释放。未出现阻塞本任务的无关回归失败。

## 写入 CTE 同语句源视图（任务22）

`execute_relational_delete_with_output` 在有界写入 CTE 期间优先从共享源行视图选取候选，兄弟 SELECT 读取同一行集；普通 DELETE、写入应用及错误恢复仍复用既有逻辑。
