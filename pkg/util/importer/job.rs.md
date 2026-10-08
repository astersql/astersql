# `pkg/util/importer/job.rs`

## 文件定位

`job.rs` 是 `astersql-util-importer` crate 的并发作业调度层，位于“DDL 已解析并执行、数据库连接已建立”之后，负责把配置中的作业数转换为批量事务写入。crate 入口 `pkg/util/importer/lib.rs` 将 `job` 声明为公开模块并重导出其公开项；直接生产调用入口是 `pkg/util/importer/importer.rs::process`，它把解析完成的 `Arc<Table>`、每个 worker 对应的数据库连接、`job_count`、`worker_count` 和 `batch` 传给 `process_jobs`。

该文件不解析 SQL、不创建或关闭连接，也不实现数据库驱动。表结构与生成规则来自 `parser::Table`，INSERT 文本由 `db::generate_row_data_batch` 生成，事务能力通过 `db::Database` / `db::DatabaseTransaction` 注入。`pkg/util/importer/Cargo.toml` 将本目录定义为 `astersql-util-importer` 库；当前普通 `[dependencies]` 为空，解析器等路径依赖只列在永不成立的 `target.'cfg(any())'.dependencies` 下，因此本文所述实现目前依赖 crate 内部模块和标准库抽象，而不是直接绑定外部 SQL 驱动。

## 核心职责

- `process_jobs` 校验并发配置，建立容量为 `16 * worker_count` 的有界 token 通道，启动一个生产者和指定数量的 worker，回收线程并生成 `ProcessReport`。
- 每个成功接收的 `()` token 代表一条待生成、待插入的行；token 本身不携带行数据，实际 INSERT 在 worker 侧调用 `generate_row_data_batch` 时按 `Table` 动态生成。
- `do_job` 为单个 worker 累积 token；达到 `batch` 时提交一批，通道关闭后再提交不足一批的尾部数据。
- `do_insert` 保证一个批次只开启一个事务，逐条执行该批次的 INSERT，然后提交事务。
- 线程 panic、数据生成错误、开启事务/执行/提交错误会转换或透传为 `ImporterError`；只有所有线程和写入均成功时才返回报告。

这些职责对应 `pkg/util/importer/job.rs::{process_jobs,do_job,do_insert,ProcessReport}`，而完整导入前后的连接创建、DDL 执行和连接关闭属于 `pkg/util/importer/importer.rs::process`。

## 主要符号

- `pub struct ProcessReport { jobs, elapsed, transactions_per_second }`：成功结果。`jobs` 是请求的 token 总数，`elapsed` 从通道/线程创建前计时到全部线程回收后，`transactions_per_second` 实际按 `job_count / elapsed.as_secs()` 计算；字段名沿用 TPS 概念，但分子是行作业数而不是已提交事务数。耗时不足整秒时固定为 `-1`。
- `fn do_insert(table: &Table, database: &dyn Database, count: usize)`：内部批次写入函数。先一次性生成 `count` 条 INSERT，再 `begin`，逐条 `transaction.execute`，最后 `commit`。任一步骤的 `ImporterError` 通过 `?` 返回。
- `fn do_job(table: Arc<Table>, database: Arc<dyn Database>, batch, jobs)`：内部 worker 函数。它独占一个数据库连接的 `Arc`，但与其他 worker 共享 `Arc<Mutex<mpsc::Receiver<()>>>`；局部 `count` 只记录本 worker 尚未提交的 token 数。
- `pub fn process_jobs(table, databases, job_count, worker_count, batch)`：本文件唯一公开行为入口。它要求 `worker_count > 0`、`batch > 0` 且 `databases.len() >= worker_count`，并且只使用 `databases` 的前 `worker_count` 个元素。

文件没有 trait、模块级常量、条件编译项或全局可变状态。公开 API 是 `ProcessReport` 与 `process_jobs`；其余函数均为模块私有。

## 执行流程

1. `importer.rs::process` 完成表/索引解析，创建 `worker_count` 个连接并执行 DDL，然后调用 `process_jobs`。
2. `process_jobs` 先检查 worker 数、连接数和批大小。任一约束不满足时立即返回 `ImporterError::InvalidConfig`，不会创建线程。
3. 函数记录 `Instant::now()`，创建容量为 `16 * worker_count` 的 `sync_channel`，并以 `Arc<Mutex<_>>` 包装唯一的标准库 `Receiver`。
4. 生产者线程循环 `job_count` 次发送空 token。若所有接收端已被释放，`send` 返回错误，生产者提前退出；发送端随线程结束而释放，worker 随后从 `recv` 得到断开错误并结束接收循环。
5. 协调者从 `databases.iter().take(worker_count)` 为每个 worker 克隆表、对应连接和共享接收端。worker 每收到一个 token 就增加局部计数；计数恰好达到 `batch` 时调用 `do_insert`，成功后归零。
6. 通道关闭时，`do_job` 跳出循环；如果局部计数大于零，再用同一个连接提交尾批，随后返回。
7. 协调者在等待线程前主动 `drop(receiver)`。这保证 worker 因写入错误提前退出后，协调者不会意外保留最后一个接收端并让满载的有界通道永久阻塞生产者。
8. 协调者逐个 `join` worker，将 panic 映射为 `WorkerPanic`，并保留遍历顺序中遇到的第一个 worker 错误；随后回收生产者。生产者 panic 优先由其 `join` 直接返回，之后再传播已保存的 worker 错误。
9. 全部成功后计算耗时并返回报告。`jobs` 填写原始 `job_count`；少于一整秒时 TPS 为 `-1`，否则为整数除法结果。

## 数据与状态

- `Table` 以 `Arc<Table>` 在 worker 间共享。`job.rs` 只借用它生成 SQL；表内列生成器可能维护唯一值状态，具体同步与取值规则由 `pkg/util/importer/parser.rs::Table`、列数据结构及 `pkg/util/importer/db.rs::generate_row_data_batch` 负责。
- `()` 是无负载的作业 token。总 token 数由 `job_count` 决定，行值延迟到实际处理 token 的 worker 中生成，因此通道内存与行 SQL 长度无关。
- 每个 worker 的 `count` 是线程局部状态。一个批次不会跨 worker 合并，所以总事务数取决于 token 在 worker 间的分配；每个 worker 最多产生一个不足 `batch` 的尾批。
- 每个 worker 使用 `databases` 中不同位置的一个 `Arc<dyn Database>`；额外连接被忽略。`do_insert` 每批创建新事务，事务对象在提交时按 `commit(self: Box<Self>)` 被消费。
- 成功报告不读取真实提交计数，而直接记录请求的 `job_count`。若任何 worker 失败，函数不返回部分完成报告。

## 依赖与调用关系

上游生产链为 `pkg/util/importer/importer.rs::process → process_jobs`。`pkg/util/importer/lib.rs` 同时公开重导出 `process_jobs` 和 `ProcessReport`，因此 crate 使用者也可绕过完整导入流程直接调用，但仓库文本检索到的生产调用点是 `importer.rs`。

下游调用链为：

`process_jobs → thread::spawn / mpsc::sync_channel → do_job → do_insert → generate_row_data_batch → Database::begin → DatabaseTransaction::{execute,commit}`。

错误类型来自 `pkg/util/importer/config.rs::ImporterError`；表元数据来自 `pkg/util/importer/parser.rs::Table`；数据库和事务接口及 SQL 生成函数来自 `pkg/util/importer/db.rs`。标准库提供 `Arc`、`Mutex`、有界 MPSC、线程、`Instant` 与 `Duration`。

RustCodeGraph 索引将 `job.rs` 识别为 11 个符号的 Rust 文件，并识别 `job_test.rs`、`tests.rs` 对它的使用。精确文本检索进一步确认 `importer.rs` 的生产调用边。RustCodeGraph 的 `callers`/`callees` 命令在本次检查的 30 秒窗口内未返回结果，因此本文没有把未取得的图边当作证据。

## 错误处理与边界

- `worker_count == 0`、`batch == 0` 或连接数少于 worker 数统一返回 `InvalidConfig`。`job_count == 0` 是合法输入：线程仍会建立并回收，不开启事务，成功报告的 `jobs` 为 0。
- `generate_row_data_batch` 在事务开始前生成完整批次；生成失败时该批次不会开启事务。`begin`、任一 `execute` 或 `commit` 失败都会终止对应 worker 并最终使 `process_jobs` 失败。
- 接口没有显式 `rollback`。执行中途失败时事务仅被丢弃，是否以及如何回滚取决于具体 `DatabaseTransaction` 实现的析构/驱动语义；扩展真实驱动时必须验证这一点，不能假定本文件执行了回滚。
- `recv` 的任何错误都被视作生产结束，而非 `ImporterError::ChannelClosed`；虽然错误枚举中存在 `ChannelClosed`，本文件当前不构造它。
- `jobs.lock().unwrap()` 在互斥锁中毒时会令 worker panic，最终通常映射为 `WorkerPanic`。生产者的普通 `send` 失败被当作已有 worker 退出后的收尾信号，不单独报告通道错误。
- 首个被保存的 worker 错误按 worker 句柄的创建/遍历顺序决定，而不是实际发生时间。生产者若 panic，则其 `WorkerPanic` 在代码控制流上先于保存的 worker 错误返回。
- `16 * worker_count` 和 `job_count as i64` 在极端 `usize` 输入下没有显式溢出防护；正常配置应限制到可创建线程、通道和连接的实际规模。

## 并发与资源生命周期

并发模型是一个生产者、多个消费者。标准库 MPSC 的 `Receiver` 不能克隆，因此用 `Mutex` 共享：同一时刻只有一个 worker 能在 `recv` 上等待并取得下一个 token；锁在单次 `recv` 返回后释放，耗时的数据生成和事务写入不持锁，所以不同 worker 的数据库批次仍可并行执行。这种设计把取队列过程串行化，但不会把数据库写入串行化。

有界容量 `16 * worker_count` 提供背压：worker 处理较慢时生产者阻塞在 `send`，避免一次性保存全部作业。协调者创建完 worker 后释放自己的 `receiver` 引用，再先回收全部 worker、后回收生产者。`pkg/util/importer/job_test.rs::worker_error_does_not_deadlock_a_full_job_channel` 专门验证 worker 在首批 `begin` 失败、64 个 token 足以填满单 worker 通道时，调用能在一秒内返回数据库错误而非死锁。

连接本身由 `importer.rs::process` 在本调用外创建并在结果产生后统一关闭；`job.rs` 只持有 worker 生命周期内的 `Arc` 克隆。线程全部是作用域外 `thread::spawn` 的拥有型线程，但 `process_jobs` 在返回前逐一 `join`，不会把后台线程句柄泄露给调用者。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/importer/job.go`：Rust 生产者循环对应 `addJobs`，`do_insert` 对应 `doInsert`，`do_job` 对应 `doJob`，`process_jobs` 合并了 Go 的 `doProcess` 与 `doWait`。两边都使用容量为 `16 * workerCount` 的空 token 通道、每 worker 一个数据库连接、满批提交和关闭通道后的尾批提交，并在完整秒数为零时报告 `-1`，否则用作业数除以秒数。

主要差异如下：

- Go 写入错误调用 `log.Fatal`，进程级终止；Rust 将生成、数据库和提交错误作为 `Result` 返回，线程 panic 映射为 `WorkerPanic`，更适合库边界。
- Go 用可直接被多个 goroutine 接收的 channel，并以 `doneChan` 等待 worker；Rust 标准库 Receiver 不可克隆，因此增加 `Arc<Mutex<Receiver>>` 并用 `JoinHandle` 等待。
- Go `doWait` 直接打印报告且 `doProcess` 无返回值；Rust 返回结构化 `ProcessReport`，由上层决定展示方式。
- Rust 增加参数合法性检查和“协调者释放接收端”的失败收尾逻辑；后者由独立 Rust 回归测试覆盖。Go 同目录当前没有 `job_test.go`，不能据此声称 Go 侧已有等价失败测试。
- Go 以 `time.Now().Unix()` 的整秒差计算 TPS；Rust 以单调 `Instant` 的 `Duration::as_secs()` 计算，避免墙钟跳变，但仍保留整秒截断与不足一秒为 `-1` 的外部语义。

## 扩展指南

- 改变调度、背压或取消策略时，主要接入点是 `process_jobs`。应保持“生产者在所有接收端消失后可退出”和“所有线程在函数返回前被回收”的不变量，并在 `pkg/util/importer/job_test.rs` 增加独立并发回归测试，不要把测试嵌入生产文件。
- 改变批次形成规则时修改 `do_job`；需要同时验证单 worker 的整批/尾批，以及多 worker 下每个 worker 各自尾批导致的事务数。现有 `pkg/util/importer/tests.rs::generation_and_job_processing_preserve_row_and_batch_counts` 是 5 行、2 worker、batch=2 的基线。
- 改变事务原子性或错误恢复时修改 `do_insert` 及 `pkg/util/importer/db.rs::{DatabaseTransaction,Database}`。若引入显式回滚，必须同时定义 execute/commit 失败的优先级和测试替身行为，并对照 Go 的进程终止语义评估兼容性。
- 若报告要表达真实“事务每秒”，不能继续用 `job_count` 作分子；需要从 worker 汇总实际提交批次数，并评估字段语义变化对调用者的兼容影响。若只优化性能，应保留现有 jobs/sec 行为或另增字段。
- 新增配置约束时应在 `process_jobs` 和上层 `importer.rs::process` 之间明确唯一责任，避免错误消息不一致；同步更新 `config.rs::ImporterError` 和 `job_test.rs`。
- 增加真实数据库实现或外部依赖时，应在 `pkg/util/importer/Cargo.toml` 建立实际启用且可复现的依赖声明；当前 `cfg(any())` 下的依赖不会参与编译，不能把它们视作现有运行时接线。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/importer` 确认目标、Go 对照和独立测试均在索引中；`node --file pkg/util/importer/job.rs --offset 1 --limit 260` 读取了完整 132 行并报告该文件有 11 个符号；`query process_jobs/do_job/do_insert` 确认目标函数签名。`callers job.rs::process_jobs` 在 30 秒内未返回，故调用边改由文本检索验证。
- 源码：`pkg/util/importer/job.rs`（完整实现）、`pkg/util/importer/importer.rs`（生产入口）、`pkg/util/importer/lib.rs`（模块公开与重导出）、`pkg/util/importer/db.rs`（数据库/事务接口与批量 SQL 生成）、`pkg/util/importer/config.rs`（配置与错误）、`pkg/util/importer/parser.rs`（`Table` 定义）。该目录不存在 `doc.go`。
- crate 边界：`pkg/util/importer/Cargo.toml`，包名为 `astersql-util-importer`、库入口为 `lib.rs`，普通依赖为空，路径依赖位于 `cfg(any())` 条件段。
- Go 对照：`pkg/util/importer/job.go`；生产调用与更外层流程另核对 `pkg/util/importer/importer.rs`。同目录未找到针对 job 调度的 Go `*_test.go`。
- Rust 测试：`pkg/util/importer/job_test.rs::worker_error_does_not_deadlock_a_full_job_channel`；`pkg/util/importer/tests.rs::generation_and_job_processing_preserve_row_and_batch_counts`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行固定章节结构检查，并人工复核本文区分了已验证事实、实现限制和扩展风险。
