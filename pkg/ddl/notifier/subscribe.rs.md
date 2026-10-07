# `pkg/ddl/notifier/subscribe.rs`

## 文件定位

本文件是 `astersql-ddl-notifier` crate 的订阅与分发实现。crate 入口 `pkg/ddl/notifier/lib.rs` 以 `mod subscribe; pub use subscribe::*;` 将这里的 API 暴露给调用方；同 crate 的 `publish.rs` 负责写入 schema change，`store.rs` 提供事件存储、会话和事务抽象，`events.rs` 定义 `SchemaChange` 与 `SchemaChangeEvent`。

从完整产品链看，它位于“DDL 完成后持久化事件”与“统计信息等订阅者消费事件”之间：轮询 `Store::List`，逐 handler 处理事件并更新 `processedByFlag`，所有已登记 handler 都完成后再删除事件。当前 Rust 生产代码已经通过 `pkg/ddl/persistent_actions.rs` 等文件使用发布侧 API，但仓库搜索显示 `NewDDLNotifier` 的 Rust 调用只在 `pkg/ddl/notifier/testkit_test.rs`；与 owner、Domain 和统计模块的生产装配仍见 Go 的 `pkg/domain/domain.go`、`pkg/statistics/handle/handle.go` 和 `pkg/statistics/handle/autoanalyze/refresher/refresher.go`。因此，本文件的机制已有 Rust 实现和独立测试，但不能据此声称 Rust 主程序已完成订阅侧接线。

`pkg/ddl/notifier/Cargo.toml` 将它归入 `astersql-ddl-notifier`，直接依赖 `astersql-ddl-session`、元数据 model、parser AST、Serde/JSON 与 `thiserror`；没有 feature 条件。本文件也没有条件编译项，只有运行时使用的 `cfg!(test)` 分支。

## 核心职责

- 定义订阅回调 `SchemaChangeHandler`、稳定的 `HandlerID` 及内置 ID。
- 维护 handler 注册表，并在成为 owner 时固化 handler 位图。
- 启停一个按 `poll_interval` 周期运行的后台线程。
- 按存储顺序、分批读取 `SchemaChange`，对每个 handler 保持事件先后约束。
- 在同一悲观事务内执行 handler 副作用和 `processedByFlag` 的旧值校验更新，成功提交后才修改内存中的 flag。
- 当事件的 flag 等于启动时固化的 handler 位图时删除事件。
- 区分“尚未就绪、稍后重试”和普通错误，并累计可观测的错误字符串。

它不是 DDL job 状态机、schema version 同步器或事件发布器；它消费已经由发布侧持久化的记录。其投递保证依赖 handler 把需要原子化的修改都放在传入的 `Session` 中，不能把任意外部副作用自动变成 exactly-once。

## 主要符号

- `SchemaChangeHandler = Box<dyn FnMut(Session, &SchemaChangeEvent) -> Result<(), Error> + Send + 'static>`：可变、可跨线程移动的回调。传入克隆的事务会话和事件引用；回调实例由 notifier 串行、互斥地调用。
- `ErrNotReadyRetryLater()`：构造 `Error::NotReadyRetryLater`。该错误不会写入详细 handler 错误日志，但 handler 会在本轮被跳过、后续轮询再试。
- `HandlerID = i32` 及 `TestHandlerID`、`StatsMetaHandlerID`、`PriorityQueueHandlerID`：ID 是 `u64` 位图的位号，所以 `RegisterHandler` 强制范围为 `0..64`。`HandlerIDString` 仅为测试和 stats ID 提供专名，其余输出数值形式。
- `ProcessEventsBatchSize: AtomicUsize`：默认 1024，控制一次 `ListResult::Read` 的缓冲区长度。实现直接使用载入值创建向量，没有把 0 钳制为 1；设为 0 会使读取立即表现为无数据。
- `slowHandlerLogThreshold`：与 Go 常量对齐的 5 秒阈值，但当前 Rust 文件没有计时或慢 handler 日志逻辑，属于保留但未使用的符号。
- `NotifierInner`：被后台线程共享的状态，包括 `SessionPool`、`Arc<dyn Store>`、按 ID 排序的 `BTreeMap` handler、固化位图、轮询间隔、停止标志和错误列表。
- `DDLNotifier`：公开门面，持有 `Arc<NotifierInner>` 和至多一个 `JoinHandle`。`NewDDLNotifier` 创建停止态实例。
- `OwnerListener`：本文件自定义的 owner 生命周期 trait；`DDLNotifier` 实现它，同时提供同名固有方法作为转发入口。
- `RegisterHandler`：非法 ID panic；重复 ID 静默保留原 handler；合法新 ID 写入注册表。
- `ProcessEvents`：公开的同步单轮入口，主要供测试或显式驱动使用。
- `Stop`、`OnBecomeOwner`、`OnRetireOwner`、`Drop`：管理工作线程生命周期。
- `process_events`：分页扫描、顺序分发、错误隔离和清理的主循环。
- `process_event_for_handler`：单个事件/handler 的事务边界与位图 CAS 更新逻辑。

## 执行流程

1. 调用方用 `NewDDLNotifier(session_pool, store, poll_interval)` 创建停止态通知器，并在成为 owner 前调用 `RegisterHandler`。
2. `OnBecomeOwner` 先锁住 `worker`。若已有线程则幂等返回；否则从当前 handler ID 计算 `handlers_bitmap`，清除停止标志并启动线程。
3. 工作线程先 `park_timeout(poll_interval)`，醒来后检查停止标志，再调用 `process_events`。因此成为 owner 后不会立即处理，首轮至少等待一个轮询间隔；`Stop` 可用 `unpark` 提前唤醒退出。
4. `process_events` 分别取得 list session 和 process session，调用 `Store::List` 建立有序游标，然后按 `ProcessEventsBatchSize` 循环读取。
5. 每条 change 都先复制当前 handler ID 列表。`BTreeMap` 使同一事件的 handler 按 ID 升序执行；每个 handler 对事件的跨记录顺序则跟随 `ListResult` 的存储顺序。
6. `process_event_for_handler` 发现对应 bit 已置位时直接成功，避免重做已提交 handler。否则开启悲观事务、执行回调、以旧 flag 和新 flag 调用 `Store::UpdateProcessed`，最后提交；任一步失败都回滚。只有提交成功才更新当前 `change.processedByFlag`。
7. 某 handler 在本轮对一条事件失败后，其 ID 加入 `skip_handlers`，本批扫描剩余事件均不再交给它，防止越过失败事件破坏单 handler 顺序。其他 handler 仍可继续。
8. 每条事件处理后，如果当前 flag 恰好等于 owner 启动时固化的位图，就用独立 session 调用 `DeleteAndCommit`。测试构建且位图为 0 时特意保留事件；非测试构建的零 handler 情况会删除事件。
9. 扫描结束或遇到可传播的读取错误后，总会调用 list 的 `close`，并把 list/process session 归还池。后台线程把 `process_events` 返回的顶层错误追加到 `errors`。

## 数据与状态

持久化进度由 `SchemaChange.processedByFlag: u64` 表示：第 `id` 位为 1 意味着该 handler 的事务已经提交。`handlers_bitmap` 是成为 owner 瞬间所有已注册 ID 的并集，也是删除条件。这个位图不会因 owner 运行期间注册 handler 而更新；虽然 Rust 用 `Mutex` 让注册动作在内存层面免于数据竞争，语义上仍应遵守 Go 注释中的约束：所有 handler 必须在启动前注册。

`handlers` 使用 `BTreeMap`，带来确定的 ID 顺序；handler 类型是 `FnMut`，所以调用时必须取得注册表的可变锁。`errors` 保存普通处理错误、删除失败以及后台单轮失败的字符串，`Errors()` 返回快照而不清空。`NotReadyRetryLater` 不写详细错误，但仍触发该 handler 的本轮跳过。

`skip_handlers` 只活在一次 `process_events` 调用中；下次轮询会重新尝试。`changes` 缓冲区在多次 `Read` 间复用，存储实现负责覆盖有效的前 `count` 个槽位。

删除条件使用严格相等而非包含判断。这要求已提交 flag 与当前固化 handler 集合一致；如果持久化行含有不在当前位图中的历史 bit，当前实现不会删除。反过来，启动后新增 handler 不在固化位图中，可能导致事件在新 handler 处理前已满足删除条件。这两点都说明 HandlerID 集合及注册时机属于持久化兼容契约。

## 依赖与调用关系

上游与入口：

- `pkg/ddl/notifier/lib.rs` 再导出本文件全部公开 API。
- Rust 搜索中，`NewDDLNotifier`、`RegisterHandler` 和 owner 生命周期入口的直接有效调用集中在 `pkg/ddl/notifier/testkit_test.rs`；RustCodeGraph 将目标文件标记为被 4 个文件使用，但其中 `pkg/util/sem/v2/sql_rule.rs`、`pkg/util/watcher/*.rs` 是同名符号造成的粗粒度文件关联，不能作为 notifier 生产调用证据。
- Go 生产链由 `pkg/domain/domain.go` 创建 table store 和 notifier，将同一 store 传给 DDL 发布侧，并把 notifier 包进 stats owner listener；`pkg/statistics/handle/handle.go` 注册 stats handler，auto-analyze refresher 注册 priority queue handler。

下游调用：

- `SessionPool::{Get, Put}` 提供 list、process 和 delete 会话；`Put` 会回滚遗留状态。
- `Store::List` / `ListResult::Read` 提供有序分页扫描，`Store::UpdateProcessed` 完成旧值校验更新，`Store::DeleteAndCommit` 删除完成记录。
- `Session::{BeginPessimistic, Commit, Rollback}` 划定 handler 与 flag 更新的共同事务。
- `SchemaChange` 提供 `(ddlJobID, subJobID)` 主键、事件和值位图。
- 标准库的 `Arc`、`Mutex`、原子量和 `thread::{spawn, park_timeout, unpark}` 实现共享与生命周期控制。

Cargo 依赖是 crate 级边界而非全都由本文件直接引用：本文件通过 crate 再导出的类型间接依赖 `ddl-session` 和事件 model/AST；JSON 与 `thiserror` 主要由相邻的 `store.rs`、`events.rs` 使用。

## 错误处理与边界

- Handler ID 不在 `0..64` 会 panic；重复 ID 不替换已有回调，也不返回错误。
- handler 已处理过事件时直接成功，不开启事务。
- `BeginPessimistic`、handler、`UpdateProcessed` 或 `Commit` 失败都会返回错误；后三者显式 `Rollback`，开始事务本身失败时没有事务可回滚。
- `UpdateProcessed` 接收旧 flag，阻止短时双 owner 把另一方进度覆盖掉。`store.rs` 的内存实现会以“row 已被另一 owner 更新”的错误拒绝缺行或旧值不符；事务中的 handler 副作用也随之回滚。
- 普通 handler 错误记录事件主键和 handler 名称；NotReady 只表示保留并重试。两类错误都会停止该 handler 在当前扫描中的后续投递。
- 删除失败只记录错误并继续扫描，不让整轮失败；记录保留后可再次处理删除。因为所有 handler bit 已置位，下一轮不会重复调用 handler。
- `ListResult::Read` 的错误作为 `process_events` 返回值向上传播；无论成功失败都会关闭游标、归还两个长期会话。
- mutex poison 均通过 `expect` 转为 panic；后台线程没有 `catch_unwind`。与 Go 的 `WaitGroupWrapper.RunWithRecover` 相比，Rust 后台线程 panic 会终止线程，之后 `worker` 仍保存已完成的 handle，重复 `OnBecomeOwner` 不会重启，直到 `Stop` 取出并 join。
- `Stop` 忽略线程 join 的 panic 结果；`Drop` 调用 `Stop`，尽量避免孤儿线程。

## 并发与资源生命周期

`DDLNotifier` 可由共享引用调用：可变集合放在 `Mutex` 中，停止状态和位图使用原子量。`OnBecomeOwner` 通过持有 `worker` 锁保证只启动一个线程；`OnRetireOwner` 设置 Release 停止标志、唤醒线程并 join，工作线程用 Acquire 观察。再次成为 owner 时会重新计算 handler 位图并创建新线程。

handler 的调用发生在持有整个 `handlers` mutex 期间，因此不同 handler 不并行，注册也会等待正在执行的 handler；长耗时 handler 会阻塞该 notifier 的所有分发。`slowHandlerLogThreshold` 当前并未用于观测这种阻塞。

list session 在游标完整生命周期内持有，process session 在整轮扫描中复用；每次删除临时取得单独 session 并立即归还。`close()` 在归还 list session 前执行。`SessionPool::Put` 会回滚，防止未完成事务泄漏到后续用途。

这里的 owner 约束是生命周期协议而非分布式互斥实现：`OwnerListener` 只接收成为/卸任通知。短时双 owner 安全由持久化旧值校验和事务回滚兜底，测试 `test_2_owner_for_a_short_time` 覆盖这一点。

## 与 Go 版本的对应关系

主要结构与 `pkg/ddl/notifier/subscribe.go` 对齐：相同的 64 位 HandlerID 契约、启动时位图、周期轮询、每轮失败 handler 跳过后续事件、悲观事务、旧 flag 更新、全部完成后删除，以及测试模式零 handler 不删除。

已验证的差异包括：

- Go handler 接收 `context.Context` 和 `sessionctx.Context`；Rust handler 接收自有 `Session` 克隆，没有取消 context。
- Go `ErrNotReadyRetryLater` 是包级 error 值；Rust 用函数构造枚举变体。
- Go 注册表是普通 map 且明确不并发安全；Rust 用 `Mutex<BTreeMap>`，执行顺序因此按 ID 确定，但启动后注册的语义风险仍存在。
- Go ticker 与 context 负责退出；Rust 使用 park/unpark、原子停止标志和 join。
- Go 在 handler 超过五秒时记录慢日志，并为循环设置内部请求来源、日志类别和 tracing；Rust只保留阈值常量，没有这些可观测性接线。
- Go 后台启动包装 panic recovery；Rust 线程 panic 不恢复，`Stop` 忽略 join 错误。
- Go 生产接线已经由 Domain、stats owner 和统计 handler 完成；当前 Rust 搜索没有相应的 `NewDDLNotifier` 生产构造调用。
- Rust 增加了公开 `ProcessEvents` 与 `Errors`，便于同步驱动及断言累计错误；Go 主要通过日志和包内测试观察。

这些差异应被视为当前移植状态，不能为了文档一致性把 Go 的 tracing、慢日志、panic recovery 或生产装配描述成 Rust 已实现。

## 扩展指南

- 新增订阅者时，先分配全局唯一、长期稳定且小于 64 的 `HandlerID`，在 owner 启动前注册；不要复用已写入持久化位图的 ID。同步扩展 `HandlerIDString`，并在 `pkg/ddl/notifier/testkit_test.rs` 增加多 handler、重试和清理断言。
- handler 的数据库副作用必须使用传入的 `Session`，这样才能与 `UpdateProcessed` 同事务提交；网络调用、消息发送等外部副作用需要另行设计幂等键，不能依赖本文件自动提供 exactly-once。
- 修改删除条件、位图格式或注册时机时，要评估已有 `mysql.tidb_ddl_notifier.processed_by_flag` 的兼容性，以及 owner 切换期间旧/新 handler 集合不同的行为。
- 修改分页或顺序逻辑时，保留“同一 handler 遇错后不接收后续事件”的不变量，并同步 `test_basic_pub_sub`、`deliver_order_and_cleanup`、`test_paginated_list`。
- 修改事务流程时，同步 `test_2_owner_for_a_short_time`、`test_begin_twice`、`test_handlers_see_pessimistic_txn_error` 和 `test_commit_failed`；特别要验证 handler 副作用与 flag 更新不能部分提交。
- 若补齐 Rust 生产接线，应在 Domain/owner 层复用单一发布 store、在启动前完成 stats/priority handler 注册，并补充独立集成测试；这属于本文件之外的接线工作。
- 若实现慢 handler 日志、tracing 或 panic recovery，应以 Go 行为为基准，而不是删除保留常量或简化生命周期。
- Rust 测试继续放在独立的 `testkit_test.rs`，不要把测试嵌入 `subscribe.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/ddl/notifier/subscribe.rs` 确认目标有 20 个符号；`node --file ... --offset 1/261` 读取并核对完整 310 行源码；`query` 唯一定位 Rust 的 `process_events`、`process_event_for_handler`，并同时定位 Go/Rust 两个 `NewDDLNotifier`。精确 `callers/callees` 查询在 30 秒内未返回输出，因此调用边又用仓库引用搜索核验。
- Rust 源码：`pkg/ddl/notifier/subscribe.rs`；crate 边界与再导出：`pkg/ddl/notifier/Cargo.toml`、`pkg/ddl/notifier/lib.rs`；事务和存储语义：`pkg/ddl/notifier/store.rs`；发布侧生产使用：`pkg/ddl/persistent_actions.rs` 及相邻 persistent DDL 文件。
- Rust 独立测试：`pkg/ddl/notifier/testkit_test.rs`，覆盖基本重试和顺序、多 handler 清理、事件类型、短时双 owner、分页、悲观事务、handler 错误与 commit 失败。
- Go 对照：`pkg/ddl/notifier/subscribe.go`；生产装配与调用：`pkg/domain/domain.go`、`pkg/statistics/handle/handle.go`、`pkg/statistics/handle/autoanalyze/refresher/refresher.go`；Go 测试：`pkg/ddl/notifier/testkit_test.go`。
- 本任务是纯文档分析，未运行 Cargo。结构验收应确认本文恰有任务要求的 11 个固定二级标题，并人工复核所有“当前已接线/未接线”结论都有上述搜索或文件证据。
