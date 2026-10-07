# [`br/pkg/glue/lib.rs`](lib.rs)

## 文件定位

`br/pkg/glue/lib.rs` 是 Cargo 包 `astersql-br-pkg-glue` 的 crate 根。`br/pkg/glue/Cargo.toml` 以 `[lib] path = "lib.rs"` 指定该入口，并以 `package.metadata.porting.go-package = "br/pkg/glue"` 标明它覆盖 Go 的 `br/pkg/glue` 包。它本身不实现终端排版、TiDB 会话或进度算法，而是把这些能力组织成一个可依赖的公共门面。

文件通过 `#[path]` 声明三个生产模块：`console_glue.rs`、`glue.rs`、`progressing.rs`；通过 `#[cfg(test)]` 挂载独立的 `parity_test.rs` 与 `console_glue_test.rs`（`lib.rs:16-31`）。crate 级 `allow` 保留 Go 移植接口的命名与阶段性未使用符号，测试逻辑没有放入生产文件。

## 核心职责

1. 确立 BR glue crate 的编译边界：控制台适配、BR 对 TiDB/KV 的抽象接口和进度展示均由同一 crate 提供，但实现仍分文件维护。
2. 形成 crate 根 API：`console_glue::*` 与 `glue::*` 全量重导出；`progressing` 只重导出 `LogBar`、`MultiProgress`、`NopMultiProgress`、`OnlyOneTask`、`ProgressBar`、`ProgressWaiter`、`TerminalBar`、`TerminalMultiProgress`（`lib.rs:33-40`）。内部 `BarState`、`ProgressGroup`、`PbProgress` 等状态机不会从根暴露。
3. 为具体实现 crate 提供稳定契约。`br/pkg/gluetidb`、`br/pkg/gluetikv`、`br/pkg/conn` 与 `br/pkg/gluetidb/mock` 的 Cargo manifest 均直接依赖本 crate；它们从根导入 `Glue`、`Session`、`Storage`、`Context`、`SecurityOption`、`CIStr` 等类型。
4. 保持 Go/Rust 公共行为可校验：`parity_test.rs` 汇总 glue、console 与 progressing 的契约测试，`console_glue_test.rs` 独立覆盖 ANSI、输入扫描和 Frame 排版。

## 主要符号

`lib.rs` 自身没有函数、类型或常量定义，主要符号是模块声明与重导出集合：

- `pub mod console_glue`：公开 `ConsoleGlue`、`ConsoleOperations`、`StdIOGlue`、`NoOPConsoleGlue`、`BufferConsoleGlue`、`PrettyString`、`Frame`、`Table`、`ExtraField`、颜色辅助函数和终端操作。调用方既可写 `astersql_br_pkg_glue::ConsoleOperations`，也可沿 `console_glue` 模块路径访问公开符号。
- `pub mod glue`：公开 `Glue`、`Session`、`BatchCreateTableSession`、`Progress` trait，`WithProgress` 生命周期包装器，以及 `Context`、`Storage`、`Domain`、元数据占位类型和 `ClientCLP`/`ClientSql`。
- `pub mod progressing`：公开进度实现模块。crate 根有意只重导出稳定契约与实现类型；`ConsoleOperations::StartProgressBar`、`StartMultiProgress` 由该模块的 `impl` 提供，但调用方通常经根级 `ConsoleOperations` 使用。
- `parity_test`、`console_glue_test`：仅在 `cfg(test)` 下可见的私有模块。`parity_test.rs::crate_root_exports_go_public_progress_types` 明确验证 `TerminalBar` 与 `TerminalMultiProgress` 可以从 crate 根引用。

需要区分两个名为 `Progress` 的层次：`glue::Progress` 是 BR 单任务进度契约；`progressing::ProgressBar`/`MultiProgress` 是多条进度展示契约。根门面同时公开它们，但不会自动互相转换。

## 执行流程

`lib.rs` 没有运行时入口；它在编译期组织以下典型调用链：

1. 具体 glue 实现（例如 `br/pkg/gluetikv/glue.rs` 或 `br/pkg/gluetidb/glue.rs`）从 crate 根实现 `Glue`、`Session` 或 `Storage`，把 BR 任务与存储、Domain、SQL/DDL 会话隔离开。
2. BR 调用方持有 `&dyn Glue`，按需调用 `GetDomain`、`CreateSession`、`Open`、`UseOneShotSession` 或 `StartProgress`。`br/pkg/conn/conn.rs` 例如从根导入 `Domain`、`Glue`、`SecurityOption`、`Storage` 来管理连接边界。
3. 需要单任务生命周期保护时，调用 `WithProgress`。它先执行 `Glue::StartProgress`，再以局部 `Drop` guard 保证回调成功、返回错误或 panic unwind 时都调用 `Progress::Close`（`glue.rs:260-282`）。
4. 需要控制台能力时，`GetConsole` 优先采用 `Glue::AsConsoleGlue`；没有控制台实现则退化到 `NoOPConsoleGlue`。`ConsoleOperations` 在此基础上完成提示、表格、Frame 折行和任务展示。
5. `StartProgressBar`/`StartMultiProgress` 根据输出是否是 TTY 分流：TTY 使用共享 `BarState`/`ProgressGroup` 渲染，非 TTY 使用 `DummyProgress` 或 `LogBar` 输出日志式完成信息。`OnlyOneTask == -1` 选择 spinner 风格，`total == 0` 立即完成（`progressing.rs`）。

## 数据与状态

crate 根没有自己的全局状态，也不创建线程或持有 I/O。实际状态归属如下：

- `glue::Context` 用 `Arc<AtomicBool>` 在克隆之间共享取消标志；`Glue`、`Progress`、`Storage`、`ConsoleGlue` 等 trait 带有适合跨线程使用的 `Send`/`Sync` 约束。
- `ConsoleOperations` 持有 `Arc<dyn ConsoleGlue>`。`BufferConsoleGlue` 用 `Arc<Mutex<...>>` 共享输入游标和输出缓冲；`StdIOGlue` 面向进程 stdin/stdout，`NoOPConsoleGlue` 丢弃输出并提供空输入。
- `PrettyString` 同时保存带 ANSI 的 `pretty`、去转义的 `raw` 和转义区间，使 `Frame` 能按可见宽度切分而不把控制码计入宽度。
- 进度实现以原子计数记录 current/total/completed/aborted/rendered，以 `Mutex` 保护最终消息与条集合。`TerminalMultiProgress` 共享一个 `ProgressGroup`；`LogBar` 以原子递减剩余量。
- `glue.rs` 当前用 crate 内的 `Storage`、`Domain`、`DBInfo`、`TableInfo` 等轻量类型代替完整 TiDB 类型。其模块注释明确这是避开 arm64 grpcio 重建路径的迁移期边界，不应把这些占位类型描述成完整生产 TiDB 实现。

## 依赖与调用关系

本 crate 的直接 Cargo 依赖只有 `astersql-errors`，用于 `Glue`/`Session`/进度等待的 `SharedError`。终端、同步和线程行为由标准库实现；当前 Cargo manifest 没有直接引入 Go 版使用的 `fatih/color`、`mpb`、`term`、TiDB domain/kv/sessionctx 或 PD client。

直接 Cargo 上游包括：

- `br/pkg/gluetidb/Cargo.toml`：实现 TiDB glue 与 Session，并从根导入 `CIStr`、`Context`、`Domain`、`Glue`、`Session`、`Storage` 等。
- `br/pkg/gluetikv/Cargo.toml`：实现 TiKV/CLI glue，从根导入 `ClientCLP`、`ConsoleGlue`、`Context`、`Glue`、`Progress`、`SecurityOption`、`Storage` 等。
- `br/pkg/conn/Cargo.toml`：连接管理通过根级 `Glue`、`Storage`、`SecurityOption` 和 `Domain` 建立窄接口。
- `br/pkg/gluetidb/mock/Cargo.toml`：测试/移植 Mock 实现根级 `Session` 等契约。

RustCodeGraph 显示三个生产实现文件被多处直接使用：`glue.rs` 的使用方包含 `br/pkg/conn/conn.rs`、`br/pkg/restore/snap_client/client.rs`、`br/pkg/task/common.rs`、`restore.rs`、`stream.rs`；`console_glue.rs` 被 BR 命令、glue 实现及测试使用；`progressing.rs` 被 restore split 等路径使用。crate 根自身主要产生模块与重导出边，因此普通函数 callers/callees 不足以表达其价值，Cargo 依赖和根级导入才是直接证据。

## 错误处理与边界

- `lib.rs` 不捕获或转换错误。`glue.rs` 的 fallible 接口统一返回 `SharedError`；`WithProgress` 原样返回回调错误，同时用 RAII 保证 `Close`。
- `ConsoleGlue::In`/`Out` 返回 `io::Result`，控制台操作中的写入多数采用尽力而为策略；格式化输出失败不会在 crate 根升级为业务错误。扩展调用方不能假定所有控制台写入均可观察到错误。
- 非交互 `PromptBool` 返回 `true`，避免无人值守任务阻塞；空输入/EOF 或空回答在交互路径返回 `false`。终端宽度探测失败回退为 80。
- `PrettyString::SplitAt` 以 raw 字节位置切分，负数会 panic，超过可见长度则返回全文与空右半；它不是 Unicode 字符宽度算法，新增多字节/宽字符支持不能只修改根导出。
- `StartProgressBar` 对非 TTY 忽略 `ExtraField` 并使用日志式进度；`ProgressWaiter::Wait` 的 TTY 路径可因 `Context` 取消返回 `context canceled`。`Close` 和组 `Wait` 设计为幂等，但调用方仍应完成或终止所有条，避免等待条件永远不满足。
- 根级通配重导出会把新增的 `console_glue`/`glue` 公开符号自动加入公共 API；而 `progressing` 新符号默认不会根级可见，必须显式更新列表。这一不对称是扩展时的重要兼容边界。

## 并发与资源生命周期

`lib.rs` 只在编译期接线，不拥有运行时资源。其公开契约中，`Glue: Send + Sync`、`Progress: Send + Sync`、`Storage: Send + Sync` 允许实现被多个 BR 工作线程共享；`Session: Send` 则保留单所有者可移动语义。

`WithProgress` 的局部 guard 把进度关闭与回调作用域绑定，是单任务最明确的资源边界。TTY 进度的 `Wait` 会启动一个等待线程，并在调用线程轮询取消或完成；`ProgressGroup::Wait` 每 10ms 检查所有 bar 是否完成/中止。`TerminalMultiProgress::Wait` 等待整组，`TerminalBar::Done` 只终止当前条。非 TTY 的 `NopMultiProgress::Wait` 是空操作，完成观测依赖每个 `LogBar::Increment` 把剩余量减至零。

控制台共享写入由 `Arc<Mutex<dyn Write + Send>>` 串行化；缓冲输入/输出也由 Mutex 保护。原子状态主要使用 Relaxed，幂等关闭/渲染切换使用 SeqCst。新增并发实现必须维持“最终文案最多渲染一次”和“错误路径仍关闭进度”的不变量。

## 与 Go 版本的对应关系

Go 没有与 `lib.rs` 一一对应的 crate 根文件；Rust 门面覆盖整个 Go `br/pkg/glue` 包：

- `glue.rs` 对应 `glue.go`：`Glue`、`Session`、`BatchCreateTableSession`、`Progress` 和 `WithProgress` 的职责一致。Rust 用 trait object、`Result<_, SharedError>` 和 Drop guard 对应 Go interface、error 与 `defer p.Close()`。
- `console_glue.rs` 对应 `console_glue.go`：控制台抽象、NoOP/stdio 实现、交互提示、表格、ANSI 字符串与 Frame 排版语义相同；Rust 额外提供 `BufferConsoleGlue` 作为可复用测试/非 TTY 适配器。
- `progressing.rs` 对应 `progressing.go`：TTY/非 TTY 分流、单任务哨兵、可等待进度和多进度接口对齐。Rust 当前以本地 `BarState`/`ProgressGroup` 模拟 Go `mpb`，并非同一渲染库。

重要迁移差异是 Rust `glue.rs` 明确采用本地 stand-in 类型，`Context` 也只有取消位、不承载 Go `context.Context` 的 deadline/value；`progressing.rs` 的终端模拟只保留公开行为，不等价于 `mpb` 的完整刷新机制。相关 Cargo 注释同样说明真实 TiKV/Domain/grpc 接线仍受当前瘦身边界约束。

Go `console_glue_test.go` 的彩色字符串、切分与 Frame 金标准已移植到独立 `console_glue_test.rs`。Rust `parity_test.rs::go_rust_public_contract_matches` 进一步覆盖客户端常量、非 TTY Prompt、`WithProgress` 成功/错误关闭、列表截断、ShowTask 和多进度；后续测试还覆盖零总量、取消等待及完成文案只渲染一次。

## 扩展指南

- 新增生产模块时，应在本文件增加明确的 `#[path] pub mod`；新增测试继续放入独立 `*_test.rs` 并用 `#[cfg(test)]` 私有挂载，不能把测试写进生产源。
- 新增 `console_glue` 或 `glue` 的公开符号会被 `pub use ...::*` 自动提升到根级 API，应检查名字冲突和兼容面；新增 `progressing` 公共类型若希望调用方从根导入，则必须同步修改显式重导出列表，并扩展 `crate_root_exports_go_public_progress_types`。
- 扩展 `Glue`/`Session` 时必须同步具体实现 `gluetidb`、`gluetikv`、mock、conn 调用点和 `parity_test.rs`，并对照 Go `glue.go`，不能以默认空实现缩减接口语义。
- 修改终端排版、ANSI 解析或输入扫描时，同步维护 `console_glue_test.rs` 与 Go `console_glue_test.go` 的意图；尤其注意 raw 字节位置、TTY 检测失败、默认宽度和非交互 Prompt 边界。
- 修改进度完成、取消或等待策略时，应在 `parity_test.rs` 覆盖成功、错误、零总量、非 TTY、TTY、多条进度和重复 Close/Wait；性能风险集中在 10ms 轮询、锁竞争和频繁终端写入，兼容风险集中在完成文案与 Close/Done 语义。
- 若未来用真实 TiDB/PD/mpb 类型替换 stand-in，应在上游 crate 独立完成依赖接线，并验证 arm64/grpc 构建边界；不能仅修改 `lib.rs` 的重导出来宣称迁移完成。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/glue` 列出三个生产 Rust 文件、crate 根、两个 Rust 测试及三个 Go 对照文件。
- RustCodeGraph：`node --file br/pkg/glue/lib.rs --offset 1 --limit 200` 核对 40 行 crate 根、三个生产模块、两个 `cfg(test)` 模块和选择性重导出；`query ConsoleGlue`、`query Glue --kind trait`、`query Progress --kind trait` 消除同名 Go/Rust/桩符号歧义。
- RustCodeGraph：读取 `glue.rs`、`console_glue.rs`、`progressing.rs` 的索引源码，核对 trait、TTY 分流、RAII、原子/锁与等待线程；`node crate_root_exports_go_public_progress_types` 验证根级 `TerminalBar`/`TerminalMultiProgress` 导出契约。
- Cargo/调用方：读取 `br/pkg/glue/Cargo.toml` 以及 `br/pkg/gluetidb/Cargo.toml`、`br/pkg/gluetikv/Cargo.toml`、`br/pkg/conn/Cargo.toml`、`br/pkg/gluetidb/mock/Cargo.toml`；用 `rg` 核对这些 crate 对 `astersql_br_pkg_glue` 根级符号的真实导入。
- Go 对照：读取 `br/pkg/glue/glue.go`、`console_glue.go`、`progressing.go`，核对接口、控制台和进度职责；读取 `console_glue_test.go` 核对彩色字符串与 Frame 测试意图。
- 独立 Rust 测试：读取 `br/pkg/glue/parity_test.rs` 和 `console_glue_test.rs`；确认它们由 `lib.rs` 在 `cfg(test)` 下挂载，覆盖公共契约、错误路径关闭、TTY/非 TTY、ANSI/输入/排版和根级导出。
- 本任务只新增说明文档，按计划不运行 Cargo；结构验证命令及退出结果在交付证据中报告。
