# `pkg/util/workloadrepo/snapshot.rs`

## 文件定位

本文件属于 `astersql-util-workloadrepo` crate 的快照协调层。crate 入口 `pkg/util/workloadrepo/lib.rs` 以私有模块 `snapshot` 装入本文件，再通过 `pub use snapshot::*` 暴露自由函数；文件中的方法则扩展 `pkg/util/workloadrepo/worker.rs` 定义的 `worker`（在本文件中别名为 `Worker`）。`pkg/util/workloadrepo/Cargo.toml` 指定 crate 根为 `lib.rs`，唯一直接外部依赖是 `chrono`；本文件自身只使用 crate 内的后端抽象、表描述、常量和 SQL 构造函数。

它处在“生成全局快照号”和“把一组 INFORMATION_SCHEMA 表写入 WORKLOAD_SCHEMA 历史表”之间：`takeSnapshot` 分配并发布快照号，`startSnapshot` 按已经给定的快照号执行一次落库。当前 Rust 版本不是 Go 版本中的常驻监听任务；`startSnapshot(snapID)` 返回一个只执行一次的 `FnOnce` 闭包，调用者必须显式调用该闭包。

## 核心职责

1. 通过 `RepositoryBackend::{kv_get, kv_create, kv_cas}` 封装 snap ID 键的读取、首次创建和 CAS 更新（`Worker::{etcdGet, etcdCreate, etcdCAS}`）。
2. 在 etcd/KV 键缺失时，以 `HIST_SNAPSHOTS` 中的最大 `SNAP_ID` 为恢复基线（`queryMaxSnapID`、`Worker::getSnapID`）。
3. 在发布新 snap ID 前 upsert 元数据开始时间，最多按 `snapshotRetries` 重试竞争流程（`upsertHistSnapshot`、`Worker::takeSnapshot`）。
4. 为每张 `snapshotTable` 惰性生成并缓存 `INSERT ... SELECT`，绑定同一个 snap ID 和当前实例 ID，并行执行各表快照（`Worker::snapshotTable`、`Worker::startSnapshot`）。
5. 快照结束后写回 `END_TIME`，并把所有表级错误按换行拼入 `ERROR`（`Worker::updateHistSnapshot`）。
6. 解析并更新 Worker 内存中的快照间隔配置（`Worker::{changeSnapshotInterval, resetSnapshotInterval}`）。当前 Rust Worker 没有 ticker 字段，因此这里只改状态，不负责调度。

## 主要符号

- `Worker::etcdCreate(&self, key, value) -> Result<(), String>`：调用 `kv_create`；后端返回 `false` 表示键已存在或条件创建失败，转换为带键值上下文的错误。
- `Worker::etcdGet(&self, key) -> Result<String, String>`：调用 `kv_get`，将不存在的 `None` 规范为 `""`。
- `Worker::etcdCAS(&self, key, old, new) -> Result<(), String>`：只有后端确认 CAS 成功才返回 `Ok`，条件失败时保留旧值、新值上下文。
- `queryMaxSnapID(backend) -> Result<u64, String>`：执行 `SELECT MAX(SNAP_ID)`；首行首列为 `UInt` 时返回该值，为 `Null` 时返回 0，其它结果形状或类型报错。
- `Worker::getSnapID() -> Result<u64, String>`：空字符串映射为 `errKeyNotFound`，非空值必须可解析为 `u64`。
- `upsertHistSnapshot(backend, snapID)`：为 snap ID 写入/刷新 `BEGIN_TIME`。重复 ID 使用 `ON DUPLICATE KEY UPDATE`，以容忍 CAS 竞争前多个候选者写入同一元数据行。
- `Worker::updateHistSnapshot(snapID, errors)`：设置 `END_TIME`；非空错误列表以 `"\n"` 连接并通过两个占位符交给 `COALESCE(CONCAT(ERROR, ?), ERROR, ?)`，空列表绑定两个 `NULL`。
- `Worker::snapshotTable(snapID, tableIndex)`：按索引取得表描述；首次执行时调用 `buildInsertQuery` 填充 `repositoryTable.insertStmt`，随后释放表清单锁，再执行 SQL。
- `Worker::takeSnapshot() -> Result<u64, String>`：最多尝试 `snapshotRetries` 次读取基线、写元数据、创建/CAS KV；成功返回新 ID。
- `Worker::startSnapshot(snapID) -> impl FnOnce() -> Result<(), String>`：快照执行工厂；筛出所有 `tableType == snapshotTable` 的索引，每表启动一个 scoped thread，收集业务错误，最后更新元数据。
- `Worker::resetSnapshotInterval(newRate)` 与 `changeSnapshotInterval(value)`：分别直接写入 `WorkerState.snapshotInterval`，以及解析字符串并在值变化时调用前者。

本文件没有模块级常量、类型、trait 或条件编译项；用到的 `snapIDKey`、`snapshotRetries`、表名和系统变量名来自 `const.rs`，`RepositoryBackend`、`Value`、`repositoryTable` 与 `worker` 来自 `worker.rs`。

## 执行流程

手动分配快照号的路径是：`worker.rs::takeSnapshot` 从全局 Worker 槽位取出已启用实例，然后调用本文件的 `Worker::takeSnapshot`。后者在每轮重试中先调用 `getSnapID`；若错误精确等于 `errKeyNotFound`，则调用 `queryMaxSnapID` 从 SQL 历史恢复，并标记应执行条件创建，否则使用现存 KV 值。接着以 `snapID + 1` 调用 `upsertHistSnapshot`。最后，恢复路径调用 `etcdCreate`，正常路径调用 `etcdCAS(old, new)`；只有 KV 更新成功才把新 ID 返回给调用者。

一次落库的路径是：调用者取得 snap ID 后调用 `startSnapshot(snapID)` 并执行返回的闭包。闭包先在持锁期间扫描 `workloadTables`，仅保存快照类表的索引；之后用 `std::thread::scope` 为每个索引并行调用 `snapshotTable`。单表方法在短暂持锁期间校验索引、惰性生成并复制 SQL 和目标表名，解锁后通过 `runQuery` 执行。所有线程结束后，闭包收集每个 `Result` 中的错误字符串，并无条件调用 `updateHistSnapshot` 写结束状态。

间隔变更路径独立于上述两条路径：`changeSnapshotInterval` 只接受能解析为 `i32` 的字符串；值变化时更新 `WorkerState`。Go 的系统变量层负责范围约束，Rust 方法本身故意不钳制范围。

## 数据与状态

- 全局协调状态是后端 KV 的 `snapIDKey`（`/tidb/workloadrepo/snap_id`）。其十进制字符串值代表最近已发布的快照号；缺键与空字符串在本文件中等价。
- SQL 元数据状态位于 ``WORKLOAD_SCHEMA`.`HIST_SNAPSHOTS``。`takeSnapshot` 先写 `BEGIN_TIME`，`startSnapshot` 完成后写 `END_TIME` 和可选 `ERROR`。因此 CAS 最终失败时可能遗留已 upsert 的元数据行；源码和 Go 注释把双 owner 极端竞争下的这一结果视为可接受。
- 每张 `repositoryTable` 的 `insertStmt` 是惰性缓存。`snapshotTable` 在 `workloadTables: Mutex<Vec<_>>` 内只生成一次并克隆出来，SQL 执行不占用该锁。
- `WorkerState.snapshotInterval` 是进程内配置状态。Rust 版本没有 `snapshotTicker`，所以修改该字段不会自动产生周期快照。
- `snapshotTable` 的 SQL 参数固定为 `[Value::UInt(snapID), Value::String(instanceID)]`；`buildInsertQuery` 决定其与 `%?` 占位符及源表列的对应关系。

## 依赖与调用关系

上游方面，RustCodeGraph 将 `snapshot.rs::startSnapshot` 标为 `snapshotTable` 的调用者，并将 Go `worker::startSnapshot` 标为 Go `takeSnapshot`/`snapshotTable` 的协调者。对 Rust 仓库的直接搜索进一步确认：公开的全局入口 `pkg/util/workloadrepo/worker.rs::takeSnapshot` 调用 `Worker::takeSnapshot`；`worker_test.rs` 直接执行 `startSnapshot(snapID)()`；生产 Rust 代码中没有调用 `startSnapshot` 的常驻调度接线。`lib.rs` 再导出自由函数 `takeSnapshot`，但 Worker 方法只通过类型自身可达。

下游方面，`snapshotTable` 调用 `pkg/util/workloadrepo/table.rs::buildInsertQuery` 和 `worker.rs::runQuery`；`queryMaxSnapID`、`upsertHistSnapshot`、`updateHistSnapshot` 也通过 `runQuery` 进入 `RepositoryBackend::execute`。KV 方法进入同一 trait 的 `kv_create`、`kv_get`、`kv_cas`。实例标识由 `Worker::instanceID` 从受锁的 `WorkerState` 克隆取得。

RustCodeGraph 的精确结果包含 `startSnapshot -> snapshotTable`、`snapshotTable -> buildInsertQuery/runQuery`，但常见符号名解析也混入无关 Go 节点；因此上述 Rust 生产接线范围同时以 `rg` 的全仓直接引用核对，未把图中的噪声当成真实调用边。

## 错误处理与边界

- 所有可恢复业务错误使用 `Result<_, String>` 向上传播，并在 KV 条件失败、生成 SQL、执行目标表插入等边界补充操作上下文。
- `queryMaxSnapID` 只接受 `UInt` 或 `Null`；没有首行、没有首列或其它值类型统一返回 `no rows returned when querying max snap id`。该文案覆盖了“类型不符”，名称比实际条件窄。
- `getSnapID` 区分缺键和非法整数：前者触发 SQL 恢复，后者作为一般读取错误参与重试，不会错误地走 create 路径。
- `takeSnapshot` 每个阶段失败都会继续下一轮，最终返回最后一次错误。由于循环至少运行 `snapshotRetries`（当前为 5）次，正常配置下 `last` 会被设置；若常量未来改为 0，当前实现将返回空错误字符串，这是扩展时应防守的边界。
- `snapshotTable` 对越界索引返回 `table index out of range`；表定义生成和执行错误都包含 `destTable`，便于定位失败历史表。
- `startSnapshot` 会记录各表返回的 `Err` 并继续收敛，但线程 panic 通过 `handle.join().unwrap()` 再次 panic，不会转换成元数据错误；`Mutex::lock().unwrap()` 也会在锁中毒时 panic。
- 即使某些表失败，只要 `updateHistSnapshot` 成功，闭包整体返回 `Ok(())`；表级失败通过数据库 `ERROR` 列表达，而不是作为闭包错误返回。若元数据更新失败，才返回 `Err`。
- `changeSnapshotInterval` 只校验 `i32` 语法，不校验正数或 Go 系统变量的 900..=7200 范围；调用它的更高层必须先完成范围规范化。

## 并发与资源生命周期

KV 的 create/CAS 是跨节点并发控制点，保证竞争者只有一个能发布给定的下一快照号；SQL upsert 发生在 CAS 之前，因此它不是发布成功的判据。最多五次循环为瞬时竞争或后端错误提供有限重试，没有退避。

表清单由 `Mutex<Vec<repositoryTable>>` 保护。`startSnapshot` 先生成稳定索引集合再释放锁；各 scoped thread 调用 `snapshotTable` 时分别短暂重新加锁。生成语句后会把 SQL 和目标表名克隆出来，实际数据库 I/O 在锁外完成，避免串行化所有插入。由于索引集合与执行之间允许其他代码修改向量，安全扩展时不应在快照并发期间删除或重排表项；当前生产代码没有这种动态修改接线。

`std::thread::scope` 保证所有表线程在闭包离开前 join，并允许线程借用 `&Worker`。本文件没有异步 runtime、通道、事务对象或后台线程所有权；每次 `startSnapshot` 调用创建的 OS 线程只活到该次快照结束。后端由 Worker 的 `Arc<dyn RepositoryBackend>` 持有，线程共享借用 Worker，不额外延长 Worker 生命周期。状态锁和表锁均使用阻塞式 `std::sync::Mutex`。

## 与 Go 版本的对应关系

主要算法直接对应 `pkg/util/workloadrepo/snapshot.go`：KV create/get/CAS、缺失 snap ID 时查询 SQL 最大值、先 upsert 后 CAS、按表并行 INSERT、错误合并写回，以及间隔字符串解析的意图均被保留。`snapshot_test.rs` 验证 Rust 错误拼接和目标表上下文；`worker_test.rs::TestRecoverSnapID`、`TestAdminWorkloadRepo`、`TestSnapshotTimingWorker` 验证恢复、全局入口和参数绑定。

当前差异必须视为尚未等价接线，而不是简化后的同等实现：

- Go 的 `startSnapshot(ctx)` 是长期运行的 etcd watch + ticker 循环；watch 收到多个事件时取最后一个 ID，ticker 只由 owner 调 `takeSnapshot`。Rust 的 `startSnapshot(snapID)` 是给定 ID 的一次性执行闭包，不监听 KV、不判断 owner、也不负责周期触发。
- Go 的 KV 操作使用带 `etcdOpTimeout` 的 context；Rust 后端 trait 没有 context/timeout 参数。
- Go 每张表从 session pool 获取独立 session 并归还；Rust 共享 `RepositoryBackend`，资源隔离由后端实现承担。
- Go 的 ticker 在 `resetSnapshotInterval` 中立即 reset；Rust 没有 ticker，只更新状态。Rust 测试 `change_snapshot_interval_matches_go_hook_without_extra_clamping` 仅证明解析和状态赋值。
- Go 用 `errors.Join` 合并错误；Rust 用换行连接字符串，正常错误文本相符，但不保留结构化错误链。
- Go 的启动流程在 `worker.go` 中把 `startSnapshot` 注册为后台 goroutine；Rust 生产代码当前没有对应调用，只有测试显式运行一次性闭包。

## 扩展指南

- 若补齐常驻快照调度，应在 Worker 生命周期层新增明确的任务句柄、取消机制和 owner 判定，再复用 `takeSnapshot` 与一次性落库核心；不要把无限循环塞入现有 `FnOnce` 而不解决 `stop`、重启和资源回收。同步新增独立测试文件中的取消、owner 切换、多个 KV 事件和间隔重置用例。
- 若改变 snap ID 协议，应同时审查 `getSnapID`、`queryMaxSnapID`、`upsertHistSnapshot` 和 create/CAS 顺序，并保持“缺键恢复”“竞争失败重试”“元数据允许重复 upsert”不变量。对应测试应覆盖 CAS 冲突、非法 KV 值、SQL MAX 为 NULL/无行和重试耗尽。
- 若新增快照表或改变列绑定，优先修改 `worker.rs::defaultWorkloadTables` 或表描述，再核对 `table.rs::buildInsertQuery` 的占位符顺序；不要在 `snapshotTable` 中拼接特例 SQL。测试继续放在独立的 `snapshot_test.rs`、`table_test.rs` 或 `worker_test.rs`，不要内嵌到生产文件。
- 若允许运行时修改 `workloadTables`，应把索引任务改成稳定身份或快照副本，避免扫描后重排导致索引指向另一张表。
- 若要让调用者感知部分表失败，需要改变 `startSnapshot` 的返回契约，同时仍保证 `updateHistSnapshot` 被调用；这属于兼容性变化。并行表数当前等于快照表数，表规模扩张时应评估线程上限或有界执行器。
- 外部后端实现必须保证 `kv_create`/`kv_cas` 真正具备原子条件语义；仅返回布尔值但非原子实现会破坏全局唯一 snap ID。

## 验证依据

- 生产源码：`pkg/util/workloadrepo/snapshot.rs`（本文件全部 13 个已索引符号）、`worker.rs`（类型、后端 trait、全局 `takeSnapshot`、默认表）、`table.rs`（`buildInsertQuery`）、`const.rs`（键、重试数、表名和系统变量）、`lib.rs`（模块与再导出）。包内不存在 `doc.go`。
- crate 配置：`pkg/util/workloadrepo/Cargo.toml`，确认 crate 名、`lib.rs` 入口、`chrono` 依赖和 Go 包映射元数据。
- Go 对照：`pkg/util/workloadrepo/snapshot.go` 与 `worker.go`，核对 etcd 协议、ticker/watch、owner、session pool、后台任务接线和系统变量范围职责。
- 独立测试：`pkg/util/workloadrepo/snapshot_test.rs`；`worker_test.rs` 中 `TestAdminWorkloadRepo`、`TestSnapshotTimingWorker`、`TestSettingSQLVariables`、`TestRecoverSnapID`；Go 的 `worker_test.go` 中竞争建表/手动快照、周期快照、停止重启和 snap ID 恢复场景。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件并列出本包 22 个文件；查询得到 `snapshot.rs` 13 个符号、`startSnapshot -> snapshotTable` 和 `snapshotTable -> buildInsertQuery/runQuery`。因未限定语言的同名查询含无关节点，Rust 上游接线另以 `rg` 全仓引用结果核验。
- 未运行 Cargo 或运行时测试：任务为纯文档分析，按计划只执行文档结构检查；行为结论来自上述源码、调用图和已有测试。
