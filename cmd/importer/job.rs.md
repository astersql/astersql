# `cmd/importer/job.rs`

## 文件定位

本文件是 `astersql-cmd-importer` crate 的并发导入调度层，对照 Go 文件 `cmd/importer/job.go`。crate 根 `cmd/importer/lib.rs` 通过 `pub mod job` 暴露本模块；二进制从 `cmd/importer/bin_main.rs` 进入 `cmd/importer/lib.rs::main`，再到 `cmd/importer/main.rs::run_with_args`。后者完成配置、表结构解析、连接创建和 DDL 执行后，在 `main.rs:128-134` 调用 `doProcess`，因此本文件位于“表和连接已就绪”之后、“关闭连接”之前。

`cmd/importer/Cargo.toml` 将该目录定义为 `astersql-cmd-importer` 的库与二进制，且只声明 `toml`、`serde_json` 两个外部依赖；本文件自身仅使用 Rust 标准库线程、同步通道、时间 API，以及 crate 内的 `db`、`parser`、`stubs` 模块。它不是门面或未接线桩：生产主流程和 `parity_test.rs` 都直接调用其 API。不过，数据库句柄 `stubs::DB` 是当前 crate 的记录型兼容实现，而非真实 `database/sql` 驱动，这一点限制了对真实数据库事务行为的推断。

## 核心职责

本文件把 `jobCount` 个无载荷任务分配给若干 worker；每个 worker 将任务按 `batch` 聚合，调用 `genRowDatas` 生成逐行 INSERT SQL，在一个事务中依次执行并提交。全部 worker 完成后，模块打印总任务数、秒级耗时和粗粒度 TPS。

它刻意不负责配置解析、DDL 解析、连接创建/关闭或单行数据的具体生成规则。上游 `run_with_args` 提供 `Arc<table>` 和 `&[DB]`；下游 `db.rs::genRowDatas` 负责生成 SQL，`stubs::DB::Begin`、`Tx::Exec`、`Tx::Commit` 提供事务接口。`doProcess` 返回时 worker 已 join，但连接仍保持打开，随后由 `main.rs::run_with_args` 调用 `closeDBs`。

另一个关键职责是弥合 Go 与 Rust 通道模型差异：Go 的多个 goroutine 可直接竞争同一个 `jobChan`；标准库 `mpsc::Receiver` 不可克隆，所以 Rust 版本增加单独 dispatcher，按轮询把总任务流转发给每个 worker 的私有同步通道。

## 主要符号

- `pub fn addJobs(jobCount: isize, jobChan: mpsc::SyncSender<()>)`：生产恰好 `jobCount` 个 `()` 占位任务。参数按值取得唯一 sender；函数返回时 sender drop，向消费者表达 EOF。发送失败被忽略，意味着下游提前退出时生产者不会再升级错误。
- `pub fn doInsert(table: &table, db: &DB, count: isize)`：生成 `count` 条 INSERT，开启一个事务，逐条 `Exec`，最后 `Commit`。生成、开启、执行、提交任一阶段失败都调用 `stubs::fatal`，其当前实现以 panic 模拟 Go `log.Fatal`。
- `pub fn doJob(table: Arc<table>, db: DB, batch: isize, jobChan: mpsc::Receiver<()>, doneChan: mpsc::SyncSender<()>)`：单 worker 循环。每收到一个任务增加本地计数；计数等于 `batch` 时提交整批并清零；输入关闭后，如仍有余数则提交尾批，然后发送完成信号。
- `pub fn doWait(doneChan: mpsc::Receiver<()>, start_unix: i64, jobCount: isize, workerCount: isize)`：执行 `workerCount` 次接收尝试，然后用 Unix 秒计算耗时及 TPS并打印摘要。它不区分收到完成信号与通道断开；worker panic 的最终传播由后续 join 负责。
- `pub fn doProcess(table: Arc<table>, dbs: &[DB], jobCount: isize, workerCount: isize, batch: isize)`：模块总入口。建立总任务/完成通道、启动生产者、检查递增列、建立 dispatcher 与 worker 私有通道、启动 worker、等待完成、join worker，并恢复第一个 worker panic。
- `fn unix_now() -> i64`：唯一私有函数。取得 `SystemTime` 相对 `UNIX_EPOCH` 的整秒数；系统时间早于 epoch 或转换失败时返回 `0`。

文件没有模块级常量、类型定义、trait、`impl` 或条件编译项。五个业务函数均为公开 API，但生产主链只从 `doProcess` 进入；其余公开函数主要用于内部组合和独立测试。

## 执行流程

1. `main.rs::run_with_args` 在 DDL 成功执行后调用 `doProcess`，传入解析完成的表、按配置创建的 DB 列表和 `JobCount/WorkerCount/Batch`。
2. `doProcess` 先以 `16 * max(workerCount, 1)` 创建总任务同步通道，以 `max(workerCount, 1)` 创建完成通道，并记录秒级开始时间。
3. 生产者线程执行 `addJobs`：发送固定数量的空任务，随后以 sender drop 关闭总任务流。
4. 主线程扫描 `table.columns`；只要任一 `column.incremental` 为真，实际 worker 数强制改为 `1`，避免多个 worker 并发推进列内共享的递增生成状态。
5. 按最终 worker 数创建每个容量为 `16` 的私有通道。dispatcher 从总任务通道读取，每个任务发送给 `fan_txs[i % workerCount]`，形成确定性的轮询分发；总任务关闭后，dispatcher drop 全部私有 sender，使 worker 的 `recv` 循环结束。
6. 主线程按私有 receiver 的枚举下标选择 `dbs[i]`，克隆 `Arc<table>`、`DB` 和完成 sender，并为每个 worker 启动线程。
7. `doJob` 将收到的任务累计成批。完整批和输入结束后的非空尾批分别调用 `doInsert`；例如测试中的 3 个任务、批大小 2 会产生 `2 + 1` 两次事务提交。
8. 主线程 drop 自己持有的完成 sender，`doWait` 等待 worker 信号或通道断开，打印摘要；随后逐个 join worker。若有 worker panic，记录第一个 panic payload，在所有 handle 都已 join 后通过 `resume_unwind` 向调用者传播。

## 数据与状态

任务本身是零大小的 `()`，只表示“生成并插入一行”，不携带预生成 SQL。这样数据生成发生在具体 worker 的 `doInsert` 内；批内 SQL 由 `Vec<String>` 暂存，事务提交后释放。

`table` 用 `Arc<table>` 在线程间共享。`parser.rs::table` 聚合列定义；列内的 `incremental` 标志决定是否把 worker 数降到 1，而实际生成状态通过列中 `Arc<datum>` 等对象共享。`DB` 按值 clone 给 worker；当前 `stubs.rs::DB` 内部是 `Arc<Mutex<DbInner>>`，所以 clone 共享执行记录、begin/commit 计数和失败开关，而不是创建新连接。

每个 worker 只维护局部 `count`，没有全局批计数。总任务通道容量按调用时的原始 worker 数估算，私有通道固定容量 16；完成通道的容量也在递增列扫描前创建，因此“递增列降为单 worker”不会缩小已建立的缓冲区，但不会改变任务数或完成等待次数（后者使用修改后的 worker 数）。

TPS 使用 `jobCount / (now_unix - start_unix)` 的整数除法；不足一秒时保持 `-1`。这是展示统计，不参与提交控制。

## 依赖与调用关系

上游生产调用链为 `bin_main.rs::main → astersql_cmd_importer::main → entry::main → run_with_args → job::doProcess`。RustCodeGraph 对目标文件确认的内部调用边为：

- `doProcess → addJobs`
- `doProcess → doJob`
- `doProcess → doWait`
- `doProcess → unix_now`（开始时间）
- `doJob → doInsert`
- `doWait → unix_now`（结束时间）

源码还给出图索引未解析成精确边的下游接口：`doInsert → db::genRowDatas → genRowData`，以及 `doInsert → DB::Begin → Tx::Exec/Commit`。`genRowDatas` 每次调用 `genRowData` 生成一条 SQL并保留首个错误，不负责事务或批量拼成单条语句。

测试调用者位于 `cmd/importer/parity_test.rs`：它直接使用 `addJobs` 和 `doProcess`。同目录没有 `job_test.rs` 或 Go `job_test.go`；Go 侧最近的 `db_test.go` 只覆盖数据格式辅助函数，任务调度的直接回归证据来自 Rust parity 测试。

## 错误处理与边界

`doInsert` 采用终止式失败策略：`genRowDatas`、`Begin`、任意 `Exec` 或 `Commit` 返回错误都会调用 `fatal`。当前 `fatal` 是 panic，因此发生在 worker 时由 `doProcess` 的 join 捕获并在收齐 handle 后重新抛出；`parity_test.rs::worker_database_failure_does_not_deadlock` 用失败的 `Begin` 和一秒超时验证该路径不会永远卡在完成等待。代码没有显式 rollback；真实事务在异常退出/句柄释放时如何回滚取决于底层实现，当前记录型 `Tx` 也没有 `Rollback` API，不能从本文件声称更强保证。

通道发送和接收结果大多被忽略。其意义是下游提前关闭时尽快收尾，但也不会返回可诊断错误。`doWait` 即便收到 `RecvError` 仍继续固定次数循环；所有完成 sender 都 drop 后后续接收会立即失败，避免 worker panic 造成等待死锁，真正错误稍后由 join 传播。

调用方必须维持隐含前置条件：`dbs.len()` 至少等于最终 `workerCount`，否则 `dbs[i]` 索引 panic；正常工作还要求 `workerCount > 0`、`batch > 0`、`jobCount >= 0`。当前函数没有显式校验这些值。零/负 worker 会创建不了 worker，dispatcher 提前结束，调用可在未导入任务的情况下返回；零/负 batch 不会形成预期的固定批次；负 jobCount 不产生任务。`unix_now` 失败退为 0，极端情况下会得到失真的耗时/TPS。

## 并发与资源生命周期

本文件可能创建三类线程：一个 producer、一个 dispatcher、`workerCount` 个 worker。producer 和 dispatcher 的 `JoinHandle` 没有保存；它们以通道所有权形成退出协议。正常路径上，所有 worker 发出完成信号意味着 dispatcher 已关闭各私有通道且 worker 已清空尾批；随后 worker handle 被显式 join。producer 的 sender drop 是 dispatcher 结束的前提，因此正常返回时任务生产链已闭合。

总任务通道提供背压，producer 最多领先有限任务；dispatcher 到每个 worker 的容量 16 再提供一级背压。轮询分发保证每个成功接收的任务只被发送给一个 worker，但不同 worker 的事务执行顺序没有全局保证。出现 `incremental` 列时单 worker 约束消除了并发生成顺序的不确定性。

`Arc<table>` 让表元数据活到最后一个 worker 结束；每个 `DB` clone 的真实资源语义由 `DB` 实现决定。`doProcess` 不关闭 DB，`parity_test.rs::contract_resource_cleanup` 明确断言调用后连接仍开放，再由 `closeDBs` 关闭。事务对象只在 `doInsert` 调用期间存在；成功时显式 commit，失败时通过 panic 离开。

## 与 Go 版本的对应关系

`cmd/importer/job.go` 提供逐函数对应：`addJobs` 发送后 `close(jobChan)`；`doInsert` 生成 SQL、`Begin`、逐条 `Exec`、`Commit`；`doJob` 完整批与尾批提交；`doWait` 等待 worker 并计算秒级 TPS；`doProcess` 建通道、遇到 incremental 列降为一个 worker、启动任务与 worker 并等待。

Rust 保留了上述业务顺序与 fatal 风格，但存在三项实现差异：

1. Go worker 竞争一个共享 channel；Rust 使用 dispatcher 加 worker 私有 channel，并按轮询分配。因此任务归属更确定，但各 worker 实际完成顺序仍由调度和数据库耗时决定。
2. Go `doWait` 在收齐信号后显式关闭 `doneChan`；Rust 依靠 sender/receiver drop。Rust 还保存 worker handles，并在等待后 join、重新传播第一个 panic，这是防止线程失败被静默吞掉的本地接线。
3. Go 使用 `time.Time` 打印格式化开始/结束时间；Rust 传递和打印 Unix 秒整数。TPS 的秒级整数算法及小于一秒时为 `-1` 的约定保持一致，但日志文本中的时间表示并不完全相同。

此外，Go 版接收真实 `*sql.DB`；本 crate 当前接收 `stubs::DB`。所以 parity 测试能验证调用顺序、提交计数、失败传播和资源所有权，不能替代真实驱动上的隔离性、回滚或吞吐验证。

## 扩展指南

若要调整任务分配或并发度，首要修改点是 `doProcess`，并同步检查：递增列仍只能由单 worker推进、`dbs` 与实际 worker 数一致、所有 sender 最终可 drop、producer/dispatcher 失败不会留下永久等待。改变轮询策略时应新增独立测试，验证任务不丢失、不重复，以及不同 worker 的分配预期；测试应放在现有独立文件 `cmd/importer/parity_test.rs` 或新建独立 `*_test.rs`，不要嵌入 `job.rs`。

若要改变批处理或事务策略，修改 `doJob`/`doInsert`，并至少覆盖整批、尾批、零任务、生成失败、Begin/Exec/Commit 失败。必须先核对 `cmd/importer/job.go` 的对应语义；若有意偏离，应在文档和 parity 测试中明确兼容性影响。批越大，SQL 字符串暂存和事务占用越高；批越小，提交开销越高。

若要返回可恢复错误而非 fatal，需要同时重新设计 worker 返回通道、`doWait` 与 join 的错误聚合，不能只把 `fatal` 改成 `Result`，否则可能出现其他 worker/dispatcher仍阻塞或错误丢失。若接入真实 Rust 数据库客户端，应在上游独立仓库按仓库依赖政策移植并发布 tag，且要补充真实事务 rollback、连接线程安全和取消语义验证，不能把外部依赖复制进本仓库。

配置侧也应增加并验证 `workerCount > 0`、`batch > 0`、`jobCount >= 0` 及 `dbs.len()` 下界，避免让本模块当前的隐式前置条件演化成静默少导数据。任何修改完成后应保持 Rust 源码与独立测试分离，并先运行 `cargo fmt --all`；本次纯文档任务不修改或运行 Rust。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7032 个 Rust 文件；`files --filter cmd/importer` 确认目标、Go 对照、入口及 parity 测试均在索引中。
- RustCodeGraph `node --file cmd/importer/job.rs`：核对文件全部 208 行、五个公开函数、一个私有函数、无条件编译项，并报告该文件被 `cmd/importer/parity_test.rs` 使用。
- RustCodeGraph `query`：分别定位 Rust/Go 的 `addJobs`、`doInsert`、`doJob`、`doWait`、`doProcess`，避免同名节点混淆。
- RustCodeGraph 精确 callee 查询：确认 `doProcess → addJobs/doJob/doWait/unix_now` 与 `doJob → doInsert`；`doWait → unix_now` 由目标源码核对。精确 caller 查询在当前共享索引负载下超时，故上游调用者改由 `main.rs`、`lib.rs` 和 `rg` 的直接引用结果交叉验证，不据此猜测其他调用者。
- 已读生产文件：`cmd/importer/job.rs`、`cmd/importer/main.rs`、`cmd/importer/lib.rs`、`cmd/importer/bin_main.rs`、`cmd/importer/db.rs`、`cmd/importer/parser.rs`、`cmd/importer/stubs.rs`，以及 `cmd/importer/Cargo.toml`。
- 已读 Go 对照：`cmd/importer/job.go` 全文及 `cmd/importer/main.go` 调用点。逐函数流程一致；dispatcher、join/panic 传播与 Unix 秒日志是已明确记录的 Rust 差异。
- 已读测试：`cmd/importer/parity_test.rs`。直接证据包括 `addJobs(3)` 后 drain 得到 3 个任务、3 jobs/batch 2 得到 2 次 commit、`doProcess` 不关闭 DB，以及 Begin 失败不会死锁并向外传播 panic。同目录未发现独立 `job_test.rs`/`job_test.go`。
- 本任务只生成说明文档，按计划不运行 Cargo。交付前使用任务指定命令验证恰有 11 个固定二级标题，并人工复核文档回答文件存在原因、运行链路、安全扩展位置与验证边界。
