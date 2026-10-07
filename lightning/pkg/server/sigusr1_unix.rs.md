# `lightning/pkg/server/sigusr1_unix.rs`

## 文件定位

该文件是 `astersql-lightning-pkg-server` crate 的 Unix 平台 `SIGUSR1` 适配层。`lightning/pkg/server/lib.rs` 在 `cfg(unix)` 下把它装配成私有模块 `sigusr1`，再用 `pub use sigusr1::*` 导出其公开入口；非 Unix 平台改选 `sigusr1_other.rs`。crate 边界由 `lightning/pkg/server/Cargo.toml` 定义，包名为 `astersql-lightning-pkg-server`，根文件为 `lib.rs`，本实现直接使用其 `libc = "0.2"` 依赖。

在完整应用中，直接生产调用点是 `lightning/pkg/server/lightning.rs` 的 `Lightning::GoServe`：它向 `handleSigUsr1` 注册闭包，使未配置状态服务地址时可以在收到 `SIGUSR1` 后尝试启动 HTTP 监听，已经启动时则记录现有地址。因此本文件只负责可靠地把 Unix 异步信号转交给普通 Rust 执行上下文，不负责 HTTP 服务策略本身。

## 核心职责

1. `handleSigUsr1` 接收可在线程间移动且拥有静态生命周期的回调，并把回调追加到进程级注册表。
2. `dispatcher` 首次使用时创建一对 `UnixStream`、安装进程级 `SIGUSR1` 处理器，并启动唯一的读取线程。
3. `on_sigusr1` 在 C 信号处理器上下文中只读取已发布的文件描述符并调用 `libc::write` 写入一个字节；日志和业务回调都留到普通线程执行。
4. 读取线程将每个成功读到的字节解释为一次通知，对当时注册的全部处理器逐个广播。

文件不解析信号载荷、不管理 HTTP 状态，也不提供注销、停机或恢复旧信号处理器的接口。其行为是进程级且常驻的，而不是某个 `Lightning` 实例私有的。

## 主要符号

- `type Handler = Arc<Mutex<Box<dyn Fn() + Send>>>`：擦除具体闭包类型。`Arc` 允许在持有注册表锁时复制处理器列表；每个闭包外层的 `Mutex` 使只要求 `Fn() + Send`、不要求 `Sync` 的回调仍可由后台线程安全持有和调用。
- `struct Dispatcher { handlers, _writer }`：`handlers` 是所有注册回调的互斥向量；`_writer` 保持 socket 写端存活，其下划线名称表示字段主要承担资源所有权而非直接业务访问。
- `DISPATCHER: OnceLock<Dispatcher>`：保证初始化和信号处理器安装在进程内至多执行一次，并为读取线程提供全局注册表。
- `WRITE_FD: AtomicI32`：在信号处理器不能锁注册表的约束下发布写端原始文件描述符；初值 `-1` 表示尚不可写。
- `extern "C" fn on_sigusr1(...)`：交给 `libc::sigaction` 的低层入口。它忽略信号参数和 `write` 返回值；文件描述符有效时尝试写一个值为 `1` 的字节。
- `fn dispatcher() -> &'static Dispatcher`：惰性初始化函数，负责 socket pair、非阻塞写端、`sigaction` 和后台线程的完整装配。
- `pub fn handleSigUsr1<F>(handler: F)`：模块唯一公开 API，泛型约束为 `F: Fn() + Send + 'static`；它触发初始化并永久登记回调。

## 执行流程

第一次调用 `handleSigUsr1` 时，`dispatcher()` 进入 `DISPATCHER.get_or_init`。初始化闭包先用 `UnixStream::pair` 建立本地全双工 socket 对，只把写端设为非阻塞；随后把写端原始 fd 以 `Ordering::Relaxed` 存入 `WRITE_FD`。接着构造清空信号掩码、`sa_flags = 0` 的 `libc::sigaction`，将 `on_sigusr1` 安装到 `SIGUSR1`，并启动持有 reader 的后台线程。初始化完成后创建空处理器向量，调用方再把本次闭包装箱并追加进去。

信号到达时，内核调用 `on_sigusr1`。该函数读取 `WRITE_FD`；若值非负，就尝试向 socket 写入一个字节，然后立即返回。后台线程在 reader 上阻塞读取，单次最多读取 64 字节。读到 `size > 0` 后，它对每个字节记录一次 `received signal` debug 日志，克隆当前处理器快照，再依次加锁并调用每个处理器。

后续调用 `handleSigUsr1` 不会重新建 socket、安装信号处理器或启动线程，只向同一个向量追加回调。处理器快照在释放注册表锁后才执行，所以回调本身再次调用 `handleSigUsr1` 不会因注册表锁而死锁；新注册的回调从之后的通知开始生效，不会加入正在处理的这次快照。

## 数据与状态

持久状态只有两个进程级静态量。`DISPATCHER` 从未初始化单向转为包含 handler 列表和 writer 的已初始化状态，不能重置；`WRITE_FD` 从 `-1` 转为 writer 的 fd，代码中不会改回无效值。处理器列表只增长，没有去重或移除操作，因此重复注册同一业务动作会导致每次通知重复执行，并长期持有闭包捕获的资源。

socket 中的每个字节代表一次待分发通知，但它不是可靠队列：写端非阻塞且 `on_sigusr1` 不检查 `libc::write` 的结果。当缓冲区已满或写入失败时，该次通知会被丢弃；这与信号通知允许合并的定位一致，却不适用于必须精确计数的事件。一次 reader 读取可以批量取得最多 64 次通知，仍按字节逐次记录日志和广播。

`Ordering::Relaxed` 仅用于发布整数 fd；初始化顺序是先保存 fd、后安装 `sigaction`。注册表的内容由 `Mutex` 保护，回调对象也各有一把独立 `Mutex`。

## 依赖与调用关系

上游装配链为 `lib.rs` 的 `cfg(unix)` 模块选择与公开再导出，生产调用链为 `lightning.rs::Lightning::GoServe -> handleSigUsr1 -> dispatcher`。RustCodeGraph 将 `handleSigUsr1` 到 `dispatcher` 识别为直接调用边，并将 `dispatcher` 到 `on_sigusr1` 识别为函数引用；`on_sigusr1` 实际由操作系统信号机制回调，不是普通 Rust 调用。

下游依赖包括：`std::sync::{OnceLock, Mutex, Arc}` 管理全局初始化和回调共享，`std::os::unix::net::UnixStream` 构造进程内通知通道，`std::thread` 承载普通执行上下文，`libc::{sigaction, sigemptyset, write, SIGUSR1}` 对接 Unix API，`crate::log` 与 `crate::zap` 在读取线程中记录信号。Cargo 清单中的 `libc` 是本文件唯一直接可辨认的外部 crate 依赖；日志模块来自本 crate 的本地边界实现。

`sigusr1_unix_test.rs` 也是直接调用者，但只在 `cfg(all(test, unix))` 下由 `lib.rs` 纳入。RustCodeGraph 对少数常见名称（例如 `write`、`read`、`store`）给出了跨文件误匹配，因此这些边以目标源码中的完全限定调用为准，不把误匹配文件视为真实依赖。

## 错误处理与边界

初始化错误采用不可恢复策略：socket pair 创建失败、设置非阻塞失败、`sigaction` 返回非零都会 `expect`/`assert_eq!` 并使首次注册调用 panic。handler 注册表或单个 handler 的 mutex 一旦中毒，读取线程上的 `expect` 也会 panic。业务回调 panic 没有被 `catch_unwind` 隔离，会终止唯一分发线程，使以后到达的通知不再执行。

信号入口刻意不传播错误。fd 尚未发布时直接忽略信号；`libc::write` 的返回值和 `errno` 均被忽略，所以满缓冲区、无效 fd 或中断写入只表现为通知缺失。读取循环使用 `while let Ok(size) = reader.read(...)`，任何读取错误都会静默结束线程，EOF 也会退出；当前 API 没有健康检查或重启机制。

`sigaction` 会替换进程此前的 `SIGUSR1` action，代码没有保存、链接或恢复旧 action。初始化期间，action 已安装但 `DISPATCHER` 尚未完成写入 `OnceLock` 的短窗口内，如果后台线程已经读到通知，`DISPATCHER.get().expect(...)` 可能 panic；这是从初始化顺序可见的竞争边界。该 API 因而假定由应用启动路径较早注册，并且进程中没有另一个组件争用 `SIGUSR1`。

## 并发与资源生命周期

初始化成功后恰有一个 dispatcher 读取线程，它顺序处理所有信号字节和所有回调。慢回调或永久阻塞的回调会推迟同一通知中的后续回调以及所有后续通知；文件没有超时、任务拆分或并行执行。虽然每个 handler 有独立 mutex，当前只有这一个读取线程负责调用，所以它主要用于满足共享闭包的线程安全表示，而不是提供回调并行度。

读取线程克隆 `Vec<Arc<...>>` 后释放 `handlers` 锁再执行业务逻辑，注册操作只在克隆期间短暂竞争，不会覆盖现有快照。回调按向量注册顺序被遍历，但调用方不应把跨信号精确计数当作保证，因为非阻塞写可能合并/丢弃通知。

`OnceLock`、writer 和全部回调存活到进程结束；reader 被移动到无 join handle 的后台线程。没有显式 shutdown、join、handler unregister、关闭 socket 或恢复信号 action 的生命周期阶段。这适合 Lightning 进程级运维钩子，但不适合需要在同一进程内反复创建并完全销毁服务实例的场景。

## 与 Go 版本的对应关系

Go 对照文件 `lightning/pkg/server/sigusr1_unix.go` 的 `handleSigUsr1` 每次调用都会创建容量为 1 的 `chan os.Signal`，调用 `signal.Notify(ch, syscall.SIGUSR1)`，再启动一个 goroutine 逐次记录日志并调用该次传入的 handler。Rust 保留了“信号接收与业务回调分离”“缓冲通知可合并”“普通执行上下文中记录日志并执行回调”的核心语义。

实现结构并非逐行等价：Rust 只有一个进程级 `sigaction`、socket pair 和读取线程，向全局列表中的全部 handler 广播；Go 则为每次注册保留各自 channel 和 goroutine，由 `os/signal` 向每个注册 channel 送达。Rust 回调要求 `Send + 'static`，Go 闭包没有对应的静态类型约束。Rust 使用整数 signal 字段记录 `SIGUSR1`，Go 使用 `zap.Stringer` 记录 `os.Signal`。两者都没有在本文件中提供注销路径，且缓冲饱和时均不承诺逐信号精确计数。

Go 文件未附带同目录的专用 Go 测试；当前可执行的移植证据来自独立 Rust 测试 `sigusr1_unix_test.rs`。该测试注册两个 handler，调用 `libc::raise(SIGUSR1)`，并要求两个 channel 消息都在两秒内到达，验证了 Rust 全局 dispatcher 的广播语义，而没有覆盖高频信号、初始化失败、panic、注销或 shutdown。

## 扩展指南

若要改变注册或分发语义，首要修改点是 `Handler`、`Dispatcher.handlers` 与 `handleSigUsr1`，并在独立的 `sigusr1_unix_test.rs` 中增加测试；不要把测试内嵌到生产源文件。若要提高故障隔离，可在读取线程的 handler 调用处设计 panic 隔离、超时或任务分发，但必须明确回调顺序和背压语义，避免无界线程增长。

若要支持注销，应返回稳定的注册 token，并定义注销与已取得快照的并发关系，同时处理闭包捕获资源的释放。若要支持可重启生命周期，还需共同设计 reader 退出、writer 关闭、线程 join、`WRITE_FD` 失效和旧 `sigaction` 恢复；仅从向量删除 handler 不足以释放进程级资源。

若要减少初始化竞争或增强错误可见性，应重点调整 `dispatcher` 中“发布 fd—安装 action—启动线程—提交 `OnceLock`”的顺序及读取错误处理。信号处理器内仍必须限制为异步信号安全操作，不能移入日志、锁、分配或业务回调。任何变更都应同时核对 `lightning.rs::Lightning::GoServe` 的实际使用方式和 Go 对照语义，并增加至少包含连续信号、慢/异常 handler、多次注册和资源清理的独立测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter lightning/pkg/server` 列出目标源、平台替代实现、模块入口、Go 对照和独立测试。
- RustCodeGraph `node --file lightning/pkg/server/sigusr1_unix.rs`：读取了完整 122 行源码，确认 `Handler`、`Dispatcher`、`DISPATCHER`、`WRITE_FD`、`on_sigusr1`、`dispatcher`、`handleSigUsr1` 及其实现。
- RustCodeGraph `node handleSigUsr1`、`node dispatcher`、`node on_sigusr1`：确认 `handleSigUsr1 -> dispatcher`、`dispatcher -> on_sigusr1` 的调用/引用关系，以及独立 Rust 测试调用公开入口；常见函数名产生的跨文件候选未被当作事实。
- RustCodeGraph `node GoServe` 与 `node --file lightning/pkg/server/lightning.rs --offset 430 --limit 80`：确认生产入口 `Lightning::GoServe` 注册回调，以及回调启动或报告 HTTP server 的当前行为。
- 直接读取 `lightning/pkg/server/lib.rs` 和 `lightning/pkg/server/Cargo.toml`：确认 Unix 条件装配、公开再导出、测试装配、workspace crate 边界及 `libc` 依赖。
- 直接读取 `lightning/pkg/server/sigusr1_unix.go`、`lightning/pkg/server/lightning.go` 和 `lightning/pkg/server/sigusr1_unix_test.rs`，并搜索 Lightning 测试：确认 Go 注册模型、上层运维用途、Rust 双 handler 广播断言，以及没有找到专用 Go `SIGUSR1` 测试。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的结构命令确认目标文件存在且恰有 11 个固定二级章节，并人工复核源文件定位、执行流程、安全扩展点和验证边界。
