# `pkg/statistics/handle/initstats/load_stats_page.rs`

## 文件定位

本文件属于独立 crate `astersql-statistics-handle-initstats`，由同目录 `lib.rs` 的私有模块 `load_stats_page` 引入并通过 `pub use load_stats_page::*` 对外再导出。它提供一个按表 ID 范围分发初始化统计加载工作的通用并发 worker，以及非 lite 初始化过程使用的全局百分比。crate 的 `Cargo.toml` 表明直接依赖仅包括 `anyhow`、`crossbeam-channel`、配置桥接和日志桥接；本文件实际使用其中的 `anyhow`、通道与日志桥接。

当前 Rust 仓库搜索只找到同目录 `migration_aster_unit_test.rs` 调用 `NewRangeWorker`、`LoadStats`、`SendTask` 和 `Wait`，未找到 Rust 生产调用者。因此，该文件是已经实现并有迁移测试的公开能力，但其在 Rust 完整应用中的生产接线尚未验证。对应的 Go 实现则由 `pkg/statistics/handle/bootstrap.go` 接入完整统计初始化主链。

## 核心职责

- `RangeWorker` 把调用方提供的 `Task` 送入容量为 1 的有界通道，并由指定数量的 OS 线程并发调用 `processTask`。
- 每个回调无论返回成功还是错误，都会在进度日志器存在时计为已完成，并按“进入本阶段前的百分比 + 本阶段完成比例 × 阶段权重”更新 `InitStatsPercentage`。
- 回调错误只写后台错误日志，不停止其他任务，也不由 `Wait` 返回；这与 Go 对照文件的容错语义一致。
- `Wait` 通过销毁唯一发送端关闭通道，再 join 全部 worker，形成显式的投递、排空、回收生命周期。

## 主要符号

- `AtomicFloat64 { bits: AtomicU64 }`：以 `f64::to_bits`/`from_bits` 在 `AtomicU64` 中保存浮点数。`new` 是常量构造器，`Load`、`Store` 均使用 `Ordering::SeqCst`；它只提供整值原子读写，不提供原子加法。
- `InitStatsPercentage: AtomicFloat64`：进程级全局初始化进度，初值为 `0.0`。注释明确其只服务非 lite 模式。
- `singletonStatsSamplerLogger() -> Logger`：用 `OnceLock` 缓存一个后台采样日志器。工厂参数为 60 秒、阈值 1，并添加 `category=stats` 字段；返回值通过 clone 共享。
- `Task { StartTid, EndTid }`：可复制的表 ID 范围描述。字段本身不强制端点关系；Go 上游的分页策略按 `[StartTid, EndTid)` 约定生成任务：单表使用 `tableID..tableID+1`，整段分页使用 `tid..tid+initStatsStep`。
- `ProcessTask`：`Fn(Task) -> anyhow::Result<()> + Send + Sync + 'static` 的内部 trait object，要求回调可被多个线程共享。
- `RangeWorker`：保存任务名、发送/接收端、回调、任务总数与完成数、阶段百分比参数、并发度及线程句柄。`taskSender` 和 `workers` 使用 `Mutex` 支持通过共享引用改变生命周期状态。
- `NewRangeWorker(...) -> RangeWorker`：创建容量为 1 的通道，捕获当前全局百分比作为阶段基线，并初始化日志器和共享计数器；它不自动启动线程。
- `RangeWorker::LoadStats(&self)`：按 `concurrency` 启动消费者线程，并保存所有 `JoinHandle`。
- `RangeWorker::loadStats(...)`：单线程消费循环，执行回调、记录错误、递增完成数、刷新进度和写采样日志。
- `RangeWorker::SendTask(&self, Task)`：克隆仍存在的发送端并同步发送；通道满时阻塞，关闭后调用会 panic。
- `RangeWorker::Wait(&self)`：取走并销毁发送端，等待通道中的任务被排空，然后 join 当前保存的全部线程。`completed_task_count` 是为观察完成数提供的原子读取接口。

## 执行流程

1. 上游构造 worker 时传入阶段名、实际加载回调、并发数、预计任务总数和该阶段占用的百分比步长。构造函数读取一次 `InitStatsPercentage`，固定为本阶段基线。
2. 调用 `LoadStats` 后创建 `concurrency` 个线程。每个线程持有接收端、回调、完成计数、日志器和只读进度参数的共享副本。
3. 上游调用 `SendTask`。容量为 1 的通道形成背压：当缓冲区被占用且消费者尚未接走任务时，生产者等待。
4. 任一消费者取得任务后调用 `processTask`。`Err` 被格式化后写入 `BgLogger`，循环继续处理后续任务。
5. 若 `progressLogger` 为 `Some`，线程用 `fetch_add(1) + 1` 得到该任务的完成序号，计算 `completed / taskCnt * totalPercentageStep + totalPercentage`，原子写入全局进度并记录 `load <taskName> [completed/taskCnt]`。
6. 上游完成投递后调用 `Wait`。发送端被取走后，所有接收循环在清空缓冲区后退出；随后逐个 join，保证回调不再运行。

Go 生产主链在 `bootstrap.go` 中对 histogram、TopN、bucket 三个阶段重复使用“构造 → `LoadStats` → 策略生成任务 → `Wait`”序列。全量策略按固定 ID 步长生成范围，局部刷新策略按表 ID 列表生成单表范围。

## 数据与状态

- `Task` 只携带两个 `i64`，不拥有表数据；具体 SQL、缓存和内存限制均封装在调用方闭包中。
- 通道有一个缓冲槽。`Receiver` 可克隆，crossbeam 在多个消费者之间把每个任务交给其中一个消费者，不广播任务。
- `completeTaskCnt` 是所有线程共享的 `AtomicU64`。失败任务也会递增；所以它表示“回调尝试完毕的任务数”，不是成功数。
- `taskCnt` 是调用方声明的预计总量，不会根据实际发送数校正。最终百分比只有在实际完成数与它一致时才恰好推进一个 `totalPercentageStep`。
- `totalPercentage` 在构造时快照全局值，之后不随外部写入变化。每次完成任务都会根据该固定基线重算绝对值。
- `progressLogger` 当前由构造函数固定为 `Some`，但字段设计允许内部逻辑跳过计数、百分比和进度日志。当前公开构造路径不会产生 `None`。

## 依赖与调用关系

Rust 文件的下游依赖如下：`std::thread`/`JoinHandle` 管理消费者；`Arc` 共享回调、字符串和计数；`Mutex` 保护发送端及线程句柄；`OnceLock` 初始化单例日志器；`crossbeam_channel::bounded` 提供多消费者有界通道；`anyhow::Result` 统一回调错误；`crate::util::logutil` 提供后台日志与采样工厂。这些日志符号由 `lib.rs` 转发自 `astersql-util-logutil`。

Rust 上游证据目前限于 `lib.rs` 的公开再导出和 `migration_aster_unit_test.rs` 的直接测试，未发现 Rust 生产调用边。Go 对照主链为 `Handle::initStatsWithSession` 选择加载策略与并发度，然后 `initStatsHistogramsConcurrently`、`initStatsTopNConcurrently`、`initStatsBucketsConcurrently` 分别创建 `RangeWorker`；`maxTidStrategy` 或 `tableListStrategy` 负责调用 `SendTask`。因此这些 Go 调用只能证明设计来源和完整应用中的预期位置，不能证明 Rust 主服务已经接线。

## 错误处理与边界

- `processTask` 返回的 `Err` 被记录后吞掉，`Wait` 无返回值，调用方无法从 worker 汇总任务失败。扩展时不能把 `Wait` 当成“所有任务成功”的证明。
- `SendTask` 在 `Wait` 之后 panic（`task channel is closed`）；同目录 `sending_after_wait_matches_go_closed_channel_failure` 明确验证该 Go 兼容行为。
- 第二次调用 `Wait` 会因发送端已被取走而 panic（`task channel is already closed`）。worker 的关闭操作不是幂等的。
- worker 线程中的回调若 panic，线程会提前退出；`Wait` 在 join 时再 panic（`load stats worker panicked`）。这不同于普通 `Err` 的仅记录行为。
- 任一生命周期 mutex 被持锁线程 panic 后会中毒，后续访问以明确的 `expect` 信息 panic。
- `taskCnt == 0` 且实际收到任务时会发生浮点除零，得到非有限进度而非 Rust 整数式 panic；构造函数不校验任务数、并发度、范围合法性或百分比步长。`concurrency == 0` 时没有消费者，后续发送超过缓冲容量可能永久阻塞。
- 实际完成数超过 `taskCnt` 会把进度推进超过本阶段权重；少于 `taskCnt` 则达不到阶段目标。调用方必须保持任务计数与发送数量一致。

## 并发与资源生命周期

`LoadStats` 创建真实 OS 线程而非异步任务。回调要求 `Send + Sync + 'static`，其捕获资源必须能安全跨线程并活到线程结束。容量 1 的通道限制待处理队列，但不限制每个正在执行的回调所占资源；实际并行上限是 `concurrency`。

推荐生命周期严格为一次 `NewRangeWorker`、一次 `LoadStats`、若干 `SendTask`、一次 `Wait`。当前 API 没有阻止多次 `LoadStats`，多次调用会追加更多消费者；也没有阻止 `LoadStats` 与 `Wait` 并发，调用方应避免这种竞态。`Wait` 通过关闭最后一个保存在 worker 中的发送端使接收循环终止，并取走句柄确保只 join 一次。

完成编号由 `fetch_add` 保证唯一且全序，但“取得编号”和“写入全局百分比”是两个独立原子操作：较小编号的线程可能晚于较大编号写入，从而令观察到的全局百分比短暂回退。`SeqCst` 保证单次原子操作的全局顺序，不能把这两个操作合并成事务。此外 `InitStatsPercentage` 是进程全局值，同时运行多个 worker 时会互相覆盖；现有 Go 主链按阶段串行等待，规避了这一用法风险。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/initstats/load_stats_page.go`。Rust 保留了 Go 的公开命名、`Task` 字段、容量 1 通道、阶段基线与权重公式、失败后继续、关闭后发送失败以及等待全部消费者的核心语义。

实现映射为：Go `atomicutil.Float64` 对应 `AtomicFloat64`；Go `chan Task` 对应 crossbeam `Sender/Receiver`；Go `util.WaitGroupWrapper` 对应 `Vec<JoinHandle<()>>`；Go `atomic.Uint64.Add` 对应 `AtomicU64::fetch_add`；Go 采样日志工厂对应 Rust 的 `OnceLock<Logger>`。Rust 额外公开 `completed_task_count` 供测试/观察，并用 `Mutex<Option<Sender<_>>>` 表示发送端只能关闭一次。

两端值得注意的差异是错误日志格式：Go 通过结构化 `zap.Error` 写错误字段，Rust 当前将完整错误链格式化进字符串。线程 panic、mutex poisoning 是 Rust 实现特有的失败表面；Go goroutine panic 也会失败，但没有 join 时再传播这一阶段。Rust `NewRangeWorker` 返回值而非 Go 指针，但内部共享状态使其仍可通过共享引用驱动。

## 扩展指南

- 新增取消、超时或失败汇总时，主要接入点是 `ProcessTask`、`loadStats` 和 `Wait`。若改变“单任务错误只记录并继续”的契约，应同步修改 Go 对照语义或明确记录迁移差异。
- 调整并发模型或队列容量时，应保留背压与排空保证，并在 `migration_aster_unit_test.rs` 增加独立测试；不要把测试写回本生产文件。
- 修改进度算法时，应处理零任务、实际任务数不匹配、多个 worker 共享全局值及并发写入回退。若要求单调进度，可考虑比较交换循环或集中式进度发布，而不能仅替换内存序。
- 新增生产接线时，应从 Rust 统计 handle 的阶段编排处调用本 crate，并逐一对照 Go `bootstrap.go` 的 histogram、TopN、bucket 回调与策略；不能仅证明 worker 自身可编译就声称完整初始化已迁移。
- 修改 `Task` 的范围约定时，应同时核对 Go `maxTidStrategy`、`tableListStrategy`、分页 SQL 生成逻辑及对应 Rust 调用方，尤其避免闭区间/半开区间造成重复或漏表。
- 至少同步覆盖：失败仍计数并继续、请求并发度、关闭后发送、零并发/零任务策略、回调 panic、重复关闭和进度单调性。现有直接 Rust 测试文件是 `pkg/statistics/handle/initstats/migration_aster_unit_test.rs`；应用级 Go 行为测试集中在 `pkg/statistics/handle/handletest/initstats/init_stats_test.go`，但没有同名 Go 单元测试专门覆盖 worker。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/statistics/handle/initstats/load_stats_page.rs` 读取了目标文件 1–230 行并列出 13 个符号。`query RangeWorker`、`query LoadStats`/`loadStats` 定位 Rust/Go 对照符号；精确 `callers`/`callees` 没有返回可判读边，因此未把它作为生产接线证据。
- 目标源码：`pkg/statistics/handle/initstats/load_stats_page.rs`，核对 `AtomicFloat64`、`InitStatsPercentage`、日志器、`Task`、`RangeWorker` 及全部方法。
- crate 边界：`pkg/statistics/handle/initstats/Cargo.toml` 与 `pkg/statistics/handle/initstats/lib.rs`，核对依赖、模块声明、公开再导出和独立测试挂载。
- Go 对照与生产入口：`pkg/statistics/handle/initstats/load_stats_page.go`、`pkg/statistics/handle/bootstrap.go`，核对字段、公式、通道生命周期、任务策略以及 histogram/TopN/bucket 阶段调用顺序。
- Rust 直接测试：`pkg/statistics/handle/initstats/migration_aster_unit_test.rs`，覆盖失败任务仍完成与推进进度、四消费者并发、`Wait` 后发送 panic。Go 应用级相关测试路径为 `pkg/statistics/handle/handletest/initstats/init_stats_test.go`。
- 全仓 `rg` 复核：Rust 侧除目标实现与上述迁移测试外未发现这些 API 的生产调用；Go 侧调用集中于 `bootstrap.go`。仓库在 `pkg/statistics` 下未发现 `doc.go`，因此没有可补读的目标包契约文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行固定 11 章节结构检查，并人工复核唯一生产物与源码路径、符号和接线状态一致。
