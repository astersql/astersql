# [`pkg/lightning/duplicate/detector.rs`](detector.rs)

## 文件定位

本文件实现 `astersql-lightning-duplicate` crate 的重复键检测编排层。它位于键写入与真正的有序区间扫描之间：`KeyAdder` 把 `(用户键, key_id)` 编码后交给外部排序器，`Detector::detect` 完成排序、确定扫描边界并启动多个 `Worker`，具体的相邻键比较和重复组回调由同 crate 的 `worker.rs` 执行。

crate 入口 `pkg/lightning/duplicate/lib.rs` 将本文件的公开项全部再导出；根 `Cargo.toml` 以 `facade_lightning_duplicate` 引入该 crate，`pkg/lib.rs::lightning::duplicate` 再次对外导出。当前 Rust Lightning importer 尚未直接接线到这里：`lightning/pkg/importer/dup_detect.rs` 使用的是 `lightning/pkg/importer/stubs.rs::duplicate` 中的 Go 风格桩。对应的 Go 主链 `lightning/pkg/importer/dup_detect.go` 则调用 `pkg/lightning/duplicate/detector.go::NewDetector`。因此，本文件是已实现、可独立测试的真实 detector，但不能据现有调用证据宣称 Rust importer 已使用它。

## 核心职责

- `new_detector` 保存共享的 `ExternalSorter`，并给日志器增加 `component=duplicate.Detector` 字段。
- `Detector::key_adder` 为每个写入方创建独立 sorter `Writer`，使多个生产者能够增量提交待检测键。
- `KeyAdder::{add, flush, close}` 保持外排写入协议：编码内部键、写空 value、显式刷写和关闭。
- `Detector::detect` 先排序，再建立覆盖全部内部键的半开区间，按配置并行运行 `Worker`，原子统计“重复用户键组”的数量并返回首个失败。
- `DetectOptions::ensure_defaults` 补齐 Go 对齐的默认并发度和空操作 handler，使只关心重复组数量的调用者无需实现回调。
- `Handler` 定义每个 worker 的有状态回调协议；本文件只定义协议与默认实现，重复组识别发生在 `worker.rs::Worker::scan_iterator`。

## 主要符号

- `pub struct Detector { sorter, logger }`：检测生命周期的入口。`sorter` 是 `Arc<dyn ExternalSorter>`，供多个 scoped thread/worker 共享；字段不公开。
- `pub fn new_detector(sorter, logger) -> Detector`：公开构造函数；返回值不是 `Arc`，但内部 sorter 已共享。
- `Detector::key_adder(&CancellationToken) -> Result<KeyAdder, Error>`：公开写入入口，直接传播 `ExternalSorter::new_writer` 错误。
- `Detector::detect(&CancellationToken, Option<&mut DetectOptions>) -> (i64, Result<(), Error>)`：公开检测入口。即使失败也返回当前已经计入的重复组数，以保留 Go `(numDups, error)` 语义。
- `Detector::detect_inner`：私有编排实现，处理默认值、排序、区间、线程、取消、任务队列和首错收集。
- `Detector::get_range_bounds`：私有边界计算。读取排序器首尾项，解码为 `InternalKey`，并在末尾用户键后追加 `0` 字节构造排他的上界。
- `pub struct KeyAdder`：持有独占 `Box<dyn Writer>` 与复用的 `key_buf`；不是共享写入器。
- `KeyAdder::{add, flush, close}`：公开 writer 包装。`add` 调用 `internal.rs::encode_internal_key`，把 value 固定为空切片。
- `pub type HandlerConstructor`：可在线程间共享的 `Arc<dyn Fn(&CancellationToken) -> Result<Box<dyn Handler>, Error> + Send + Sync>`；每个 worker 调用一次，得到自己的 handler。
- `pub struct DetectOptions`：包含 `concurrency: i64` 与可选 `handler_constructor`。默认派生值为零/空，再由 `ensure_defaults` 转为运行时默认值。
- `pub trait Handler: Send`：定义 `begin(key) -> append(key_id)... -> end()` 的重复组协议以及最终 `close()`。
- `NopHandler`：私有默认 handler，四个回调均成功且无副作用。

## 执行流程

1. 调用者通过 `new_detector` 绑定 sorter 和 logger，再从 `Detector::key_adder` 获取一个或多个 `KeyAdder`。
2. `KeyAdder::add` 清空但复用 `key_buf`，构造 `InternalKey(key, key_id)`，通过 `encode_internal_key` 生成“memcomparable 用户键 + 原始 key_id”，然后执行 `Writer::put(encoded, [])`。这一编码让 sorter 先按用户键、再按 key_id 排序。
3. 写入者应调用 `flush`（按需）和 `close`；本文件不在 `Drop` 中隐式完成 writer。
4. `Detector::detect` 创建共享 `AtomicI64`，调用 `detect_inner`，最后无论成功失败都读取并返回累计值。
5. `detect_inner` 就地补齐传入的 options；若调用者传 `None`，则只在本次调用的局部默认对象上补齐。
6. sorter 在 `"sort keys"` 日志任务中执行 `sort`。失败会记日志并立即返回，尚未创建 iterator 或 worker。
7. `get_range_bounds` 用 iterator 的 `first`/`last` 获取总范围；空 sorter 得到两个默认空键，因 `start >= end` 返回成功且计数为零。非空时，末键的用户键追加 `0`，使最大内部键也落在 `[start, end)` 内，包括 `0xff` 等边界用户键。
8. 以一个覆盖全范围的 `Task` 初始化 `TaskQueue`，创建派生的 `CancellationToken`、首错槽 `Mutex<Option<Error>>`，并从 options 取出 handler 工厂。
9. `std::thread::scope` 按 `concurrency` 启动线程。每个线程先构造自己的 handler，再创建 `Worker` 并运行；`Worker` 从队列领取/拆分区间，在排序流中把相同用户键的连续记录报告为一个重复组，并用共享原子计数加一。
10. 任一工厂或 worker 失败时，线程只保存第一个错误，取消派生 token 并 `TaskQueue::abort`，从而清除待办计数并唤醒协调者。主线程等待队列 idle 后关闭队列，scope 退出会 join 全部线程。
11. 无论成功失败，scope 之后都取消派生 token，以对齐 Go `errgroup.Wait` 完成后的 context 生命周期；调用者传入的 token 不被取消。最后取出并返回首错，或返回成功。

## 数据与状态

`Detector` 本身只保存 sorter 与 logger，不缓存业务键。全部待检测数据位于 sorter；`KeyAdder::key_buf` 只是单个 adder 内复用的编码暂存区。`key` 是判重维度，`key_id` 用于定位原始输入，同一 `key` 下按字典序排列；测试 `verify_results` 和 `detector_reports_duplicate_groups_and_sorted_key_ids` 都验证了每组至少两个 key_id、顺序稳定且能够回指原输入。

重复数量 `num_dups` 表示重复“组”数，不是重复记录数。它由 `Worker::scan_iterator` 在首次确认一组相邻同键记录时以 `Ordering::Relaxed` 增加，因此不承担跨字段同步职责。检测失败时计数可能是部分结果：`detector_test.rs::test_detector_failure_contracts` 明确验证 `Next`、`End`、`Close` 等不同失败点对应的部分计数。

`DetectOptions` 若以 `Some(&mut ...)` 传入会被就地修改：非正并发度替换为 `goish::runtime::GOMAXPROCS(0)`，空工厂替换为 `NopHandler` 工厂；`test_detector_empty_and_default_handler` 验证了这个可观察副作用。

## 依赖与调用关系

上游公开路径是 `pkg/lightning/duplicate/lib.rs` 的 `pub use detector::*`，以及 `pkg/lib.rs::lightning::duplicate` 的 facade 再导出。仓库搜索只发现本 crate 的 `detector_test.rs` 和 `migration_aster_unit_test.rs` 直接调用 Rust `new_detector`；当前 importer 的同名调用落在其本地桩而非本 crate。Go 侧真实上游是 `lightning/pkg/importer/dup_detect.go::{run, addKeys}`：建立磁盘 sorter、并发写键、按冲突策略构造 handler，然后检测重复。

直接下游如下：

- `util/extsort::ExternalSorter`：创建 writer、排序、创建首尾/区间 iterator；`Writer` 接收编码键。
- `internal.rs::{InternalKey, encode_internal_key, decode_internal_key, compare_internal_key}`：定义排序表示、边界解码与逻辑比较。
- `worker.rs::{Task, TaskQueue, Worker}`：执行半开区间扫描、动态拆分、handler 回调和重复组计数。
- `tokio_util::sync::CancellationToken`：向 sorter/handler/worker 传递取消，并隔离调用者 token 与 worker 派生 token。
- `lightning/log`：为排序阶段和后续 worker 日志提供结构化 logger。
- `goish::runtime::GOMAXPROCS`：提供与 Go 一致的缺省 worker 数。

`pkg/lightning/duplicate/Cargo.toml` 声明的运行时依赖正对应以上边界：`astersql-util-codec`、`astersql-util-extsort`、`astersql-lightning-log`、`goish` 与 `tokio-util`；`rand`、`tempfile` 仅供测试。

## 错误处理与边界

外部错误统一使用 `extsort::external_sorter::Error`（可装箱的动态错误）原样传播。`key_adder`、`add`、`flush`、`close` 不包装 writer 错误；`test_key_adder_preserves_writer_errors` 以指针同一性验证这一点。sort、iterator 创建、首尾读取/解码、handler 构造和 worker 扫描错误也保持原错误身份；多 worker 同时失败时只返回抢先写入 `first_error` 的错误。

`get_range_bounds` 无论闭包成功或失败都会调用 iterator `close`，但刻意忽略 close 的返回值，因此首尾读取/解码结果优先。worker 侧 iterator 与 handler 的关闭规则位于 `worker.rs`：iterator close 错误被忽略；handler close 在扫描成功时可成为返回错误，但不会覆盖已有扫描错误。`test_detector_failure_contracts` 和 `test_detector_closes_iterator_on_invalid_range_key` 覆盖这些优先级与关闭次数。

空输入不是错误；范围比较为 `start >= end` 时直接返回零。非法编码的首/末内部键会在任何 worker 启动前失败。`Detector::detect` 不自动替调用者关闭 sorter，也不替 `KeyAdder` 关闭 writer；调用顺序错误或遗漏关闭由底层 sorter 契约承担。

内部 `Mutex::lock` 和 handler 工厂存在 `expect`：错误槽 mutex 若 poisoned、默认 handler 工厂异常缺失会 panic，而不是返回 `Error`。正常路径中 `ensure_defaults` 保证工厂存在。非正并发度会先被修正；因此不会出现“零 worker 但仍等待初始任务”的死锁。

## 并发与资源生命周期

多个 `KeyAdder` 可以从同一个 `Detector` 创建并由不同线程各自持有；每个 adder 拥有自己的 writer 和缓冲。测试 `test_detector` 以 10 个 scoped thread 写入 10 万个键，验证此用法。单个 `KeyAdder` 没有共享同步，不能把同一实例并发可变使用。

检测线程使用 `std::thread::scope`，所以 `detect_inner` 返回前所有 worker 都已 join。sorter、任务队列、计数器和首错槽通过 `Arc` 共享；`TaskQueue` 内用 `Mutex + Condvar` 管理队列、pending、close/abort。worker handler 不共享，每个线程由 `HandlerConstructor` 单独创建，因此 handler 只要求 `Send`，工厂则要求 `Send + Sync`。

错误时派生 token 的取消负责通知正在运行的 worker，`abort` 负责解除队列 pending 等待；两者共同避免构造器失败或扫描失败导致 `wait_until_idle` 挂起。成功时也显式取消派生 token，`test_detector_cancels_worker_context_after_success` 验证 worker 看到已取消，而调用者 token 仍有效。

资源清理是显式的：调用者关闭每个 `KeyAdder` 和最终 sorter；`get_range_bounds` 关闭自己的 iterator；每个 worker 的每个任务扫描关闭 iterator，并在任务完成后关闭其 handler。若动态拆分让一个 worker 处理多个任务，同一个 handler 会在每个任务末尾调用 `close`，这是当前 `Worker::run_task` 的实际行为，扩展 handler 时不能假定只关闭一次。

## 与 Go 版本的对应关系

本文件逐项对应 `pkg/lightning/duplicate/detector.go`：`Detector/NewDetector`、`KeyAdder`、`Detect/DetectOptions`、`Handler/HandlerConstructor` 和 `nopHandler` 均保持同一职责。Rust 命名采用 snake_case，取消上下文改为 `CancellationToken`，动态接口改为 `Arc<dyn ...>`/`Box<dyn ...>`。

Go 使用 `errgroup.WithContext`、容量为 1 的 `taskCh` 和 `WaitGroup`；Rust 用派生 token、`TaskQueue` 与 scoped threads 重建同样的取消和待办记账。Rust 额外显式保存“首个”错误，确保其他线程随后观察取消而返回的错误不会替换根因；`test_detector_keeps_root_error_when_other_constructor_observes_cancellation` 固化了该契约。

Go 的 `Detect` 在排序或取范围失败时返回 `0, err`；Rust 外层计数器设计使失败时也返回已完成扫描的部分计数。扫描前失败仍为零，扫描中失败的部分计数由 Rust 测试明确约束。两端都将末尾用户键追加零字节形成排他上界，并将默认并发度设为 `GOMAXPROCS(0)`。

Go 接口注释说明 `Append` 的 key_id 按字典序到达，且传入切片调用后可能变化。Rust 的借用切片同样不能由 handler 长期借用；需要保留时必须复制，`detector_test.rs::Collector::append` 正是这样做。当前 Rust importer 尚未替换其本地桩，所以 Go importer 的完整运行时接线只能作为目标语义证据，不能作为本 Rust 文件已经被生产主链调用的证据。

## 扩展指南

- 若改变写入格式或判重维度，应同时修改 `KeyAdder::add`、`internal.rs` 的编解码/比较以及 `worker.rs::scan_iterator`，并扩展独立的 `internal_test.rs`、`detector_test.rs`、`worker_test.rs`；不得把测试嵌入本源文件。
- 若新增检测选项，应在 `DetectOptions` 与 `ensure_defaults` 接入，核对 Go `DetectOptions` 的默认值和可观察的就地修改语义，并测试 `None`、零值、负值及显式值。
- 若改变并发或错误聚合，应保持三项不变量：调用者 token 不被取消、失败不会让 pending 等待挂起、根错误身份不被后续取消错误覆盖。相关回归点是 `test_detector_fail`、`test_detector_failure_contracts` 与 `test_detector_keeps_root_error_when_other_constructor_observes_cancellation`。
- 自定义 handler 必须按状态机实现并复制需要跨回调保留的字节；同时容忍当前 worker 的实际 close 生命周期。新增持久化 handler 时应针对 begin/append/end/close 每个失败点补独立测试。
- 若把 Rust importer 从 `stubs.rs::duplicate` 切换到本 crate，需要显式适配 Go 风格 API（`Context`、PascalCase 方法、单一 `Result<i64>`）与本文件 API（`CancellationToken`、snake_case、`(i64, Result)`），并以 importer 集成测试证明真实接线；不能仅删除桩或依靠 facade 再导出。
- 大数据路径上的性能敏感点包括 `key_buf` 复用、sorter 磁盘 I/O、worker 数量和每 1000 键一次的拆分/取消检查。修改时应避免每键分配或把 handler 变为全局锁热点。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/lightning/duplicate` 找到本 crate 的 Rust/Go 实现与测试；`node --file pkg/lightning/duplicate/detector.rs --offset 1 --limit 260` 和后续区间读取确认本文件 279 行的全部定义；同样读取 `worker.rs` 与 `internal.rs` 核对直接下游实现。
- RustCodeGraph 精确符号查询：`query new_detector --kind function` 定位 `detector.rs:46`，`query Detector --kind struct` 同时区分 Rust、Go 与其他同名 detector。对常见 impl 方法的 `query/callers` 未能稳定区分同名符号且一次查询长时间无输出，因此按技能规则用路径限定的仓库搜索补足调用证据，没有把模糊图结果当作结论。
- 读取的生产与装配文件：`pkg/lightning/duplicate/detector.rs`、`internal.rs`、`worker.rs`、`lib.rs`、`Cargo.toml`，根 `Cargo.toml`、`pkg/lib.rs`，以及 `lightning/pkg/importer/dup_detect.rs`、`stubs.rs`。
- Go 对照：`pkg/lightning/duplicate/detector.go`；真实 Go 上游为 `lightning/pkg/importer/dup_detect.go`。
- 独立测试：`pkg/lightning/duplicate/detector_test.rs`、`migration_aster_unit_test.rs` 与 Go `detector_test.go`。它们覆盖并发多 writer、重复组和 key_id 顺序、默认值、空输入与末键边界、派生取消、首错身份、writer/sorter/iterator/handler 失败和资源关闭。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前以指定命令检查目标存在且恰有 11 个固定二级标题，并人工复核没有将 importer 桩误写成真实接线。
