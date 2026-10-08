# `pkg/util/channel/channel.rs`

## 文件定位

该文件是工作区 crate `astersql-util-channel` 的实际业务实现，crate 边界由 `pkg/util/channel/Cargo.toml` 定义，入口 `pkg/util/channel/lib.rs` 通过 `pub mod channel` 和 `pub use channel::*` 导出本文件 API。根工作区在 `Cargo.toml` 中以 `facade_util_channel` 指向该 crate，`pkg/lib.rs` 又将其公开为 `pkg::util::channel` 风格的门面。

它移植自同目录 Go 文件 `pkg/util/channel/channel.go`，解决的是并发收尾阶段“接收并丢弃 channel 中的所有后续值，直至发送侧全部关闭”的问题。它不是通用队列，也不是一次性的非阻塞清缓存函数。

## 核心职责

- `DrainableChannel<T>` 把“阻塞接收一个值，关闭后返回无值”抽象成最小接口，使清理算法不绑定到具体 channel 类型。
- 针对 `std::sync::mpsc::Receiver<T>` 的实现将标准库 `recv()` 的 `Result<T, RecvError>` 映射成 `Option<T>`。
- `Clear` 反复接收并立即丢弃元素，只有在实现方返回 `None` 时结束，从而对齐 Go 的 `for range ch {}`。

当前 Rust 生产代码中没有检索到已接线的 `Clear` 调用；可执行用法位于独立测试 `pkg/util/channel/migration_aster_unit_test.rs`。例如 `pkg/executor/unionexec/union.rs`、`pkg/executor/join/index_lookup_merge_join.rs` 中的对应调用仍在迁移注释内。因此，本文件已经提供可用 API，但不能据此声称 Rust 执行器关闭链已经全面采用它。

## 主要符号

- `pub trait DrainableChannel<T>`：公开泛型 trait。唯一方法 `fn recv(&mut self) -> Option<T>`；`Some(T)` 表示收到一个待丢弃元素，`None` 是排空结束信号。
- `impl<T> DrainableChannel<T> for std::sync::mpsc::Receiver<T>`：标准库多生产者、单消费者 channel 的适配。调用阻塞式 `Receiver::recv(self)`，通过 `.ok()` 丢弃错误种类，仅保留值/关闭二态。
- `pub fn Clear<T, V>(mut ch: V) where V: DrainableChannel<T>`：公开清理入口。它按值取得 `V` 的所有权，通过可变借用连续调用 `recv`。名称上的 `#[allow(non_snake_case)]` 用于保持 Go API 名称兼容。

文件没有模块级常量、结构体、枚举、条件编译项或内部辅助函数。

## 执行流程

1. 调用者把实现了 `DrainableChannel<T>` 的接收端按值交给 `Clear`。
2. `Clear` 调用 `ch.recv()`；对标准库接收端，这一步进入阻塞式 `std::sync::mpsc::Receiver::recv`。
3. 收到 `Some(value)` 时，循环体不处理该值；本轮结束时 `_value` 被丢弃，然后继续接收。
4. channel 暂时为空但仍存在发送端时，`recv` 等待后续消息，不会让 `Clear` 提前返回。
5. 缓冲区已空且所有发送端都被释放后，标准库返回 `RecvError`，适配层将其变为 `None`，循环结束；随后 `ch` 本身也随函数返回被释放。

独立测试 `clear_drains_buffered_values_until_the_channel_is_closed` 覆盖“先缓存、后关闭、再完整排空”，`clear_waits_for_close_instead_of_stopping_when_temporarily_empty` 覆盖“空闲期间继续等待，发送端关闭后退出”。

## 数据与状态

本文件不保存全局状态。状态完全来自传入的接收端：缓冲值、仍存活的发送端以及 channel 的关闭状态。

`Clear` 消费接收端所有权，所以调用返回后调用者不能继续从同一 `std::sync::mpsc::Receiver<T>` 接收。每个收到的 `T` 都在循环迭代末尾析构；测试使用 `DropProbe(Arc<AtomicUsize>)` 计数，证明缓冲的值确实被消费和释放。适配层不会复制、积累或返回消息。

## 依赖与调用关系

下游依赖只有 Rust 标准库：`std::sync::mpsc::Receiver::recv` 提供阻塞接收和断开检测；本 crate 的 `Cargo.toml` 没有声明第三方依赖或 feature。

导出链是 `pkg/util/channel/channel.rs` → `pkg/util/channel/lib.rs` → 根工作区依赖别名 `facade_util_channel` → `pkg/lib.rs` 的 `util::channel` 门面。RustCodeGraph 将本文件识别为 5 个符号，并确认 `DrainableChannel` 位于本文件；由于 `Clear` 名称高度通用，图查询未能给出可靠的生产调用边，随后用精确文本检索确认 Rust 侧只有独立测试中的实际调用和执行器迁移注释中的未接线调用。

Go 侧直接调用证据包括 `pkg/executor/distsql.go`、`pkg/executor/sample.go`、`pkg/executor/analyze_col_sampling.go`，以及多个 join、sort、union 执行器文件。典型目的都是在错误或关闭路径中持续消费结果，使仍在发送的后台 goroutine 不会因无人接收而永久阻塞。这些 Go 调用说明原工具在完整应用中的位置，但不等同于 Rust 调用已经接通。

## 错误处理与边界

公开 API 不返回 `Result`。标准库 `recv` 的唯一错误表示发送侧全部断开；实现用 `.ok()` 把它解释为正常终止，而不是对外错误。因此 `Clear` 不能区分自定义实现中的不同结束原因，自定义 `DrainableChannel` 也必须自行把结束条件归一成 `None`。

重要边界如下：

- channel 已关闭且仍有缓冲值时，先返回全部缓冲值，之后才返回 `None`。
- channel 已关闭且为空时，立即结束。
- channel 暂时为空但发送端仍存活时，会阻塞，而不是“清掉当前已有元素后返回”。
- 若某个发送端永不释放且不再发送，`Clear` 将永久等待；调用者必须先建立可靠的取消、关闭或发送端退出顺序。
- `recv` 或元素析构发生 panic 时，本文件不捕获 panic；正常的 Rust 栈展开规则生效。
- 自定义实现若反复返回 `Some`、永久阻塞或错误地提前返回 `None`，`Clear` 不提供超时、公平性或正确性补偿。

## 并发与资源生命周期

`std::sync::mpsc::Receiver<T>` 是单消费者接收端；`Clear` 按值取得它并在当前线程同步运行，不创建线程、任务、锁或额外 channel。等待行为由 `Receiver::recv` 管理，发送者可在其他线程继续发送。

安全的生命周期顺序通常是：先通知生产者停止或确保它们最终释放全部 `Sender`，再让 `Clear` 消费残留值，最后等待生产者退出或完成其他资源回收。Go 的 `pkg/executor/distsql.go` 明确记录了排空结果 channel 是为了避免后台 worker 因继续写入而永久阻塞；但 Rust 调用方仍需根据自身所有权和关闭协议复核顺序，不能机械调用。

消息资源在每次循环迭代末尾释放，接收端在 `Clear` 返回时释放。这里没有容量控制或背压策略；容量与发送阻塞行为取决于传入的 channel 实现。当前标准库适配对应 `mpsc::Receiver`，并未为 `SyncSender` 的接收端提供不同逻辑，因为两类发送端共享同一种 `Receiver<T>`。

## 与 Go 版本的对应关系

Go 源文件 `pkg/util/channel/channel.go` 的核心实现是 `func Clear[T any, V chan T | <-chan T](ch V) { for range ch {} }`。Rust 对应关系为：

- Go 的 `T any` 对应 Rust 未附加约束的 `T`。
- Go 的 `chan T | <-chan T` 接收能力约束对应 `DrainableChannel<T>`；当前只为 `std::sync::mpsc::Receiver<T>` 实现。
- Go 的 `for range ch` 对应 `while let Some(_value) = ch.recv()`，二者都消费至 channel 关闭，空闲但未关闭时都会等待。
- Go 参数本身是 channel 句柄；Rust `Clear` 按值取得接收端所有权，体现单消费者接收端的所有权模型，调用后无法复用该接收端。
- Go 对发送/接收均可的 `chan T` 和只接收 `<-chan T` 都可调用；Rust 没有在此暴露发送能力，只要求接收 trait。

语义差异主要在扩展面：Go 类型集合天然覆盖语言 channel，Rust 必须为每种接收器显式实现 `DrainableChannel`。当前没有 async channel、超时或非阻塞 `try_recv` 适配。

## 扩展指南

若要支持另一种同步接收器，应在本文件为它实现 `DrainableChannel<T>`，并确保 `None` 只表示“不会再出现任何值”，不能把“暂时为空”映射为 `None`。若接收 API 能返回多类错误，需要明确哪些错误是正常关闭，哪些必须传播；现有 trait 无法传播错误，若业务确需错误或超时，应设计新的 API，而不是悄悄改变 `Clear` 的结束语义。

若要支持异步 channel，应新增异步抽象/入口并保持同步 `Clear` 的兼容性，避免在同步 trait 中阻塞异步执行器线程。性能方面，当前逐项接收和析构是有意行为；批量排空优化必须保留析构时机、关闭等待和背压解除语义。

测试必须继续放在独立文件，不得内嵌到 `channel.rs`。优先扩展 `pkg/util/channel/migration_aster_unit_test.rs`，至少覆盖：缓冲值全部释放、暂时为空不提前结束、多个发送端仅在最后一个关闭后结束，以及新增适配器特有的关闭/错误边界。若把 API 接入执行器，还应在对应执行器的独立测试中验证关闭顺序和后台发送者不会泄漏或死锁。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/util/channel/channel.rs`、`channel.go`、`lib.rs` 和 `migration_aster_unit_test.rs`；`node --file pkg/util/channel/channel.rs` 核对了完整 46 行实现；`query DrainableChannel` 定位到 trait；针对 `Clear` 的通用名称查询存在歧义，因此未把模糊结果当成调用证据。
- Rust 源与模块：`pkg/util/channel/channel.rs`、`pkg/util/channel/lib.rs`。
- crate 与门面：`pkg/util/channel/Cargo.toml`、根 `Cargo.toml` 的 `facade_util_channel` 路径依赖、`pkg/lib.rs` 的 `util::channel` 再导出。
- Go 对照与真实使用：`pkg/util/channel/channel.go`、`pkg/executor/distsql.go`、`pkg/executor/sample.go`、`pkg/executor/analyze_col_sampling.go`。
- 独立 Rust 测试：`pkg/util/channel/migration_aster_unit_test.rs`；其中两个测试分别验证缓存排空/析构和等待最终关闭。
- Rust 接线现状：精确检索 `facade_util_channel`、`channel::Clear`、`Clear(receiver)`；实际 Rust 调用只出现在上述独立测试，`pkg/executor/unionexec/union.rs` 与 `pkg/executor/join/index_lookup_merge_join.rs` 等位置仍为注释迁移线索。
- 本任务是只增说明文档的静态分析，按计划不运行 Cargo；交付前另以任务指定命令检查固定章节数量。
