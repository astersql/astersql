# `br/pkg/glue/console_glue.rs`

## 文件定位

该文件属于 Cargo 包 `astersql-br-pkg-glue`（`br/pkg/glue/Cargo.toml`），由 crate 根 `br/pkg/glue/lib.rs` 以 `pub mod console_glue` 挂载并通过 `pub use console_glue::*` 再导出。它是 BR 与控制台之间的端口和表现层：一端接收 `Glue::AsConsoleGlue` 提供的 I/O 能力，另一端向 BR 调用方提供提示、列表、表格、定宽换行和彩色文本等操作。

它不是进度条状态机本身。`ConsoleOperations::ShowTask` 会调用同 crate 的 `progressing.rs` 中实现的 `ConsoleOperations::StartProgressBar`；后者再按输出是否为 TTY 选择终端进度或 dummy 日志进度。RustCodeGraph 将本文件识别为被 `glue.rs`、`progressing.rs`、`console_glue_test.rs`、`parity_test.rs`、`br/cmd/br/stubs.rs` 等文件使用。

当前 Rust 生产接线的直接证据是 `br/pkg/gluetikv/glue.rs`：其 `Glue::AsConsoleGlue` 返回内嵌 `StdIOGlue`。`GetConsole` 在实现方不提供控制台时退化为 `NoOPConsoleGlue`，因此 SQL/嵌入式调用不会向用户终端输出。目标目录没有 `doc.go`；包边界由 `lib.rs`、Cargo 元数据和 Go 同路径实现共同确定。

## 核心职责

1. 用 `ConsoleGlue` trait 抽象输入、输出、TTY 探测和终端宽度，使 CLI、嵌入式调用及测试共享同一组上层操作。
2. 用 `ConsoleOperations` 实现打印、扫描、布尔确认、列表截断、表格布局及单任务进度入口。
3. 用 `ExtraField`、`WithTimeCost`、`WithConstExtraField`、`WithCallbackExtraField` 和 `printFinalMessage` 组装任务完成信息。
4. 用 `PrettyString` 维护“带 ANSI 的文本”和“去 ANSI 的可见文本”之间的位置映射，再由 `Frame` 按可见宽度折行。
5. 提供三种端口实现：真实标准输入输出 `StdIOGlue`、完全丢弃 I/O 的 `NoOPConsoleGlue`，以及可预置输入和捕获输出的 `BufferConsoleGlue`。
6. 提供轻量 `color` 子模块，以 SGR 属性码模拟 Go `fatih/color` 的本文件所需子集，并由全局 `color::NoColor` 控制是否发射 ANSI。

## 主要符号

- `defaultTerminalWidth: i32 = 80`：TTY 宽度探测不可用时的回退值。
- `color::{NoColor, Attribute, Color, New, *String}`：保存 SGR 属性并包装文本；`NoColor` 是 `AtomicBool` 全局开关，使用 relaxed 原子读写。
- `ConsoleOperations { glue: Arc<dyn ConsoleGlue> }`：业务门面。`Arc` 允许克隆门面并把同一端口交给进度输出适配器。
- `ExtraField = Box<dyn FnMut() -> [String; 2] + Send>`：完成行中的动态键值字段。`FnMut` 允许 `WithTimeCost` 缓存首次求值结果。
- `WithTimeCost()`：捕获 `Instant`，首次调用时把耗时四舍五入到毫秒并缓存；键固定为 `take`。
- `format_duration(Duration)`：把毫秒精度时长格式化成 `0s`、整数秒、毫秒或小数秒形式。
- `ConsoleOperations::{ShowTask, PromptBool, Scanln, Print, Println, Printf, RootFrame, CreateTable}`：任务、交互和渲染入口。
- `PrintList<T>`：打印标题和项目；`maxItemsDisplay > 0` 时截断并报告剩余数，同时用 `eprintln!` 记录完整调试表示。
- `Table<'a>`：保存两列字符串；`Print` 根据最长键和至少 40 列的值区决定单行布局或两行紧凑布局。
- `ConsoleGlue: Send + Sync`：要求实现 `Out`、`In`；TTY 判定默认 false，宽度默认 80。
- `GetConsole(&dyn Glue)`：调用 `Glue::AsConsoleGlue`，没有实现时返回 NoOP 门面。
- `StdIOGlue`：绑定 `stdin/stdout`；Unix 上通过 `ioctl(TIOCGWINSZ)` 读取 stdin 的列数，其他平台回退 80。
- `BufferConsoleGlue`、`SharedWriter`、`SharedReader`：用 `Arc<Mutex<...>>` 共享捕获缓冲和输入游标。
- `PrettyString`、`NewPrettyString`、`find_ansi_escapes`、`strip_ansi`：识别 `ESC[...m`，保存带色串、裸串和转义字节区间。
- `Frame<'a>`：保存 `offset`、剩余 `width` 和控制台引用；负责折行、续行缩进和子区域派生。

## 执行流程

控制台选择流程如下：调用方把 `&dyn Glue` 交给 `GetConsole`；函数查询 `AsConsoleGlue`，存在则放进 `ConsoleOperations`，否则安装 `NoOPConsoleGlue`。纯 TiKV CLI 的 Rust 实现通过 `br/pkg/gluetikv/glue.rs::AsConsoleGlue` 返回 `StdIOGlue`。随后所有打印与交互均经 `ConsoleOperations.glue` 取得端口。

单任务流程从 `ShowTask` 开始。它以 `OnlyOneTask` 调用 `StartProgressBar`，返回一个只可调用一次的完成闭包；闭包先 `Inc` 再 `Close`。`StartProgressBar` 定义在 `progressing.rs`：非 TTY 走 `DummyProgress`，TTY 走 `PbProgress`；普通进度完成行会调用本文件的 `printFinalMessage`，但 `OnlyOneTask` 分支使用 spinner 的 DONE 文案，传入的附加字段不会用于该分支的最终渲染。

交互确认流程由 `PromptBool` 控制。输入不是终端时直接返回 true；交互时打印 `"(y/N) "`，再由 `Scanln` 逐字节读取一行。仅 `y`（忽略 ASCII 大小写）返回 true，空输入或 `n` 返回 false，其他单 token 会重新询问；EOF、读取错误、无 token 或额外 token 都导致扫描失败，`PromptBool` 将其视为 false。

表格流程先由 `Table::maxKeyLen` 取键的最大字节长度，再调用 `RootFrame().OffsetLeftWithMinWidth(maxLen + 2, 40)`。宽度足够时键右对齐，值从冒号后开始并在续行保持偏移；宽度不足时键不填充，值另起一行。值会加粗，再经 `PrettyString` 和 `Frame::Print` 按可见宽度分段。

ANSI 折行流程先由 `NewPrettyString` 扫描合法 SGR 区间并剥离它们得到 `raw`。`SplitAt` 把 raw 字节下标映射回 pretty 字节下标，构造左右两段并重新基准化右段的转义区间。`Frame::Print` 反复按 `width` 切片，片段之间输出换行和 `offset` 个空格。

## 数据与状态

`ConsoleOperations` 自身只保存一个共享 trait object，没有可变业务状态。具体端口决定状态位置：`StdIOGlue` 无字段；`NoOPConsoleGlue` 无字段；`BufferConsoleGlue` 的输出是 `Arc<Mutex<Vec<u8>>>`，输入是 `Arc<Mutex<Cursor<Vec<u8>>>>`，宽度是构造后可覆盖的 `i32`。

`ExtraField` 闭包拥有自己的捕获状态。`WithTimeCost` 保存起点和缓存的 `Duration`；缓存以零时长为“尚未缓存”哨兵，因此若四舍五入结果仍为零，下一次求值会重新计算。常量字段在构造时完成字符串化，回调字段则在每次求值时重新产生值。

`PrettyString` 同时保存 `pretty`、`raw` 和 `escapeSequencePlace`。区间是 pretty 字符串中的半开字节范围 `[start, end)`；`Len` 返回 raw 的字节长度，不是 Unicode 字符数或终端显示列宽。`Frame` 和 `Table` 借用 `ConsoleOperations`，因此不能比对应门面活得更久。

`color::NoColor` 是进程级共享状态；修改它会影响所有后续 `Color::Sprint`。测试显式把它设为 false，但没有在本文件中提供作用域恢复机制，新增并行测试时需避免相互污染。

## 依赖与调用关系

crate 内部依赖有两条。其一是 `crate::glue::Glue`：`GetConsole` 调用 `Glue::AsConsoleGlue`。其二是 `crate::progressing::{OnlyOneTask, ProgressWaiter}`：`ShowTask` 依赖 `StartProgressBar` 返回对象的 `Inc/Close`，而 `progressing.rs` 反向使用本文件的 `ConsoleOperations`、`ExtraField`、`color` 和 `printFinalMessage`。这是有意的同 crate 模块协作，不是独立 Cargo 依赖。

标准库承担全部 I/O、同步和计时：`Read/Write/IsTerminal`、`Arc/Mutex`、`Instant/Duration`。Unix 终端宽度通过本文件声明的 C `ioctl` 调用获得；Linux/Android 与其他 Unix 使用不同的 `TIOCGWINSZ` 常量。`br/pkg/glue/Cargo.toml` 唯一声明的外部依赖是 `astersql-errors`，但本文件本身不直接使用它，它由同 crate 的 `glue.rs`/`progressing.rs` 使用。

上游接线方面，`br/pkg/gluetikv/glue.rs` 的具体 `Glue` 返回 `StdIOGlue`；`GetConsole`、`PrintList`、`ShowTask` 的当前 Rust 直接调用证据主要位于 `br/pkg/glue/parity_test.rs`。同目录 `console_glue_test.rs` 直接覆盖 `PrettyString`、`Frame`、`Scanln` 和时长格式。RustCodeGraph 的文件级关系还列出 `br/cmd/br/stubs.rs` 等使用者，但精确 callers/callees 查询本次没有返回可消费的边，因此本文不据此声称更多具体生产调用。

## 错误处理与边界

`Print`、`Println` 和 `Printf` 对 `Out()` 失败直接返回，并忽略 `write!`、`writeln!` 和 `flush` 的错误。这保证嵌入路径不会因展示失败而 panic，但也意味着磁盘满、断管等输出故障不会反馈给业务调用方。`PrintList` 的完整项目日志通过 stderr 输出，不走可注入的 `ConsoleGlue`。

`Scanln` 会传播底层读取错误。空白行以 `InvalidData` 返回，无换行且没有 token 以 `UnexpectedEof` 返回；读到第一个 token 后如果还有第二个 token，则把第一个 token写入 `ans` 后返回 `InvalidData`。它用 `String::from_utf8_lossy` 处理非法 UTF-8，因此输入内容可能被替换字符归一化。

`PrettyString::SplitAt` 对负数显式 panic，超过或等于 raw 长度则返回“全文、空串”。实现和 Go 一样按字节计数，但 Rust 字符串切片还要求 UTF-8 字符边界；若非 ASCII 文本的切点落在多字节字符内部，`self.raw[..n]` 或 pretty 切片会 panic。当前测试覆盖 ASCII 和 ANSI，不证明任意 Unicode 显示宽度正确；全角字符、组合字符也不会按真实终端列宽计算。

ANSI 解析器只接受 Go 正则 `\x1b\[(?:(?:\d+;)*\d+)?m` 对应的 SGR 形式：空参数 `ESC[m` 合法，前导分号等非法形式会保留为普通文本。它不解析其他终端控制序列。

Unix `terminal_width_from_fd` 仅以 `ioctl` 返回码判断成功；若调用成功但列数为零，会返回零而不是 80。`Frame::Print` 对宽度小于等于零直接原样输出。`BufferConsoleGlue` 和最终消息使用的 Mutex 均以 `expect` 处理中毒，锁持有者 panic 后的后续访问也会 panic。

## 并发与资源生命周期

`ConsoleGlue` 要求 `Send + Sync`，`ConsoleOperations` 用 `Arc` 共享实现，因而可跨线程克隆。`BufferConsoleGlue` 每次 `In/Out` 返回新的包装器，但所有包装器锁住同一输入游标或输出 Vec；单次 `read/write` 被串行化，跨多次调用的复合操作并不原子。例如 `Print` 对多个参数逐次写入，两个线程并发打印时仍可能在参数边界交错。

`SharedWriter::flush` 是空操作，缓冲数据在最后一个 `Arc` 释放时销毁；`output_string` 只做快照式克隆，不清空缓冲。`StdIOGlue` 每次返回标准流句柄包装器，文件描述符由标准库/进程管理，本文件不主动关闭 stdin/stdout。

`ShowTask` 返回 `FnOnce`，类型系统防止同一完成闭包被重复调用。实际进度对象的完成和等待生命周期由 `progressing.rs` 管理：该闭包按顺序推进一次并关闭。`WithTimeCost` 的闭包是 `Send` 但不是 `Sync`，通过 `FnMut` 保证缓存更新需要独占调用；进度模块把最终消息闭包放在 Mutex 中串行求值。

全局 `NoColor` 使用 relaxed 原子访问，只保证开关读写无数据竞争，不提供与其他状态的顺序关系；这对独立的展示配置足够。终端宽度、TTY 状态和输入输出端口均在调用时查询，没有长期缓存。

## 与 Go 版本的对应关系

主要结构逐项对应 `br/pkg/glue/console_glue.go`：`ConsoleOperations`、三类 ExtraField 构造器、`ShowTask`、`PrintList`、提示与扫描、`Table`、`ConsoleGlue`/NoOP/StdIO、`PrettyString` 和 `Frame` 均保留了 Go 名称和总体分支。`console_glue_test.rs` 复刻 Go `console_glue_test.go` 的彩色文本长度、切片一致性和 Frame 折行金标准；额外 Rust 测试补充了 `Scanln` 单 token、额外 token、毫秒取整和 ANSI 正则语法。

Rust 用 `Glue::AsConsoleGlue` 替代 Go 的运行时类型断言；用显式 `in_is_terminal/out_is_terminal/terminal_width` trait 方法替代对 `*os.File` 和 `x/term` 的检查；用 `Result<Box<dyn Read/Write + Send>>` 表达获取端口失败，而 Go 接口直接返回 reader/writer。Go 的 `fmt.Fscanln` 被逐字节扫描逻辑模拟。

着色方面，Rust 内置最小 SGR 实现而非依赖 `fatih/color`。Go 的 `color.NoColor` 是库全局布尔值，Rust 对应为 `AtomicBool`。终端宽度方面，Go 调用 `term.GetSize`；Rust 在 Unix 直接调用 `ioctl`，非 Unix 固定回退 80。

存在应谨慎对待的实现差异：Rust `PrintList<T>` 要求 `Display + Debug`，并以 stderr 代替 Go 结构化日志；Rust 打印 API接收字符串切片/已格式化文本，不支持 Go `...any` 的通用格式化；Rust 的 UTF-8 字符串切片会在非字符边界 panic，而 Go 字符串切片是任意字节切片。当前证据表明 ASCII/ANSI 主路径与 Go 测试一致，不能据此宣称完整 Unicode 终端布局等价。

## 扩展指南

新增控制台后端时实现 `ConsoleGlue`，至少提供 `In/Out`；若希望启用交互提示、真实进度或正确布局，还应覆盖 `in_is_terminal`、`out_is_terminal`、`terminal_width`。在具体 `Glue` 实现中通过 `AsConsoleGlue` 返回 `Arc`，否则 `GetConsole` 会静默退化为 NoOP。不要让 SQL/嵌入式路径意外继承真实 stdin，从而在无人值守执行中阻塞。

新增渲染功能优先接入 `ConsoleOperations`；新增进度行为则应修改 `progressing.rs`，不要在本文件复制状态机。若调整完成行字段，需同时检查 `ExtraField`、`printFinalMessage`、`StartProgressBar` 的普通条与 `OnlyOneTask` 两条分支，明确附加字段是否应该在 spinner 模式出现。

修改 ANSI 或换行逻辑时应扩展独立测试 `br/pkg/glue/console_glue_test.rs`，并同步核对 Go 测试 `console_glue_test.go`。至少覆盖转义区间重定位、切点恰在转义前后、非法序列、宽度不足退化和多段颜色跨行。若要支持 Unicode，应先明确“字节数、Unicode 标量数、grapheme 数、终端列宽”中的目标语义，再避免直接用任意字节下标切 Rust `str`。

修改交互/错误策略时扩展 `Scanln`、`PromptBool` 和失败 I/O 替身测试；尤其要决定非 TTY 默认 true 是否仍满足自动化安全要求，以及输出错误是否继续被吞掉。修改共享缓冲时需测试并发追加顺序和锁中毒策略。所有 Rust 测试仍应保持在独立 `*_test.rs` 文件中，不应内嵌回生产源文件。

兼容性风险主要是 CLI 文案和默认确认语义；正确性风险集中在 ANSI/UTF-8 下标、窄终端和 I/O 错误；性能风险较低，但当前 `PrettyString::SplitAt` 会克隆字符串和区间，长文本反复折行可能产生二次方级复制，优化时必须保持颜色边界与 Go 输出兼容。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/glue` 确认目标 Rust/Go、独立测试、模块入口和进度文件均已索引。
- RustCodeGraph：`node --file br/pkg/glue/console_glue.rs` 分段读取 1–928 行，核对全部常量、类型、函数、trait、impl、Unix 条件编译和注释；文件关系报告目标被 15 个文件使用。
- RustCodeGraph：`query ConsoleGlue`、`query GetConsole`、`query StartProgressBar` 确认 Rust/Go 同名入口及 `StartProgressBar` 位于 `progressing.rs:394`。精确 `callers/callees` 查询未产生可消费输出，因此未把缺失的符号级边作为事实。
- 已读 Rust 边界与接线：`br/pkg/glue/lib.rs`、`br/pkg/glue/glue.rs`、`br/pkg/glue/progressing.rs`、`br/pkg/gluetikv/glue.rs`。
- 已读 crate 声明：`br/pkg/glue/Cargo.toml`，确认包名、库入口、Go 包映射和依赖边界。
- 已读 Go 对照：`br/pkg/glue/console_glue.go`；已读 Go 测试：`br/pkg/glue/console_glue_test.go`。
- 已读 Rust 独立测试：`br/pkg/glue/console_glue_test.rs`、`br/pkg/glue/parity_test.rs`。后者覆盖列表截断、非 TTY 默认确认、GetConsole NoOP 回退、ShowTask 生命周期和公开常量。
- 用 `rg` 补查图命令未给出的直接 Rust 调用：当前 `GetConsole`、`ShowTask`、`PrintList` 的明确调用分别位于 `br/pkg/glue/parity_test.rs:272`、`:279`、`:222`；`AsConsoleGlue` 的生产覆盖位于 `br/pkg/gluetikv/glue.rs:306`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰含 11 个固定二级标题，并人工复核未修改 Rust、Go、Cargo 或 `plan.md`。
