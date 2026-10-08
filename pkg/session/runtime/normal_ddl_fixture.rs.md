# `pkg/session/runtime/normal_ddl_fixture.rs` 逻辑说明

## 文件定位

`normal_ddl_fixture.rs` 属于 `astersql-session` crate（`pkg/session/Cargo.toml`）的 runtime 测试辅助模块。`pkg/session/runtime.rs:95-96` 以 `#[cfg(test)] mod normal_ddl_fixture;` 引入它，因此它不进入普通生产构建，也不是 normal DDL 服务的线上入口。文件的所有项均为 `pub(super)`，只向 `runtime` 父模块及其子模块中的独立测试提供共享夹具。

该文件虽以非 `*_test.rs` 命名，但实际职责是为 normal DDL 回归构造可执行的内存 Domain、系统会话池、基础表、Go 兼容的持久化元数据和 DDL Job。真实服务实现位于 `pkg/session/runtime/normal_ddl_service.rs`；本文件只是其测试环境的可复用搭建层。

## 核心职责

- `Fixture::new` 创建基于内存 KV 的完整 `Domain`，建立 `SystemSessionPool`，通过 SQL 创建 `test.normal_ddl_target`，并将数据库/表信息按 Go meta 键布局写入实际 MVCC Store。
- `Fixture::insert` 用 Rust 版 `build_alter_table_mode_job` 构造 `Normal -> Import` 的 `ActionAlterTableMode` Job，指定 ID/状态后按 Go 线上格式编码，再插入 `mysql.tidb_ddl_job`。
- `Fixture::queue` 查询指定队列记录，并用 `decode_go_history_job` 还原为 `Job`，便于测试观察持久状态。
- `Fixture::reader` 在当前存储版本上建立 `SnapshotReader`，供测试无修改地检查表、库、schema version 和历史 DDL Job。
- `Drop for Fixture` 统一关闭系统会话池和 Domain，避免测试后留下可借用会话或后台资源。
- `hex` 和 `hash` 分别服务于 SQL 中的二进制字面量，以及 TiDB `structure.Hash` 形式的 meta hash key 构造。

## 主要符号

- `pub(super) fn hex(b: &[u8]) -> String`：将每个字节转为两位小写十六进制，为 `X'...'` SQL 字面量生成无分隔符文本。
- `pub(super) fn hash(h: &[u8], f: &[u8]) -> astersql_kv::Key`：用 `EncodeBytes` 包装 `m`、用 `EncodeUint(..., 'h')` 加入 hash 类型标记，再编码 hash field，返回可直接写入 KV 的 meta key。
- `pub(super) struct Fixture`：持有 `Arc<Domain>`、`Arc<SystemSessionPool>` 及基准库表的 `db`/`table` ID。它不存储单独 transaction，每个操作按需借用会话或新建快照。
- `Fixture::new() -> Self`：夹具唯一构造器，所有失败都以 `unwrap` 转为测试 panic。
- `Fixture::insert(&self, id: i64, state: JobState)`：插入固定类型 75（Alter Table Mode）的非 reorg Job，可由调用者选择初始 `JobState`。
- `Fixture::queue(&self, id: i64) -> Option<Job>`：不存在队列行时返回 `None`；行存在但查询、字段或解码失败时 panic。
- `Fixture::reader(&self) -> astersql_meta::SnapshotReader`：先读取 `CurrentVersion("global")`，再获取该版本快照，返回按 Go meta 契约读取的只读器。
- `impl Drop for Fixture`：先 `pool.close()`，再 `domain.close()`；清理顺序与夹具的借用关系一致。

## 执行流程

1. 测试调用 `Fixture::new`。`CreateAnalyzeSession` 在 `pkg/session/runtime/session.rs:2653` 创建带 wall-clock TSO 的内存 mock KV，以零 schema/stats lease 初始化 Domain，执行 canonical bootstrap，并尝试初始化统计信息。
2. 夹具将全局系统变量 `tidb_cdc_write_source` 设为 `9`，并以同一 Domain 创建 `SystemSessionPool`。会话执行 `CREATE TABLE test.normal_ddl_target ...`，随后从 info schema 取得真实库表 ID。
3. 为了让后续 DDL worker 从持久化 meta 而非仅从 info schema 读到完整对象，构造 transaction，用 `hash("DB:<id>", "Table:<id>")` 写入 `EncodeTableInfo`，再用 `hash("DBs", "DB:<id>")` 写入 Public 状态的 `DBInfo`，最后一次提交。
4. 需要标准 AlterTableMode 队列项的测试调用 `insert`。它以 `cdc_write_source=41`、`sql_mode=7`和夹具的真实 ID 构造 Job/参数，断言该迁移不是 no-op，再设置测试 ID/状态、以 Go 格式编码并通过 SQL 插入 DDL 队列。
5. 测试用 `queue` 读回当前队列状态，或用 `reader` 检查 MVCC 快照中的 meta/历史 Job。`normal_ddl_test.rs` 中的服务回归还会将此 pool 交给 `NormalDdlService`，等待队列项转入 history。
6. 离开作用域时 `Drop` 关闭 pool 与 Domain。如测试提前显式关闭它们，后续 Drop 依赖对应 `close` 的可重复调用性。

## 数据与状态

`Fixture` 的长寿命状态是 Domain、系统会话池和两个对象 ID。基准表固定为 `test.normal_ddl_target(id int primary key, payload varchar(40))`，但各测试可继续通过 pool 执行 SQL 或直接修改 meta，因此 `db`/`table` 是跨这两种访问路径的稳定键。

`new` 刻意维护两个视图：SQL/Domain info schema 中的表，以及 MVCC Store 中按 Go meta 布局编码的 DB/Table 记录。注释中的“non-empty, complete Go table metadata”对应后者；这是 `SnapshotReader` 和持久化 DDL action 能够读到完整 schema 的前提。

`insert` 生成的 Job 是 version-2 AlterTableMode 载荷：对象从 `Normal` 转向 `Import`，Job 的 `cdc_write_source`/`sql_mode` 是 41/7，而 Domain 全局 `tidb_cdc_write_source=9` 用于另一条 SQL 提交路径的测试对照；两者不应混为同一个来源。队列行的 `reorg=0`、`type=75`、`processing=0`，`schema_ids`/`table_ids` 使用夹具 ID。

## 依赖与调用关系

文件的直接上游是九个独立 Rust 测试模块：`normal_ddl_test.rs`、`normal_ddl_create_table_test.rs`、`normal_ddl_create_materialized_view_test.rs`、`normal_ddl_create_materialized_view_log_test.rs`、`normal_ddl_create_materialized_view_shadow_test.rs`、`normal_ddl_drop_materialized_view_test.rs`、`normal_ddl_materialized_view_partition_test.rs`、`normal_ddl_index_reorg_initialization_test.rs` 和 `normal_ddl_masking_policy_test.rs`。它们使用 `Fixture`、`hex` 或 `hash`，没有生产调用者。

直接下游包括：`CreateAnalyzeSession`（创建 mock storage/Domain 并 bootstrap）；`SystemSessionPool::new/acquire/close`（借用可执行系统会话）；`Domain` 的全局变量、info schema、table lookup 和 storage handle；`astersql_ddl_jobsubmit` 的 AlterTableMode Job 构造/参数编码；`astersql_meta_model` 的 Job/DB/Table 编解码；`astersql_meta::SnapshotReader` 与 Go Job 解码；以及 `astersql_util_codec`/`astersql_kv` 的 meta key 和 transaction API。

`pkg/session/Cargo.toml` 将该模块归入 `astersql-session`，并显式声明 `astersql-domain`、`astersql-ddl-jobsubmit`、`astersql-kv`、`astersql-meta`、`astersql-meta-model`、`astersql-util-codec` 和 mockstore 依赖。文件本身没有 feature 分支；唯一编译边界是父模块的 `#[cfg(test)]`。

## 错误处理与边界

这是测试夹具，所以对环境搭建、SQL、KV transaction、元数据编解码和会话池错误统一使用 `unwrap`，以 panic 立即中止当前测试。这是夹具契约，不应复制到生产路径。`insert` 还以 `assert!(!noop)` 锁定 `Normal -> Import` 必须创建 Job；若 TableMode 迁移规则改变，夹具会显式失败。

`queue` 只将“没有行”解释为 `None`；它不区分其他可恢复错误，也假定第一列是有效 `job_meta`。SQL 由整数 ID 和夹具内部产生的十六进制数据拼接，不接受外部字符串；若将其泛化为可变名称/载荷，应改用参数绑定或明确的 SQL 转义。

`reader` 是调用时快照，不会随后续提交自动前进；要观察新状态应重新调用 `reader()`。`hash` 只实现 meta hash 键的特定编码，不是通用哈希函数，更不提供密码学保证。

## 并发与资源生命周期

本文件不直接创建线程、async 任务或 channel。并发与后台生命周期来自 Domain、normal DDL service 及 `SystemSessionPool`。`SystemSessionPool::new` 内部使用容量为 5 的 advanced session pool；该容量限制 idle 资源，而非并发借用者数。`acquire` 返回 lease，lease 离开作用域后由 pool 回收。

`Fixture` 以 `Arc` 共享 Domain 和 pool，因此测试可将弱引用、pool clone 或 Domain clone 交给服务/线程。但 `Drop` 只会发出 `close`，不能强制立即释放还被其他 `Arc` 持有的对象；扩展并发测试时必须在 Fixture 析构前停止 worker、释放 lease 和额外 clone。清理先关 pool 后关 Domain，避免新借用发生在 Domain 关闭之后。

## 与 Go 版本的对应关系

仓库中没有同路径、同名的 Go `normal_ddl_fixture.go`，所以本文件不是单一 Go 源文件的逐函数翻译，而是将 Go 测试基础设施与持久化契约组合成 Rust 共享夹具。`pkg/meta/meta.go` 定义 `mMetaPrefix="m"`、hash data type 和 `DBs -> DB:<id> -> Table:<id>` 布局，是 `hash`、DBInfo/TableInfo 直接写入的 Go 语义依据。

`Fixture::insert` 的直接行为对照是 `pkg/ddl/jobsubmit/table_mode.go::BuildAlterTableModeJob`：两版都验证模式迁移，相同模式返回 no-op，否则生成 version 2、`ActionAlterTableMode`、query `"skip"`、含 HistoryInfo、CDC source/SQL mode 和 involving-schema 信息的 Job。`pkg/ddl/jobsubmit/table_mode_test.go` 覆盖正常构造、no-op 和 Import-to-Restore 非法迁移；Rust 夹具固定选用必定非 no-op 的 Normal-to-Import。

`Job::encode(...)/decode_go_history_job`、`EncodeTableInfo` 和 `EncodeDBInfo` 明确要求 Go 兼容 wire/meta 格式。因此扩展夹具不应为了方便而写入 Rust 私有结构序列化，否则 normal DDL 回归会绕过真实兼容边界。

## 扩展指南

新增通用的 normal DDL 测试前置时，应优先扩展 `Fixture` 的小粒度方法，并保持源文件中不内嵌 `#[test]`；具体回归应放在上述同目录独立 `*_test.rs` 中，再由 `runtime.rs` 的 `#[cfg(test)] mod ...` 接入。只对某个 action 有意义的构造器应留在对应测试文件，不应把 Fixture 扩张为无边界的 Job 工厂。

如需支持其他 Job 类型或可变表结构，必须同时核对：Go jobsubmit/model 载荷，`mysql.tidb_ddl_job` 的 type/reorg/schema_ids/table_ids/processing 字段，Go 兼容 Job 编解码，以及持久化 meta key 布局。修改 `hash` 时需以 `pkg/meta/meta.go`/structure codec 为契约，否则 SnapshotReader 将读不到夹具写入的数据。

扩展资源或并发场景时，应为所有 worker 增加确定性停止/等待，确保 lease 先于 Fixture 释放，并避免依赖 sleep 作为唯一同步手段。若改变快照观察行为，新增回归应明确验证“旧 reader 保持旧版本，重新 `reader()` 看到新提交”。

## 验证依据

本说明直接核对了目标源 `pkg/session/runtime/normal_ddl_fixture.rs`、模块接入 `pkg/session/runtime.rs`、crate 声明 `pkg/session/Cargo.toml`、Domain 创建入口 `pkg/session/runtime/session.rs::CreateAnalyzeSession`、会话池生命周期 `pkg/session/runtime/system_session.rs::SystemSessionPool`，以及九个直接引用夹具的独立 `normal_ddl*_test.rs` 文件。`pkg/session` 下未找到 `doc.go`。

Go 语义证据来自 `pkg/meta/meta.go`、`pkg/ddl/jobsubmit/table_mode.go` 和 `pkg/ddl/jobsubmit/table_mode_test.go`；Rust 构造器对照为 `pkg/ddl/jobsubmit/table_mode.rs`。`normal_ddl_test.rs` 中的代表用例证明 `Fixture::insert/queue/reader` 被用于观察 queue-to-history、表模式、schema version、失败保留与服务关闭；其他八个测试模块复用相同基础设施覆盖创建表、物化视图、分区、index reorg 初始化和 masking policy。

RustCodeGraph `status` 显示当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/normal_ddl_fixture.rs` 确认目标已索引；`node --file ... --offset 1 --limit 500` 读取了完整 132 行；`node/callees CreateAnalyzeSession` 确认其下游为 mock storage、Domain bootstrap 和 stats 初始化。图对常见名 `Fixture`/`hex`/`hash` 无法稳定消歧义，因此调用者列表以 `rg` 的精确模块引用与测试源码交叉核实，不将空图结果解释为“无调用者”。本任务是纯文档分析，按计划不运行 Cargo。
