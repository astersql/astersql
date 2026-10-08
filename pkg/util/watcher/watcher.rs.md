# `pkg/util/watcher/watcher.rs`

## 文件定位

`watcher.rs` 是 `astersql-util-watcher` crate 的轮询式文件系统监视器实现。crate 根 `pkg/util/watcher/lib.rs` 公开 `event` 与 `watcher` 模块并再导出其公开项；工作区根 `Cargo.toml` 以 `facade_util_watcher` 引入该 crate，`pkg/lib.rs` 又通过 facade 对外再导出。因此，本文件提供的是可从 AsterSQL Rust facade 使用的文件监视 API，而事件类型与操作位定义位于相邻的 `event.rs`。

RustCodeGraph 对 `NewWatcher` 的调用者查询只找到 `pkg/util/watcher/watcher_test.rs` 中的两处测试调用；仓库文本检索还找到 `migration_aster_unit_test.rs` 中的迁移测试调用，没有找到 Rust 生产代码直接构造这个 Watcher。因而当前可证实状态是“已公开、由测试覆盖的 Go 移植实现”，尚不能据此声称它已经接入某条生产应用主链。Go 生产实现位于同目录 `watcher.go`。

## 核心职责

- `NewWatcher` 构造监视器及事件、错误和关闭通道。
- `Watcher::Add` 和 `Watcher::Remove` 维护用户注册的根路径以及这些路径的当前元信息快照。目录只展开一层，不递归遍历孙级内容。
- `Watcher::Start` 启动一个后台线程，按指定 `Duration` 周期重新列举路径并比较前后快照。
- `poll_events` 根据路径、修改时间、大小、权限模式以及底层文件身份推导 `Modify`、`Chmod`、`Rename`、`Move`、`Create` 和 `Remove` 事件。
- `Watcher::Close` 通知线程退出、等待回收、断开公开接收端的发送端并清空内部监视状态。

该实现是轮询器，不依赖 inotify、kqueue 等操作系统事件 API。其观察精度和延迟受轮询周期及文件系统元信息精度限制；短于一个轮询周期、且最终快照相同的中间变化可能不可见。

## 主要符号

- `WatcherError::{Started, Closed, Io(String)}`：分别表示重复启动、关闭后操作及带上下文的文件系统错误。`Display` 输出稳定的人类可读文本，且实现了 `std::error::Error`。
- `ErrWatcherStarted`、`ErrWatcherClosed`：与 Go 包同名错误对应的公开常量，值分别为上述两个状态枚举。
- `State`：内部可变快照，`names` 保存调用方注册的根路径，`files` 保存根路径自身及目录直接子项的 `FileInfo`。
- `Inner`：跨线程共享状态。它持有事件/错误发送端、关闭通道、生命周期原子量、串行化操作的互斥锁、快照及工作线程句柄。
- `Watcher`：公开句柄。`Events` 和 `Errors` 是调用方消费的接收端，`inner: Arc<Inner>` 供前台方法与后台线程共享状态。
- `NewWatcher() -> Watcher`：建立两个容量为 0 的同步通道和一个容量为 1 的关闭通道，初始状态为未启动、未关闭、空快照。
- `Watcher::{Start, Close, Add, Remove}`：公开生命周期和路径管理 API。
- `do_watch`、`list_for_all`、`poll_events`、`send_event`：后台轮询、全量列举、差异计算和可中断发送的内部主链。
- `do_remove`：从根集合及快照删除指定路径；若目标是目录，同时移除其直接子项。
- `listForName`：公开的单路径列举函数；返回目标自身，目录时再返回直接子项。
- `event`、`io_error`：分别组装 `Event` 和给 `io::Error` 添加路径上下文。

本文件没有 trait、宏或条件编译项。平台条件逻辑在 `event.rs` 的 `FileInfo::Mode` 与 `FileInfo::same_file` 中。

## 执行流程

1. 调用 `NewWatcher` 得到未启动句柄。`Events`/`Errors` 均为零容量通道，关闭信号通道容量为 1。
2. 调用 `Add(path)`。方法取得 `operation` 锁，拒绝已关闭实例，通过 `listForName` 建立初始快照，再把根路径写入 `State::names`、把列举结果合并进 `State::files`。列举失败时状态不变并返回 `WatcherError::Io`。
3. 调用 `Start(duration)`。`running.compare_exchange(0, 1)` 保证只能有一个启动者；关闭检查通过后，线程执行 `do_watch`。
4. 每个 tick 到达后，`do_watch` 持有 `operation` 锁，调用 `list_for_all` 重列举所有根路径，再调用 `poll_events` 与旧快照比较。事件全部处理成功后，才用当前快照替换 `State::files`。
5. `poll_events` 先计算路径集合差：旧有新无进入 `removes`，新有旧无进入 `creates`。仍在同一路径的对象若修改时间或大小变化则发送 `Modify`，mode 变化则发送 `Chmod`。
6. 对删除候选和创建候选按 `FileInfo::same_file` 配对。同一底层文件且父目录相同为 `Rename`，父目录不同为 `Move`；事件路径和文件信息都取变化前的旧路径/旧快照。已配对项不再产生 `Create` 或 `Remove`。
7. 未配对项依次发送全部 `Create`，再发送全部 `Remove`。`HashMap` 的遍历次序不稳定，所以同类多个事件之间没有顺序保证。
8. `Remove(path)` 在 `operation` 锁下停止跟踪根路径及其直接子项，不向通道发送事件。
9. `Close()` 仅在 `running` 从 1 成功切换为 0 时执行关闭：设置 `closed`、发送关闭信号、等待线程退出、丢弃事件和错误发送端并清空状态。未启动实例调用 `Close` 会直接返回，实例仍可后续 `Add`/`Start`。

## 数据与状态

`State::names` 和 `State::files` 的分离是核心不变量：前者表达用户意图，后者表达上次成功轮询后保存的观察结果。目录根本身也存在于 `files`，因此目录元信息变化可能产生目录事件；测试辅助函数会显式跳过目录事件。

同一文件可因多个根路径的列举结果合并到同一个 `HashMap<PathBuf, FileInfo>` 中，路径相同的后写值覆盖前值。监视目录是非递归的：`listForName` 使用 `fs::read_dir` 读取直接条目，但不会继续展开其中的子目录。此不变量由 `migration_listing_is_non_recursive_and_remove_stops_tracking` 验证。

`running` 是 `AtomicI32`，实际只使用 0 和 1；`closed` 是不可逆的 `AtomicBool`。二者不是单一状态机：`Start` 先把 `running` 置 1，再检查 `closed`。因此已关闭实例再次 `Start` 首次返回 `Closed` 时会留下 `running == 1`，后续启动会返回 `Started`；这与同路径 Go 实现的检查顺序一致，但调用方不应依赖重复调用后的错误优先级。

事件携带 `event.rs::Event { Path, Op, FileInfo }`。`Modify` 的判据是修改时间或大小任一变化，弥补部分文件系统时间戳精度较低的问题；它仍不能检测元信息和最终大小均未变化的内容重写。

## 依赖与调用关系

crate 边界由 `pkg/util/watcher/Cargo.toml` 定义：运行时唯一外部依赖是 `crossbeam-channel = "0.5"`，`tempfile = "3"` 只供测试使用；`package.metadata.porting.go-package` 指回 `pkg/util/watcher`。该 crate 没有 feature 声明。

内部调用链如下：

`NewWatcher` → 构造 `Watcher/Inner/State` 与 `bounded` 通道；`Watcher::Start` → `thread::spawn` → `do_watch`；`do_watch` → `list_for_all` → `listForName`/`do_remove`，随后调用 `poll_events` → `event` → `send_event`。`Watcher::Add` 也直接调用 `listForName`，`Watcher::Remove` 直接调用 `do_remove`。

下游数据依赖来自 `event.rs`：操作位常量、`Event` 和 `FileInfo`，尤其是 `FileInfo::{ModTime, Size, Mode, same_file, IsDir}`。标准库提供文件元信息、路径、线程、原子量和互斥锁。RustCodeGraph 能确认 `Start → do_watch`，并确认 `poll_events` 调用 `send_event` 与 `event`；它对部分标准库/方法调用存在误配或缺边，因此本文同时以目标源码核对内部链路。

上游方面，`lib.rs` 和 `pkg/lib.rs` 建立公开导出；已验证的具体 Rust 调用者仅为 `watcher_test.rs` 与 `migration_aster_unit_test.rs`。没有发现 Rust 生产调用点，后续若接入 binlog 或其他文件滚动流程，应以实际新调用点更新此结论。

## 错误处理与边界

- `Add` 的 `fs::metadata`、`fs::read_dir`、目录项读取或元信息读取失败会由 `io_error` 包装成 `WatcherError::Io`，消息含 `name`/`directory` 和路径。已关闭时优先返回 `Closed`，不会访问文件系统。
- `Start` 重复调用返回 `Started`；关闭后启动返回 `Closed`，但前述原子量检查顺序会影响再次调用的错误值。
- `Remove` 对未注册或已从快照消失的路径成功返回，体现幂等删除语义；关闭后则返回 `Closed`。
- `list_for_all` 遇到单个根的列举错误时把错误发送到 `Errors`，继续处理其他根。当前实现通过检查错误字符串是否包含 `"not found"` 决定是否自动 `do_remove`。这不等价于 Go 的 `os.IsNotExist(errors.Cause(err))`，而且依赖平台错误文本；在常见 `"No such file or directory"` 文本下可能无法移除根，导致后续轮询重复报错。该差异应视为当前实现事实，不应描述为可靠的 NotFound 分类。
- 一次轮询中 `Modify` 和 `Chmod` 判据分别执行，若两者同时变化，代码可能发送两个事件；文件顶部 Go 注释中“多操作只发送一个事件”的描述并非这段实际控制流的保证。
- rename/move 配对依赖同一轮询前后都能看到对象，且目标必须位于被监视范围内。移到未监视目录只能表现为 `Remove`，从未监视目录移入只能表现为 `Create`。
- Unix 通过 device + inode 识别同一文件；非 Unix 的 `same_file` 恒为 `false`，因此非 Unix 上 rename/move 会退化为 create/remove 事件对。
- 对互斥锁的 `.unwrap()` 意味着内部线程或调用者若在持锁期间 panic 并毒化锁，后续操作也会 panic，而非转成 `WatcherError`。

## 并发与资源生命周期

`Arc<Inner>` 让工作线程持有共享状态；`worker` 中的 `JoinHandle` 由 `Close` 取出并 `join`。`operation: Mutex<()>` 把 `Add`、`Remove` 与一次完整轮询串行化，防止路径集合和快照在列举/比较/替换之间被前台修改。`state` 另有细粒度互斥锁，用于读取或更新集合。

`Events` 和 `Errors` 使用 `bounded(0)`，发送方必须等待接收方。这提供背压并避免无限积压，但也意味着后台线程可能长期停在单个事件或错误发送上；在它持有 `operation` 锁期间，`Add`/`Remove` 也会被阻塞。`send_event` 和错误发送均同时监听 `close_rx`，所以 `Close` 能打断阻塞发送，再通过 `join` 安全回收线程。

关闭通道容量为 1，`Close` 使用 `try_send`，避免关闭方自身阻塞。随后移除 `event_tx`/`error_tx` 的最后发送端，使仍被调用方持有的 `Receiver` 观察到断开；`close_disconnects_event_and_error_channels` 对此有专门回归测试。`Close` 不消耗或销毁调用方持有的接收端。

当前类型没有 `Drop` 实现。如果调用方启动后直接丢弃 `Watcher` 而未调用 `Close`，工作线程持有的 `Arc<Inner>` 仍会存活并继续轮询，且可能因无人消费零缓冲通道而阻塞。因此成功 `Start` 后应保证显式 `Close`。

## 与 Go 版本的对应关系

Rust 文件按 `pkg/util/watcher/watcher.go` 逐项移植：`WatcherError` 常量对应 Go 的包级错误；`State`/`Inner` 拆分后承载 Go `Watcher` 中的 `names`、`files`、`running`、互斥量、通道和等待组职责；Rust 后台线程加 `JoinHandle` 对应 goroutine 加 `sync.WaitGroup`；`crossbeam_channel::select!` 对应 Go `select`。

事件判定顺序、非递归列举、rename/move 使用旧路径、`Add` 先完成完整列举才写状态、`Remove` 静默移除以及 `Close` 断开事件/错误通道，均与 Go 主体语义对齐。`watcher_test.rs::test_watcher` 对照 Go `watcher_test.go::TestWatcher` 覆盖 Create → Modify → Chmod → Rename → Remove → Create → Move；`migration_aster_unit_test.rs` 额外覆盖非递归列举、Remove 停止跟踪及 Start/Close 状态机。

已确认的差异包括：Rust 使用 `PathBuf` 而非字符串路径；错误使用枚举而非 `pingcap/errors` 包装链；非 Unix 无法可靠实现 Go `os.SameFile` 等价物；缺失路径识别使用错误消息子串而非结构化 `IsNotExist`；Rust rename/move 配对找到一个目标后会退出当前内层循环。Rust 测试还比现有 Go 测试多验证了关闭后接收端断开。

## 扩展指南

- 新增事件类型或改变判定条件时，应同时修改 `event.rs` 的操作位/`FileInfo` 能力、`poll_events` 的优先次序，并扩展独立的 `watcher_test.rs`；若目标是继续与 Go 对齐，还要核对 `event.go`、`watcher.go` 和 `watcher_test.go`。
- 若要支持递归监视，接入点是 `listForName` 和 `do_remove`，同时必须定义符号链接、循环、权限失败、目录动态新增及大目录性能语义；不可只递归列举而忽略删除状态清理。
- 若要提供可靠的缺失路径处理，应把 `WatcherError::Io(String)` 扩展为保留 `io::ErrorKind` 或等价结构化信息，并调整 `list_for_all`，避免依赖本地化错误文本。此变更需新增缺失根路径的独立回归测试。
- 若要改善背压，可调整 `NewWatcher` 中通道容量或提供事件聚合，但必须明确队列上限、丢弃策略、Close 唤醒保证及 Add/Remove 的阻塞变化，并进行并发与性能测试。
- 若要使资源自动回收，可为拥有者设计显式 guard 或 `Drop` 策略；直接在当前 `Watcher` 上实现 `Drop` 前需处理公开 `Receiver` 和共享所有权语义，避免 clone/析构时过早关闭。
- 新的 Rust 测试应继续放在同目录独立测试文件中，不嵌入 `watcher.rs`。生命周期边界适合扩展 `migration_aster_unit_test.rs`，端到端文件事件适合扩展 `watcher_test.rs`。

兼容风险主要是公开字段/错误值和事件顺序；性能风险主要是每次 tick 全量 `metadata/read_dir`、快照克隆以及 rename/move 候选的二次方配对；正确性风险集中在平台元信息差异、轮询丢失瞬时变化和无消费者造成的阻塞。

## 验证依据

- 目标实现：`pkg/util/watcher/watcher.rs`，核对了全部 343 行及其中 `WatcherError`、`State`、`Inner`、`Watcher`、`NewWatcher`、公开方法和所有内部函数。
- 事件模型：`pkg/util/watcher/event.rs`，核对操作位、`FileInfo` 的平台差异和 `Event` 查询方法。
- crate 与导出：`pkg/util/watcher/Cargo.toml`、`pkg/util/watcher/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/util/watcher/watcher.go`、`pkg/util/watcher/watcher_test.go`；Cargo 元数据明确记录 `go-package = "pkg/util/watcher"`。
- Rust 独立测试：`pkg/util/watcher/watcher_test.rs`、`pkg/util/watcher/migration_aster_unit_test.rs`。覆盖事件序列、非递归列举、Remove、重复 Start、关闭后操作及通道断开。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/util/watcher` 列出 8 个 Go/Rust 文件；`node --file` 读取目标、事件、入口及测试；`query` 定位 `NewWatcher`、`do_watch`、`poll_events`、`listForName`；`callers/callees` 确认 `Start → do_watch`、`poll_events → send_event/event`，并确认 `NewWatcher` 的已索引 Rust 调用者仅在 `watcher_test.rs`。
- 仓库文本检索：确认工作区成员和 facade 依赖/再导出，并确认 `NewWatcher` 的其他 Rust 调用仅存在于 `migration_aster_unit_test.rs`，未发现生产调用点。
- 本任务为纯文档分析，按计划不运行 Cargo。最终仅执行固定 11 章节结构检查，并人工复核本文没有把未验证的生产接线或平台能力表述为已支持。
