# `pkg/lightning/duplicate/worker.rs`

## 文件定位

本文件实现 Lightning 重复键检测器的并行扫描执行层。上游 `pkg/lightning/duplicate/detector.rs` 先通过 `Detector::detect_inner` 对写入 `ExternalSorter` 的内部键排序，取得首尾边界，再创建一个 `TaskQueue` 初始区间并启动多个 `Worker`；本文件随后把已排序的内部键流按半开区间 `[start_key, end_key)` 扫描，将相邻且业务键相同的记录汇成重复组，通过 `Handler` 回调交给调用方处理。

该文件属于 Cargo 包 `astersql-lightning-duplicate`（`pkg/lightning/duplicate/Cargo.toml`），由 `pkg/lightning/duplicate/lib.rs` 的私有 `mod worker` 纳入 crate，并通过 `pub use worker::*` 再导出其中的公开项。当前只有 `gen_split_key` 是对 crate 外公开的函数；`Task`、`TaskQueue` 和 `Worker` 均为 `pub(crate)`，其余状态和辅助方法仅在本模块内部可见。

## 核心职责

1. `TaskQueue` 用互斥锁、条件变量和 `pending` 计数模拟 Go 版本中容量为 1 的 `taskCh` 加 `WaitGroup`：分发初始区间、接受动态拆分任务、等待全部任务完成，并处理正常关闭或异常中止。
2. `Worker::run` 持续领取任务；`Worker::run_task` 为每个任务建立日志作用域，保证任务记账和 `Handler::close` 被执行，并把扫描错误或资源关闭错误返回给 `Detector`。
3. `Worker::scan_iterator` 在排序迭代器上做相邻比较，把同一用户键对应的多个 `key_id` 按 `begin → append... → end` 协议报告为一个重复组，同时原子累加重复组数。
4. 长任务每处理 1000 个键检查一次取消状态并尝试从剩余区间生成新任务，使空闲 Worker 能参与扫描。
5. `gen_split_key` 和 `common_prefix_len` 生成字典序中间键；其字节算术刻意保持 Go `byte`/`uint8` 的回绕行为。

## 主要符号

- `Task { start_key, end_key }`：待扫描的 `InternalKey` 半开区间。边界同时包含用户键和 `key_id` 的排序语义，比较由 `compare_internal_key` 完成。
- `QueueState`：在同一把 `Mutex` 下维护 `queue: VecDeque<Task>`、队列内及执行中的总任务数 `pending`、正常关闭标志 `closed` 与异常终止标志 `aborted`。
- `TaskQueue::with_initial`：放入唯一初始任务并把 `pending` 设为 1。
- `TaskQueue::receive`：优先弹出已有任务；队列关闭/中止时返回 `Closed`，调用令牌取消时返回 `Canceled`，否则以 10 ms 条件变量超时等待来周期性观察取消。
- `TaskQueue::try_split`：只有队列为空且未关闭/中止时才入队一个拆分任务，并在成功后增加 `pending`。这对应 Go 中对容量 1 channel 的非阻塞发送。
- `TaskQueue::{complete_task, wait_until_idle, close, abort}`：分别完成一个任务、等待 `pending == 0`、禁止后续拆分并唤醒 Worker、以及清空队列并强制把待完成计数归零。
- `Worker`：持有共享的 `ExternalSorter`、`TaskQueue`、`AtomicI64` 重复组计数、该 Worker 独占的 `Handler`，以及结构化 `Logger`。
- `Worker::{run, run_task, scan_task, scan_iterator}`：从任务循环逐层进入单任务日志/清理、迭代器生命周期和实际重复扫描逻辑。
- `gen_split_key(start_key, end_key)`：公开的拆分键算法；覆盖相等键、前缀、中间字节存在空隙、相邻字节以及尾部 `0xff` 等情况。
- `common_prefix_len`：返回两个字节切片首个不同位置，或较短切片长度。

## 执行流程

1. `Detector::detect_inner`（`detector.rs`）排序输入，用 `get_range_bounds` 取得最小内部键和排他性上界，并调用 `TaskQueue::with_initial`。
2. `Detector` 为每个并发槽构造独立 `Handler` 和 `Worker`，在线程中调用 `Worker::run`。任一构造或 Worker 错误会记录首个错误、取消派生令牌并调用 `TaskQueue::abort`。
3. `Worker::run` 从队列领取 `Task`。正常关闭返回成功；等待期间观察到取消则返回 `Interrupted`；取得任务则进入 `run_task`。
4. `run_task` 记录初始边界，调用 `scan_task`，随后无论扫描成功与否都执行 `complete_task` 和 `handler.close()`，最后记录实际结束边界与已处理键数。
5. `scan_task` 从 sorter 新建迭代器，调用 `scan_iterator`，并在扫描返回后忽略迭代器 `close` 的结果、返回扫描结果。
6. `scan_iterator` 编码任务起点并 `seek`，逐项解码 `InternalKey`；当当前内部键达到 `end_key` 时停止，因此区间上界不包含在本任务内。
7. 当前用户键与前一用户键相同且尚未进入重复组时，依次调用 `Handler::begin(key)`、追加前一条和当前条的 `key_id`，并把 `num_dups` 加一；组内后续记录只追加 `key_id`。遇到新用户键或扫描结束时调用 `Handler::end()`。
8. 每 1000 次迭代，Worker 先检查取消，再在当前用户键与任务上界的用户键之间计算拆分键。只有拆分键严格大于当前键且 `try_split` 成功时，才把原任务的 `end_key` 收缩到拆分点；新任务负责 `[split_key, old_end)`。
9. 所有任务的 `pending` 归零后，`Detector` 的协调线程从 `wait_until_idle` 返回并调用 `close`，使仍在 `receive` 中的 Worker 正常退出；线程作用域结束后派生令牌即使成功也会被取消，但调用者传入的令牌不受影响。

## 数据与状态

外排输入键由 `pkg/lightning/duplicate/internal.rs` 定义：`InternalKey` 包含用户键 `key` 和来源标识 `key_id`。`encode_internal_key` 先对用户键做 memcomparable 编码，再原样追加 `key_id`，因此 sorter 输出先按用户键、再按 `key_id` 排序。扫描只用 `key` 判断是否属于同一重复组，却按排序结果顺序上报 `key_id`；`detector_test.rs::verify_results` 明确断言每组至少两项、`key_id` 非递减且能回指原输入。

队列的重要不变量是：`pending` 表示“排队中 + 正在执行”的任务总数。初始值为 1；拆分成功后加一；每个已领取任务在 `run_task` 返回扫描结果后减一。`try_split` 把“检查队列为空、入队、增加 pending”置于同一锁内，避免 Go 版本 `len(taskCh) == 0` 与非阻塞发送之间的竞争在 Rust 实现中破坏记账。`abort` 是故障路径的强制收敛点，会清队列、清 pending，并同时设置 `aborted` 和 `closed`。

`previous_key` 与 `current_key` 在循环尾部用 `swap` 复用其内部缓冲，降低逐键分配。`processed_keys` 只用于任务结束日志；`num_dups` 使用 `Ordering::Relaxed`，因为这里只需要跨线程原子计数，最终值由线程作用域结束后的 `Detector::detect` 读取，不承担其他状态同步职责。

## 依赖与调用关系

直接上游是 `pkg/lightning/duplicate/detector.rs::Detector::detect_inner`：它构造 `TaskQueue` 和 `Worker`，调用 `Worker::run`，在成功路径调用 `wait_until_idle`/`close`，失败路径调用 `abort`。再上游是 `Detector::detect`；输入通过同文件的 `KeyAdder` 写入 sorter。

直接下游包括：

- `pkg/lightning/duplicate/internal.rs` 的 `InternalKey`、`encode_internal_key`、`decode_internal_key` 和 `compare_internal_key`，提供边界编码、迭代项解码与内部键全序。
- `astersql-util-extsort`（在 Cargo 中以 `extsort-dependency` 引入）的 `ExternalSorter` 和迭代器 trait，提供 `new_iterator`、`seek`、`valid`、`next`、`take_error` 与 `close`。
- `pkg/lightning/duplicate/detector.rs::Handler`，接收重复组生命周期回调；每个 Worker 的实例由 `HandlerConstructor` 独立构造。
- `tokio_util::sync::CancellationToken`，用于父子取消传播；本文件没有 Tokio 异步任务，Worker 实际由 `std::thread::scope` 启动。
- `astersql-lightning-log`（经 `lightning-log-dependency` 再导出）的 `Logger`、`Field` 和 `Level`，记录任务边界、处理数量和错误。

Cargo 清单还声明 `goish` 与 codec 依赖，但本文件不直接使用；它们分别服务于同 crate 的默认并发度和内部键编码。`rand`、`tempfile` 是测试依赖。

## 错误处理与边界

排序器创建迭代器、内部键解码、迭代器延迟错误以及 `Handler::{begin, append, end, close}` 都使用同一个外排 `Error` 类型向上传播。`run_task` 的优先级与 Go defer 逻辑一致：扫描成功而 `Handler::close` 失败时返回关闭错误；扫描已经失败时保留扫描错误，不用关闭错误覆盖。`scan_task` 始终尝试关闭已成功创建的迭代器，但明确忽略 `iterator.close()` 的错误。

取消有两个观察点：空队列等待时 `receive` 最多以约 10 ms 的轮询粒度检查；扫描期间每 1000 个迭代项显式检查一次。观察到取消后构造 `io::ErrorKind::Interrupted`。Sorter 自身的方法还会收到同一个令牌，但其内部响应时机属于外排实现的契约。

半开区间比较使用完整 `InternalKey`，而动态拆分点的 `key_id` 为空。拆分只有在生成的用户键严格大于当前用户键时才生效，因此不会把当前正在聚合的同键重复组切到另一个任务。`gen_split_key` 对相等键返回副本；对 `start` 为 `end` 前缀的情况追加 `end` 下一字节的一半；对分歧字节有间隙时取中点；否则沿 `start` 补 `0xff`。Rust 使用 `wrapping_add`/`wrapping_sub` 对齐 Go 无符号字节溢出，包括测试专门覆盖的降序非前缀输入。

互斥锁或条件变量中毒通过 `expect("duplicate task queue poisoned")` 触发 panic，而不是转换为业务错误。这是内部并发状态被破坏时的硬失败边界。`complete_task` 在 `pending == 0` 时不会下溢，但正常协议要求每个已领取任务只完成一次。

## 并发与资源生命周期

多个 Worker 共享 `Arc<dyn ExternalSorter>`、`Arc<TaskQueue>` 和 `Arc<AtomicI64>`；`Handler` 则归单个 Worker 独占。任务队列所有复合状态都受同一 `Mutex<QueueState>` 保护，`Condvar` 用于两类唤醒：新拆分任务唤醒一个等待 Worker，关闭、归零或中止唤醒所有等待者。

资源顺序为：Detector 创建派生取消令牌和 scoped threads；每个任务创建独立 sorter 迭代器；任务扫描结束先关闭迭代器，再完成队列记账，再关闭 Handler；所有 pending 任务完成后关闭队列；线程退出后取消派生令牌。异常时首个错误线程取消派生令牌并 abort 队列，其余 Worker 随后从等待或周期检查中退出。`detector_test.rs::test_detector_keeps_root_error_when_other_constructor_observes_cancellation` 验证并发失败保留根错误而不是后续取消错误。

`Handler::close` 是按任务执行，而不是只在 Worker 线程最终退出时执行，这一点直接继承 Go `runTask` 中的 defer。扩展 Handler 时必须使其生命周期符合该调用方式，并保证 `end` 完成的组在 `close` 前已被持久化或发送。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/lightning/duplicate/worker.go`。Rust `Task`/`Worker`、`run`、`run_task`、1000 项检查周期、相邻键状态机、重复组计数、迭代器关闭、Handler 关闭和拆分算法均按 Go 对应结构实现。

主要表示差异是并发原语：Go 使用容量为 1 的 `chan task` 与 `sync.WaitGroup`，Rust 用 `TaskQueue { Mutex<QueueState>, Condvar }` 合并实现相同的队列和待完成记账。Go `select` 同时等待 channel 与 `ctx.Done()`；Rust 在 `receive` 中用条件变量短超时观察 `CancellationToken`。Go `errgroup` 的协调语义由 `detector.rs` 的 scoped threads、首错槽、派生令牌和 `abort` 组合实现。

Rust `gen_split_key` 额外把 Go `uint8` 算术显式写成 wrapping 运算，避免 debug 构建中的整数溢出 panic；`worker_test.rs::split_key_preserves_go_byte_overflow` 覆盖了 Go 测试之外的降序输入。其余六组标准边界与 `worker_test.go::TestGenSplitKey` 一一对应，并同时出现在 Rust `worker_test.rs::test_gen_split_key` 与 `migration_aster_unit_test.rs::split_key_matches_go_cases`。

## 扩展指南

- 修改重复组判定或回调顺序时，应集中改 `Worker::scan_iterator`，并同步扩展独立测试 `pkg/lightning/duplicate/detector_test.rs`；至少验证二元/多元重复组、单键不报告、`key_id` 有序，以及 `begin`/`append`/`end` 任一步失败时的部分计数和资源清理。不要把测试内嵌进 `worker.rs`。
- 修改拆分策略时，应同时检查 `gen_split_key`、`TaskQueue::try_split` 和任务上界收缩逻辑，保持“拆分点严格大于当前用户键”“不切开同键组”“pending 只在入队成功时增加”。同步维护 `worker_test.rs` 和 Go 对照 `worker_test.go` 的边界用例。
- 修改队列容量、关闭或取消语义时，应连同 `detector.rs::Detector::detect_inner` 审查，因为 `wait_until_idle → close` 和错误路径 `cancel → abort` 是完整协议的一部分。重点风险是等待方永久阻塞、pending 泄漏/提前归零、首错被取消错误覆盖。
- 修改清理策略时，应保留 Go 的错误优先级：扫描错误优先于 Handler 关闭错误，并明确决定是否继续忽略迭代器关闭错误。`detector_test.rs::test_detector_failure_contracts` 是最接近的回归面。
- 调整 `CHECK_INTERVAL` 或增加更频繁的同步会改变取消延迟、锁竞争和任务拆分开销；性能验证应使用足够大的已排序输入，并观察多 Worker 的负载均衡，而不能只依赖 `gen_split_key` 单测。
- 若改变公开接口，注意当前 crate 通过 `lib.rs` 通配再导出 `gen_split_key`；`TaskQueue` 与 `Worker` 的 crate 内可见性则有意把调度细节限制在实现层。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/lightning/duplicate` 确认目标源、Go 对照和独立测试均已索引。
- RustCodeGraph 源码/结构查询：`node --file pkg/lightning/duplicate/worker.rs --offset 1 --limit 260`、`node --file pkg/lightning/duplicate/worker.rs --offset 245 --limit 180`，核对本文件全部 364 行；`node --file pkg/lightning/duplicate/detector.rs --offset 1 --limit 260`，核对直接上游构造、调用、等待、关闭和中止关系。
- RustCodeGraph 符号查询：`query gen_split_key --kind function`、`query TaskQueue --kind struct`、`query Worker --kind struct`，定位目标实现及测试符号；宽泛 `Worker`/`callers` 查询存在跨仓库同名歧义，最终调用边以精确文件节点为准。
- crate 与模块依据：`pkg/lightning/duplicate/Cargo.toml`、`pkg/lightning/duplicate/lib.rs`；目标目录不存在 `doc.go`。
- 实现与 Go 对照：`pkg/lightning/duplicate/worker.rs`、`pkg/lightning/duplicate/worker.go`、`pkg/lightning/duplicate/internal.rs`、`pkg/lightning/duplicate/detector.rs`。
- 测试依据：`pkg/lightning/duplicate/worker_test.rs`、`pkg/lightning/duplicate/worker_test.go`、`pkg/lightning/duplicate/detector_test.rs`、`pkg/lightning/duplicate/detector_test.go`、`pkg/lightning/duplicate/migration_aster_unit_test.rs`。其中 `test_detector`/`verify_results` 验证大规模并发输入、重复组完整性和 key_id 顺序；`test_detector_failure_contracts` 验证扫描/回调错误身份与清理次数；取消及构造失败测试验证派生上下文和 abort 收敛。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令验证文件存在性和章节数，并人工复核上述符号、调用边、边界与扩展点均有直接来源。
