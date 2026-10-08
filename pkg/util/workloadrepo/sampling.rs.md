# `pkg/util/workloadrepo/sampling.rs`

## 文件定位

该文件是 `astersql-util-workloadrepo` crate 中主动采样逻辑的实现，crate 入口 `pkg/util/workloadrepo/lib.rs` 以私有模块 `mod sampling` 装入它。文件没有定义新类型，而是为 `pkg/util/workloadrepo/worker.rs` 的 `worker`（在 crate 根以 `WorkloadRepoWorker` 类型别名公开）补充四个公开方法：`samplingTable`、`startSample`、`resetSamplingInterval` 和 `changeSamplingInterval`。

这里的“采样”是把 `repositoryTable.tableType == samplingTable` 的瞬时 INFORMATION_SCHEMA 视图写入 WORKLOAD_SCHEMA 历史表。默认清单由 `worker.rs::defaultWorkloadTables` 提供，当前包含 `PROCESSLIST`、`DATA_LOCK_WAITS`、`TIDB_TRX`、`MEMORY_USAGE` 和 `DEADLOCKS`。与 Go 实现不同，Rust 的 `worker.rs::startRepository` 当前只完成建表、就绪检查和实例 ID 初始化，没有把 `startSample` 接入常驻调度主链；仓库内 Rust 调用点目前位于 `sampling_test.rs` 和 `worker_test.rs`。

## 核心职责

- `Worker::samplingTable` 完成单张表的一次 `INSERT ... SELECT`：需要时通过 `table.rs::buildInsertQuery` 惰性生成并缓存 SQL，再通过 `worker.rs::runQuery` 执行，并绑定当前实例 ID。
- `Worker::startSample` 生成一个稍后执行的一次性闭包。闭包筛选采样表、为每张表创建 scoped thread、等待全部线程结束，并使单表返回的 `Err` 不妨碍其他表被尝试。
- `Worker::changeSamplingInterval` 把系统变量 hook 传入的字符串解析为 `i32`，在值变化时更新 `WorkerState.samplingInterval`；`resetSamplingInterval` 承担实际状态写入。
- 本文件不负责历史表创建、周期定时、任务取消、重试或日志记录。这些能力在 Go 版中部分存在，但不能据此推断 Rust 已支持。

## 主要符号

- `pub fn samplingTable(&self, tableIndex: usize) -> Result<(), String>`：按下标取得可变表配置。越界返回 `"table index out of range"`；`insertStmt` 为空时调用 `buildInsertQuery`；随后克隆 SQL、释放表锁，以 `[Value::String(self.instanceID())]` 为参数执行查询。
- `pub fn startSample<'a>(&'a self) -> impl FnOnce() -> Result<(), String> + 'a`：返回借用当前 worker 的 `FnOnce`。调用闭包才真正筛选和采样；返回值总是 `Ok(())`，除非子线程 panic 或互斥锁中毒导致当前线程 panic。
- `pub fn resetSamplingInterval(&self, newRate: i32)`：持有 `state` 互斥锁，把 `samplingInterval` 直接设为新值；不创建、停止或重置 ticker。
- `pub fn changeSamplingInterval(&self, value: &str) -> Result<(), String>`：使用 `str::parse::<i32>` 解析；解析失败时返回包含 `repositorySamplingInterval` 常量和原值的错误。解析成功后仅在值不同的情况下调用 `resetSamplingInterval`。
- 文件级别只有 `use crate::worker::worker as Worker` 和 `use crate::*` 两项导入；没有模块级常量、trait、新类型或条件编译项。

## 执行流程

一次显式采样轮次从调用 `worker.startSample()` 开始，但这一步只构造闭包；继续调用返回的闭包后才执行以下流程：

1. 锁定 `workloadTables`，遍历并收集所有 `tableType == samplingTable` 的下标，然后立即释放锁。
2. 进入 `std::thread::scope`，每个下标各启动一个 scoped thread，并在该线程调用 `self.samplingTable(index)`。
3. `samplingTable` 再次锁定表列表，校验下标；若 SQL 尚未缓存，就由 `buildInsertQuery` 查询源表列并写入 `repositoryTable.insertStmt`。
4. SQL 被克隆到局部变量后释放列表锁；随后读取缓存的实例 ID，并调用 `runQuery`。生成的采样 SQL含一个实例 ID 占位参数，时间戳由 SQL 中的 `now()` 生成。
5. 父线程逐一 `join` 所有 handle。正常返回（无论子线程返回 `Ok` 还是 `Err`）都继续；子线程 panic 则以 `resume_unwind` 在父线程重新抛出。全部完成后闭包返回 `Ok(())`。

采样间隔更新是另一条独立路径：`changeSamplingInterval` 解析字符串、比较当前状态，必要时调用 `resetSamplingInterval` 写入新值。它不会触发一次采样，也不会改变任何线程或定时器。

## 数据与状态

核心共享状态来自 `worker.rs::worker`：`workloadTables: Mutex<Vec<repositoryTable>>` 保存表配置和缓存 SQL，`state: Mutex<WorkerState>` 保存 `instanceID` 与 `samplingInterval`，`backend: Arc<dyn RepositoryBackend>` 提供列发现、实例 ID 和 SQL 执行能力。

`samplingTable` 对 `insertStmt` 采用惰性缓存：首次使用时根据源表列定义生成，后续采样直接复用。它在锁内生成并克隆字符串、锁外执行后端调用，避免慢 SQL 执行长期占用表列表锁。实例 ID 来自 `Worker::instanceID()` 的状态快照；正常启动时由 `worker.rs::readInstanceID` 在 `startRepository` 中填充。

`startSample` 先保存下标而不是保存表引用，因此线程间不共享可变表引用；不过若其他代码在筛选后改变表向量的长度或顺序，下标可能失效或指向不同表，届时由 `samplingTable` 的越界检查或实际新位置决定结果。当前代码没有在一轮采样期间冻结表清单的不变量。

## 依赖与调用关系

上游方面，RustCodeGraph 能读取本文件，但未把 Rust `impl Worker` 方法解析成可查询符号；用仓库调用点搜索核对后，`startSample` 的直接 Rust 调用者只有 `sampling_test.rs` 与 `worker_test.rs`，`changeSamplingInterval` 也只出现在这些测试中，`samplingTable` 则由 `startSample` 内部调用。`lib.rs` 装入本模块，但 `worker.rs::startRepository` 和 `start` 都未调用采样入口。因此当前 Rust 生产主链尚无持续采样接线。

下游方面，`samplingTable` 调用 `table.rs::buildInsertQuery`、`Worker::instanceID` 和 `worker.rs::runQuery`；后者直接委托 `RepositoryBackend::execute`。`buildInsertQuery` 还依赖 `RepositoryBackend::source_columns`，并写入 `repositoryTable.insertStmt`。`startSample` 依赖 `std::thread::scope` 和两个 `Mutex`；间隔更新只依赖标准库整数解析与 `WorkerState`。

`pkg/util/workloadrepo/Cargo.toml` 声明该 crate 的库入口为 `lib.rs`、Go 对照包为 `pkg/util/workloadrepo`，唯一直接外部依赖是 `chrono`；本文件本身不直接使用 `chrono`，其执行依赖均来自标准库和 crate 内部接口。

## 错误处理与边界

`samplingTable` 会传播三类可恢复错误：表下标越界、`buildInsertQuery` 的列发现/表类型错误，以及 `runQuery` 的后端执行错误。SQL 生成失败时不会写入有效缓存；执行失败时已经生成的 `insertStmt` 会保留，后续显式采样仍会复用它。本路径使用 `runQuery` 而不是 `execRetry`，所以单表执行没有本地重试。

`startSample` 有意忽略每个子线程正常返回的 `Result`：`handle.join()` 的 `Ok(Err(_))` 与 `Ok(Ok(()))` 都匹配 `Ok(_)`。这保证失败表不阻断同轮其他表，但也意味着闭包最终的 `Ok(())` 不能证明所有表采样成功，而且 Rust 当前没有对应的错误日志。子线程 panic 则不会被吞掉；它被重新抛出。所有 `.lock().unwrap()` 在互斥锁中毒时同样会 panic。

`changeSamplingInterval` 只校验能否解析为 `i32`，不检查 0、负数或 Go 系统变量层的 0..=600 范围；`sampling_test.rs::change_sampling_interval_matches_go_hook_contract` 明确验证 `-1` 和 `601` 均可由 hook 接受。超过 `i32` 表示范围或非整数文本会返回错误，且保留原状态。由于 Rust 当前没有 ticker，间隔变化只修改可查询状态，不会改变实际调度频率。

## 并发与资源生命周期

每轮 `startSample` 使用 scoped threads：子线程可以安全借用 `&Worker`，并保证在 `std::thread::scope` 返回前全部完成，不会形成脱离 worker 生命周期的后台任务。线程数等于该轮筛选出的采样表数，没有线程池或并发上限；扩展大量采样表时应评估线程创建开销和后端并发压力。

表锁的生命周期限定在下标筛选、SQL 生成和缓存读取阶段，后端 SQL 执行发生在锁外。不同表的 `samplingTable` 调用仍会短暂串行竞争同一个 `workloadTables` 锁，但其查询执行并行。`state` 锁用于读取实例 ID或采样间隔；`changeSamplingInterval` 的“比较”和“写入”分属两次加锁，在并发配置更新时不是单个原子读改写事务。

与 Go 版不同，Rust 文件没有 `time::Ticker`、`context` 取消、session pool 借还、常驻循环或 wait group。闭包只运行一轮，资源在闭包返回时全部回收；停止仓库也没有需要在本文件中取消的采样任务。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/workloadrepo/sampling.go`。两版都按 `samplingTable` 类型过滤表、惰性构建 `INSERT ... SELECT`、绑定实例 ID、并发处理各表，并让单表失败不阻断其他表。Rust 的 `table.rs::buildInsertQuery` 延续了采样表 SQL 使用 `now(), %?` 的核心语义。

差异必须视为当前迁移状态，而非等价实现：

- Go `startSample` 会先重置 ticker，随后循环等待 ticker 或 context 取消；Rust `startSample` 只返回执行一次采样的闭包。
- Go `startRepository` 会启动采样、快照和 housekeeper goroutine；Rust `startRepository` 没有调用 `startSample`，所以当前生产启动不会自动采样。
- Go `resetSamplingInterval(0)` 停止 ticker，非零值重置周期；Rust 同名方法只写状态。
- Go `changeSamplingInterval` 在 ticker 已存在时立即应用新周期，并可通过 failpoint 制造解析错误；Rust 无 ticker 和该 failpoint。两版都把范围限制留给上层系统变量定义，而 hook 本身只做整数解析。
- Go 单表失败会写日志，且会从 session pool 获取/归还会话；Rust 使用抽象 `RepositoryBackend`，单表错误在聚合闭包中被静默忽略。

Go 的持续调度接线证据位于 `worker.go::startRepository`，配置范围证据位于 `worker.go::init` 注册的 `repositorySamplingInterval`（最小 0、最大 600）。

## 扩展指南

若新增采样表，优先修改 `worker.rs::defaultWorkloadTables`，将其标记为 `samplingTable`，并确认 `table.rs::buildInsertQuery` 能根据源表列生成兼容 SQL；应在独立的 `sampling_test.rs` 或 `worker_test.rs` 增加过滤、参数绑定和失败隔离测试，不要把测试写进生产文件。

若要补齐 Go 版的周期调度，接入点应是 `worker.rs::startRepository/start/stop` 与本文件的 `startSample/resetSamplingInterval/changeSamplingInterval`。设计必须同时定义定时器所有权、取消与 join 生命周期、0 间隔语义、并发更新规则及 panic/错误可观测性，不能仅循环调用现有闭包。还应保持 Go 的系统变量范围校验与 hook 解析职责分离。

若改变错误策略，需要特别评估“尝试所有表”的既有不变量：可以收集并汇报各表错误，但不应因首个错误跳过剩余表。若允许运行时修改 `workloadTables`，应消除“先存下标、后重新索引”造成的竞态语义，例如为一轮采样建立稳定任务快照。性能变更需关注每表一线程、SQL 缓存失效、后端并发度和共享锁竞争。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/util/workloadrepo` 列出目标源、Go 对照、模块入口和独立测试；`node --file pkg/util/workloadrepo/sampling.rs --offset 1 --limit 260` 返回目标文件完整 86 行；同样读取了 `sampling_test.rs` 完整 149 行。精确 `query samplingTable/startSample/resetSamplingInterval/changeSamplingInterval --kind method` 只返回 Go 符号，故 Rust 上游调用关系由 `rg` 调用点搜索补齐，并在正文明确索引限制。
- 生产源码：`pkg/util/workloadrepo/sampling.rs`（四个方法及线程/锁逻辑）、`table.rs::buildInsertQuery`（SQL 生成和缓存）、`worker.rs::{RepositoryBackend, WorkerState, worker, defaultWorkloadTables, runQuery, startRepository}`（状态、后端与启动链）、`const.rs::repositorySamplingInterval`、`lib.rs::mod sampling`。
- crate 声明：`pkg/util/workloadrepo/Cargo.toml` 的包名、`lib.rs` 入口、`chrono` 依赖和 `package.metadata.porting.go-package`。
- Rust 独立测试：`sampling_test.rs::change_sampling_interval_matches_go_hook_contract`、`sampling_test.rs::sampling_round_attempts_all_sampling_tables_and_ignores_table_errors`；`worker_test.rs::{TestMultipleWorker, TestSamplingTimingWorker, TestSettingSQLVariables}`。
- Go 对照与测试：`sampling.go::{samplingTable, startSample, resetSamplingInterval, changeSamplingInterval}`、`worker.go::{init, initializeWorker, startRepository}`、`table.go::buildInsertQuery`，以及 `worker_test.go` 中的采样间隔、启动和恢复相关用例。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务文件指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复查了当前接线、迁移差异、错误边界和扩展入口均有上述源码或测试依据。
