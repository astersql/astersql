# `br/pkg/utils/worker.rs`

## 文件定位

`worker.rs` 属于 `astersql-br-pkg-utils` library crate。crate 根 `br/pkg/utils/lib.rs` 通过 `#[path = "worker.rs"] pub mod worker` 编入该文件，并把这里的全部公开能力再次导出到 crate 根：`PanicToErr`、`CatchAndLogPanic`、`AsyncStreamBy`、`WorkerResult`、`WorkerTokenChannel`、`BuildWorkerTokenChannel` 及两个容量常量。独立测试由同一 crate 根在 `cfg(test)` 下挂载 `br/pkg/utils/worker_test.rs`，测试逻辑没有内嵌在生产文件中。

本文件移植自同路径 `br/pkg/utils/worker.go`，集中提供三类互相独立的基础能力：把 panic 转为错误或日志、把阻塞式生成器包装为带背压的异步流、构造预填充的并发令牌通道。RustCodeGraph 与全仓精确搜索确认：当前 Rust 生产代码尚未直接调用这些导出符号，已验证的直接上游只有 `br/pkg/utils/lib.rs` 的公开再导出和 `worker_test.rs`；`br/pkg/backup/prepare_snap/stream.rs::AsyncStreamBy`、`br/pkg/restore/snap_client/stubs.rs::BuildWorkerTokenChannel` 是各自模块的同名局部实现，并非本文件的调用者。Go 版本则已接入 stream helper、snapshot restore 和 prepare-snapshot 主链。因此本文描述的是已编译、已测试的公共移植实现，而不声称 Rust BR 生产流程已经完成同等接线。

## 核心职责

- `PanicToErr` 执行闭包，正常时保留返回值，panic 时构造带上下文的 `ErrUnknown`、记录 Warn 并返回 `Err`（`worker.rs:48-76`）。
- `CatchAndLogPanic` 执行尽力而为的闭包，正常时返回 `Some(value)`，panic 时只记录 Warn 并返回 `None`（`worker.rs:78-94`）。
- `AsyncStreamBy` 在独立线程中反复调用生成器，通过零容量同步通道逐帧交付成功项，并以一个错误帧终止流（`worker.rs:96-143`）。
- `WorkerTokenChannel` 用共享互斥状态和条件变量实现可克隆、固定容量的令牌池；`BuildWorkerTokenChannel` 负责输入容量的默认化、上限钳制和预填充（`worker.rs:145-229`）。

文件不管理具体 BR 任务、不调度业务 future，也不创建通用线程池；“worker”在此特指 panic 边界、生成线程和并发令牌这些调度辅助原语。

## 主要符号

- `DefaultWorkerTokenChannelSize: u32 = 128`：请求容量为 0 时采用的默认值。
- `MaxWorkerTokenChannelSize: u32 = 30 * 1024 * 1024`：允许的容量上限，防止异常配置导致过大的逻辑令牌池。
- `panic_message(Box<dyn Any + Send>) -> String`：私有 panic payload 格式化器。依次识别 `&'static str`、`String`，其他 payload 使用 trait object 的 Debug 表示。
- `panic_into_shared_error(...) -> SharedError`：私有转换器，以克隆的 `astersql_br_pkg_errors::ErrUnknown` 为 cause，并用 `Annotate` 添加 `panicked when executing, message: ...` 上下文。
- `PanicToErr<F, T>(f) -> Result<T, SharedError>`：公开 panic-to-error 边界。泛型闭包是一次性调用且要求 `UnwindSafe`。
- `CatchAndLogPanic<F, T>(f) -> Option<T>`：公开 panic-to-log 边界，同样只调用闭包一次。
- `Result<T>`：异步流帧，包含公开字段 `Err: Option<SharedError>` 与 `Item: T`；以 Go 风格字段名保持移植代码形态。crate 根将它别名再导出为 `WorkerResult`，避免与标准 `Result` 混淆。
- `AsyncStreamBy<T, F>(generator) -> mpsc::Receiver<Result<T>>`：公开流构造器。`T` 必须 `Default + Send + 'static`，生成器必须可变调用、可发送且为 `'static`。
- `WorkerTokenState { available, capacity }`：私有状态；二者均为 `u32`，构造后 `capacity` 不变。
- `WorkerTokenChannel`：公开的可克隆句柄，所有 clone 共享 `Arc<(Mutex<WorkerTokenState>, Condvar)>`。
- `acquire` / `try_acquire` / `release`：分别阻塞取得、非阻塞尝试取得、阻塞归还一个令牌。
- `available` / `capacity`：读取当前余量或固定容量的观察方法。
- `BuildWorkerTokenChannel(size) -> WorkerTokenChannel`：公开构造入口；0 改为默认值，超上限改为最大值，否则保持调用方容量。

本文件没有 trait、enum、宏或条件编译项。

## 执行流程

panic 路径如下：调用者把工作闭包交给 `PanicToErr` 或 `CatchAndLogPanic`；函数以 `catch_unwind(AssertUnwindSafe(f))` 建立 unwind 边界。闭包正常返回时直接包装结果。发生 panic 时，两者都通过 `panic_message` 提取消息；前者再调用 `panic_into_shared_error`，记录带 `ShortError` 的 Warn 并返回 `Err`，后者记录带 `panic` 字段的 Warn 并返回 `None`。panic 不会越过这两个 API，但进程以 abort 处理 panic 的构建模式不受 `catch_unwind` 保护。

`AsyncStreamBy` 的流程是：

1. 创建容量为 0 的 `sync_channel`，所以发送方必须等待接收方消费，不能提前累计帧。
2. spawn 一个后台线程并把生成器与 sender 移入线程；调用者立即得到唯一 receiver。
3. 线程循环调用 `generator()`。成功时发送 `{ Err: None, Item: item }`；receiver 已被丢弃则 send 失败，线程立即退出。
4. 生成器首次返回错误时，发送 `{ Err: Some(err), Item: T::default() }`，无论发送是否成功都退出。
5. 线程退出会 drop sender，receiver 随后观察到通道断开；因此错误最多作为一个末帧出现，不会在错误后继续生成。

令牌通道构造时先规范化 `size`，然后建立 `available == capacity == size` 的状态。`acquire` 在余量为 0 时通过条件变量循环等待，醒来后重新检查条件，再令余量减一；`try_acquire` 在无令牌时立即返回 false；`release` 在通道已满时循环等待，否则令余量加一。每次成功改变余量后都 `notify_all`，唤醒可能等待“非空”或“非满”的线程。

## 数据与状态

panic helper 不保存跨调用状态；panic payload 只在本次调用内消费。由 `Annotate` 返回的 `SharedError` 保留 `ErrUnknown` cause 和可读上下文，供调用方继续传播。

异步流的状态由后台线程独占的 `FnMut` 生成器、一个 `SyncSender<Result<T>>` 和调用方持有的 `Receiver` 构成。零容量通道把消费进度直接反馈给生产线程，内存中不会形成无界队列。错误帧必须使用 `T::default()` 填充 `Item`，因此消费者应先检查 `Err`，不能把错误帧的 `Item` 当作有效业务值。

令牌池只维护 `available` 与 `capacity`。正常配对使用时维持 `0 <= available <= capacity`；初始时全部令牌可用。`capacity` 没有动态调整入口。clone 只复制 `Arc`，不复制令牌状态，所以所有句柄共同竞争同一额度。观察方法取得同一互斥锁，返回调用瞬间的快照，释放锁后数值可能立即被其他线程改变。

## 依赖与调用关系

RustCodeGraph 核对到的内部调用/引用关系为：

```text
PanicToErr
  -> panic_into_shared_error -> panic_message
  -> log::Warn + ShortError

CatchAndLogPanic
  -> panic_message
  -> log::Warn + Field::string

AsyncStreamBy
  -> std::sync::mpsc::sync_channel(0)
  -> std::thread::spawn
  -> worker::Result<T>

BuildWorkerTokenChannel
  -> DefaultWorkerTokenChannelSize / MaxWorkerTokenChannelSize
  -> WorkerTokenState + WorkerTokenChannel
```

`br/pkg/utils/Cargo.toml` 确认 crate 名为 `astersql-br-pkg-utils`、映射 Go package `br/pkg/utils`。本文件的非标准库直接依赖分别是：`astersql-br-pkg-errors` 提供 `ErrUnknown`，`astersql-br-pkg-logutil` 提供日志、字段和短错误格式，`astersql-errors` 提供 `Annotate` 与 `SharedError`；它们都在该 manifest 中以 workspace 路径依赖声明。线程、panic 捕获、MPSC、`Arc`、`Mutex` 和 `Condvar` 来自标准库。

当前 Rust 上游仅确认 `br/pkg/utils/lib.rs` 和 `br/pkg/utils/worker_test.rs`。Go 侧直接调用包括 `br/pkg/streamhelper/advancer.go`、`collector.go`、`advancer_daemon.go` 的 panic helper，`br/pkg/restore/snap_client/import.go` 的令牌通道，以及 `br/pkg/backup/prepare_snap/stream.go` 的异步流。Rust 中出现的同名函数属于局部复制实现，不能视为本模块的接线证据。

## 错误处理与边界

`PanicToErr` 只转换 panic，不转换闭包正常返回的业务错误；若闭包本身返回 `Result`，成功分支会得到嵌套结果，由调用者处理。`panic_into_shared_error` 对 `Annotate` 使用 `expect("annotate panic")`；给定 `Some(ErrUnknown)` 的当前调用按接口预期会返回错误值，但若底层契约改变导致 `None`，恢复路径本身会再次 panic。`CatchAndLogPanic` 有意丢弃 panic，只能通过日志和 `None` 观察失败。

两项 API 都要求闭包类型实现 `UnwindSafe`，即使内部包了 `AssertUnwindSafe`；包含某些内部可变状态的闭包可能无法直接通过类型检查。unwind 后闭包捕获对象是否保持业务不变量仍由调用者负责，函数不会回滚副作用或释放闭包之外的资源。

`AsyncStreamBy` 没有“正常结束”哨兵：生成器只能持续成功，直到返回错误或 receiver 被丢弃。因此即使业务完成，也要用一个终止错误结束并由消费者解释。错误帧发送到零容量通道时，如果消费者仍持有 receiver 却不继续接收，后台线程会阻塞；丢弃 receiver 才使发送失败并结束。spawn 失败会由标准库直接 panic，且返回值没有 `JoinHandle`，调用者无法取得线程 panic 或显式 join。

令牌方法对 poisoned mutex 全部使用 `unwrap()`，任一持锁线程 panic 造成 poison 后，后续调用也会 panic。`release` 在 `available == capacity` 时阻塞，这能阻止溢出，但多归还一次且没有其他线程 acquire 时会永久等待；API 没有 RAII guard 自动确保 acquire/release 配对。容量输入已限制为 `u32`，0 不是“禁用并发”，而会变成 128；超过 30M 则静默改值并告警。

## 并发与资源生命周期

每次 `AsyncStreamBy` 调用生成一个 detached OS 线程。线程生命周期由生成器返回错误或 receiver drop 驱动；函数不保存 join handle。零容量通道保证一帧一握手的背压：生成器只有在上一帧被接收后才能进入下一轮。receiver 不能 clone，因此消费端天然是单消费者；`T` 与生成器需要 `Send + 'static`，确保移入线程后安全存活。

`WorkerTokenChannel` 的所有句柄通过 `Arc` 共享资源；最后一个句柄 drop 时 mutex、condvar 和状态一起释放。等待线程本身持有一个 clone 或借用句柄，所以正常等待期间共享状态不会提前销毁。`while` 循环处理虚假唤醒并在锁内维护余量。`notify_all` 发生在每次成功 acquire/release 后，正确但可能在高竞争下造成惊群；当前实现优先保持双向等待语义，没有公平性或 FIFO 保证。

panic helper 在调用线程同步执行，不创建资源管理线程。它们只能捕获同一线程中穿过闭包边界的 unwind；子线程 panic 必须在子线程内部使用 helper 或通过 join 单独处理。

## 与 Go 版本的对应关系

Rust 的两个容量常量及 `BuildWorkerTokenChannel` 的 0 默认化、30M 上限钳制、Warn 文案与 Go `worker.go` 对齐。Go 返回预填充的 `chan struct{}`，调用者以接收取得令牌、发送归还令牌；Rust 将同样语义显式化为 `WorkerTokenChannel::{acquire, try_acquire, release}`，并额外提供只读快照。Go 对满 channel 的归还也会阻塞，因此 Rust 的误配风险与其一致，但 Rust 当前没有 `select`/context 取消等待的等价入口。

Go `PanicToErr` 与 `CatchAndLogPanic` 设计为 `defer` recovery 函数；Rust 没有等价 defer，因此改为接收并立即执行闭包。前者在 Go 中覆写命名返回错误，在 Rust 中返回 `Result<T, SharedError>`；后者在 Go 中没有返回值，在 Rust 中用 `Option<T>` 区分成功和已吞掉的 panic。两边都记录 Warn，但 Go 额外附带 `zap.StackSkip` 的堆栈位置，Rust 当前只记录短错误或 panic 文本。

两边的 `AsyncStreamBy` 都采用无缓冲/零容量通道、循环调用生成器、成功发送 Item、首次错误发送错误帧后关闭。Rust 为构造错误帧增加 `T: Default` 约束；Go 泛型零值不需要显式 trait。Go goroutine defer `close(out)`，Rust 依靠线程退出 drop sender 达到 receiver 结束。Go `Result<T>` 的零值 `Err == nil`，Rust 对应 `Option::None`。

独立 Rust 测试覆盖了 Go 核心意图，并额外验证零容量背压、非阻塞尝试取得、阻塞等待归还以及容量观察。当前同目录没有 `worker_test.go`；Go 对照依据来自生产源码与上述真实调用点，不能声称存在专属 Go 单元测试。

## 扩展指南

- 若要把这些工具接入 Rust BR 生产链，先复用 crate 根再导出的符号，并替换同名局部实现前逐项比较类型与结束语义；尤其不能把 `prepare_snap/stream.rs::AsyncStreamBy` 或 `snap_client/stubs.rs::BuildWorkerTokenChannel` 自动视为等价调用。
- 给 `AsyncStreamBy` 增加正常完成、取消或超时能力时，需明确区分“业务错误末帧”和“正常 EOF”，并同步检查零容量背压、receiver drop 和线程退出。独立测试应继续放在 `br/pkg/utils/worker_test.rs`。
- 若希望可靠管理后台线程，应考虑返回可 join/cancel 的句柄；这会改变现有仅返回 receiver 的 API 和 drop 行为，需要评估所有未来调用者的资源生命周期。
- 若调整令牌算法，必须保持初始预填充、容量不越界及 acquire/release 配对语义。引入 RAII permit 可减少漏归还风险，但要定义 permit drop 时的 poison 和线程退出行为。
- 若要求 context-aware acquire 或公平调度，`Condvar + notify_all` 需要重构；同时增加取消竞态、多等待者顺序、误归还和 poison 的独立测试，并评估高并发惊群的性能成本。
- 修改 panic 转换时需保持 `ErrUnknown` 分类和 Go 文案兼容；若补充 backtrace，应控制日志体积并避免在敏感 payload 上泄露信息。
- 任何 Rust 源码修复都应同步更新 `worker_test.rs`，而不是把 `#[cfg(test)]` 测试放入 `worker.rs`；本次任务仅写文档，未修改运行时代码。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、其中 7,032 个 Rust 文件。
- RustCodeGraph `node --file br/pkg/utils/worker.rs --offset 1 --limit 260`：完整读取目标文件 229 行，核对常量、私有 helper、公开函数、`Result<T>`、令牌状态和全部方法分支。
- RustCodeGraph `query`、`callers`、`callees`：查询 `PanicToErr`、`CatchAndLogPanic`、`AsyncStreamBy`、`BuildWorkerTokenChannel`、`WorkerTokenChannel`；确认主要内部边，包括 `PanicToErr -> panic_into_shared_error/ShortError`、`CatchAndLogPanic -> panic_message`、`AsyncStreamBy -> Result<T>`、构造器对两个容量常量和两个令牌类型的引用。调用者查询未返回本文件公开函数的 Rust 生产调用。
- 全仓 Rust 精确搜索：确认 `br/pkg/utils/lib.rs` 的模块挂载/再导出与 `br/pkg/utils/worker_test.rs` 的测试调用；确认其他生产文件中的 `AsyncStreamBy`、`BuildWorkerTokenChannel` 是局部同名定义。
- `br/pkg/utils/Cargo.toml`：核对 crate 边界、Go package 元数据以及 errors/logutil 路径依赖。
- `br/pkg/utils/worker.go`：核对 Go panic recovery、无缓冲异步流及预填充令牌 channel 的原始语义。
- `br/pkg/utils/worker_test.rs`：核对成功/panic、错误末帧、零容量背压、令牌取得归还、容量钳制和阻塞唤醒边界。
- Go 调用搜索：核对 `br/pkg/streamhelper/{advancer.go,collector.go,advancer_daemon.go}`、`br/pkg/restore/snap_client/import.go`、`br/pkg/backup/prepare_snap/stream.go` 的生产接线；同时确认不存在同目录 `worker_test.go`。
- 按任务约束未运行 Cargo。交付验证仅执行固定十一章节结构检查和文档自审；仓库声明的 `.agents/skills/tidb-verify-profile` 在当前工作区不存在，因而无法调用其 Ready 封装，改为直接执行本任务指定的文档验证。
