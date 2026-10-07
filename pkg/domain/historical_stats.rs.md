# `pkg/domain/historical_stats.rs`

## 文件定位

本文件属于 `astersql-domain` crate；crate 根在 `pkg/domain/lib.rs`，通过 `pub mod historical_stats` 公开该模块。它位于一次统计分析完成后的历史快照写入链上：`Domain` 持有一个 `HistoricalStatsWorker`，由 `Domain::new_with_storage_handle` 创建，再由 `Domain::analyze_stats_table` 使用（`pkg/domain/domain.rs:901, 1462-1467, 5210-5243`）。

文件名和顶部迁移注释沿用 Go 的“异步 dump worker”概念，但当前可执行 Rust 实现是一个小型有界任务队列和存储后端抽象；真正的 Domain 后端适配、统计句柄以及调用编排都在 `pkg/domain/domain.rs`。文件第 23—112 行的大段注释是 Go 实现的迁移对照稿，不是会参与编译的 Rust API。

## 核心职责

- 用 `HistoricalStatsStore` 隔离元数据存在性检查与历史统计持久化，使 worker 不依赖具体统计句柄类型（`historical_stats.rs:115-121`）。
- 用标准库同步有界通道保存待处理的物理表 ID，并提供非阻塞投递和非阻塞取出（`historical_stats.rs:123-149, 161-168`）。
- 在持久化前再次检查表是否存在；不存在时正常跳过，存在时把实际 dump 委托给 store（`historical_stats.rs:151-159`）。

本文件不负责判断全局历史统计开关、启动后台线程、选择分区或序列化 JSON。当前 Rust 调用方在 `Domain::analyze_stats_table` 中先检查 `Handle::historical_enabled()`，而具体 dump 与产物完整性检查由 `DomainHistoricalStatsStore` 完成（`pkg/domain/domain.rs:1085-1123, 5197-5243`）。

## 主要符号

- `pub trait HistoricalStatsStore: Send + Sync`：worker 的后端契约。`table_exists(table_id) -> Result<bool, String>` 区分“表不存在”与“检查失败”；`dump_historical_stats(table_id) -> Result<(), String>` 执行写入。`Send + Sync` 允许其通过 `Arc<dyn HistoricalStatsStore>` 跨线程共享。
- `pub struct HistoricalStatsWorker`：包含私有 `SyncSender<i64>`、由 `Mutex` 包装的 `Receiver<i64>` 和共享的 store。字段私有意味着外部只能通过方法维持队列和持久化不变量。
- `HistoricalStatsWorker::new(store, capacity)`：建立 `sync_channel(capacity.max(1))`；传入零容量也会变成容量 1，因而不会创建 rendezvous channel（`historical_stats.rs:134-143`）。
- `send_table_to_dump_historical_stats(&self, table_id) -> bool`：调用 `try_send`；成功返回 `true`，队列满或接收端断开均返回 `false`（`historical_stats.rs:145-149`）。
- `dump_historical_stats(&self, table_id) -> Result<bool, String>`：表不存在返回 `Ok(false)`，成功 dump 返回 `Ok(true)`，后端任一步骤失败原样返回 `Err(String)`（`historical_stats.rs:151-159`）。
- `get_one_historical_stats_table(&self) -> Option<i64>`：加接收端互斥锁后调用 `try_recv`；有值返回 `Some(id)`，队列空或通道断开返回 `None`（`historical_stats.rs:161-168`）。虽然注释称“仅测试使用”，当前生产路径 `Domain::analyze_stats_table` 也调用它。

## 执行流程

当前 Rust 主链如下：

1. `Domain::new_with_storage_handle` 创建共享 `stats_handle`，用它构造 `DomainHistoricalStatsStore`，再以容量 32 创建 worker（`pkg/domain/domain.rs:1452-1467`）。
2. `Domain::analyze_stats_table` 在统计句柄锁内完成 analyze，取得版本、历史统计开关和统计画像；随后持久化当前 meta/直方图（`pkg/domain/domain.rs:5197-5220`）。
3. 若 `historical_enabled` 为假，调用方直接返回 analyze 版本，worker 完全不参与（`pkg/domain/domain.rs:5221-5223`）。
4. 若开启，调用 `send_table_to_dump_historical_stats`。队列不能接收时，调用方把 `false` 转成包含 table ID 的 `DomainError::Stats`（`pkg/domain/domain.rs:5224-5231`）。
5. 同一调用栈立即调用 `get_one_historical_stats_table`；若没有取到任务，则报 `historical stats task disappeared`（`pkg/domain/domain.rs:5232-5236`）。
6. 调用 worker 的 `dump_historical_stats`。worker 先通过 store 检查 table ID 是否仍在统计元数据中；若存在，再执行 store dump（`historical_stats.rs:152-158`）。
7. `DomainHistoricalStatsStore::dump_historical_stats` 锁住统计句柄，以 `u64::MAX` 作为版本上界执行 dump，并要求历史 JSON block 存在、非空且每块非空（`pkg/domain/domain.rs:1101-1123`）。
8. worker 成功后，调用方还执行 `stats_context().record_historical_stats_to_storage(queued)`，然后返回 analyze 版本（`pkg/domain/domain.rs:5237-5243`）。

因此当前 Rust 路径虽然使用 channel，但消费发生在生产者的同步调用内；本文件自身没有事件循环或后台任务。

## 数据与状态

- 队列元素只有 `i64` table ID，不携带 schema、版本或重试次数；所需统计状态在消费时由 store/统计句柄重新读取。
- `SyncSender` 和 `Receiver` 共享同一有界队列。容量下限为 1；Domain 的实际配置是 32，而 Go `SetupHistoricalStatsWorker` 使用 16（`pkg/domain/domain.go:1787-1792`）。
- `store: Arc<dyn HistoricalStatsStore>` 绑定 worker 的持久化实现及其生命周期。生产实现 `DomainHistoricalStatsStore` 再共享 `Arc<Mutex<Handle<DomainStatsBackend>>>`（`pkg/domain/domain.rs:1085-1090`）。
- 本文件没有全局变量、时间戳、指标或重试状态，也没有条件编译项。Go 的 `enableDumpHistoricalStats`、failpoint 和成功/失败指标没有出现在当前可执行 Rust 文件中。

## 依赖与调用关系

直接依赖全部来自标准库：`Arc` 负责后端共享所有权，`Mutex` 串行化接收操作，`std::sync::mpsc::sync_channel` 提供有界队列。因而 `pkg/domain/Cargo.toml` 不需要为本文件新增第三方依赖；该清单确认 crate 名为 `astersql-domain`，库入口是 `lib.rs`。

RustCodeGraph 将本文件标为 9 个符号，并给出两个使用文件：`pkg/domain/domain.rs` 和 `pkg/session/runtime/ttl_timer.rs`。精确源码检索确认实际 worker 类型、trait 实现及方法调用均位于 `pkg/domain/domain.rs`；`ttl_timer.rs` 对本文件没有可见的符号级直接引用，因此不能据此声称存在 TTL 调用边。

主要已核实调用边是：

- `Domain::new_with_storage_handle` → `HistoricalStatsWorker::new`。
- `Domain::analyze_stats_table` → `send_table_to_dump_historical_stats` → `get_one_historical_stats_table` → `dump_historical_stats`。
- `HistoricalStatsWorker::dump_historical_stats` → `HistoricalStatsStore::table_exists` → `HistoricalStatsStore::dump_historical_stats`。
- 动态分派的生产实现是 `DomainHistoricalStatsStore`，其下游是 `Handle<DomainStatsBackend>` 的统计元数据、dump 和历史 JSON 查询接口。

## 错误处理与边界

- 构造时使用 `capacity.max(1)`，所以容量零不会 panic，也不表示“无缓冲”。
- 非阻塞投递把 `TrySendError::Full` 和 `TrySendError::Disconnected` 都压缩为 `false`；需要区分这两类故障时必须扩展返回类型。
- 非阻塞读取把 `TryRecvError::Empty` 和 `TryRecvError::Disconnected` 都压缩为 `None`；当前调用方只能统一报告“任务消失”。
- 接收端互斥锁中毒会由 `expect("historical stats queue poisoned")` 触发 panic，不会转换成 `Result`（`historical_stats.rs:163-166`）。生产 store 内统计句柄锁中毒也采用相同的 panic 策略（`pkg/domain/domain.rs:1092-1095, 1104-1107`）。
- `table_exists == false` 是正常的竞态结果，返回 `Ok(false)` 且不调用 dump；调用方目前用 `?` 接受这个布尔值而不检查，因此 analyze 仍可继续执行后续记录步骤（`pkg/domain/domain.rs:5237-5241`）。
- store 错误字符串不在本文件补充上下文；生产 store 则明确报告“没有已分析统计”“dump 未持久化”或“JSON block 为空”。

## 并发与资源生命周期

`HistoricalStatsWorker` 的公开方法只借用 `&self`。sender 本身支持并发投递；receiver 不是 `Sync`，因此通过 `Mutex` 保证同时只有一个消费者执行 `try_recv`。store 的 `Send + Sync` 约束和 `Arc` 允许 worker 被共享，但本文件没有显式实现线程创建、关闭或 join。

发送端和接收端由同一个 worker 持有，正常存活期间通道不会因某一侧提前 drop 而断开；两端随 worker 一起释放。当前 Domain 把 worker 作为值字段持有，生命周期与 Domain 一致（`pkg/domain/domain.rs:901`）。当前生产调用同步入队后立即出队，因此容量主要是防御性边界；若未来恢复独立消费者，队列满时仍是丢弃/失败而不是阻塞或自动重试。

## 与 Go 版本的对应关系

`pkg/domain/historical_stats.go` 是直接语义参照：两端都以有界 channel 接收 table ID，发送和测试取出都不阻塞，并在 dump 前确认目标对象可用。

已确认的差异如下：

- Go worker 保存 `sessionctx.Context`，在 `DumpHistoricalStats` 中经 Domain/InfoSchema 解析普通表或分区、数据库名和表元数据；Rust worker改为 `HistoricalStatsStore`，当前生产实现只按统计 meta 的 `physical_id` 判断存在，再由统计 handle dump。
- Go 发送前受原子开关和 `sendHistoricalStats` failpoint 控制，队列满会记录 warning；Rust 方法无开关、failpoint 或日志，只返回布尔值，开关检查由调用方负责。
- Go dump 再次读取 `tidb_enable_historical_stats`，分别记录成功/失败指标，并返回带库表名的上下文错误；Rust worker 不做二次开关检查和指标更新，错误是 `String`。
- Go `GetOneHistoricalStatsTable` 用 `-1` 表示空队列；Rust 使用 `Option<i64>`，避免哨兵值。
- Go 由 `StartHistoricalStatsWorker` 启动受 Domain wait group/exit channel 管理的 goroutine；Rust 当前没有等价后台循环，而是在 `analyze_stats_table` 中同步消费。
- Go 队列容量为 16；Rust Domain 构造容量为 32。

因此顶部第 21—112 行注释描述的是更完整的 Go 迁移目标，不能当成当前 Rust 已实现行为。

## 扩展指南

- 若要恢复真正异步 worker，应在 Domain 生命周期层增加明确的启动、退出、drain/join 逻辑；不要把线程生命周期藏进本文件的单次方法。同步更新 `Domain::analyze_stats_table`，避免生产者再立即消费同一队列。
- 若需要可靠投递或重试，应先把 `bool` 改成能区分 full/disconnected 的结果，并定义重复 table ID、背压、关机期间任务的处理策略；不要直接换成阻塞 `send`，否则 analyze 路径可能被卡住。
- 若补齐 Go 的表/分区语义，应在 `HistoricalStatsStore` 或 Domain 适配层表达所需元数据，而不是让通用 worker反向依赖 InfoSchema。需要同步验证普通表、物理分区、已删除表和无统计表。
- 若增加错误上下文或指标，最合适的边界分别是 worker 的两个 store 调用和 `DomainHistoricalStatsStore::dump_historical_stats`；应保留“表不存在是可跳过结果”与“后端查询失败”的区别。
- 测试必须放在独立 Rust 测试文件，不能内嵌到 `historical_stats.rs`。可新增同目录 `historical_stats_test.rs` 并从测试装配入口引用，覆盖最小容量、满队列、FIFO、空队列、表不存在、检查失败、dump 失败和锁中毒策略；端到端语义继续同步 `pkg/executor/historical_stats_test.rs`。Go 对照测试为 `pkg/executor/historical_stats_test.go`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/domain/historical_stats.rs` 显示目标文件有 9 个符号；`node --file ... --offset 1 --limit 400` 返回完整 169 行源码并报告两个文件级使用者；`query` 精确确认 `HistoricalStatsStore`、`HistoricalStatsWorker` 及三个公开方法。图的 `callers/callees` 查询未在限定时间内产生结果，所以调用边又以源码引用核实，没有把未返回的图边当成证据。
- 读过的生产文件：`pkg/domain/historical_stats.rs`、`pkg/domain/domain.rs` 的 store 实现/构造/分析调用链、`pkg/domain/lib.rs` 的模块公开声明、`pkg/domain/Cargo.toml` 的 crate 边界，以及 Go 对照 `pkg/domain/historical_stats.go`、`pkg/domain/domain.go`。
- 读过的独立测试：`pkg/executor/historical_stats_test.rs` 与 `pkg/executor/historical_stats_test.go`。Rust 测试验证开关关闭不记录、开启后 analyze 产生可解码 dump、分区静态/动态模式、历史回退、表删除/过期 GC 等端到端语义；它没有直接隔离测试本文件的队列满、断连或 store 错误分支。
- 本任务是纯文档分析，按计划不运行 Cargo；完成判据是上述事实交叉核对与任务指定的 11 章节结构检查。
