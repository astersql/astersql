# `pkg/session/runtime_test/query.rs`

## 文件定位

该文件是 `astersql-session` crate 的查询与写入端到端回归测试分组，而不是生产运行时模块。`pkg/session/lib.rs` 仅在 `#[cfg(test)]` 下装配 `runtime_test.rs`，后者再以 `#[path = "runtime_test/query.rs"] mod query;` 引入本文件。因此这里的 12 个私有 `#[test]` 函数只在测试构建中执行，不导出 API，也不进入服务器正常请求路径。

测试统一从 `crate::runtime::CreateAnalyzeSession` 建立带内存存储和运行时 domain 的 `ConcreteSession`，通过 `ConcreteSession::execute` 走真实的 SQL 解析、计划、查询/DML 执行及结果集读取路径。文件顶部已经明确将范围限定为表达式类型语义、`information_schema` 客户端兼容结果，以及 `INSERT SELECT` 的冲突、原子性、时间戳复用和外连接行为。

这一本质上的测试文件与总计划所述“排除测试文件”规则存在分类张力；本文按编号任务要求记录现有事实，不把它描述为生产候选或线上入口。

## 核心职责

文件通过可观察 SQL 结果或测试钩子固定以下兼容契约：

- `cte_string_projection_uses_string_comparison_semantics` 与 `mixed_string_integer_comparison_uses_double_semantics` 固定字符串投影、数值强制转换和混合字符串/整数比较的 MySQL/TiDB 语义，尤其覆盖超过 IEEE-754 精确整数范围的值。
- `concrete_information_schema_answers_connector_j_metadata_queries` 与 `concrete_information_schema_exposes_check_constraints` 固定 JDBC 风格表、列、主键、例程、参数元数据以及标准/扩展 CHECK 约束视图的行形态。
- 两个投影类 `INSERT SELECT` 测试固定派生表、`UNION ALL`、笛卡尔积、过滤、排序，以及字符串、数值、日期函数组合后的落表值。
- `multi_row_insert_reuses_one_conflict_snapshot_timestamp` 固定一条多行 autocommit INSERT 只使用一个开始时间戳和一个提交时间戳。
- `bulk_load_primary_key_skip_requires_an_exact_plain_insert_table` 与 `plain_primary_key_insert_skips_existing_row_scan` 直接检查主键冲突扫描优化的启用条件。
- `insert_select_reuses_insert_conflict_and_atomicity_semantics` 固定重复键失败的语句级原子性，以及 `IGNORE`、`ON DUPLICATE KEY UPDATE` 的处理结果。
- `insert_select_outer_joins_fall_back_from_streaming` 固定左右外连接未匹配侧的 NULL 扩展值能够安全写入。
- `nested_window_and_scalar_subquery_use_window_evaluation` 固定窗口函数嵌套标量子查询时仍按窗口分区求值。

## 主要符号

本文件没有常量、类型、trait、impl、公开函数或条件编译项；模块自身的条件编译位于 `pkg/session/lib.rs`。全部符号均为零参数、无返回值的私有测试函数：

| 符号 | 断言重点 |
| --- | --- |
| `cte_string_projection_uses_string_comparison_semantics` | CTE 的字符串投影与空字符串按字符串语义比较，同时用 `0 = ''` 对照数值强制转换。 |
| `mixed_string_integer_comparison_uses_double_semantics` | 字符串与整数混合比较转为 DOUBLE，两个相邻大整数因精度舍入比较相等。 |
| `concrete_information_schema_answers_connector_j_metadata_queries` | 表类型、JDBC 类型码、类型名、序号、可空性、自增标记、主键和空例程/参数结果。 |
| `concrete_information_schema_exposes_check_constraints` | `CHECK_CONSTRAINTS` 和 `TIDB_CHECK_CONSTRAINTS` 暴露一致的约束身份与表达式，扩展视图还带表名和数值表 ID。 |
| `insert_select_executes_derived_union_cross_join_filter_and_order_expressions` | 派生表、`UNION ALL`、两次 cross join、过滤表达式和最终排序产生准确行集。 |
| `insert_select_executes_order_loader_projection` | `concat`、`lpad`、`mod`、decimal cast、`elt`、`timestampadd` 的组合投影精确落表。 |
| `multi_row_insert_reuses_one_conflict_snapshot_timestamp` | 读取 `TSORequestCountForTest` 前后差值，要求单语句恰为 2。 |
| `bulk_load_primary_key_skip_requires_an_exact_plain_insert_table` | 仅配置库表名匹配且无需读取既有行时，允许跳过已提交主键检查。 |
| `insert_select_reuses_insert_conflict_and_atomicity_semantics` | 普通重复键失败不留下前缀行；随后 `IGNORE` 插入新行，upsert 更新冲突行。 |
| `insert_select_outer_joins_fall_back_from_streaming` | LEFT/RIGHT JOIN 的匹配和未匹配行落表，SQL NULL 由测试结果集编码为 `"<nil>"`。 |
| `plain_primary_key_insert_skips_existing_row_scan` | `replace`、`ignore`、upsert、二级唯一索引任一存在都要求加载既有行；四项全否时可省略。 |
| `nested_window_and_scalar_subquery_use_window_evaluation` | `count(*) over (partition by ...)` 在 `coalesce`/标量子查询组合中输出 `2, 2, 1`。 |

## 执行流程

典型测试按“建立隔离环境—执行 SQL—消费结果—断言行为”推进：

1. `CreateAnalyzeSession`（`pkg/session/runtime/session.rs`）创建 domain 与 `ConcreteSession`；每个测试持有自己的 session，因此测试间的数据库名和数据互不依赖。
2. 测试调用 `ConcreteSession::execute`（`pkg/session/runtime/dispatch.rs`），依次创建数据库/表、切换当前数据库、写入种子数据并执行待验证语句。
3. 查询语句返回 `Vec<ConcreteRecordSet>`；测试取第一个结果集并反复调用 `Next`。固定行数的测试同时断言末尾为 `None`，防止意外多行。
4. 写入语句进入 `execute_relational_insert`（`pkg/session/runtime/dml.rs`）。`INSERT SELECT` 的来源行由 `execute_insert_select_join`（`pkg/session/runtime/source.rs`）和查询表达式求值逻辑生成，再复用普通 INSERT 的冲突与提交路径。
5. 失败路径通过 `.err()` 捕获错误并检查 `Duplicate entry`；随后重新查询目标表，验证原有行保留且失败语句没有写入任何前缀行。

两个辅助条件测试不执行 SQL，而是直接调用 `ConcreteSession::relational_insert_requires_existing_rows` 和 `ConcreteSession::bulk_load_skips_committed_primary_key_check`，将优化条件穷举为稳定的布尔契约。

## 数据与状态

测试状态主要位于每个 `ConcreteSession` 及其对应的内存 mock storage/domain 中。数据库和表均在测试内创建；没有跨测试共享的模块级可变变量。

- 结果集行在本测试边界表现为 `Vec<String>`；数值、decimal、datetime 和 SQL NULL 分别断言为规范字符串（例如 `97.00`、毫秒时间文本和 `"<nil>"`）。这验证的是 session 适配器对外可观察的编码，而不只是内部 Datum。
- 元数据测试依赖 DDL 后 domain/infoschema 可立即观察到表、列、索引及 CHECK 约束；`TABLE_ID` 只约束为可解析的 `i64`，避免依赖不稳定的具体分配值。
- 多行 INSERT 测试读取同一 storage 的累计 TSO 请求计数，仅比较语句前后差值；不依赖此前 bootstrap 消耗的绝对计数。
- DML 原子性测试把目标表已有 `(2,20)` 与来源 `(1,10),(2,22)` 分开建模，使第二行冲突发生前的第一行成为“不得残留的前缀”。
- `relational_insert_requires_existing_rows` 的四个布尔输入依次表示 REPLACE、IGNORE、存在 ON DUPLICATE 子句、存在二级唯一索引。`bulk_load_skips_committed_primary_key_check` 还要求 `ASTERSQL_BULK_LOAD_ASSUME_ABSENT_TABLE` 所表达的 `database.table` 与实际目标大小写不敏感地完全匹配。

## 依赖与调用关系

上游装配链为 `pkg/session/lib.rs` 的测试模块 → `pkg/session/runtime_test.rs` → 本文件。RustCodeGraph 将 12 个测试函数均定位在本文件，未发现生产调用者；测试框架通过 `#[test]` 注册并调用它们。

本文件使用 `use super::*` 继承 `runtime_test.rs` 导入的 `ConcreteSession` 等测试上下文，并直接调用以下下游边界：

- `crate::runtime::CreateAnalyzeSession`：创建规范分析/执行 session。
- `ConcreteSession::execute`：SQL 入口；生产定义位于 `pkg/session/runtime/dispatch.rs`。
- `ConcreteRecordSet::Next`：逐行读取并显式检查耗尽状态。
- `session.domain().storage().with_storage(...)` 与 `TSORequestCountForTest`：只用于时间戳请求计数的测试观测。
- `ConcreteSession::relational_insert_requires_existing_rows`、`bulk_load_skips_committed_primary_key_check`：位于 `pkg/session/runtime/dml.rs` 的 `pub(crate)` 优化判定函数。

`pkg/session/Cargo.toml` 把该 crate 定义为 `astersql-session`，库入口为 `lib.rs`，移植元数据指向 Go 包 `pkg/session`。本文件本身没有直接引用第三方 crate；其能力经父模块与 session 运行时传递，实际覆盖 parser、planner、expression、infoschema、KV/mockstore 等 crate 边界。唯一 feature `nextgen` 没有在本文件中产生条件分支。

## 错误处理与边界

测试准备和成功路径普遍用 `expect`，其消息指出失败阶段；这符合回归测试需要快速定位步骤的目的，但不是生产错误恢复策略。结果集读取也检查 `SessionResult`，并在预期行之后断言 `None`。

明确覆盖的错误边界是重复主键：`insert_select_reuses_insert_conflict_and_atomicity_semantics` 要求错误文本包含 `Duplicate entry`，并用后续查询证明整个语句回滚。该断言刻意不绑定完整错误文本或错误类型，但会对用户可见关键词变化敏感。

元数据空集合通过 `routines`、`parameters` 查询返回成功且首个 `Next` 为 `None`；CHECK 表达式先去除空白再检查关键片段，以容忍格式化差异。外连接测试只验证等值连接下的 NULL 扩展；窗口测试只验证给定分区及单行标量子查询。网络故障、真实 TiKV、显式事务隔离、并发写冲突和多结果集均不在本文件覆盖范围内。

## 并发与资源生命周期

每个测试同步执行，没有显式线程、异步任务、锁或通道。`CreateAnalyzeSession` 返回的 domain/session 由局部变量持有，测试函数结束时按 Rust 所有权释放；record set 同样在局部作用域内消费。本文件没有显式 `Close` 调用，生命周期依赖具体记录集和 session 的析构实现。

事务方面，`execute_relational_insert` 在无现存事务且非悲观事务时创建语句事务，使同一 INSERT 的冲突探测与写入共享快照；写入最终经 autocommit 应用/提交路径完成。时间戳计数测试把此资源生命周期固定为一次 Begin 获取开始时间戳、一次 Commit 获取提交时间戳。失败原子性测试则从最终表状态验证临时语句修改没有部分提交。

这些测试没有并发调度保证，也不能证明真实 PD/TiKV 的资源关闭行为；它们验证的是内存 mock storage 上的 session 级生命周期和可观察事务边界。

## 与 Go 版本的对应关系

`pkg/session/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/session"` 表明 crate 的整体 Go 对照包，但仓库中没有与 `runtime_test/query.rs` 同路径或同名的一一对应 Go 文件，若称整文件为机械翻译并无证据。

可以确认的语义对照包括：

- `pkg/planner/core/casetest/join/join_test.go` 的 issue 67731 用例同样断言字符串 `9007199254740993` 与整数 `9007199254740992` 比较为真；这直接支持混合类型转 DOUBLE 的兼容预期。`pkg/expression/builtin_compare_test.go` 也覆盖相邻大整数与字符串/整数组合的精度行为。
- Go 的 `pkg/infoschema/tables.go` 定义 `CHECK_CONSTRAINTS` 与 `TIDB_CHECK_CONSTRAINTS`，`pkg/executor/infoschema_reader.go` 提供两类视图的数据读取实现；Rust 测试从 SQL 结果侧固定其兼容形态。
- Go 的 `pkg/executor/insert_test.go`、`pkg/session/test/txn/txn_test.go` 和 `pkg/executor/test/writetest/mview_log_write_test.go` 覆盖 `INSERT SELECT`、`INSERT IGNORE SELECT` 与 `ON DUPLICATE KEY UPDATE` 等行为，但没有发现与本文件每个 SQL 场景完全相同的单一测试。
- 外连接、窗口/子查询在 Go planner/executor 测试中有广泛覆盖；本文件的价值是将这些能力组合到 Rust `ConcreteSession` 的真实 SQL 执行链中，而不是替代相应的 planner/executor 单元测试。

因此扩展时应优先保持用户可见 SQL 结果与 Go TiDB 一致；若新增断言声称来自 Go，应记录准确的 Go 文件和测试符号，而不是仅引用 `pkg/session` 包级映射。

## 扩展指南

- 新增同类端到端查询/DML回归时，应继续放在独立测试文件（本文件或按主题拆出的同目录 `*_test` 模块），不要把测试逻辑嵌入 `runtime/*.rs` 生产文件；同时在 `runtime_test.rs` 通过测试模块装配。
- 表达式问题应先明确输入 SQL 类型和预期 coercion，再断言最终行；涉及格式化时避免绑定无关空白。对应生产入口通常是 `pkg/session/runtime/query.rs` 的表达式求值以及 expression/planner crate。
- `information_schema` 扩展需同时核对标准列、TiDB 扩展列、空集合和稳定排序；生产修改可能落在 infoschema/executor 层，session 测试只承担端到端契约。
- `INSERT SELECT` 扩展应至少同步检查普通成功、冲突失败原子性、IGNORE、upsert、NULL、外连接及行顺序是否只在显式 `ORDER BY` 下断言。关键生产位置是 `execute_relational_insert`、`execute_insert_select_join` 和 `relational_query_expression_value`。
- 修改冲突扫描优化时，必须同步更新两个布尔条件测试；任何新冲突语义或唯一性来源默认应令 `relational_insert_requires_existing_rows` 返回 true，除非能证明逐键检查仍完整。
- 修改事务创建/提交时，应保留 TSO 差值断言并关注真实 TiKV 回归；mock 计数只能证明当前内存存储调用次数，不能替代分布式事务验证。
- 性能风险主要来自把普通主键 INSERT 退化为全表既有行扫描，或把可流式的 SELECT 全量物化；正确性风险主要是类型强制转换漂移、NULL 扩展丢失、冲突语义分叉和失败后部分写入。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标文件已被索引；`files --filter pkg/session/runtime_test/query.rs` 返回该文件。
- RustCodeGraph `node --file pkg/session/runtime_test/query.rs --offset 1/500`：读取全部 652 行；逐一 `query --kind function` 定位 12 个测试函数。图查询还定位 `bulk_load_skips_committed_primary_key_check` 于 `pkg/session/runtime/dml.rs:2106`、`relational_insert_requires_existing_rows` 于同文件 `:2097`。测试函数无生产调用边；其真实上游是 Rust 测试框架与模块装配。
- 已读 Rust/Cargo 路径：`pkg/session/runtime_test/query.rs`、`pkg/session/runtime_test.rs`、`pkg/session/lib.rs`、`pkg/session/Cargo.toml`，以及直接生产证据 `pkg/session/runtime/dml.rs`、`pkg/session/runtime/source.rs`、`pkg/session/runtime/query.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/session.rs`。`pkg/session` 下未发现 `doc.go`。
- 已核对 Go 路径：`pkg/planner/core/casetest/join/join_test.go`、`pkg/expression/builtin_compare_test.go`、`pkg/infoschema/tables.go`、`pkg/executor/infoschema_reader.go`、`pkg/executor/insert_test.go`、`pkg/session/test/txn/txn_test.go`、`pkg/executor/test/writetest/mview_log_write_test.go`。未找到本文件的同路径 Go 对照。
- 相关独立 Rust 测试搜索包括 `pkg/session/mysql_dml_compat_test.rs`、`pkg/session/dml_runtime_test.rs`、`pkg/planner/core/casetest/join/join_test.rs` 和 `pkg/planner/core/casetest/windows/window_with_exist_subquery_test.rs`；这些提供相邻能力覆盖，但本文件 12 个函数才是本文描述场景的直接回归证据。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核未将测试文件误写为生产入口。
