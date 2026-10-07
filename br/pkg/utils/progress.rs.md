# `br/pkg/utils/progress.rs`

## 文件定位

`progress.rs` 属于 `astersql-br-pkg-utils` library crate；crate 根 `br/pkg/utils/lib.rs` 通过 `#[path = "progress.rs"] pub mod progress` 将它编入，并在 `cfg(test)` 下另行挂载 `progress_test.rs`。它移植自同目录 `progress.go`，目标是提供一个可跨线程递增、定时输出、可由取消或显式关闭终止的 BR 进度打印器。

当前接线状态需要与设计目标区分：RustCodeGraph 能确认本文件内部链路及独立测试调用，但没有找到来自其他 Rust 生产文件的 `StartProgress`/`ProgressPrinter` 调用。相邻的 `br/pkg/gluetikv/glue.rs::StartProgress` 还明确使用本地 `CounterProgress`，注释说明这是为避免 utils/logutil 依赖而保留的替代实现。因此，本文件当前是已编入 utils crate、具备行为测试的移植实现，尚不是 Rust BR Glue 主链实际使用的进度实现。Go 生产链则由 `br/pkg/gluetikv/glue.go::StartProgress` 调用 `utils.StartProgress`。

## 核心职责

- `ProgressPrinter` 保存任务名称、总量和原子当前值，为调用者提供 `Inc`、`IncBy`、`GetCurrent`、`Close` 四个操作（`progress.rs:63-122`）。
- `goPrintProgress` 建立控制/完成通道并启动后台线程，以一秒为周期把原子计数复制为“已渲染进度”（`progress.rs:126-208`）。
- `emit_progress` 将一帧进度转换为终端文本、测试 JSON 或结构化日志；三条输出路径互斥（`progress.rs:240-301`）。
- `StartProgress` 把“构造”和“启动”组合成一个入口；`StartProgressWithWriter` 暴露测试 sink 注入入口（`progress.rs:313-338`）。
- `estimate_remaining` 根据平均速度线性估算剩余时间（`progress.rs:303-310`）。

本文件只负责内存计数、展示和生命周期同步，不保存业务 checkpoint，不决定任务成功或失败，也不把取消转换成可返回的业务错误。

## 主要符号

- `type LogFunc = Arc<dyn Fn(&str, Vec<Field>) + Send + Sync>`：可跨线程共享的结构化日志回调。缺省回调经 `info_log` 转发到 `astersql_br_pkg_logutil::log::L().Info`。
- `type TestProgressSink = Arc<dyn Fn(&str) + Send + Sync>`：测试专用 JSON 行接收器，绕过日志系统。
- `ProgressPrinter`：公开句柄。`name` 成为日志的 `step` 字段；`total` 是完成分母；`redirectLog` 决定是否禁止终端输出；`progress` 是共享原子计数；`closeMu` 串行化关闭；`closeCh` 和 `closed` 分别持有退出通知 sender 与退出确认 receiver。
- `NewProgressPrinter(name, total, redirectLog) -> ProgressPrinter`：只初始化状态，不启动线程。新对象的 `closeCh`、`closed` 都是 `None`，调用者若直接 `Close` 只会收到告警。
- `ProgressPrinter::Inc` / `IncBy`：用 `AtomicI64::fetch_add(..., Ordering::Relaxed)` 增量更新；允许暂时超过 `total`，展示阶段再封顶。
- `ProgressPrinter::GetCurrent`：用 Relaxed load 返回原始累计值，因此可能大于 `total`。
- `ProgressPrinter::Close`：在 `closeMu` 保护下取得并清空 sender，发送关闭通知，再等待后台线程发送完成确认。sender 已不存在时记录 `closing no-started progress printer`。
- `ProgressPrinter::goPrintProgress`：安装通道并生成后台线程；这是包内测试与组合入口共同使用的核心执行函数。
- `ProgressLogLine::from_value`：从 JSON 值读取 `P/C/E/R/S` 五个字符串字段；字段缺失或类型错误时返回 `None`。
- `emit_progress`：计算百分比、速度和剩余时间，并选择输出通道。
- `StartProgress`：生产意图入口，返回已启动的打印器。
- `StartProgressWithWriter`：额外接受 `TestProgressSink` 的测试辅助入口；RustCodeGraph 未发现生产调用。
- `progress_error`：仅原样返回 `SharedError` 的未使用预留钩子，不构成现有错误处理能力。

本文件没有模块级常量、trait、enum 或条件编译项；固定的一秒周期和 10 毫秒最大轮询片段直接写在 `goPrintProgress` 中。

## 执行流程

1. 调用者通过 `NewProgressPrinter` 构造句柄，或者用 `StartProgress` 一次性构造并启动。后者的图内调用链是 `StartProgress -> NewProgressPrinter -> goPrintProgress`。
2. `goPrintProgress` 创建 `close_tx/close_rx` 与 `done_tx/done_rx` 两对 MPSC 端点，把用于控制和等待的两端写入打印器，然后克隆原子计数并生成线程。
3. 线程记录起始时间，选择调用方日志回调或默认 `info_log`，计算是否使用终端，并把第一次 tick 安排在启动后一秒。
4. 循环每次最多等待 10 毫秒，使它既能及时收到 `Close`，也能频繁检查 `Context` 取消。普通超时尚未到 tick 时继续等待；到 tick 时读取当前原子值，以 `min(current, total)` 得到展示值并调用 `emit_progress`。
5. 收到显式关闭通知时，不读取当前值，直接以 `total/total` 输出最终帧，发送完成确认并退出。因此 `Close` 表示“按完成收尾”，而非“停止但保留进度”。
6. 检测到 `ctx.is_cancelled()` 时，以最后一次 tick 保存的 `rendered` 输出最终帧并退出。取消发生在首个 tick 前时，该值仍为 0；这刻意对齐 Go `pb.Bar::Finish` 只保留已复制到 bar 的值。
7. `emit_progress` 在终端路径写 stdout；测试 sink 存在时投递 JSON 字符串；否则把 JSON 中的五个字段转换为 `step/progress/count/speed/elapsed/remaining` 结构化日志字段。

## 数据与状态

原始业务进度只存在于 `Arc<AtomicI64>` 中。`Inc`、`IncBy` 与 `GetCurrent` 使用 Relaxed 顺序，因为展示允许短暂滞后，也没有其他数据需要借该原子建立 happens-before 关系。原始值不封顶；每秒渲染时才使用 `current.min(total)`，所以 `GetCurrent` 和用户看到的计数可能不同。

后台线程另有局部 `rendered`，表示最后一次 tick 已经复制到展示层的值。它是取消语义的关键状态：取消不会读取更新更快的原子计数，而是保留这份已渲染快照。显式 `Close` 则忽略两者，强制输出 `total`。

输出 JSON 的字段契约为：`P` 百分比、`C` 当前/总数、`E` 已用时间、`R` 预计剩余时间、`S` 平均速率。`total == 0` 被视为 100%，避免除零；速度使用 `speed_base / elapsed`；剩余时间只在线性平均速度成立时估算，`current <= 0` 或 `current >= total` 时为零。

关闭状态由两个 `Mutex<Option<...>>` 保存。启动后 `Some(sender/receiver)` 表示句柄拥有一次有效关闭握手；`Close` 用 `take` 消耗端点，使并发或重复关闭不会再次向同一后台线程发送。`redirectLog`、`name`、`total` 在启动时复制进线程，之后不可修改。

## 依赖与调用关系

内部调用链由 RustCodeGraph 核对为：

```text
StartProgress / StartProgressWithWriter
  -> NewProgressPrinter
  -> ProgressPrinter::goPrintProgress
       -> is_terminal_output
       -> emit_progress
            -> estimate_remaining
            -> ProgressLogLine::from_value
            -> LogFunc / TestProgressSink / stdout
```

标准库依赖承担原子计数、互斥、MPSC 通道、线程、计时与输出。`crate::stubs::context::Context` 提供 `is_cancelled`；`astersql-br-pkg-logutil` 提供 `Field` 和日志入口；`serde_json` 生成/读取五字段 JSON；`astersql-errors::SharedError` 当前只被预留函数引用。`br/pkg/utils/Cargo.toml` 确认该 crate 是 `br/pkg/utils` Go package 的 library port，并直接声明上述 logutil、errors 与 `serde_json` 依赖。

模块入口 `br/pkg/utils/lib.rs` 公开 `progress` 模块，但没有在 crate 根再次导出其中函数。RustCodeGraph 的已验证上游限于 `progress_test.rs` 和本文件的组合函数；`br/pkg/gluetikv/glue.rs` 的 Rust 生产实现仍走 `CounterProgress`。因此扩展时不能仅因 Go Glue 使用本模块，就假定 Rust Glue 已接线。

## 错误处理与边界

计数和渲染 API 不返回 `Result`。日志写入、stdout 写入、关闭发送以及完成确认发送的错误都被忽略；这使进度报告不会打断备份/恢复业务，但也意味着输出失败对调用者不可见。`ProgressLogLine::from_value` 失败时静默跳过日志帧，不过当前 JSON 由同一函数构造，正常路径字段是完整字符串。

`Mutex::lock` 使用 `expect`，锁若因持锁线程 panic 而 poison，调用方会 panic。`Close` 在有效启动后同步等待 `closed.recv()`；若后台线程未能正常发送，recv 在 sender 被丢弃时会返回错误并被忽略，而不是永久要求成功确认。未启动或已经消耗 sender 的 `Close` 只告警。

边界值方面，`total == 0` 显示 100%；正 total 下超额累计在渲染时封顶；负 `total` 没有输入校验，`min(current, total)` 与百分比计算会产生不符合通常进度语义的结果，调用方应保证总量非负。`IncBy` 也接受负数，文件本身不维护单调性。`estimate_remaining` 是匀速外推，任务速度变化时只提供近似值。

## 并发与资源生命周期

每次 `goPrintProgress` 调用都会生成一个未保存 `JoinHandle` 的后台线程。线程持有进度原子的 `Arc`、上下文、日志回调和可选测试 sink；正常退出条件只有显式关闭、上下文取消或关闭通道断开。若调用者既不 `Close`、也不取消 Context，线程将持续到进程结束，即使 `ProgressPrinter` 句柄已丢弃；句柄 drop 会丢弃 sender，后台下一次接收可观察到通道断开并退出，但该路径不发送最终帧或完成确认。

`Close` 通过 `closeMu` 串行化，避免两个调用者同时 `take` 通道并重复等待。关闭通知会唤醒 `recv_timeout`，不必等待下一秒 tick；后台输出最终 100% 帧后才发送 done，因此 `Close` 返回时可认为最终帧处理已经完成。取消检查最多受 10 毫秒轮询片段影响；测试验证取消和 Close 都能快速结束。

重复调用 `goPrintProgress` 没有防护：它会覆盖句柄中保存的控制/完成端点，却留下先前线程继续运行。安全用法是不超过一次启动，并由所有递增方共享该句柄的引用。当前 `ProgressPrinter` 本身未实现 `Clone`，若需跨线程持有通常应在外层使用 `Arc<ProgressPrinter>`。

## 与 Go 版本的对应关系

Rust 的 `ProgressPrinter`、构造函数、四个公开方法以及 `StartProgress` 与 `progress.go` 同名语义对齐。两边都使用原子 i64 累计；每秒把原子值复制到展示层；超额值封顶；Context 取消保留当前已渲染位置；`Close` 强制到 100% 并等待打印协程/线程收尾。Rust 独立测试复刻 Go `TestProgress` 的 50%、100%、超额封顶、Close 抬满和 cancel 保留 25% 场景。

实现方式存在明确差异：Go 使用 `cheggaaa/pb` 处理模板、刷新和终端检测，Rust 自行计算百分比、速度、剩余时间并组装 JSON。Go 非终端 bar 的 refresh rate 是两分钟、测试 writer 是两秒，但原子值由独立一秒 ticker更新；Rust每个一秒 tick 都直接发出一帧。Rust `is_terminal_output` 当前恒为 `false`，所以没有实际终端进度条；Go 会用 `term.IsTerminal(stdout)` 检测 TTY。Rust 终端分支即使未来启用，其 20 字符条和 Go pb 模板也不是逐字符等价。

Go `wrappedWriter::Write` 的 JSON 反序列化失败会返回错误，Rust 的同类转换失败会静默丢帧。Go `Close` 在 `closeCh != nil` 时不会把通道字段清空，重复 Close 可能继续发送并等待；Rust 用 `Option::take` 将关闭做成单次握手，重复调用会告警。Rust 还新增了 `StartProgressWithWriter`、首 tick 前取消回归测试和 Close 立即唤醒测试，这些是为精确锁定 Go 可观察语义的测试设施。

最重要的迁移差距不是算法，而是接线：Go `gluetikv` 调用 `utils.StartProgress`；Rust `gluetikv` 目前返回本地 `CounterProgress`，不打印、不记录名称/总量，也没有关闭线程。因此本文档描述的是本文件自身真实行为，不能据此声称 Rust BR 命令已经获得同等进度输出。

## 扩展指南

- 若要让 Rust BR 主链真正使用该实现，应从 `br/pkg/gluetikv/glue.rs::StartProgress`（以及需要的 gluetidb 委托链）评估依赖接线，并确保返回类型实现 `br/pkg/glue/glue.rs::Progress`；不能只改本文件。同步扩展独立测试，而不要把测试嵌入 `progress.rs`。
- 若启用终端输出，应修改 `is_terminal_output`，核对 stdout/stderr 选择、非 TTY/CI 行为和 `redirectLog` 优先级，并在独立测试中覆盖终端、日志、测试 sink 三条互斥路径。
- 若改变刷新周期，应同时检查首帧延迟、取消使用 `rendered` 的语义、Close 唤醒延迟和日志量；相关测试是 `progress_waits_for_first_tick_but_close_is_immediate` 与 `cancel_preserves_last_rendered_progress_before_first_tick`。
- 若增加错误感知，不应把现有 `progress_error` 误当成钩子实现；需要设计返回/通知契约，并决定输出失败是否影响业务。当前 API 有意吞掉展示错误。
- 若允许多次启动或自动 drop 收尾，需要显式管理 `JoinHandle` 与线程所有权，避免覆盖通道和遗留线程。任何改动都应保持 `Close` 的一次性同步收尾不变量。
- 若调整百分比、计数或日志字段，需保持 Go 消费者依赖的 `P/C/E/R/S` 与结构化字段名称兼容，并同步 `br/pkg/utils/progress_test.rs`、`progress_test.go` 的对应断言。
- 性能风险主要来自刷新频率、每帧 JSON 分配和日志写入；正确性风险集中于取消/关闭竞态、负计数、重复启动及最终帧顺序。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标源码被完整读取为 345 行。
- RustCodeGraph `node --file br/pkg/utils/progress.rs`：核对全部类型、函数、方法、字段和执行分支。
- RustCodeGraph `explore`：核对 `StartProgress -> goPrintProgress -> emit_progress` 流程；确认 `NewProgressPrinter`、`goPrintProgress`、`Inc`、`Close` 的调用者包括三个独立 Rust 测试，并确认 `emit_progress -> estimate_remaining`。
- `br/pkg/utils/lib.rs`：确认生产模块挂载和独立 `progress_test.rs` 测试挂载；不存在测试逻辑内嵌。
- `br/pkg/utils/Cargo.toml`：确认 crate 名称、library 边界、Go package 元数据及 logutil/errors/serde_json 依赖。
- `br/pkg/utils/progress.go`、`br/pkg/utils/progress_test.go`：核对原始 Go API、ticker、终端/日志输出、取消与 Close 语义，以及 50%/100%/封顶/取消测试意图。
- `br/pkg/utils/progress_test.rs`：核对移植测试，并额外确认首 tick 延迟、Close 立即输出、首 tick 前取消保留 0% 三项边界。
- `br/pkg/gluetikv/glue.rs:134-153,258-273` 与 `br/pkg/gluetidb/glue.rs:370-380`：确认 Rust 生产 Glue 当前使用本地计数器及委托关系，未调用本文件。
- 按任务约束未运行 Cargo；交付验证只执行固定十一章节的结构检查，并人工复核文档未把未接线实现描述为已在生产主链使用。
