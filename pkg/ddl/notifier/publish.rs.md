# `pkg/ddl/notifier/publish.rs`

源文件：[publish.rs](publish.rs)

## 文件定位

本文件是 `astersql-ddl-notifier` crate 的 schema 变更事件发布边界。crate 入口 `pkg/ddl/notifier/lib.rs` 通过 `mod publish; pub use publish::*;` 将这里的类型和函数公开给 DDL 执行层；crate 归属和本地依赖见 `pkg/ddl/notifier/Cargo.toml`。它不负责生成 DDL 事件或消费事件，只把已经构造好的 `SchemaChangeEvent` 组装为持久化记录并交给存储层。

当前 Rust 生产主链的直接入口是 `pkg/ddl/persistent_actions.rs::async_notify_event`：该函数跳过内存库/系统库、解析 multi-schema 子任务编号后，调用 `PubSchemaChangeInTransaction`，使通知记录与 worker 已有 SQL 事务共用生命周期。另一公开入口 `PubSchemeChangeToStore` 面向 `Store` 抽象，主要由 notifier 的独立 Rust 测试以及可注入的存储实现使用；RustCodeGraph 文件节点同时显示 `publish.rs` 被 `store.rs`、`testkit_test.rs` 和 `persistent_actions.rs` 引用。

## 核心职责

1. 用 `(ddlJobID, subJobID)` 标识一次 DDL job 或其中一个子 job，把事件载荷与订阅方处理位图封装为 `SchemaChange`。
2. 发布新记录时强制把 `processedByFlag` 初始化为 `0`，表示尚无订阅 handler 完成处理；调用者不能借发布 API 预置处理状态。
3. 提供两种持久化适配方式：`PubSchemeChangeToStore` 委托动态分派的 `Store::Insert`；`PubSchemaChangeInTransaction` 则构造固定的 `mysql.tidb_ddl_notifier` 插入，通过调用者提供的单次执行闭包复用 worker 活跃事务。
4. 保持薄边界：事件 JSON 编码、SQL 生成、重复键检测和实际事务行为均由 `pkg/ddl/notifier/store.rs` 中的存储实现承担，本文件只组装记录并原样传播错误。

## 主要符号

- `pub struct SchemaChange`：持久化行的 Rust 表示，派生 `Clone`、`Debug`、`Eq` 和 `PartialEq`。`ddlJobID: i64` 是父 DDL job ID；`subJobID: i64` 是子任务索引，普通 DDL 使用 `-1`；`event: SchemaChangeEvent` 是具体事件；`processedByFlag: u64` 是按 handler 位编号记录处理完成状态的位图。字段公开，供 `store.rs` 和订阅逻辑读写。
- `pub fn PubSchemeChangeToStore(session: &Session, ddl_job_id: i64, sub_job_id: i64, event: SchemaChangeEvent, store: &dyn Store) -> Result<(), Error>`：构造初始记录并调用 `store.Insert`。它不自行开启或提交事务；`Session` 当前是否处于事务中决定内存 `TableStore` 是暂存还是立即应用，SQL store 则使用其关联的 DDL session。
- `pub fn PubSchemaChangeInTransaction(ddl_job_id: i64, sub_job_id: i64, event: SchemaChangeEvent, execute: impl FnOnce(&str, &[SqlValue]) -> Result<(), Error>) -> Result<(), Error>`：把记录传给 `InsertSchemaChangeSQL("mysql", "tidb_ddl_notifier", ...)`。`FnOnce` 明确一次发布只执行一次 SQL 回调，函数本身不取得 session 所有权，也不创建事务。
- 本文件没有模块级常量、trait、`impl`、条件编译项或后台任务；命名沿用 Go API，crate 根通过 `#![allow(non_snake_case)]` 接受该风格。

## 执行流程

通用 `Store` 路径如下：

1. 调用者提供已有 `Session`、job 标识、完整事件和 `Store`。
2. `PubSchemeChangeToStore` 移入事件，构造 `SchemaChange { ddlJobID, subJobID, event, processedByFlag: 0 }`。
3. 函数调用 `Store::Insert` 并直接返回其 `Result`。`TableStore::Insert` 在 `pkg/ddl/notifier/store.rs` 中先调用 `SchemaChangeEvent::MarshalJSON`；SQL backend 生成参数化 INSERT 并在当前 session 执行，内存 backend 则按 session 状态暂存或自动提交。

生产 worker 事务路径如下：

1. `pkg/ddl/persistent_actions.rs::async_notify_event` 先跳过系统 schema；传入 `-1` 且存在 `multi_schema_info` 时，用其 `seq` 作为 `sub_job_id`。
2. 它调用 `PubSchemaChangeInTransaction`；本文件同样构造处理位图为 0 的 `SchemaChange`。
3. `InsertSchemaChangeSQL` 序列化事件，并生成向 `mysql.tidb_ddl_notifier(ddl_job_id, sub_job_id, schema_change, processed_by_flag)` 插入的 SQL，其中最后一列固定为 0。
4. `async_notify_event` 提供的闭包把整数参数和 JSON 字节绑定到 worker query；JSON 字节转换为十六进制字面量以避免 SQL 字符串转义问题，然后通过 `JobExecutionContext::query(..., "publish-schema-change")` 在当前事务执行。
5. 任一步失败都会沿 `Result` 返回；本文件不吞错、不重试，也不提交或回滚。事务最终成败由外层 DDL worker 决定。

## 数据与状态

`SchemaChange` 自身没有内部可变性。其逻辑主键是 `(ddlJobID, subJobID)`；Go 对照 `pkg/ddl/notifier/publish.go` 明确要求该二元组在集群内唯一，Rust `TableStore` 用 `BTreeMap<(i64, i64), StoredChange>` 保存并在 `InsertOperation::validate` 中拒绝重复键。真实表同样由这两个 ID 区分普通 job 与 multi-schema/batched DDL 的各子事件。

`event` 是 `pkg/ddl/notifier/events.rs::SchemaChangeEvent`。持久化边界调用 `MarshalJSON`，因此表内保存的是事件 JSON，而不是 Rust 内存布局。`processedByFlag` 的每一位对应一个 handler；发布时恒为 0，后续由订阅路径的 `Store::UpdateProcessed` 以旧值校验方式更新，从而检测短暂双 owner 等并发覆盖。

`subJobID == -1` 是普通 DDL 的约定值。生产入口还会在调用本文件前把隐式 `-1` 转换为 `multi_schema_info.seq`；因此发布函数不解析 job 类型，也不校验 ID 的取值范围，它只持久化调用者给出的标识。

## 依赖与调用关系

上游：

- `pkg/ddl/persistent_actions.rs::async_notify_event` 是已检索到的 `PubSchemaChangeInTransaction` 生产调用者，负责系统 schema 过滤、子 job ID 解析和 worker SQL 执行适配。
- `pkg/ddl/notifier/testkit_test.rs` 与 `store_test.rs` 多处调用 `PubSchemeChangeToStore`，覆盖发布、订阅、读取、失败与事务存储行为。
- crate 根 `pkg/ddl/notifier/lib.rs` 公开再导出本文件 API，因此外部 crate 使用 `astersql_ddl_notifier::...` 路径。

下游：

- `crate::Store::Insert` 是通用入口的唯一直接操作；其接口和 `TableStore` 实现在 `pkg/ddl/notifier/store.rs`。
- `crate::InsertSchemaChangeSQL` 是事务内入口的唯一直接操作，负责 JSON 编码、SQL 文本和 `SqlValue` 参数构造。
- `crate::SchemaChangeEvent` 提供事件载荷及 `MarshalJSON`；`crate::Error` 统一承载 JSON、DDL session 和业务错误。

`pkg/ddl/notifier/Cargo.toml` 没有 feature 开关；直接依赖包括用于真实 session 的 `astersql-ddl-session`、事件模型所需的 `astersql-meta-model`/`astersql-parser-ast`，以及 `serde`、`serde_json`、`thiserror`。本文件没有直接使用外部 crate 名，而是通过 crate 根再导出的本地抽象工作。

## 错误处理与边界

- 两个发布函数都只使用 `?`/直接返回传播下游 `Error`，没有日志、重试或错误改写。日志和 DDL job 错误字符串转换位于 `async_notify_event` 等上层。
- JSON 序列化错误来自 `SchemaChangeEvent::MarshalJSON`；session/SQL 错误来自 `Store::Insert` 或执行闭包；内存 store 的重复 `(ddlJobID, subJobID)` 返回 `duplicate schema change (...)`。
- `PubSchemaChangeInTransaction` 固定真实目标为 `mysql.tidb_ddl_notifier`，不接受可配置表名。这是生产集成约束；需要测试其他表时应使用 `OpenTableStore`/`Store` 路径，而不是修改生产表常量。
- 本文件不检查空/default 事件、负 job ID 或非法子任务 ID。若这些值需要约束，应在事件构造或 DDL 调用层验证，避免让薄持久化边界重复业务规则。
- 事务内执行闭包必须绑定到已经活跃的 worker session。`InsertSchemaChangeSQL` 和本文件明确不 begin/commit；传入自动提交连接会破坏“DDL 元数据与通知记录同事务”的预期。

## 并发与资源生命周期

本文件不创建线程、锁、通道、游标或长期资源。`SchemaChange` 按值持有事件，发布函数在同步调用结束前完成组装和下游调用。

`PubSchemeChangeToStore` 借用 `Session` 与 `Store`，资源和事务所有权仍属于调用者。内存实现若 session 已在事务中，会把 `InsertOperation` 放入挂起队列，提交前统一校验后应用；非事务状态则立即校验并应用。真实 SQL backend 在所借 session 上执行 SQL。

`PubSchemaChangeInTransaction` 的执行器是 `FnOnce`，避免一个事务发布被意外重复调用；它只借用 SQL 参数切片，并在函数返回前完成执行。生产 `async_notify_event` 复用 DDL worker 事务，所以外层回滚也应撤销 notifier 插入。订阅侧的多 owner 冲突不在本文件解决，而由主键唯一性和 `UpdateProcessed` 的比较更新语义保护。

## 与 Go 版本的对应关系

`pkg/ddl/notifier/publish.go` 是 `PubSchemeChangeToStore` 和 `SchemaChange` 的直接语义来源。两版均构造 `(ddlJobID, subJobID, event, processedByFlag=0)` 记录并委托 `Store.Insert`；普通 DDL 使用 `subJobID=-1`，multi-schema 或批量建表使用子任务索引。Rust 用按值 `SchemaChangeEvent` 和 `Result<(), Error>`，Go 用事件指针、`context.Context`、session 指针和 `error`，但持久化字段语义一致。

Rust 额外提供 `PubSchemaChangeInTransaction`，用闭包桥接当前 Rust worker 的事务执行接口；Go 生产路径在 `pkg/ddl/ddl.go` 直接把 session 和 store 传给 `PubSchemeChangeToStore`。Rust 的事务内入口不是简化掉存储语义：它复用 `store.rs::InsertSchemaChangeSQL` 的同一 JSON/INSERT 逻辑，只把事务所有权保留在 worker。

Go 的 `pkg/ddl/notifier/testkit_test.go::TestPublishEventError` 通过 failpoint 验证发布错误会导致 DDL job 报错，解除故障后同一 DDL 可成功；Rust `pkg/ddl/notifier/testkit_test.rs` 的失败插入测试验证失败不落行且下一次发布恢复。Go `store.go` 与 Rust `store.rs` 都固定插入位图 0，并按 job/sub-job 顺序读取。

## 扩展指南

- 新增持久化字段时，应同步修改 `SchemaChange`、`InsertSchemaChangeSQL`、`Store` 的 SQL/内存编码与读取逻辑、真实系统表定义、Go `publish.go`/`store.go` 及独立 `store_test.rs`；必须考虑已有行和滚动升级兼容，不能只改本文件。
- 新增发布前业务校验时，优先放在产生事件的 DDL 调用层；只有所有发布者都必须满足且与持久化契约直接相关的约束才适合放在两个公开入口，并保证两条路径行为一致。
- 改变子 job ID 规则时，应修改 `persistent_actions.rs::async_notify_event` 的解析逻辑并对照 Go `ddl.go`，同时添加普通 job、multi-schema job 和显式子 ID 的独立 Rust 测试。
- 改变事务行为时，必须保持 `PubSchemaChangeInTransaction` 不自行提交的契约，否则可能出现 DDL 回滚但通知仍可见。建议在独立测试文件中新增一个直接覆盖该函数的回滚/执行器失败测试；不要把测试嵌入 `publish.rs`。
- 新 handler 不应修改发布初值；处理位分配与更新属于 `subscribe.rs`/`Store::UpdateProcessed`。handler 数量、位宽和兼容性变化需评估 `u64` 上限以及旧节点对位编号的解释。
- 性能方面，每次调用都会序列化完整事件并执行一次插入；批量化或去重必须保留 `(ddlJobID, subJobID)` 唯一性、事件顺序和外层事务原子性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ddl/notifier` 确认目标目录 15 个文件、`publish.rs` 5 个符号；`node --file pkg/ddl/notifier/publish.rs` 读取全部 78 行，并给出 `store.rs`、`testkit_test.rs`、`persistent_actions.rs` 三个文件级引用。`query publish` 定位两个 Rust 函数和 `SchemaChange`。对两个函数执行精确 `callers`/`callees` 查询均返回空集合，因此函数级调用边由后续源码引用搜索补足，未把空图结果推断为“无人调用”。
- 生产源码：`pkg/ddl/notifier/publish.rs`；`pkg/ddl/persistent_actions.rs::async_notify_event`；`pkg/ddl/notifier/store.rs::{Store, TableStore::Insert, InsertSchemaChangeSQL}`；`pkg/ddl/notifier/events.rs::SchemaChangeEvent`；`pkg/ddl/notifier/lib.rs`；`pkg/ddl/notifier/Cargo.toml`。
- Go 对照：`pkg/ddl/notifier/publish.go`、`pkg/ddl/notifier/store.go`、`pkg/ddl/ddl.go`。
- Rust 独立测试：`pkg/ddl/notifier/testkit_test.rs::test_publish_to_table_store`、`test_publish_event_error` 以及 pub/sub 顺序与重试用例；`pkg/ddl/notifier/store_test.rs::insert_initializes_processed_by_flag_to_zero`、`open_table_store_instances_share_the_same_persistent_table`、`table_store_uses_real_ddl_session_sql_transactions`。当前搜索未发现直接调用 `PubSchemaChangeInTransaction` 的 `*_test.rs`，这是已记录的覆盖缺口，而非已验证结论。
- Go 测试：`pkg/ddl/notifier/testkit_test.go::TestPublishToTableStore`、`TestPublishEventError`、`Test2OwnerForAShortTime` 等提供移植语义参照。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构检查要求目标文件存在且恰好包含上述 11 个固定二级标题。
