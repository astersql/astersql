# `pkg/ddl/testutil/testutil.rs`

## 文件定位

本文件属于 `astersql-ddl-testutil` crate，由同目录 `lib.rs` 以 `pub mod testutil` 暴露，并经根门面 `pkg/lib.rs` 的 `ddl::testutil` 再导出 crate。它不是 DDL owner、job scheduler 或 schema 状态机的生产实现，而是面向 DDL 测试的轻量适配层：用本地数据摘要和两个 trait 隔离会话执行、元数据读取、表模式变更及 `RefreshMeta` 操作。

`pkg/ddl/testutil/Cargo.toml` 将 Go 包 `pkg/ddl/testutil` 记录为移植来源，并声明 DDL、domain、session、meta、table 等依赖。不过当前文件本身除 `std::sync::mpsc::Sender` 外没有直接引用这些 crate；仓库搜索也未发现这些 Rust API 的外部调用。这说明 crate 边界和接口已建立，但本文件的具体 AsterSQL runtime 适配及 Rust 回归接线尚未落地，不能把 Go 测试覆盖视为 Rust 测试覆盖。

## 核心职责

- `SessionExecRuntime`、`SessionExecInGoroutine` 与 `ExecMultiSQLInGoroutine` 抽象并发 SQL 执行，并规定测试用 DDL SQL 不应返回结果集。
- `DDLTestRuntime` 抽象表句柄提取、索引查找、事务式表信息读取、表模式变更和元数据刷新。
- `SchemaState`、`TableMode`、`IndexInfo`、`TableInfo`、`DBInfo`、`SubJob`、`Job` 与 `RefreshMetaArgs` 提供独立于生产模型的最小数据表示。
- `MatchCancelState` 支持单作业及 multi-schema 子作业取消点匹配。
- `checkTableState`、`CheckTableMode`、`SetTableMode`、`GetTableInfoByTxn`、`RefreshMeta` 组合或转发 runtime 操作，统一测试调用形状。

这些职责均服务测试编排，不负责持久化 DDL job、推进 schema state、等待集群 schema version 同步或执行 reorg/backfill。

## 主要符号

- `SchemaState::{None, Public, Other(i32)}`：最小 schema 状态模型。`Other` 保存未显式建模的状态编码。
- `TableMode::{Normal, Import}`：最小表模式模型。与 Go `model.TableMode` 相比未表示 `Restore` 等其他模式。
- `IndexInfo`、`TableInfo`、`DBInfo`：只保留本文件断言所需字段；其中 `TableInfo` 带 `id/name/state/mode/indexes`。
- `SubJob`、`Job`、`SubStates`、`CancelState`：描述普通 job 或多个 sub-job 的取消状态期望。`Job::multi_schema_change` 与 `Job::sub_jobs` 由调用方负责保持一致，本文件没有构造器强制该不变量。
- `RefreshMetaArgs`：由 schema/table ID 和涉及的库表名组成，交给 runtime 刷新元数据。
- `SessionExecRuntime`：要求实现者可 `Clone + Send + 'static`，其错误也必须可跨线程发送；`execute` 用 `bool` 表示是否产生结果集，`record_set_error` 构造相应错误。
- `DDLTestRuntime`：以关联错误类型统一五类测试操作；没有 `Send`、`Sync` 或生命周期要求，因为这些辅助函数均在调用线程同步执行。
- 公开辅助函数：`SessionExecInGoroutine`、`ExecMultiSQLInGoroutine`、`ExtractAllTableHandles`、`FindIdxInfo`、`MatchCancelState`、`CheckTableMode`、`SetTableMode`、`GetTableInfoByTxn`、`RefreshMeta`。`checkTableState` 是模块私有组合步骤。

## 执行流程

1. `SessionExecInGoroutine` 把单条 SQL 包成单元素 `Vec`，转交 `ExecMultiSQLInGoroutine`。
2. `ExecMultiSQLInGoroutine` 移动 runtime、database、SQL 列表和 sender 到新线程，逐条调用 `SessionExecRuntime::execute`。`Ok(false)` 转为成功；`Ok(true)` 转为 `record_set_error()`；底层错误原样传递。每条语句向 `done` 发送一次结果，发送失败或该条执行失败后立即退出，否则继续下一条。
3. `ExtractAllTableHandles` 与 `FindIdxInfo` 是纯委托，分别返回 runtime 提供的物理句柄列表和可选索引摘要。
4. `MatchCancelState` 对 `Single` 要求 job 不是 multi-schema change 且顶层状态相等；对 `SubJobs` 要求 `sub_jobs` 存在、长度相同并逐项状态相等。
5. `SetTableMode` 先调用 `alter_table_mode`；只有成功后才重新读取表信息，依次确认表为 `Public` 且 mode 等于目标值，最终返回两项检查的逻辑与。
6. `GetTableInfoByTxn` 直接委托 `table_info`；事务语义必须由 runtime 实现。`RefreshMeta` 先构造拥有字符串的 `RefreshMetaArgs`，再委托 `refresh_meta`。

## 数据与状态

本文件没有全局变量、缓存或静态可变状态。所有测试状态由值对象传入，`Clone`/`Copy` 派生只用于便捷传递，不隐含共享一致性。

`checkTableState` 总会先读取实际表信息；期望为 `SchemaState::None` 时会忽略返回对象的名称和状态并视为匹配，但读取失败仍向上传播。期望为其他状态时同时要求表名等于输入快照中的名称、状态等于期望值。`CheckTableMode` 只比较 mode，不验证名称或 schema state。

`SetTableMode` 返回 `Ok(false)` 表示变更调用成功但后置状态检查不匹配，返回 `Err` 表示变更或任一读取失败。它不修改传入的 `TableInfo` 快照，也不校验 `indexes` 字段。

## 依赖与调用关系

模块入口链为 `pkg/ddl/testutil/lib.rs -> pub mod testutil -> testutil.rs`；工作区根 `Cargo.toml` 以 `facade_ddl_testutil` 引用该 crate，`pkg/lib.rs` 再将其暴露在 `ddl::testutil` 下。多个 DDL 测试 crate 在各自 `Cargo.toml` 中声明 `astersql-ddl-testutil`，但精确 Rust 搜索只发现 crate/module 装配，未发现本文件函数或 trait 的调用者。

文件内部的主要调用边为：

- `SessionExecInGoroutine -> ExecMultiSQLInGoroutine -> SessionExecRuntime::execute / record_set_error -> Sender::send`。
- `ExtractAllTableHandles -> DDLTestRuntime::extract_table_handles`，`FindIdxInfo -> DDLTestRuntime::find_index`。
- `SetTableMode -> DDLTestRuntime::alter_table_mode -> checkTableState / CheckTableMode -> DDLTestRuntime::table_info`。
- `GetTableInfoByTxn -> DDLTestRuntime::table_info`，`RefreshMeta -> DDLTestRuntime::refresh_meta`。

因此与 session、meta transaction、DDL executor 和 infoschema 的真实连接均在未来的 trait 实现者中，而不在本文件。`Cargo.toml` 的广泛依赖体现目标 crate 边界，不能作为这些依赖已被当前实现使用的证据。

## 错误处理与边界

并发 SQL 路径不包装底层 `execute` 错误；结果集出现时由 runtime 产生专用错误。失败结果会尝试发送一次，然后线程退出。若 receiver 已关闭，`send` 失败并静默结束；若 channel 无容量且接收方不消费，线程可能阻塞。线程创建使用 `std::thread::spawn`，没有返回 `JoinHandle`，调用方无法在本 API 上 join 或捕获 panic。

`DDLTestRuntime` 路径主要通过 `Result` 原样传播错误。`FindIdxInfo` 只有 `Option`，无法区分“表不存在”“索引不存在”和 runtime 内部查找故障。`MatchCancelState` 只返回布尔值：`_sql` 参数未使用，也不会像 Go 版本一样向测试框架报告类型误用、SQL 上下文或长度不等诊断。其 `SubJobs` 分支只检查 `sub_jobs` 是否存在，不额外要求 `multi_schema_change == true`；不一致的 `Job` 值可能仍匹配。

`SchemaState::Other(i32)` 和只有两个变体的 `TableMode` 都允许该测试模型与生产枚举发生语义漂移；跨边界适配时必须显式映射未知状态，并为不支持的表模式返回错误，不能默默降级。

## 并发与资源生命周期

每次 `ExecMultiSQLInGoroutine` 调用都会创建一个 detached OS 线程。runtime、SQL 字符串列表和 sender 被移动进线程，保证其生命周期至少覆盖执行过程；`R: Clone` 仅是 trait 契约，本函数本身不克隆 runtime。语句在单个线程内严格串行，但多个辅助调用之间没有排序保证。

本文件不创建或关闭 session、transaction、record set，也不持有锁。具体资源创建、数据库选择、事务边界和关闭行为全部属于 `SessionExecRuntime`/`DDLTestRuntime` 实现者的责任。`Sender` 在工作线程结束时随捕获环境释放；API 没有取消令牌、超时或 join 机制。同步的表模式和元数据辅助也没有内部并发保护，若 runtime 被多个任务共享，应由实现者提供互斥和一致性语义。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/testutil/testutil.go`。Rust 保留了九个同名辅助函数及 `SubStates` 概念，但用 trait 和值摘要替代 Go 的具体 `kv.Storage`、`sessionapi.Session`、`domain.Domain`、`ddl.Executor`、`meta.Mutator` 与 `model.*` 类型。

- Go 的 `ExecMultiSQLInGoroutine` 会创建并关闭测试 session、执行 `USE <db>`，再逐条执行 SQL；Rust 只把 database 参数交给 runtime，session 生命周期和选库语义未在本文件实现。
- Go 的 `ExtractAllTableHandles` 通过 infoschema 找表、开启事务并用 `tables.IterRecords` 扫描记录；Rust 是一次 trait 委托，是否覆盖分区及句柄类型完全取决于实现者。
- Go 的 `FindIdxInfo` 在表不存在时写 DDL 日志；Rust 仅返回 `None`。
- Go 的 `MatchCancelState` 使用真实 `model.Job`，能借助 `testing.T`/`require` 报告错误的取消状态类型、非 multi-schema job 或长度不等；Rust 使用强类型 `CancelState` 避免动态类型分支，但减少了诊断，也放宽了 `SubJobs` 与 multi-schema 标志的一致性检查。
- Go 的 `checkTableState`、`CheckTableMode` 和 `GetTableInfoByTxn` 显式通过 `kv.RunInNewTxn`/`meta.Mutator` 读取；Rust 将事务保证下放给 `DDLTestRuntime::table_info`。
- Go 的 `SetTableMode` 调用真实 `ddl.AlterTableMode`，并支持生产模型中的 `Normal/Import/Restore`；Rust 仅建模 `Normal/Import`，成功后以 `bool` 返回后置检查结果。
- Go 的 `RefreshMeta` 调用真实 DDL executor；Rust 只构造参数并委托 runtime。

Go 侧直接测试证据包括：`pkg/ddl/cancel_test.go` 的取消 hook、`pkg/ddl/column_modify_test.go` 和 `pkg/ddl/index_modify_test.go` 的并发 DDL、`pkg/ddl/table_mode_test.go` 的模式转换/并发/RefreshMeta 场景、`pkg/ddl/table_test.go::TestRefreshMetaBasic`，以及 indexmerge/fk/partition 测试中的辅助调用。当前没有对应的独立 Rust 测试文件；`operator_test.rs` 只测试相邻 `operator.rs`。

## 扩展指南

- 接入真实 Rust 测试前，应在独立测试文件中实现 mock 或生产适配的 `SessionExecRuntime`、`DDLTestRuntime`，不要把测试模块内嵌到本源文件。
- 若要达到 Go 语义等价，应优先补齐 session 创建/关闭与 `USE` 行为、事务式 meta 读取、分区表句柄扫描、真实 DDL executor 调用、`Restore` 表模式和可诊断的取消状态不变量；不能仅靠返回固定值让测试通过。
- 扩展 `SchemaState`、`TableMode` 或 `Job` 时，应同步生产模型映射，并为未知值、multi-schema 标志与 sub-job 列表不一致添加回归用例。
- 修改并发 SQL helper 时，应保留“一条语句一个结果、首个错误终止、结果集视为错误”的协议；如增加取消、超时或 join，应明确 sender 阻塞和线程清理策略。
- 修改 `SetTableMode` 时应覆盖变更失败不执行后置检查、`Public`/mode 任一不匹配返回 `false`、读取错误传播，以及并发模式转换。修改 `RefreshMeta` 时应覆盖 ID/名称原样映射，并参照 Go 的 infoschema 与 meta KV 不一致场景。
- 相关 Rust 测试应放在同目录新的独立 `*_test.rs` 文件，并由 `lib.rs` 的 `#[cfg(test)] mod ...` 接线；Go 回归意图需继续参考上述 DDL 测试文件。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖仓库；`files --filter pkg/ddl/testutil` 列出本 crate 六个源码文件；`node --file pkg/ddl/testutil/testutil.rs --offset 1 --limit 400` 返回目标文件全部 257 行和 29 个符号；`query` 确认 `SessionExecInGoroutine`、`ExecMultiSQLInGoroutine`、`MatchCancelState`、`SetTableMode` 均存在 Rust/Go 同名定义。`callers/callees` 查询未在限定时间内返回，故调用关系另以精确仓库搜索核验。
- 直接读取：`pkg/ddl/testutil/testutil.rs`、`pkg/ddl/testutil/lib.rs`、`pkg/ddl/testutil/Cargo.toml`、`pkg/ddl/testutil/testutil.go`、根 `Cargo.toml` 与 `pkg/lib.rs`。
- 测试与调用证据：`pkg/ddl/cancel_test.go`、`pkg/ddl/column_modify_test.go`、`pkg/ddl/index_modify_test.go`、`pkg/ddl/table_mode_test.go`、`pkg/ddl/table_test.go`，以及 `pkg/ddl/tests/{adminpause,fk,indexmerge,partition}` 中的 Go 调用。`rg` 未发现目标 Rust API 在其他 `.rs` 文件中的使用，也未发现其独立 Rust 测试。
- DDL 边界依据：`pkg/ddl/doc.go` 说明生产 DDL 的在线 schema version 不变量；`docs/agents/ddl/README.md` 说明 job/owner/state transition 主链。本文件只提供测试抽象，未实现该主链。
- 结构检查使用任务指定命令，要求本文档存在且固定二级标题恰为 11 个；本任务为纯文档分析，按计划不运行 Cargo。
