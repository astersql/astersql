# `br/pkg/utils/iter/combinator_types.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-utils-iter`，crate 边界由同目录
`Cargo.toml` 的 `[lib] path = "lib.rs"` 确定。`lib.rs` 将本文件注册为
`combinator_types` 模块并以 `pub use combinator_types::*` 重导出其公开类型；不过业务调用方通常不直接构造这些类型，而是使用 `combinators.rs` 中的
`Transform`、`FilterOut`、`TakeFirst`、`Map`、`MapFilter`、`TryMap`、
`FlatMap`、`ConcatAll` 和 `Enumerate` 工厂。

它位于 BR 工具链的惰性拉取迭代器层：上游遵循 `iter.rs::TryNextor<T>`，每次
`TryNext` 返回 `Emit`、`Throw` 或 `Done` 三态之一；本文件把映射、过滤、截断、
展平和编号行为实现为新的 `TryNextor`。恢复流程会组合这些积木，例如
`restore/restorer.rs::PipelineRestorerWrapper::WithSplit` 用 `FilterOut` 与 `TryMap`
串起跳过、累积和分区切分，`restore/log_client/log_file_manager.rs::FilterDataFiles`
用 `FlatMap`、`Enumerate`、`FilterOut`、`Map` 展开并过滤日志元数据。

## 核心职责

- `WorkerPool` 与 `TransformIter` 实现带并发上限、结果背压和取消传播的可失败异步映射。它是本文件唯一主动创建线程的路径。
- `FilterIter`、`TakeIter`、`PureMapIter`、`FilterMapIter` 和 `TryMapIter` 在调用者线程中同步拉取上游，分别实现过滤、限量、纯映射、映射加跳过和可失败映射。
- `JoinIter` 串联一组子迭代器，是 `FlatMap` 与 `ConcatAll` 的共同展平内核。
- `WithIndexIter` 给成功产出的元素附加从零开始的 `i32` 下标，是公开
  `Enumerate` 的实现。
- 所有组合器统一保留 `TryNextor` 的三态契约。需要跨元素类型转发结束或错误时，使用 `DoneBy` 或 `convertDoneOrErrResult` 清除不再适用的 `Item`。

## 主要符号

- `WorkerPool { limit, inflight }`：可克隆的线程配额对象。`new(n, name)` 把零并发度提升到 1；`Limit` 暴露上限；`Apply` 在 `inflight < limit` 时启动一个独立线程，并在闭包正常返回后递减计数。多个 clone 共享同一个 `Arc<AtomicUsize>`。
- `BufferedMappingCfg { bufferSize, quota }`：`TransformIter` 的内部配置。
  `bufferSize` 同时决定 outstanding 背压上限；`quota` 决定 mapper 并发度。
  `combinators.rs::Transform` 会提供默认池，并保证最终 buffer 不小于池上限。
- `TransformIter<T, R>`：持有待移动到生产者线程的 `inner`、可并发调用的
  `mapper`、启动/完成标志、取消句柄、结果接收端、outstanding 计数和生产者
  `JoinHandle`。`new` 只保存状态，`start` 在首次拉取时懒启动线程。
- `FilterIter<T>`：`filterOutIf(&T) == true` 表示丢弃，方向与标准库
  `Iterator::filter` 相反。
- `TakeIter<T>`：`n` 是剩余拉取次数；为零时不再访问上游。
- `PureMapIter<T, R>`：对每个 `Emit(T)` 同步执行不可失败的 `FnMut(T) -> R`。
- `FilterMapIter<T, R>`：mapper 返回 `(R, skip)`；`skip == true` 时继续拉取，
  `false` 时产出结果。
- `TryMapIter<T, R>`：mapper 返回 `Result<R, String>`，错误经 `Throw` 进入迭代器错误通道。
- `JoinIter<T>`：`current` 是正在消费的子迭代器，`inner` 产生后续子迭代器。
- `WithIndexIter<T>`：`index` 仅在一次有效 `Emit` 后加一；结束和错误不消耗下标。

所有结构体均为 `pub`，但字段是否公开取决于构造需要：同步组合器字段公开供
`combinators.rs` 构造；`TransformIter` 字段私有并通过 `new` 建立生命周期不变量。
文件中没有常量、枚举、条件编译项或独立 trait 定义。

## 执行流程

`TransformIter::TryNext` 的主流程如下：

1. 第一次调用时执行 `start(ctx)`。它把 buffer 规范为至少 1，选取配置池或创建默认池，建立结果 channel，并通过 `Context::WithCancel` 创建受父 context 影响的子 context。
2. `start` 将 `inner` 从 `Option` 中取走，创建 outstanding 与 active-workers 原子计数，然后启动生产者线程。
3. 生产者先等待 `outstanding < buffer`，预占一个 outstanding 名额，再从上游拉取。正常元素被提交给 `WorkerPool::Apply`；worker 执行 mapper，把 `Emit` 或 `Throw` 发到结果 channel。mapper 失败还会取消子 context。
4. 若上游返回错误，生产者发送经 `DoneBy` 换型的错误并取消；若上游正常结束，则退还刚预占但未产生结果的 outstanding 名额。之后生产者等待所有 worker 退出，使末尾结果不会因发送端过早销毁而丢失。
5. 消费者侧以 5 ms `recv_timeout` 等待结果，以便周期性检查调用方取消和生产者状态。每取出一个结果就释放一个 outstanding 名额；错误会触发取消并把迭代器标为完成。生产者已经结束且 channel 无剩余结果，或 channel 断开时返回 `Done`。
6. 调用方 context 取消时，消费者取消子 context，并立即返回字符串为
   `context canceled` 的 `Throw`。之后的调用稳定返回 `Done`。

同步类型均由一次 `TryNext` 驱动：`FilterIter` 和 `FilterMapIter` 循环跳过不需要的元素；`TakeIter` 先递减额度再拉取；两种 Map 在上游有效产出时调用 mapper；
`WithIndexIter` 包装有效产出。`JoinIter` 先消费 `current`，其正常结束后从
`inner` 换入下一个子迭代器并递归拉取，因此能跳过空子迭代器。

## 数据与状态

迭代结果由 `iter.rs::IterResult<T>` 表示，预期合法状态是
`Emit(Item=Some, Err=None, Finished=false)`、`Throw(Item=None, Err=Some,
Finished=false)` 或 `Done(Item=None, Err=None, Finished=true)`。本文件假定来自受信上游的有效产出一定带有 `Item`，所以多个实现直接 `unwrap`；非法自定义
`TryNextor` 会触发 panic，而不是转换为业务错误。

`TransformIter` 的关键不变量是：每个已拉取且尚未被消费者取走的结果占一个
outstanding 名额；正常上游结束会撤销未对应结果的预占；消费者收到结果时释放名额。
`active_workers` 单独记录已经提交但尚未执行完的 mapper，用于控制生产者退出。
`started` 保证 `inner.take().unwrap()` 只执行一次，`finished` 保证终态幂等。
`pending` 是预留的本地 `VecDeque`；当前实现没有向其中入队，因此正常路径由 channel
直通，但其出队分支仍遵循释放 outstanding 和错误终止规则。

同步组合器的可变状态很小：`TakeIter::n`、`JoinIter::current`/`inner` 和
`WithIndexIter::index`。其他状态主要是被装箱的 `FnMut`，其捕获环境会跨拉取保留。

## 依赖与调用关系

本文件只使用标准库集合、原子、channel 和线程，以及同 crate 的核心迭代器 API：
`Context`、`CancelFunc`、`TryNextor`、`IterResult`、`Emit`、`Throw`、`Done`、
`DoneBy`、`Indexed`、`convertDoneOrErrResult` 和 `source_types::empty`。同目录
`Cargo.toml` 没有声明第三方依赖或 feature，说明该实现完全位于独立 library crate
和标准库边界内。

直接构造关系记录在 `combinators.rs`：`Transform -> TransformIter::new`，
`FilterOut -> FilterIter`，`TakeFirst -> TakeIter`，`Map -> PureMapIter`，
`MapFilter -> FilterMapIter`，`TryMap -> TryMapIter`，`FlatMap/ConcatAll -> JoinIter`，
`Enumerate -> WithIndexIter`。上层通常只看见 `Box<dyn TryNextor<_>>`，因此可以继续链式组合。

RustCodeGraph 将目标文件收录为 24 个符号，并显示它被包括 `br/pkg/utils/backoff.rs`、
`br/pkg/utils/common.rs`、`br/pkg/utils/db.rs` 在内的 32 个文件关联使用；更直接的生产调用证据包括 `restore/restorer.rs::WithSplit` 和
`restore/log_client/log_file_manager.rs::FilterDataFiles`。工厂层本身被 15 个文件关联使用。

## 错误处理与边界

- 上游 `Throw` 在所有同步组合器中不调用用户闭包并原样或换型转发。
  `JoinIter` 在当前子迭代器出错时还把后续 `inner` 替换为 `empty()`，避免错误后继续拼接部分结果。
- `TryMapIter` 的 mapper 错误只转换为本次 `Throw`，结构体本身没有 finished 标志；调用方是否继续拉取由上层消费契约决定。相比之下，`TransformIter` 观察到任何上游或 mapper 错误都会取消并进入永久完成态。
- `TakeIter` 在拉取前递减 `n`。如果第 n 次上游已结束或出错，额度仍被消耗；
  `n == 0` 时则完全不触碰上游。
- `FilterIter`/`FilterMapIter` 可以连续同步拉取任意多项；若上游无限地产生全部应跳过的项，一次 `TryNext` 可能永不返回。
- `JoinIter` 用递归跳过空子迭代器；大量连续空子流会增加调用栈深度，是扩展时应评估的边界。
- `WorkerPool::new(0, ...)` 实际得到上限 1；`Transform` 的零 buffer 也在
  `start` 中变为 1。池名参数当前未用于日志或指标。
- `WorkerPool::Apply` 不捕获 worker panic。panic 会跳过 `inflight.fetch_sub`；在共享池达到上限后，后续提交可能永久自旋。这是当前代码事实，不能把它描述成具备 panic 隔离。
- worker 发送结果时忽略 channel 关闭错误；这是消费者已经结束时的清理路径。

## 并发与资源生命周期

`TransformIter` 的生产者和每个 mapper 任务都使用 OS 线程。`WorkerPool` 只限制同时执行的任务数，并不复用线程；等待配额、等待 outstanding 降低和等待 worker 清空都通过
`thread::yield_now()` 自旋让出调度，没有条件变量和公平性保证。原子操作统一使用
`SeqCst`，因此计数可见性明确，但高负载或消费者停顿时可能消耗 CPU。

取消链遵循 `iter.rs::Context` 的父到子传播：父 context 取消会使 child 的 `Done`
为真；本地 `CancelFunc` 只取消 child，不反向取消父。调用方取消、上游错误和 mapper
错误均能阻止生产者继续调度。生产者句柄没有在 `Drop` 中 join；消费者通过
`is_finished` 判断终止并靠 channel 生命周期收尾。若调用方创建后从不拉取，线程不会启动；若启动后提前丢弃且没有取消父 context，已启动任务仍会运行到发送失败或上游终止。

同步组合器不创建线程、锁或 channel；但其 trait object 要求 `Send`，闭包也要求
`Send`，使整个链可以被 `TransformIter` 的生产者线程取得所有权。`JoinIter` 对子迭代器保持独占所有权，不并行消费它们。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/utils/iter/combinator_types.go`，公开行为由
`combinator_test.go` 定义。类型对应为：`bufferedMappingCfg` ↔
`BufferedMappingCfg`、`bufferedMapping` ↔ `TransformIter`、`filter` ↔
`FilterIter`、`take` ↔ `TakeIter`、`pureMap` ↔ `PureMapIter`、`filterMap` ↔
`FilterMapIter`、`tryMap` ↔ `TryMapIter`、`join` ↔ `JoinIter`、`withIndex` ↔
`WithIndexIter`。

两版共同契约包括：Transform 懒启动、buffer 限制未消费结果、并发结果不保证顺序、
错误触发取消；FilterOut 谓词真表示丢弃；MapFilter 的 bool 真表示 skip；Join 当前子流错误后禁止后续拼接；下标只在成功产出后递增。Rust 的
`combinator_test.rs` 对拍 Go 的并发、过滤、编号、失败、截断收集和 Transform 错误场景，`parity_test.rs::go_rust_public_contract_matches` 进一步覆盖全部公开组合器，
`transform_backpressure_test.rs::transform_does_not_pull_past_unconsumed_buffer` 专门约束 buffer=2 时消费一个结果后最多发生三次上游拉取。

实现机制并非逐句相同：Go 使用 buffered `outstanding`/`results` channel、goroutine、
`sync.WaitGroup` 和 `util.WorkerPool`；Rust 使用无界结果 channel、原子 outstanding、
独立 OS 线程和轮询，实际缓冲约束由“拉取前计数、消费后减计数”实现。Go 的结果发送会在 context 取消时放弃，Rust worker 仍尝试发送但忽略失败。Go `join` 递归、Rust
`JoinIter` 也递归；两者均存在连续空子流的栈深风险。Rust 错误载体简化为 `String`，
不保留 Go error 的具体类型。

## 扩展指南

- 新增公开组合器时，把具体状态机放在本文件，把用户入口与配置规范化放在
  `combinators.rs`，并继续返回 `Box<dyn TryNextor<_>>`；不要把单元测试内嵌到生产源文件，应扩展同目录独立的 `combinator_test.rs` 或新增独立 `*_test.rs`。
- 修改 Transform 时必须同时维护三类计数/终态：outstanding 背压、active worker
  收尾、started/finished 幂等。任何新增的发送或提前返回分支都要核对是否预占、释放或转移 outstanding，并覆盖取消和上游先报错的情况。
- 如需保证结果顺序，需要在任务提交时分配序号并在消费者侧重排；可使用现有
  `pending` 字段，但必须定义错误到达时是否等待更早任务，以及 buffer 是否包含待重排结果。该变化会影响性能和 Go 对等契约，不能只在 Rust 侧静默改变。
- 如要增强池的健壮性，应优先处理 panic 后配额归还和可等待通知；改变自旋策略、
  线程复用或公平性时应增加压力/取消测试，并评估与 Go `util.WorkerPool` 的差异。
- 修改 Filter/Map/Join 语义时，同步检查 `parity_test.rs`、`combinator_test.rs`、
  `transform_backpressure_test.rs` 和 Go `combinator_test.go`。生产使用点至少复核
  `restore/restorer.rs::WithSplit` 与 `restore/log_client/log_file_manager.rs::FilterDataFiles`，尤其不要颠倒 `FilterOut` 和 MapFilter 的布尔方向。
- 下标类型当前对齐为 `i32`；若扩为更大类型，需要同步 `iter.rs::Indexed`、
  `WithIndexIter`、公开 API、Go 兼容性与序列化/调用点假设。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter
  br/pkg/utils/iter` 确认目标、模块入口、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file br/pkg/utils/iter/combinator_types.rs`：核对文件全部
  429 行及 24 个符号；`query` 精确定位 `TransformIter`、`FilterIter`、
  `JoinIter`、`WithIndexIter`。
- RustCodeGraph `node` 读取 `combinators.rs`，确认九个公开工厂到本文件类型的构造关系；读取 `iter.rs`，确认 `Context`、三态 `IterResult`、`TryNextor`、
  `DoneBy` 与 `CollectAll` 契约。
- 读取 `Cargo.toml` 与 `lib.rs`，确认 crate 名、无第三方依赖、模块注册、测试模块独立挂载及公开重导出。
- 读取 Go `combinator_types.go` 与 `combinator_test.go`，确认移植类型、背压/取消流程和行为断言；读取 Rust `combinator_test.rs`、`parity_test.rs`、
  `transform_backpressure_test.rs`，确认并发无序、错误传播、过滤方向、展平、编号和背压边界。
- RustCodeGraph/源码检查生产调用点 `restore/restorer.rs::WithSplit` 与
  `restore/log_client/log_file_manager.rs::FilterDataFiles`，确认这些组合器参与恢复流水线和日志文件筛选，而非仅供测试使用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的命令验证目标文档存在且恰好具有上述 11 个固定二级章节，并人工检查未把预期设计写成当前事实。
