# `pkg/session/runtime_test/statistics.rs`

## 文件定位

本文件是 `astersql-session` crate 的会话运行时测试子模块，而不是生产运行时实现。crate 根 `pkg/session/lib.rs` 仅在 `#[cfg(test)]` 下通过 `#[path = "runtime_test.rs"] mod runtime_test;` 装入测试聚合模块；`pkg/session/runtime_test.rs` 再以 `#[path = "runtime_test/statistics.rs"] mod statistics;` 装入本文件。因此它只进入 `cargo test` 的 lib-test 编译面，不导出公开 API，也不参与服务器正常运行。

文件存在的目的，是用一个最小端到端用例证明规范会话运行时能够初始化 Domain 和统计组件、bootstrap 出真实的 `mysql.stats_top_n` 系统表，并经普通 SQL 执行路径完成写入、查询和结果集消费。目标 crate 由 `pkg/session/Cargo.toml` 声明为 `astersql-session`，库入口为 `lib.rs`；本测试间接使用其中的 session runtime、mockstore、meta 系统表定义和统计初始化能力。

## 核心职责

本文件只有一个职责：`analyze_session_initializes_and_executes_against_mysql_stats_top_n` 验证 `CreateAnalyzeSession` 返回的规范测试会话不是基于预置 SQL 结果的桩，而能在事务型内存 mock KV 上访问 bootstrap 后的统计元数据表。

它覆盖四个相连的契约：`CreateAnalyzeSession` 成功建立 Domain/会话；`mysql.stats_top_n` 已按 bootstrap 定义存在；向表中写入一条包含二进制 `value` 的 Top-N 记录后，选择的数值列能按文本协议返回；单行被消费后再次调用 `Next` 必须返回 `None`。它不验证 ANALYZE 算法如何生成 Top-N、统计缓存刷新、二进制 `value` 的反序列化或多行排序，这些都超出本文件的断言范围。

## 主要符号

- `analyze_session_initializes_and_executes_against_mysql_stats_top_n()`（`pkg/session/runtime_test/statistics.rs:26`）：私有、无参数、无返回值的 `#[test]` 函数，也是本文件唯一自定义符号。RustCodeGraph 的精确查询只找到这一处定义；测试由 Rust 测试框架发现并调用，因此代码图没有普通函数调用者。
- `crate::runtime::CreateAnalyzeSession()`（`pkg/session/runtime/session.rs:2653`）：测试环境入口，创建带 wall-clock TSO 的内存 mock KV，设置零 schema/stats lease，初始化 `Domain`，调用 `BootstrapCanonicalDomain` 建立规范会话，然后尝试初始化统计组件。
- `ConcreteSession::execute(&self, sql)`（`pkg/session/runtime/dispatch.rs:4407`）：将 SQL 交给完整的多语句执行路径并返回 `Vec<ConcreteRecordSet>`；本用例分别用它执行 `INSERT` 和 `SELECT`。
- `TestRecordSet::Next` / `ConcreteRecordSet`（`pkg/session/testutil.rs:43`、`pkg/session/runtime/session.rs:1652`）：`use super::*` 从父测试模块带入 `TestRecordSet` trait，使 `ConcreteRecordSet::Next` 方法可用；实现最终从内部 `VecDeque<Vec<String>>` 逐行弹出，耗尽时返回 `Ok(None)`。
- 本文件没有模块级常量、类型、trait、`impl` 或条件编译项；条件编译边界位于父级 `pkg/session/lib.rs` 的 `#[cfg(test)]`。

## 执行流程

1. Rust 测试框架发现并调用 `analyze_session_initializes_and_executes_against_mysql_stats_top_n`。父模块导入的 `TestRecordSet` trait 为后续 `Next` 调用提供方法解析。
2. `CreateAnalyzeSession` 创建真实事务型内存 mock KV，并构造 `DomainConfig`：schema lease 与 stats lease 均为零；启用统计缓存内存配额配置时还会读取 `StatsCacheMemQuota`。随后 `Domain::init`、`BootstrapCanonicalDomain` 和 `domain.initialize_stats()` 依次建立 schema、系统表、会话和统计服务。
3. 测试通过 `ConcreteSession::execute` 执行一条 `INSERT`，向 `mysql.stats_top_n` 写入 `(table_id=874, is_index=0, hist_id=1, value=x'04000000000000042a', count=3)`。写入结果不参与断言；任何执行错误都会由 `expect` 立即终止测试。
4. 测试执行 `SELECT table_id,is_index,hist_id,count FROM mysql.stats_top_n`，从返回的结果集向量尾部取出一个 `ConcreteRecordSet`。若没有结果集，`pop().expect(...)` 失败。
5. 第一次 `Next` 必须得到一行四列字符串 `874`、`0`、`1`、`3`，证明数值字段已持久化并按会话结果协议转换为文本。测试刻意不查询 `value`，因此不能据此断言 blob 回读内容。
6. 第二次 `Next` 必须返回 `None`，证明当前结果集只有刚插入的一条记录且游标已经耗尽。

## 数据与状态

持久状态位于 `CreateAnalyzeSession` 创建的内存 mock KV 中，并由返回的 `Arc<Domain>` 与 `ConcreteSession` 共同关联。测试把 Domain 绑定为 `_domain`，虽不直接读取它，但该所有权会让 Domain 在会话使用期间保持存活；函数结束时二者自然析构，测试间不共享这份存储。

表结构的权威定义是 `pkg/meta/metadef/system_tables_def.rs` 中的 `CreateStatsTopNTable`：`table_id`、`hist_id` 为非空 `BIGINT`，`is_index` 为非空 `TINYINT`，`value` 为可空 `LONGBLOB`，`count` 为非空无符号 `BIGINT`，并有 `(table_id, is_index, hist_id)` 普通索引。`pkg/meta/metadef/system.rs` 的 `StatsTopNTableID` 为保留系统表 ID；`pkg/session/bootstrap.rs` 把该 ID、表名和建表 SQL列入 bootstrap 系统表清单。

查询结果在 `ConcreteRecordSet.rows: VecDeque<Vec<String>>` 中缓冲。本测试每次 `Next` 都会消耗队首：首调返回唯一一行，次调返回 `None`。没有显式事务状态、全局静态可变状态或随机输入；固定表 ID 和固定 SQL 使断言可重复。

## 依赖与调用关系

上游装配链为 `pkg/session/lib.rs` 的测试条件模块 → `pkg/session/runtime_test.rs::statistics` → 本文件的测试函数。RustCodeGraph 对测试函数的 `callers` 查询为空，符合 `#[test]` 由测试框架隐式调度的事实，而不表示测试不可达。

下游主链为测试函数 → `CreateAnalyzeSession` → `Domain::new` / `Domain::init` → `BootstrapCanonicalDomain` → `domain.initialize_stats()`，以及测试函数 → `ConcreteSession::execute` → 会话 SQL 分派 → 系统表 DML/查询 → `ConcreteRecordSet` → `TestRecordSet::Next`。RustCodeGraph 能解析 `CreateAnalyzeSession` 到 `BootstrapCanonicalDomain`、`session_error` 和日志器的边；对本测试函数本身未解析出 callees，因而上述测试体内的直接调用同时由源文件逐句核验。

crate 边界由 `pkg/session/Cargo.toml` 确认：包名为 `astersql-session`、库路径为 `lib.rs`；与该链直接相关的本地依赖包括 `astersql-domain`、`astersql-meta-metadef`、`astersql-statistics`、`astersql-statistics-handle`、`astersql-store-mockstore-mockstorage` 与会话上下文组件。测试没有直接引用外部网络服务或真实 TiKV。

## 错误处理与边界

本文件使用 `expect` 将每个失败点转化为带阶段说明的测试 panic：会话创建失败、插入失败、查询失败、结果集缺失、首行读取失败或耗尽检查失败都能从消息区分。`CreateAnalyzeSession` 内部则把 mock KV 创建、Domain 初始化和 bootstrap 错误包装为 `SessionError`；值得注意的是，`domain.initialize_stats()` 的错误仅记录日志并继续返回会话，因此本测试后续 SQL 是否成功也是对可用性的实际兜底验证。

断言边界很窄：没有 `ORDER BY`，但因为新存储只插入一行，顺序不构成不确定性；没有读取 `value`，所以十六进制 blob 只验证插入路径接受该字面量；没有显式关闭结果集，依赖局部变量析构；没有检查列元数据、影响行数、索引使用、重复键或无符号上界。若 bootstrap 意外预置 `stats_top_n` 数据，首行及耗尽断言会失败，而不是掩盖污染。

## 并发与资源生命周期

该测试是单线程顺序流程，本文件不创建线程、任务、锁、通道或后台工作。`ConcreteSession` 的测试抽象明确按单连接绑定且不保证并发安全；本用例只持有一个不可变绑定，并顺序调用 `execute`。

`Arc<Domain>` 保持 Domain 及其存储在测试期间存活；`ConcreteRecordSet` 将行缓存在 `VecDeque` 中，`Next` 逐行转移所有权。测试没有调用 `Close`，但函数返回时结果集被丢弃；因为这里的查询没有设置延迟 `store_read` 句柄，不涉及显式取消。零 schema/stats lease 避免等待租约推进，但 Domain 初始化可能建立的内部服务生命周期仍由 Domain 的析构负责，本文件不直接控制它们。

## 与 Go 版本的对应关系

Go 与 Rust 的系统表定义保持同构：`pkg/meta/metadef/system_tables_def.go::CreateStatsTopNTable` 与 Rust 常量声明相同的五列和联合索引；`pkg/session/bootstrap.go` 也用 `StatsTopNTableID`、`stats_top_n` 和该建表 SQL登记 bootstrap 表。因此本测试验证的系统表契约来自 Go 生产实现的直接移植边界。

最近的 Go 行为测试是 `pkg/session/test/session_test.go::TestRandomBinary`：它在 `NO_BACKSLASH_ESCAPES` 模式下向 `mysql.stats_top_n.value` 写入多组含特殊字符的二进制值，验证内部 SQL 写入不会破坏字节。Rust 的对应独立测试位于 `pkg/session/test/session_test.rs`，同样构造这些字节并调用 `ExecuteInternal`。本文件的用例更小：使用一个十六进制 blob，经公开的规范 `ConcreteSession::execute` 同时确认 bootstrap、普通插入、普通查询和结果集耗尽；它不是 `TestRandomBinary` 的等价替代，也没有覆盖 SQL mode/转义矩阵。

Go 没有与本函数同名的一对一测试。文档只能确认共享的表定义、bootstrap 清单和相邻二进制写入意图，不能宣称两个测试的覆盖面完全一致。

## 扩展指南

若扩展 `stats_top_n` 的会话级兼容性，优先在本文件新增独立 `#[test]`，不要把多种场景堆进现有函数；测试逻辑应继续与生产源码分离。涉及 blob 精确回读时，应把 `value` 加入查询并明确规定协议编码；涉及多行时必须增加 `ORDER BY`，避免依赖存储扫描顺序；涉及错误语义时应断言 `SessionError` 内容而非只用 `expect`。

若改变系统表 schema，需同步核对 `pkg/meta/metadef/system_tables_def.rs`、Go 对应文件、`pkg/session/bootstrap.rs` 与 `pkg/session/bootstrap.go`，并评估升级路径，而不应只修改测试。若改变测试会话初始化，接入点是 `pkg/session/runtime/session.rs::CreateAnalyzeSession`；若改变 SQL 分派或结果协议，分别检查 `pkg/session/runtime/dispatch.rs::ConcreteSession::execute` 与 `pkg/session/runtime/session.rs` 中 `ConcreteRecordSet` 的 `TestRecordSet` 实现。

相关独立测试至少包括 `pkg/session/runtime/statistics_test.rs`（ANALYZE/SHOW STATS 行为）和 `pkg/session/test/session_test.rs`（Go `TestRandomBinary` 对齐）。本文件自身是测试文件，不能再把测试内嵌进生产 `pkg/session/runtime/statistics.rs`；若新增共享测试辅助，应放在现有测试支持模块并保持生产路径无测试断言。兼容风险主要是 Go/Rust 系统表 schema 漂移和结果文本格式变化；性能风险主要来自把多行或真实统计加载场景无界加入该轻量 bootstrap 测试。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；精确 `query` 将唯一符号定位到 `pkg/session/runtime_test/statistics.rs:26`。对该测试函数的 `callers`/`callees` 未返回普通调用边；通过 `node CreateAnalyzeSession` 确认定义在 `pkg/session/runtime/session.rs:2653`，并确认其到 `BootstrapCanonicalDomain`、`session_error` 和日志器的图边。
- 已完整读取 `pkg/session/runtime_test/statistics.rs`，并核对模块入口 `pkg/session/lib.rs`、父测试模块 `pkg/session/runtime_test.rs`、crate 声明 `pkg/session/Cargo.toml`。
- 已核对直接运行时证据：`pkg/session/runtime/session.rs::CreateAnalyzeSession`、`ConcreteRecordSet` 及其 `TestRecordSet` 实现，`pkg/session/runtime/dispatch.rs::ConcreteSession::execute`，以及 `pkg/session/testutil.rs::TestRecordSet`。
- 已核对系统表与 Go 对照：`pkg/meta/metadef/system_tables_def.rs`、`pkg/meta/metadef/system_tables_def.go`、`pkg/meta/metadef/system.rs`、`pkg/session/bootstrap.rs`、`pkg/session/bootstrap.go`。
- 已核对相关独立测试：`pkg/session/runtime/statistics_test.rs`、`pkg/session/test/session_test.rs`，以及 Go 原始测试 `pkg/session/test/session_test.go::TestRandomBinary`。
- 本任务按计划是纯文档分析，未运行 Cargo 或代码测试；完成判据是上述源码/调用证据与固定十一章节的结构验证，不把未执行的运行时测试冒充验证结果。
