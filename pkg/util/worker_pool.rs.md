# `pkg/util/worker_pool.rs`

## 文件定位

本文件是 `astersql-util` crate 中的有界并发令牌池实现，由 [`pkg/util/lib.rs`](lib.rs) 以 `pub mod worker_pool` 导出。它不复用 `threadpool` crate 的常驻线程，而是预先创建固定数量的 `Worker` 令牌：提交者先取得令牌，再创建一次性线程；因此限制的是同时在途的任务数量，而不是复用操作系统线程。

当前仓库的 Rust 生产代码中没有检索到对本模块公开 API 的调用。RustCodeGraph 对 `NewWorkerPool` 的调用方查询只返回独立测试 `security_2_aster_unit_test.rs::worker_pool_limits_and_recycles_workers`。这说明该文件目前是已导出的通用迁移能力，但尚未接入 Rust 应用主链；不能据此宣称 Go 侧所有使用场景已经迁移。

## 核心职责

- `NewWorkerPool` 建立容量为 `limit` 的令牌通道，并填入 ID 为 `1..=limit` 的 `Worker`。
- `Apply`、`ApplyWithID`、`ApplyOnErrorGroup` 和 `ApplyWithIDInErrorGroup` 在启动任务前同步借出令牌，使并发数不会超过池容量。
- `RecycleGuard` 在任务正常返回或线程 unwind 时归还令牌，避免普通 Rust panic 永久耗尽池容量。
- `IdleCount`、`Limit` 和 `HasWorker` 暴露容量与瞬时空闲状态；`ApplyWorker`、`RecycleWorker` 也允许调用方手工管理令牌。

该模块只负责并发配额和任务启动，不提供任务取消、超时、排队公平性、线程复用或普通 `Apply` 任务的 join/wait 接口。

## 主要符号

- `WorkerPool { inner: Arc<WorkerPoolInner> }`：可克隆的公开句柄。克隆仅增加共享状态的引用计数，不复制容量或令牌。
- `WorkerPoolInner`：私有共享状态，保存不可变 `limit`、同一个 bounded channel 的 `workers_tx`/`workers_rx` 以及仅用于等待日志的 `name`。
- `Worker { pub ID: u64 }`：公开令牌值，ID 从 1 开始；派生了 `Clone`、`Debug`、`Eq` 和 `PartialEq`。字段名沿用 Go 风格，crate 根通过 lint allow 接受非 snake case 命名。
- `NewWorkerPool(limit: usize, name: String) -> WorkerPool`：构造并预填令牌。初始化发送失败会以 `worker pool initialization failed` panic。
- `RecycleGuard { pool, worker }`：私有 RAII 守卫；`Drop` 取走 `Option<Worker>` 并调用 `RecycleWorker`。`Option` 用于保证同一守卫只归还一次。
- `IdleCount(&self) -> usize` / `Limit(&self) -> usize` / `HasWorker(&self) -> bool`：只读状态查询。其中空闲数是并发环境下的瞬时快照，不是后续成功借令牌的保证。
- `Apply<F: FnOnce() + Send + 'static>`：借令牌后用 `std::thread::spawn` 执行无参数任务。
- `ApplyWithID<F: FnOnce(u64) + Send + 'static>`：与 `Apply` 相同，但向任务传入所借令牌的 ID。
- `ApplyOnErrorGroup<F: FnOnce() -> anyhow::Result<()> + Send + 'static>`：把任务交给 `ErrorGroupWithRecover::Go`，错误由 group 汇总。
- `ApplyWithIDInErrorGroup<F: FnOnce(u64) -> anyhow::Result<()> + Send + 'static>`：ErrorGroup 版本并传入令牌 ID。
- `ApplyWorker(&self) -> Worker` / `RecycleWorker(&self, Worker)`：公开的底层借还协议；调用方必须严格成对使用。

## 执行流程

1. `NewWorkerPool` 调用 `crossbeam_channel::bounded(limit)`，随后按升序向通道写入 `limit` 个令牌，最后把收发端、容量和名称放入 `Arc<WorkerPoolInner>`。
2. 四个高级提交 API 都在调用者线程中先执行 `ApplyWorker`。若有空闲令牌立即返回；若池已满，则先记录 debug 日志，再阻塞到其他任务归还令牌。因此这些 API 本身可能阻塞，且只有取得配额后才启动任务。
3. 普通 `Apply*` 用 `std::thread::spawn` 创建线程；ErrorGroup 版本调用 `ErrorGroupWithRecover::Go`，由后者创建线程、捕获 panic、收集 `Result`，并在配置了取消 token 时于错误发生后触发取消。
4. 新线程先构造持有 `Worker` 的 `RecycleGuard`，再调用用户闭包。闭包正常返回或 panic unwind 时，守卫析构并把令牌发送回 bounded channel。
5. `ApplyOnErrorGroup` 系列把用户闭包的 `Result<()>` 原样返回给 group；调用者必须再调用 `ErrorGroupWithRecover::Wait` 才能等待全部任务并取得首个完成时序上的错误。

手工路径是 `ApplyWorker` 取得所有权、调用方完成任意工作、最后将同一个 `Worker` 传给 `RecycleWorker`。这一路径没有守卫保护，遗漏归还会永久降低可用容量。

## 数据与状态

池的核心不变量是：正常使用时，通道中的空闲令牌数加上已借出的令牌数等于 `limit`，并且有效令牌 ID 集合为 `1..=limit`。`WorkerPool` 克隆共享同一通道，所以所有克隆共同竞争同一配额。

`crossbeam_channel::Receiver::len` 为 `IdleCount` 提供当前排队令牌数。其他线程可在查询后立即借走或归还令牌，因此 `HasWorker` 只适合观测或提示，不能作为“检查后再借取”的同步协议。真正的容量控制只发生在 `ApplyWorker` 的 receive 操作上。

模块没有显式关闭状态。收发端都存放在同一个 `WorkerPoolInner` 中，并由所有克隆共同持有；只要能调用池方法，通道通常不会断开。`name` 不参与调度或身份判断，只出现在池耗尽后的 debug 日志中。

## 依赖与调用关系

- crate 边界：[`pkg/util/Cargo.toml`](Cargo.toml) 声明 crate 名为 `astersql-util`，本文件直接使用其中的 `anyhow`、`crossbeam-channel` 和 `log` 依赖；线程与共享所有权来自标准库。
- 模块入口：[`pkg/util/lib.rs`](lib.rs) 公开 `worker_pool`，并公开相邻的 `wait_group_wrapper`。
- 下游调用：`Apply*` → `ApplyWorker` → `Receiver::try_recv`/`Receiver::recv`；任务退出 → `RecycleGuard::drop` → `RecycleWorker` → `Sender::send`。
- ErrorGroup 调用：`ApplyOnErrorGroup`/`ApplyWithIDInErrorGroup` → [`ErrorGroupWithRecover::Go`](wait_group_wrapper.rs)。后者负责线程创建、`catch_unwind`、结果通道和可选取消；worker pool 自身只负责令牌生命周期。
- 上游现状：RustCodeGraph 的 `callers` 查询仅确认 [`pkg/util/security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs) 中的 `worker_pool_limits_and_recycles_workers` 调用 `NewWorkerPool`；全仓 Rust 搜索没有发现本实现的生产调用点。仓库里其他同名 `WorkerPool`（例如 resource manager、Lightning reader 和 BR stubs）是独立实现，不应混为本模块调用者。

## 错误处理与边界

- `limit == 0` 时构造可以完成，但池内永远没有令牌；首次 `ApplyWorker` 会在 `recv` 永久等待。Rust 与 Go 对照实现都有这一行为，当前没有参数校验。
- `ApplyWorker` 在空池时无限阻塞，没有超时或取消入口。等待日志只在快速 `try_recv` 得到 `Empty` 时记录一次。
- 通道断开、初始化发送失败或归还发送失败都被视为内部不变量破坏并 panic，而不是返回可恢复错误。由于池同时持有 sender 与 receiver，正常 API 生命周期中断开分支很难到达。
- 手工归还伪造或重复克隆的 `Worker` 可能让 bounded channel 的 `send` 阻塞，并能破坏 ID 唯一性。Rust 类型只保证传入一个 `Worker` 值，不验证它来自该池；公开的 `Clone` 也意味着调用方可以复制令牌。安全扩展时不能把 `Worker::Clone` 当作额外配额。
- 普通 `Apply`/`ApplyWithID` 丢弃 `JoinHandle`，无法向调用方传播闭包 panic 或等待结果；不过在默认 panic=unwind 下 `RecycleGuard` 仍会归还令牌。进程 abort 或线程被强制终止时不保证执行析构。
- ErrorGroup 路径将闭包错误和 panic 交给 `ErrorGroupWithRecover`；worker pool 不检查结果，也不主动停止尚未开始或正在运行的其他任务。

## 并发与资源生命周期

bounded channel 同时承担配额计数和跨线程同步。获取令牌发生在启动线程之前，这避免了无限创建大量阻塞线程；代价是提交线程会在容量耗尽时阻塞。每个已取得令牌的高级任务拥有一个 `RecycleGuard`，守卫拥有 `Worker`，从而把“一次借出、一次归还”绑定到任务栈生命周期。

`WorkerPool` 和其克隆可跨线程移动，因为共享状态在 `Arc` 中，crossbeam 的收发端支持并发访问。池销毁没有显式 join：普通任务持有的 guard 又持有一个池克隆，因此即使外部句柄全部释放，内部通道也会活到最后一个任务结束；任务结束归还令牌后，该克隆才释放。ErrorGroup 路径还要求调用者维护 group 并调用 `Wait` 来 join 其线程。

该设计每次提交创建一个新的操作系统线程，不是固定线程集合。若任务非常短或提交频繁，线程创建成本可能显著；若要改成常驻线程池，必须重新核对 Go 行为、错误传播、Worker ID 稳定性及现有 API 的同步阻塞语义。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/worker_pool.go`](worker_pool.go)。两边都预填 `limit` 个从 1 开始编号的 worker，都在调用者侧先借令牌，再异步运行并 defer/RAII 归还；`IdleCount`、`Limit`、`HasWorker` 以及四个提交入口在意图和阻塞行为上逐项对应。

主要实现差异如下：

- Go 使用单个 buffered `chan *Worker`；Rust 的 crossbeam channel 将 `Sender`、`Receiver` 同时保存在共享 inner 中。
- Go 返回 `*WorkerPool`/`*Worker`；Rust 返回拥有所有权的 `WorkerPool`/`Worker`，并用 `Arc` 实现句柄克隆。
- Go 的 `defer pool.RecycleWorker(worker)` 对应 Rust 的 `RecycleGuard::drop`。二者都覆盖普通 panic/unwind 路径，但 Go 明确拒绝归还 `nil`，Rust 不存在空 `Worker` 参数；Rust 仍不验证 worker 的来源或 ID。
- Go 普通任务使用 goroutine，Rust 使用每任务一个 `std::thread`，资源成本并不等价。
- Go ErrorGroup 使用 `golang.org/x/sync/errgroup.Group`；Rust 使用仓库内 `ErrorGroupWithRecover`，额外捕获 panic，并可通过 `CancellationToken` 取消关联子树。该差异来自相邻模块，不能理解为 worker pool 自身提供取消。

Go 生产调用点分布在 BR backup/stream、streamhelper 等路径，但当前 Rust 调用图没有证明这些路径已经接到 `astersql_util::worker_pool`；不少 Rust 迁移代码使用各自的本地/stub 池。

## 扩展指南

- 增加新的提交方式时，应复用“先 `ApplyWorker`、在线程闭包最前创建 `RecycleGuard`”的结构，避免在闭包可能提前返回或 panic 的分支手写归还。
- 若增加超时或可取消获取，修改点应集中在 `ApplyWorker` 附近，并明确区分 `Empty` 与 `Disconnected`；API 需要返回 `Result`/`Option`，不能悄悄改变现有无限等待语义。
- 若强化手工借还安全性，可考虑让令牌不可克隆、让 guard 成为公开租约，或给令牌关联池身份；这些都是兼容性变更，需审查现有公开 API 使用者。
- 若加入 `limit > 0` 校验，应同步 Go 版本或记录明确的语义偏差，并增加零容量回归测试。
- 测试应放在独立文件，不嵌入 `worker_pool.rs`。现有覆盖位于 [`pkg/util/security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs)；扩展时至少补充容量耗尽会阻塞、panic 后回收、ErrorGroup 的 `Ok`/`Err`/panic、多个池克隆共享配额、错误归还/重复归还策略等场景。若新增独立测试文件，还需在 `Cargo.toml` 的显式 test target 或 `lib.rs` 测试模块接线中注册。
- 性能修改必须保留“启动任务前取得配额”的背压性质，并评估从每任务线程切换为常驻池后对调度、公平性和 shutdown 的影响。

## 验证依据

- 源码全量阅读：[`pkg/util/worker_pool.rs`](worker_pool.rs)，核对 `WorkerPool`、`WorkerPoolInner`、`Worker`、`RecycleGuard`、构造函数和全部九个公开方法；文件中没有 trait、常量或条件编译项。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/util/worker_pool.rs` 显示目标文件含 16 个符号；`query` 精确定位 Rust/Go 两个 `NewWorkerPool`、`ApplyWorker`、`ApplyOnErrorGroup`；`callers pkg/util/worker_pool.rs::NewWorkerPool` 仅返回 `worker_pool_limits_and_recycles_workers`；`callers ApplyOnErrorGroup` 为空；`callees ApplyOnErrorGroup` 返回本文件的 `ApplyWorker`。
- crate 与入口：[`pkg/util/Cargo.toml`](Cargo.toml) 和 [`pkg/util/lib.rs`](lib.rs)，核对依赖、crate 名、模块导出及显式测试目标 `security_2_aster_unit_test.rs`。
- 相邻实现：[`pkg/util/wait_group_wrapper.rs`](wait_group_wrapper.rs)，核对 `ErrorGroupWithRecover::Go`/`Wait` 的线程、panic、结果与取消生命周期。
- Go 对照：[`pkg/util/worker_pool.go`](worker_pool.go)，逐项核对构造、借还、四类提交、状态查询和零容量阻塞语义；同目录未发现专门的 Go `worker_pool_test.go`。
- Rust 测试：[`pkg/util/security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs) 的 `worker_pool_limits_and_recycles_workers` 验证容量、不同 ID、耗尽状态、手工回收、带 ID 提交及最终归还；目前未覆盖零容量、阻塞等待、panic 和 ErrorGroup 路径。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行固定章节结构检查，并人工复核文档没有把未接线能力描述为生产主链现状。
