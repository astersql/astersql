# [`pkg/ddl/notifier/store.rs`](./store.rs)

## 文件定位

`store.rs` 是 `astersql-ddl-notifier` crate 的持久化与事务适配层。crate 入口 `pkg/ddl/notifier/lib.rs` 将本文件公开再导出；发布侧 `PubSchemeChangeToStore` / `PubSchemaChangeInTransaction` 写入 schema change，订阅侧 `process_events` 通过本文件的 `Store`、`ListResult`、`SessionPool` 读取、确认和删除事件。它保存的是 DDL 完成后供订阅者消费的通知记录，不负责创建或调度 DDL job，也不推进 schema state。

该文件同时承载两条后端路径：`Session::FromDDLSession` 包装 `ddl_session::Session`，使 `TableStore` 对真实内部 SQL session 执行 `mysql.tidb_ddl_notifier` 一类表操作；没有 SQL backend 的 `Session::default()` 则使用进程内共享 `BTreeMap`，供 Rust 测试和当前内存运行路径复现相同的事务边界。`pkg/ddl/notifier/Cargo.toml` 表明本 crate 直接依赖 `astersql-ddl-session`、事件模型、AST、`serde_json` 和 `thiserror`。

## 核心职责

1. 以 `Store` trait 定义通知事件的插入、带旧值校验的处理位图更新、删除并提交、有序分页读取和计数接口。
2. 以 `Session` 抽象 Begin/Commit/Rollback、悲观事务及 SQL 执行；内存路径采用“先校验全部操作，再统一应用”的协议，使 handler 暂存副作用与 `processedByFlag` 更新共同成功或共同丢弃。
3. 以 `(ddl_job_id, sub_job_id)` 作为稳定顺序键，将 `SchemaChangeEvent` 在持久化边界编码为 JSON，读取时重建 `SchemaChange`。
4. 以 `old_processed_by -> new_processed_by` 条件更新防止 owner 短时重叠时覆盖另一 owner 的进度；冲突必须导致 handler 所在事务回滚。
5. 管理列表读取的 session 所有权：首次 `Read` 开启事务，调用者最终执行 `CloseFn`，其行为是回滚并释放读取事务。

## 主要符号

- `Error`：统一错误枚举。`NotReadyRetryLater` 表示 handler 可重试，`Message` 保存业务或故障注入信息，`Json` 和 `Session` 分别透传 JSON、DDL session 错误；`is_not_ready` 供订阅器区分可重试错误。
- `SqlSessionBackend` / `DdlSessionBackend`：私有适配接口及真实实现。`execute` 使用 `RequestSource::Ddl` 调用 `execute_internal`，以每批 1024 行耗尽 record set，并无论是否成功读取都尝试 `close`。
- `Session`：可克隆事务句柄，内部共享 `SessionState`、测试可见的 `committed_effects`，并可选持有 SQL backend。公开方法包括 `FromDDLSession`、`ExecuteSQL`、`Begin`、`BeginPessimistic`、`Commit`、`Rollback`、`IsPessimistic`、故障注入和 `StageEffect`。
- `TransactionOperation`：内存事务操作协议；`InsertOperation`、`UpdateOperation`、`DeleteOperation`、`EffectOperation` 分别负责插入、位图 CAS、删除和测试副作用。
- `SessionPool`：由工厂创建 session；`Get` 当前每次新建，`Put` 在归还前调用 `Rollback`，因此“池”表达生命周期契约而非对象复用缓存。
- `Store`：持久化抽象。`Insert`、`UpdateProcessed`、`DeleteAndCommit`、`List` 是生产流程接口，`Count` 是内存状态观察接口。
- `ListResult` / `CloseFn`：增量读取游标及其资源关闭函数。`Read` 最多覆盖调用方 slice 长度个槽位，返回 0 表示读完。
- `TableStore` / `OpenTableStore`：内置实现与构造入口。`TABLES` 以 `(db, table)` 为键，使同一进程内同名句柄共享 `TableData`。
- `TableListResult` / `SqlListResult`：分别在 `BTreeMap` 和 SQL 表上按复合键做 keyset pagination。
- `InsertSchemaChangeSQL`：在调用者已经持有的 worker SQL 事务中执行 INSERT；它明确不自行 Begin 或 Commit，供 `PubSchemaChangeInTransaction` 使用。
- `sql_i64`、`sql_u64`、`sql_bytes`：校验 SQL 行列类型和整数范围；`overwrite_slot` 在复用缓冲槽时调用事件的 `overwrite_from`，再刷新 job id 和处理位图。

## 执行流程

发布流程有两种入口。普通入口 `publish.rs::PubSchemeChangeToStore` 构造 `SchemaChange` 后调用 `Store::Insert`；`TableStore::Insert` 先 `MarshalJSON`，SQL session 路径调用 `InsertSchemaChangeSQL`，内存路径则把 `InsertOperation` 交给 `Session::stage`。若 session 不在事务中，内存操作立即校验并应用；若已在事务中，则等 `Commit` 统一处理。已经位于 DDL worker 事务中的 `PubSchemaChangeInTransaction` 直接调用 `InsertSchemaChangeSQL`，从而让通知记录与调用者的元数据修改共用事务。

消费流程从 `subscribe.rs::process_events` 开始。它取得 list session，调用 `Store::List` 得到游标与 `CloseFn`，再按 `ProcessEventsBatchSize` 反复 `Read`。首次读取开启事务，SQL 与内存实现都执行严格大于当前 `(ddl_job_id, sub_job_id)` 的有序扫描，最多返回缓冲区大小的记录，并把最后一行键保存为下一页游标。循环结束或出错后，调用者仍执行 `close()`，随后 `SessionPool::Put` 再次回滚清理。

单个 handler 的确认由 `process_event_for_handler` 完成：已置位则跳过；否则开启悲观事务，执行 handler，并调用 `UpdateProcessed(old, new)`。SQL 路径先 `SELECT ... FOR UPDATE` 核对旧值，再 UPDATE；内存路径提交前由 `UpdateOperation::validate` 核对行存在且旧位图匹配。只有 handler、位图更新和 `Commit` 全部成功，内存中的 `change.processedByFlag` 才更新。所有已注册 handler 的位均完成后，`process_events` 调用 `DeleteAndCommit` 开新事务删除该行。

## 数据与状态

持久化行由复合主键语义 `(ddlJobID, subJobID)`、JSON 事件和 `processedByFlag: u64` 组成。`Insert` 无论传入的 `SchemaChange.processedByFlag` 为何都写入 0，表示尚无订阅者处理。`subJobID` 对普通 DDL 通常为 -1，对 multi-schema/合并任务表示子任务索引；由于合法 DDL job ID 为正，两个列表实现都以 `(0, 0)` 作为尚未开始的游标。

内存后端的 `TABLES` 是进程级 `LazyLock<Mutex<HashMap<...>>>`；每张逻辑表映射到 `Arc<Mutex<TableData>>`，内部 `BTreeMap` 同时提供唯一键检查和复合键排序。此数据没有落盘能力，生命周期仅覆盖进程；真实持久化依赖 SQL backend 访问外部已创建的表。

`SessionState` 记录是否在事务中、是否为悲观事务、下一次 Begin/Commit 故障及挂起操作。`Commit` 在持有 session mutex 时先逐项 `validate`，SQL backend 则随后提交真实事务；状态清理后，内存 `apply` 在锁外执行。`Rollback` 丢弃挂起操作并退出事务。`committed_effects`、`FailNextBegin` 和 `FailNextCommit` 是用于验证原子性的测试观察/注入状态。

列表读取复用 `Vec<Option<SchemaChange>>` 槽位。`overwrite_slot` 不总是彻底替换已有事件对象，而是通过 `SchemaChangeEvent::overwrite_from` 合并当前事件的活动字段；`store_test.rs::test_leftover_when_unmarshal` 明确证明旧的非活动字段可以暂时残留，但事件类型决定后续读取哪个字段，残留不改变语义。

## 依赖与调用关系

上游写入边包括 `publish.rs::PubSchemeChangeToStore -> Store::Insert`，以及 `PubSchemaChangeInTransaction -> InsertSchemaChangeSQL`。上游消费链为 `subscribe.rs::process_events -> Store::List/ListResult::Read`；其内部 `process_event_for_handler -> Store::UpdateProcessed`，完成全部 handler 后再走 `Store::DeleteAndCommit`。`DDLNotifier` 的 owner 生命周期控制消费线程启停，但本文件自身不创建后台线程。

下游依赖中，事件编码调用 `SchemaChangeEvent::MarshalJSON` / `UnmarshalJSON` / `overwrite_from`；真实事务和 SQL 调用落到 `ddl_session::Session`、`ExecutionContext`、`RecordSet`、`SqlValue` / `SqlRow`；内存路径依赖标准库的 `Arc`、`Mutex`、`LazyLock`、`HashMap` 和 `BTreeMap`。`TableStore` 只拼接构造时传入的库表名，表结构的建立与标识符合法性由外部装配负责。

RustCodeGraph 已索引 `store.rs`、`publish.rs`、`subscribe.rs` 及 Go 对照文件，并能定位 `OpenTableStore`、`UpdateProcessed`、`DeleteAndCommit` 与 `InsertSchemaChangeSQL`；但 `Store` / `List` 名称在全仓同名严重，精确 caller 查询对 `InsertSchemaChangeSQL` 未返回边，因此这里的模块内调用边由上述直接源码调用点补证，不将图的空结果解释为“无调用者”。

## 错误处理与边界

JSON 编解码、session 操作和 SQL record set 读取错误均向调用者返回。`DdlSessionBackend::execute` 若 drain 失败优先返回 drain 错误；若 drain 成功而 close 失败，则返回 close 错误。SQL 行缺列、类型不符、无符号/有符号整数越界均转成包含列号的 `Error::Message`；SQL List 返回数超过缓冲区也会显式报错。

插入边界的内存实现拒绝重复复合键；真实 SQL 路径依赖数据库唯一约束返回错误。`UpdateProcessed` 对“不存在”与“旧位图不匹配”统一视为并发冲突。SQL 路径用 `SELECT ... FOR UPDATE` 后检查唯一一行及旧值，内存路径在提交校验阶段检查；冲突消息明确提示可能被另一 owner 更新。删除不存在的行是幂等成功。

`DeleteAndCommit` 在 Begin、DELETE 或 Commit 任一步失败时传播错误；执行阶段错误和提交错误均回滚 SQL session。内存 Begin 后的 stage 失败也回滚。handler 路径则由 `subscribe.rs` 保证 handler、CAS 或提交任一失败都调用 `Rollback`，因此未确认事件保留待重试，`StageEffect` 也不会提交。

互斥锁中毒目前通过 `expect(...)` 触发 panic，而不是可恢复错误。库表名直接格式化进 SQL，只应来自可信内部配置，不能把未校验的用户输入传给 `OpenTableStore`。`Read` 的空缓冲区会生成 LIMIT 0 并返回 0，调用方会把它视作结束；生产调用方必须保证批大小大于 0。

## 并发与资源生命周期

所有可跨线程使用的公开抽象都满足 `Send` / `Sync` 约束；`Session`、`TableStore` 和共享表数据以 `Arc<Mutex<_>>` 协调。内存提交先校验全部操作再应用，可防止已知校验失败造成部分应用；但校验与逐个应用之间没有持有覆盖整张表的单一事务锁，多 session 真正并发提交时不等价于数据库事务的完全隔离。生产并发正确性应依赖真实 SQL 悲观事务，而内存协议主要服务行为测试。

真实 SQL 的 `UpdateProcessed` 必须在调用者开启的悲观事务中使用；`SELECT ... FOR UPDATE` 的行锁持续到 `Session::Commit` / `Rollback`。`List` 独占传入 session，直到 `CloseFn` 被调用；CloseFn 回滚只读事务。`SessionPool::Put` 也总是回滚，形成遗漏状态的第二层清理。`DdlSessionBackend::execute` 完整 drain 并 close record set，避免结果资源泄漏。

`TABLES` 让同名 `(db, table)` 句柄共享数据，且不会在句柄 drop 时移除；测试因此使用唯一表名避免相互污染。`DDLNotifier` 线程停止和 owner 切换由 `subscribe.rs` 管理，本文件只提供可在线程间共享的存储与 session 组件。

## 与 Go 版本的对应关系

`pkg/ddl/notifier/store.go` 是直接语义基准：两版都定义 Store/ListResult、以 JSON 存事件、插入位图 0、按 `(ddl_job_id, sub_job_id)` 有序分页、通过 CloseFn 回滚列表事务，并在旧位图不匹配时报告 owner 冲突。Rust 的 `SqlListResult` 对应 Go `listResult.Read/unmarshalSchemaChanges`；`pkg/ddl/notifier/store_test.go::TestLeftoverWhenUnmarshal` 与 Rust `store_test.rs::test_leftover_when_unmarshal` 都保留复用槽位中非活动字段残留的契约。

实现差异必须明确：Go `tableStore` 只操作真实 session/SQL 表；Rust `TableStore` 额外内置进程级内存后端和 `Session`/`SessionPool` 测试事务模型。Go 更新通过带 `processed_by_flag = old` 的 UPDATE 及 affected rows 判冲突；Rust SQL 路径采用 `SELECT ... FOR UPDATE` 核对后再 UPDATE，要求外层悲观事务提供锁保护。Rust 还暴露 `InsertSchemaChangeSQL`，支持复用已有 worker 事务而不自行提交。

Go 接口接收 `context.Context` 并对 List Read 建 tracing region；Rust 当前 API 没有显式 context 或 tracing。Go `Store` 无 `Count`，Rust 增加它供测试/观察内存存储。Rust `DeleteAndCommit` 的 sub-job 参数统一为 `i64`，Go 版本该方法使用 `int`。这些差异不是可以任意简化的空间；扩展时应保持外部行为、事务原子性和 JSON 契约与 Go 版本一致。

## 扩展指南

新增持久化字段时，应同时修改 `SchemaChange`/事件编码、真实 SQL INSERT/SELECT/行解码、`StoredChange`、内存 Insert/List，以及数据库表定义或迁移；还要检查 Go `store.go` 的列顺序和 JSON 兼容性。新增 Store 方法需同时更新 trait、`TableStore` 两条后端路径、测试包装 store（例如 `testkit_test.rs::FailFirstInsertStore`）和 Go 对照接口。

调整确认并发协议时，重点修改 `TableStore::UpdateProcessed`、`UpdateOperation::validate` 与 `subscribe.rs::process_event_for_handler`。不得删除旧位图校验，也不能把 handler 提交和位图提交拆开；应同步覆盖短时双 owner、悲观事务、handler 错误、Commit 失败和事件保留。调整分页时，应保持复合键严格递增、稳定排序、页间无重无漏，并同步 `SqlListResult` 与 `TableListResult`。

测试应继续放在独立文件中：局部序列化/SQL adapter 行为放 `pkg/ddl/notifier/store_test.rs`，完整发布订阅、owner 冲突和故障路径放 `pkg/ddl/notifier/testkit_test.rs`；Go 行为对照见同目录 `store_test.go` / `testkit_test.go`。若接入新的真实 session 能力，还应核对 `pkg/ddl/session` 的事务与 record set 生命周期，不在本文件复制外部 session 实现。

风险检查包括：JSON 向后兼容、SQL 列类型与整数范围、库表标识符来源、悲观锁持有时间、分页批大小、内存锁竞争和全局 TABLES 测试污染。内存路径不能作为真实持久化或跨进程 owner 协调方案。

## 验证依据

- 源码：`pkg/ddl/notifier/store.rs` 的 `Error`、`DdlSessionBackend`、`Session`、`SessionPool`、`Store`、`TableStore`、两种 ListResult 和 `InsertSchemaChangeSQL`；`pkg/ddl/notifier/publish.rs` 的两个发布入口；`pkg/ddl/notifier/subscribe.rs` 的 `process_events` / `process_event_for_handler`；`pkg/ddl/notifier/events.rs` 的 JSON 与覆盖逻辑。
- crate 边界：`pkg/ddl/notifier/Cargo.toml` 和 `pkg/ddl/notifier/lib.rs`。
- Rust 独立测试：`pkg/ddl/notifier/store_test.rs` 覆盖缓冲复用、同名表共享、位图归零和真实 SQL session 事务；`pkg/ddl/notifier/testkit_test.rs` 覆盖发布订阅、顺序与清理、发布失败、双 owner、分页、Begin/handler/Commit 错误及副作用回滚。
- Go 对照：`pkg/ddl/notifier/store.go`、`pkg/ddl/notifier/store_test.go`，并以 `publish.go`、`subscribe.go`、`testkit_test.go` 的调用点核对完整语义。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/ddl/notifier` 确认模块文件集合；`node --file pkg/ddl/notifier/store.rs --offset 1/500` 读取完整 804 行；`query` 定位 Rust/Go `OpenTableStore` 及 Rust `UpdateProcessed`、`DeleteAndCommit`、`InsertSchemaChangeSQL`。宽泛名称产生同名噪声，精确 `InsertSchemaChangeSQL` caller 未返回结果，调用边改由模块源码直接核验。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前以任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工检查所有关键行为均能追溯至上述源码或测试。
