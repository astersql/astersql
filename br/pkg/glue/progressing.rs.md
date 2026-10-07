# `br/pkg/glue/progressing.rs`

## 文件定位

`progressing.rs` 属于 Cargo 包 `astersql-br-pkg-glue`。同目录 [`Cargo.toml`](./Cargo.toml) 将该包声明为对应 Go 包 `br/pkg/glue` 的 library，库入口 [`lib.rs`](./lib.rs) 通过 `#[path = "progressing.rs"] pub mod progressing` 装配本文件，并在 crate 根重导出 `OnlyOneTask`、`ProgressWaiter`、`ProgressBar`、`MultiProgress`、`NopMultiProgress`、`LogBar`、`TerminalBar` 与 `TerminalMultiProgress`。内部的 `BarState`、`ProgressGroup`、`PbProgress`、`DummyProgress` 等实现细节保持私有。

本文件位于 BR 控制台门面与进度展示之间：它扩展 [`console_glue.rs`](./console_glue.rs) 的 `ConsoleOperations`，根据输出是否为 TTY 选择终端完成行或日志式计数；同时实现 [`glue.rs`](./glue.rs) 的 `Progress` trait。当前 Cargo 直接依赖只有 `astersql-errors`，进度状态、线程、通道、锁与输出均由 `std` 实现，并没有 Rust `mpb` 依赖。源码注释也明确称 `BarState`/`ProgressGroup` 是 Go `mpb` 行为的轻量替身，因此不能把它描述成完整动画进度条实现。

RustCodeGraph 已索引本文件（641 行、86 个符号），文件摘要显示它被 13 个 Rust 文件引用；但同名 Go/Rust 符号使精确 `callers/callees` 查询无法可靠消歧。定向源码搜索能确认当前 Rust 生产代码中的直接接线是 [`console_glue.rs`](./console_glue.rs) 的 `ConsoleOperations::ShowTask` 调用 `StartProgressBar`；`StartMultiProgress` 和显式 `ProgressWaiter::Wait` 的可执行证据来自同目录独立测试 [`parity_test.rs`](./parity_test.rs)。Go 生产树则在 `br/pkg/stream/stream_metas.go`、`br/pkg/task/stream.go` 和 `br/pkg/restore/log_client/client.go` 使用对应 API。目标目录没有 `doc.go`，也没有同名 `progressing_test.rs`/`progressing_test.go`。

## 核心职责

1. 以 `ConsoleOperations::StartProgressBar` 提供单条进度入口，并以 `OutputIsTTY` 在 `PbProgressWaiter` 与 `NoOpWaiter<DummyProgress>` 两条路径间分流。
2. 用 `BarState` 保存当前值、总量、完成/中止/已渲染状态和最终消息回调；用 `ProgressGroup` 聚合多个 bar，等待全部完成或中止后一次性收尾。
3. 通过 `ProgressWaiter` 在基础 `Progress` 契约上增加可取消等待；`PbProgress::Wait` 把阻塞式组等待放到后台线程，使调用线程能轮询 `Context` 取消。
4. 以 `OnlyOneTask = -1` 区分单任务 spinner 风格与普通进度条，并以 `total == 0` 的特殊处理保证 TTY 路径立即完成。
5. 通过 `MultiProgress`/`ProgressBar` 提供多条进度接口：TTY 使用共享 `ProgressGroup` 的 `TerminalMultiProgress`，非 TTY 使用倒计数 `LogBar`。
6. 保证 `Close`、组 `finish` 和最终行渲染幂等，避免重复 `DONE`/`ABORTED` 输出；为 `ConsoleOperations::ShowTask` 提供“一次推进并关闭”的底层能力。

## 主要符号

- `OnlyOneTask: i32 = -1`：单任务哨兵。`adjustTotal` 遇到它时把内部总量改为 1，并选择 `buildOneTaskBar`；其他负值不会获得该特殊语义。
- `coloredSpinner`、`spinner_text`、`spinner_done_text`：构造绿色加粗的 `/ - \\ |` 帧和完成后缀。当前 Rust 不逐帧刷新，`spinner_text` 的结果在建条时被丢弃，只保留与 Go 构造路径相近的行为。
- `BarState`：单条进度的共享状态。`Increment`/`IncrBy` 累计值，`SetTotal` 可改总量并强制完成，`Abort` 标记中止，`render_done` 选择最终文本，`take_render_done` 保证只渲染一次。
- `ProgressGroup`：在 `bars: Mutex<Vec<Arc<BarState>>>` 中登记条目；`Wait` 每 10 ms 检查 `all_finished`，随后由 `finish` 收集尚未渲染的最终行。`closed` 使整组收尾幂等，`out` 保存内部捕获输出但不向外暴露。
- `PbProgress` 与 `PbProgressWaiter`：TTY 单条实现。前者实现 `Progress` 以及内部可取消 `Wait`，后者是把这两套能力暴露为对象安全 `Box<dyn ProgressWaiter>` 的 newtype。
- `ProgressWaiter: Progress`：公开扩展 trait，增加 `Wait(Context) -> Result<(), SharedError>`。
- `NoOpWaiter` 与 `DummyProgress`：非 TTY 单条实现。`NoOpWaiter` 委派 `Progress` 并让 `Wait` 立即成功；`DummyProgress` 原子累计，首次 `Close` 向 stderr 打印名称、当前值和总量。
- `ConsoleOperations::{OutputIsTTY, StartProgressBar, StartMultiProgress}`：公开构造入口；私有 `startProgressBarOverDummy`、`startProgressBarOverTTY` 负责具体分流。
- `ConsoleWriter`：把 `std::io::Write` 适配到 `ConsoleOperations::Printf`。它用有损 UTF-8 解码，并把 `flush` 实现为空操作。
- `adjustTotal`、`buildProgressBar`、`buildOneTaskBar`：TTY 单条构造链。普通条把 `printFinalMessage(extraFields)` 放进完成回调；单任务条不挂额外字段。
- `ProgressBar` 与 `MultiProgress`：公开多进度抽象，分别暴露 `Increment`/`Done` 与 `AddTextBar`/`Wait`。
- `NopMultiProgress`、`LogBar`：非 TTY 多进度实现。`LogBar::Increment` 每次把剩余量减一，并在新值小于等于零时打印完成日志；`Done` 和容器 `Wait` 均为空操作。
- `TerminalMultiProgress`、`TerminalBar`：TTY 多进度实现。所有条共享同一组和 writer；单条 `Done` 中止并立即尝试渲染，容器 `Wait` 等待剩余条并输出尚未渲染的行。

## 执行流程

单条进度从 `ConsoleOperations::StartProgressBar(title, total, extraFields)` 开始：

1. `OutputIsTTY` 调用 `ConsoleGlue::out_is_terminal()`；为 false 时进入 `startProgressBarOverDummy`，为 true 时进入 `startProgressBarOverTTY`。
2. 非 TTY 路径创建 `DummyProgress` 并包装为 `NoOpWaiter`。`Inc`/`IncBy` 只增加 `current`，`Wait` 不阻塞；第一次 `Close` 输出 `progress done name=... current=... total=...`，后续 `Close` 无操作。
3. TTY 路径先建 `ProgressGroup`，再由 `adjustTotal` 选择普通条或 `OnlyOneTask` 条。普通条保存 `printFinalMessage(extraFields)` 回调；单任务条内部总量为 1，最终文案使用 spinner `DONE`。当输入 `total == 0` 时，入口调用 `SetTotal(0, true)` 立即置完成。
4. 调用 `IncBy(n)` 时，`BarState` 原子增加当前值；只有 `total >= 0 && current >= total` 才自动完成。调用 `Close` 时，尚未完成/中止的条先被 `Abort(false)`，再经 `take_render_done` 写一次最终行，最后等待组结束。
5. 调用 `Wait(ctx)` 时，后台线程执行 `ProgressGroup::Wait`；调用线程每 10 ms 检查一次 `ctx.is_cancelled()` 和通道。完成时把后台线程返回的尚未渲染行写给 `ConsoleWriter`；取消时返回 `context canceled` 错误。

`ConsoleOperations::ShowTask` 是当前 Rust 生产树中已确认的上游包装：它传入 `OnlyOneTask` 调用 `StartProgressBar`，返回的闭包依次执行 `Inc` 和 `Close`。这使单任务从 0 到 1 后以 `DONE` 收尾。

多进度从 `StartMultiProgress` 开始。非 TTY 时 `AddTextBar` 先打印 start 日志并返回 `LogBar`，每次 `Increment` 消耗一个剩余计数；TTY 时 `AddTextBar` 创建 `one_task = true` 的 `BarState` 并加入共享组。`TerminalBar::Done` 通过中止主动结束当前条，`TerminalMultiProgress::Wait` 则等待每条都已完成或中止，然后只输出尚未由单条 `Done` 输出过的最终行。

## 数据与状态

`BarState` 的 `current`、`total`、`completed` 与 `aborted` 使用 Relaxed 原子访问；计数与标志只承担最终状态判断，没有跨字段快照的一致性保证。`rendered` 使用 `SeqCst swap`，是“最终行最多一次”的关键不变量。`final_message` 放在 `Mutex<Option<Box<dyn FnMut() -> String + Send>>>` 中，因为额外字段回调可能有可变捕获，而且最终渲染可能由 `Close` 或组 `Wait` 中任一线程触发。

`ProgressGroup` 的条目向量与捕获缓冲分别由 `Mutex` 保护；`closed: AtomicBool` 使 `finish` 只能有一个调用者真正遍历并收尾。`finish` 会先克隆 `Arc<BarState>` 列表，再锁输出缓冲，避免在渲染回调期间持有 bars 锁。`out` 中的字节仅保存内部捕获副本，实际用户可见输出由 `PbProgress`/`TerminalMultiProgress` 把返回的 lines 写入 `ConsoleWriter`。

`DummyProgress.current` 与 `LogBar.total` 是可跨线程使用的原子值。`DummyProgress.closed` 防止重复关闭日志；`LogBar` 没有完成标志，所以计数到零后继续 `Increment` 会在每次调用时再次打印 done，这一点与 Go 的 `atomic.AddInt64(...) <= 0` 判断一致。`TerminalBar.group` 持有共享组以维持生命周期，当前 `Done` 方法本身不读取该字段。

总量的边界语义需要区分：`OnlyOneTask` 仅认精确的 -1；普通负总量永不通过累计自动完成，必须 `Close`/`Done` 才能解除等待；TTY 的零总量立即完成；非 TTY 单条不做自动完成判断，但其 `Wait` 本来就是立即返回。

## 依赖与调用关系

- [`lib.rs`](./lib.rs) 是模块装配和公开重导出边界；外部 crate 可从 `astersql_br_pkg_glue` 根取得公开进度 trait/类型，但无法访问 `BarState` 等内部实现。
- [`console_glue.rs`](./console_glue.rs) 提供 `ConsoleOperations`、`ConsoleGlue`、颜色函数、`ExtraField` 与 `printFinalMessage`。本文件为 `ConsoleOperations` 增加构造方法，并用 `ConsoleWriter` 回调其 `Printf`；`ShowTask` 反向调用本文件的 `StartProgressBar`。
- [`glue.rs`](./glue.rs) 提供 `Context`、`Progress` 和 `SharedError` 边界。`PbProgress`、`NoOpWaiter`、`DummyProgress` 与 `PbProgressWaiter` 实现或委派 `Progress`；`ProgressWaiter` 在其上增加等待。
- `astersql-errors` 只用于 `ProgressWaiter::Wait` 的错误返回，并在取消时通过 `New("context canceled")` 创建错误。
- RustCodeGraph `explore` 给出的可消歧关键边包括：`StartProgressBar` 调用同文件的 TTY/Dummy 分支并被 `parity_test.rs::progress_waiter_blocks_until_the_bar_finishes` 调用，`StartMultiProgress` 被 `parity_test.rs` 的公开契约和终端单次渲染测试调用。图中另有 [`glue.rs`](./glue.rs) 的 `Glue::StartProgress`/`WithProgress` 链，那是名称相近但独立的旧进度接口，不能当成本文件入口的调用边。由于图对 `Inc`、`Close`、`Wait` 等高频名称产生大量跨语言重名结果，本说明没有把模糊边当作精确生产调用关系。
- 定向源码搜索确认，当前 Rust 树之外没有生产文件直接调用 `.StartProgressBar()` 或 `.StartMultiProgress()`；Go 对照的生产调用者包括 `br/pkg/stream/stream_metas.go` 和 `br/pkg/task/stream.go` 的单条任务/删除进度，以及 `br/pkg/restore/log_client/client.go` 的多进度恢复。它们证明 Go API 的应用位置，但不等于 Rust 版本已经接入同一主链。
- [`parity_test.rs`](./parity_test.rs) 是相关独立 Rust 测试：`progress_waiter_blocks_until_the_bar_finishes`、`terminal_bar_done_is_rendered_only_once` 直接覆盖本文件；`go_rust_public_contract_matches` 覆盖非 TTY 多进度、`OnlyOneTask` 和 `ShowTask` 生命周期；`crate_root_exports_go_public_progress_types` 覆盖重导出。

## 错误处理与边界

显式业务错误只有 TTY `Wait` 的上下文取消，它返回新建的 `SharedError("context canceled")`。后台发送端断开被当作正常完成；这会隐藏后台线程 panic 或异常退出。`Progress` 的 `Close`、多进度的 `Done`/`Wait` 以及所有输出操作均无错误返回，因此 `writeln!`、`flush` 和内部缓冲写失败被有意忽略，调用者无法观察输出故障。

`bars`、`out` 和 `final_message` 的锁通过 `expect(...)` 获取，锁中发生 panic 后再次访问会因 poison 而 panic。额外字段回调也可 panic；本文件不捕获。`ConsoleWriter` 以 `String::from_utf8_lossy` 接受任意字节，因此不会因非法 UTF-8 报错，而会替换非法序列。

`Close` 的语义不是“把 current 写成 total”：未完成条会被标为 aborted，最终显示 `ABORTED`；已经自动完成的条显示 `DONE` 或额外字段。`PbProgress::Close` 最终会等待其组，而单条构造时组里只有这一条，所以正常不会长期阻塞；若未来复用同一组构造多个 `PbProgress`，这一假设会改变。

`ProgressGroup::Wait` 没有超时，只认 `Completed || Aborted`。负总量、漏调 `Done` 或未推进到总量都会让 TTY `Wait` 永久轮询；调用带 `Context` 的 `ProgressWaiter::Wait` 可以让调用者返回错误，但不能停止后台组等待线程。多进度 `Wait` 没有 `Context`，只能依赖每条正确收尾。

## 并发与资源生命周期

公开进度 trait 要求 `Send + Sync`，单条状态经 `Arc` 共享，`Inc`/`IncBy` 可由多个线程调用。原子 `fetch_add`/`fetch_sub` 保证计数更新不丢失；最终展示由 `rendered.swap(true, SeqCst)` 串行化。`ProgressGroup::finish` 另以 `closed.swap(true, SeqCst)` 串行化整组收尾，所以单条 `Done` 与整组 `Wait` 并发时也只会输出一次最终行，独立测试 `terminal_bar_done_is_rendered_only_once` 对此有直接断言。

`PbProgress::Wait` 每次调用都会新建一个 OS 线程和一个 mpsc 通道。若 `Context` 取消，调用线程立即返回，但后台线程仍在 `ProgressGroup::Wait` 中，直到 bar 后续完成或中止；若调用者随后丢弃未关闭的 waiter，这个线程可能长期存活。它不是 async task，也没有 join handle 或取消通道。10 ms 的轮询 sleep 同样占用 OS 线程，只是降低空转。

`PbProgress::Close` 先使当前条终止、尝试写最终行，再调用组 `Wait`；重复 `Close` 会因 bar 已中止/完成和 `take_render_done` 幂等而不重复输出。`DummyProgress::Close` 由独立 `closed` 标志实现同样的幂等性。多进度的 `TerminalBar::Done` 不等待整组，资源由 `TerminalMultiProgress::Wait` 统一收尾；非 TTY `LogBar::Done` 没有资源动作。

输出 writer 被 `Arc<Mutex<dyn Write + Send>>` 串行保护。最终消息回调在 `final_message` 锁内执行，耗时或重入回调会延长锁持有时间；新增回调不应反向触发同一 bar 的最终渲染。性能上，`all_finished` 每 10 ms 锁定并线性扫描全部 bars，多条数量很大时会产生 O(条数 × 轮询次数) 的检查成本。

## 与 Go 版本的对应关系

Rust 文件直接对照 [`progressing.go`](./progressing.go)：Go `pbProgress`/`ProgressWaiter`/`noOPWaiter` 分别对应 Rust `PbProgress`/`ProgressWaiter`/`NoOpWaiter`；`adjustTotal`、`buildProgressBar`、`buildOneTaskBar` 保持同名构造层次；`NopMultiProgress`/`LogBar`/`TerminalBar`/`TerminalMultiProgress` 及其接口也逐项对应。

已验证的一致语义包括：`OnlyOneTask = -1`；零总量 TTY 条立即完成；普通累计达到总量后完成；未完成时 `Close`/`Done` 走 abort；`Wait` 可因上下文取消返回；非 TTY 多进度按剩余计数小于等于零记录完成；多条 TTY 进度由容器统一等待；完成回调延迟求值额外字段。

当前实现与 Go 的重要差异或移植边界如下：

- Go 使用 `mpb.Progress`/`mpb.Bar`、400 ms 刷新率、真实 filler/decorator 和终端动画；Rust 用内存状态机与最终行模拟，不逐帧渲染百分比或 spinner，也没有 `mpb` 依赖。
- Go `OutputIsTTY` 要求 `Out()` 是 `*os.File` 并用 `term.IsTerminal(fd)` 判断；Rust 委托 `ConsoleGlue::out_is_terminal()`，因此测试胶水和自定义胶水可以自行声明 TTY。
- Go 非 TTY 单条委托 `utils.StartProgress(context.TODO(), ..., redirectLog=true)`；Rust `DummyProgress` 直接向 stderr 输出一次完成行，没有 utils 进度器的定时日志/上下文行为。
- Go `pbProgress.Close` 调用 `bar.Wait()` 和 `progress.Wait()`；Rust 没有单独的 bar wait，依靠状态标志和 `ProgressGroup::Wait`。Rust 以 `rendered` 标志显式防止 `Close` 与组等待双写。
- Go `pbProgress.Wait` 用 goroutine 和 `select`；Rust 用 OS 线程、10 ms 轮询和 `try_recv`。取消后两端的等待执行单元都可能继续等组完成，但 Rust 每次调用明确占用独立线程。
- Go 普通条展示实时百分比、绿色标题、填充样式及动态完成字段；Rust只在收尾时输出 `title :: final_message`，没有中间显示。
- Go `TerminalBar.Done` 会 `Abort` 后 `bar.Wait()`；Rust 立即渲染该条并由之后的组 `Wait` 跳过已渲染行。`parity_test.rs` 验证 Rust 早退路径只出现一次 `ABORTED`。
- Go 使用结构化 `log.Info`；Rust 非 TTY 路径使用 `eprintln!` 文本，日志字段、级别和日志后端并不等价。

## 扩展指南

- 增加单条行为时优先保持 `StartProgressBar → adjustTotal → build*Bar` 的分层；新增总量边界必须同步覆盖零、`OnlyOneTask`、其他负数、超量累计与提前 `Close`。独立测试应继续放在 [`parity_test.rs`](./parity_test.rs) 或新增同目录 `*_test.rs`，不要嵌入生产文件。
- 改最终文案时同时检查 `BarState::render_done` 的优先级：未完成且 aborted 应显示 `ABORTED`，单任务完成显示 spinner `DONE`，普通完成才调用 `printFinalMessage`。必须保留 `take_render_done` 的一次性语义并测试 `Close`/`Done` 与组 `Wait` 并发。
- 若接入真实动画库，需对齐 Go 的 TTY 判断、400 ms 刷新率、普通条样式、百分比和 ExtraField 延迟求值；外部 Rust 依赖必须按仓库规则在独立上游仓库移植、提交并打 tag，再以统一 tag Git 依赖引用，不能用本地 `[patch]` 或 vendor 副本。
- 若把等待改为 async 或可取消线程，必须明确取消后谁负责终止组等待、是否允许多次 `Wait`、后台任务如何 join，以及 `Close` 与取消竞态的输出结果。增加测试覆盖“取消后再 Close”“多个 waiter”“负总量未关闭”和丢弃 waiter。
- 扩展多进度时保持 `ProgressBar`/`MultiProgress` 的对象安全和 `Send + Sync` 契约。新增 remove/finish API 应统一 `LogBar` 与 `TerminalBar` 的主动完成语义；当前 `LogBar::Done` 为空且零后继续递增会重复日志，改变前应先确认 Go 兼容需求。
- 优化大量 bar 的轮询性能时可考虑条件变量或通知机制，但必须保留原子并发推进、全体完成条件和单次最终渲染。性能测试应独立覆盖高并发 `Increment` 和大量 bar，不应通过降低语义要求简化实现。
- 将 Rust API 接入 Go 已有 stream/restore 对应主链时，应以实际 Rust import 和 Cargo 依赖为准；目前不能仅凭 Go 调用点宣称 Rust 已接线。接线后需为 `stream_metas`、stream task 和 restore log-client 的生命周期分别增加集成级验证。

## 验证依据

- RustCodeGraph：运行 `status`，确认索引含 7032 个 Rust 文件；运行 `files --filter br/pkg/glue`，确认目标、Go 对照、crate 入口与独立测试均被索引；运行 `explore 'br/pkg/glue/progressing.rs Progress Inc StartProgress UpdateProgress IncByOne Close'`，取得 `StartProgressBar`、`StartMultiProgress`、`ProgressWaiter`、测试及文件引用摘要；运行 `node --file br/pkg/glue/progressing.rs --offset 1 --limit 260`、`--offset 231 --limit 260`、`--offset 491 --limit 180` 阅读完整 641 行。精确 `callers/callees` 因同名跨语言节点无法消歧，故只把 explore 中明确文件化的边作为图证据。
- Rust 生产源码与装配：完整核对 [`progressing.rs`](./progressing.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)；核对 [`glue.rs`](./glue.rs) 的 `Context`/`Progress` 和 [`console_glue.rs`](./console_glue.rs) 的 `printFinalMessage`/`ShowTask`。目标目录没有 `doc.go`。
- Go 对照：完整核对 [`progressing.go`](./progressing.go) 的单条、多条、TTY 分流、`mpb` 装饰和等待逻辑；定向搜索核对 `br/pkg/stream/stream_metas.go`、`br/pkg/task/stream.go`、`br/pkg/restore/log_client/client.go` 的生产调用点。未发现同名 Go 独立测试。
- Rust 独立测试：完整核对 [`parity_test.rs`](./parity_test.rs)，重点是 `go_rust_public_contract_matches`、`progress_waiter_blocks_until_the_bar_finishes`、`terminal_bar_done_is_rendered_only_once` 和 `crate_root_exports_go_public_progress_types`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核文档明确区分“当前 Rust 事实”“Go 生产调用点”和“尚未验证的 Rust 接线”，不把轻量状态机写成真实 `mpb` 动画实现。
