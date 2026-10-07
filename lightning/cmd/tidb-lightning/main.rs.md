# `lightning/cmd/tidb-lightning/main.rs`

## 文件定位

本文件是 Cargo package `astersql-lightning-cmd-tidb-lightning` 的实际进程控制模块，对照 Go 的 `lightning/cmd/tidb-lightning/main.go`。`Cargo.toml` 把 `lib.rs` 设为库入口、把 `bin_main.rs` 设为同 package 的二进制入口；运行时的直接链路是 `bin_main.rs::main -> lib.rs::main -> entry::main`，其中 `lib.rs` 通过 `#[path = "main.rs"] pub mod entry` 将本文件装入 crate。

它处在 Lightning CLI 的最外层编排边界：接收进程参数，构造 `server::Lightning`，启动状态服务，选择常驻 server mode 或单次导入，统一取消、日志和退出码。真正的数据导入、checkpoint、调度和 HTTP 服务实现位于 `astersql-lightning-pkg-server` 及其下游；本文件只调用这些能力，不实现导入算法。

`Cargo.toml` 的 `package.metadata.porting` 将整个 crate 标记为 Go package `lightning/cmd/tidb-lightning` 的 `binary` 移植单元。其直接 path 依赖只有 `astersql-lightning-pkg-progress` 和 `astersql-lightning-pkg-server`；配置、日志、信号、内存钩子和退出函数则由同 crate 的 `stubs.rs` 提供迁移期适配。

## 核心职责

1. `main()` 在任何参数处理前调用 FIPS 初始化锚点，收集去掉程序名后的参数，并把非零返回码交给可替换的 `stubs::exit`。
2. `run()` 将生产环境的 `server::Lightning` 构造封装成工厂，进入可测试的 `run_with_factory()`。
3. `run_with_factory()` 解析并校验全局配置，识别帮助/版本和非法参数触发的早退出语义，并在构造应用失败时返回 1。
4. 初始化非致命内存观测钩子；生产模式下启动信号线程，把 `SIGHUP`、`SIGINT`、`SIGTERM` 或 `SIGQUIT` 转为一次 `Stop()` 调用。
5. 对非 local backend 且未显式设置 `GOGC` 的场景应用 500 的默认 GC 百分比，然后先启动 HTTP 状态服务，再选择 `RunServer()` 或 `RunOnceWithOptions()`。
6. 把 context canceled 与 `TaskCanceled()` 联合解释为正常完成或用户取消，输出最终日志/终端消息，按条件同步文件日志，并生成与 Go 入口一致的退出码。

本文件不定义命令行 flag、配置结构或服务业务逻辑，也不拥有真实 TLS/FIPS 实现；这些边界分别位于 `stubs.rs`、server crate 和 `fips.rs`。

## 主要符号

- `pub trait LightningApp: Send`（`main.rs:36-46`）：入口所需的最小服务接口，包含 `GoServe`、`Stop`、`RunServer`、`RunOnceWithOptions` 和 `TaskCanceled`。`Send` 是信号线程可持有共享应用的必要约束。方法名保持 Go 风格以便逐项对照。
- `struct RealLightning(Box<server::Lightning>)`（`main.rs:50`）：真实 server 对象的私有适配器。它只转发 trait 方法；其中 `RunOnceWithOptions` 给 server 的第三个参数固定传空 `Vec`。
- `impl LightningApp for RealLightning`（`main.rs:52-72`）：五个转发方法，不增加重试、错误转换或清理逻辑。
- `pub fn main()`（`main.rs:77-83`）：本模块的进程入口。顺序为 FIPS 钩子、`run(std::env::args().skip(1))`、非零时 `stubs::exit(code)`。
- `pub fn run(args: Vec<String>) -> i32`（`main.rs:88-93`）：生产可调用入口。它调用 `run_with_factory(args, true, ...)`，将 `config::GlobalConfig` 转为 server 侧全局配置并交给 `server::New`。
- `pub fn run_with_factory<F, A>(...) -> i32`（`main.rs:100-255`）：主要控制流。泛型工厂 `F: FnOnce(config::GlobalConfig) -> Result<A>` 只调用一次；应用类型 `A: LightningApp + 'static`，随后放入 `Arc<Mutex<A>>` 供主线程与信号线程共享。

本文件没有模块级常量、静态变量、枚举、类型别名、宏、`unsafe` 或条件编译项。测试注入点是公开 trait、公开 `run_with_factory` 和布尔参数 `install_signals`，而非内嵌测试模块。

## 执行流程

生产执行可分为以下阶段：

1. `bin_main.rs::main()` 调用库 crate 的 `main()`，`lib.rs::main()` 再转发到本文件的 `entry::main()`。
2. `main()` 调用 `fips::init_fips_only_tls_for_boringcrypto_build()`；当前 `fips.rs` 中该函数为空，只对应非 boringcrypto Go 构建的无副作用路径。随后收集 `argv[1..]` 并调用 `run()`。
3. `run()` 启用真实信号处理并提供生产工厂。工厂通过 `config::to_server_global` 转换配置，然后调用 `server::New`，把结果包装为 `RealLightning`。
4. `run_with_factory()` 调用 `config::LoadGlobalConfig(&args, None)` 和 `config::Must`。由于测试桩允许 `Must` 记录而不是终止进程，函数立刻检查 `stubs::take_exit_code()`；帮助/版本返回 0，非法配置通常返回 2，并在此提前结束。
5. 根据 `globalCfg.App.File()` 判定是否使用文件日志；空串和 `"-"` 均不视为文件。若是文件，先在 stdout 告知实际配置路径。随后只调用一次应用工厂；构造错误打印 `failed to create lightning` 并返回 1。
6. 将应用装入 `Arc<Mutex<_>>`。`memory::InitMemoryHook()` 失败仅记错误日志。若 `install_signals` 为真，则派生线程阻塞等待四种退出信号；收到后记录信号，并在成功获取锁时调用 `Stop()`。
7. 当 backend 不是 `BackendLocal` 且 `debug::gogc_env()` 为空时调用 `debug::SetGCPercent(500)`。显式 `GOGC` 和 local backend 都保持原值。
8. 主线程先加锁调用 `GoServe()`。若失败，记录日志、写 stderr，然后直接返回 0；这是对 Go `return` 而非 `exit(1)` 的刻意保留。若配置了 `StatusAddr`，再调用 `progress::EnableCurrentProgress()`。
9. `ServerMode=true` 时加锁调用 `RunServer()`；否则建立 `server_config::Config::NewConfig()`，用全局配置填充，成功后以 `context::Background()` 调用 `RunOnceWithOptions()`。配置转换失败作为主任务错误继续进入统一收尾。
10. 若结果满足 `common::IsContextCanceledError`，先把错误清为成功；只有 `TaskCanceled()` 为真时才把 `finished` 设为 false。之后错误路径写错误日志和 stderr，成功/取消路径分别输出 `tidb lightning exit successfully` 或 `tidb lightning canceled`。
11. 文件日志场景调用 `stubs::logger_sync()`；同步失败只写 stderr，不改变主结果。最终，普通错误或“server mode 且未完成”返回并记录退出码 1，其余返回 0；外层 `main()` 对非零码再次执行真正的退出动作。

## 数据与状态

主要输入是 `Vec<String>` 形式的命令行参数。`config::LoadGlobalConfig` 生成 `config::GlobalConfig`，其在本文件中被读取的字段包括 `App.File()`、`App.Config.File`、`App.StatusAddr`、`App.ServerMode` 和 `TikvImporter.Backend`。构造应用时该配置被 clone 一次，单次导入时又经 `config::to_server_global` 转成 server 配置输入。

核心运行状态包括：

- `log_to_file: bool`：同时控制启动 banner 和结束时日志同步；`"-"` 明确代表非文件输出。
- `Arc<Mutex<A>> app`：应用的唯一共享所有权容器。主线程的服务调用和信号线程的 `Stop()` 都通过同一互斥锁串行化。
- `err: Result<()>`：承载 server mode、配置转换或 run-once 的最终结果；context canceled 可能被原地改写为 `Ok(())`。
- `finished: bool`：默认为 true，仅在 context canceled 且 `TaskCanceled()` 为真时变为 false。它与 `ServerMode` 一起参与最终退出码判定。
- `stubs` 中的退出码、exit hook、logger sync 结果及信号注入状态：这些是 crate 级测试适配状态，不由本文件定义，但 `run_with_factory` 会消费它们。

本文件不持久化数据、不持有数据库事务，也不直接管理 checkpoint。对 `globalCfg` 的使用在函数返回时结束；`server_config::Config` 只在一次性任务分支构造并移动给应用。

## 依赖与调用关系

直接上游调用关系为：

- `lightning/cmd/tidb-lightning/bin_main.rs::main -> astersql_lightning_cmd_tidb_lightning::main`；
- `lightning/cmd/tidb-lightning/lib.rs::main -> entry::main`；
- `entry::main -> run -> run_with_factory`；
- 测试侧 `parity_test.rs` 的正常、边界、错误和资源清理场景直接调用 `run_with_factory`，`main_test.rs` 的隔离 worker 调用 `entry::run`。

主要下游关系为：

- `fips.rs`：提供启动最前端的 FIPS 初始化锚点；当前为空实现。
- `stubs.rs::config`：提供配置装载、`Must`、backend 常量及到 server 配置的转换。
- `astersql-lightning-pkg-server`：提供 `Lightning`、`New`、运行配置、context、日志、错误类型，以及入口调用的五项服务能力。
- `stubs.rs::{memory, debug, os_signal}`：分别提供内存钩子、GC 环境/设置和信号等待适配。
- `astersql-lightning-pkg-progress::EnableCurrentProgress`：仅在状态地址非空时启用当前进度。
- `server::common::IsContextCanceledError`：决定某个运行错误是否进入取消归一化分支。

RustCodeGraph 的精确查询把本文件的 `run_with_factory` 定位在第 100 行，并给出五个直接调用者：`run` 及 `parity_test.rs` 中四组入口契约场景；图还显示本文件中 `GoServe`、`Stop`、`RunServer`、`RunOnceWithOptions`、`TaskCanceled` 的调用均汇入 `run_with_factory`。由于这些方法名在仓库中高度重复，调用关系只采用带目标路径的结果和源码交叉核验，不采用无路径约束的同名结果。

## 错误处理与边界

- 配置解析不是普通 `Result` 返回边界：`config::Must` 模拟 Go 的进程退出。为支持测试，`run_with_factory` 必须紧接着读取已记录退出码并返回，不能继续构造应用。
- 应用工厂失败返回 1；内存钩子失败只记录日志；两者严重性不同，不应合并。
- `GoServe` 失败返回 0，虽然看似反常，但 `main.go:81-86` 明确只打印后 `return`。修改此行为会改变脚本可见的兼容契约。
- server/run-once 的一般错误返回 1。`IsContextCanceledError` 为真时错误被清除；`TaskCanceled=false` 表示正常收尾，`TaskCanceled=true` 表示未完成的取消。
- 非 server 的取消最终返回 0；server mode 的取消最终返回 1。`parity_test.rs` 已固定非 server 取消返回 0，源码最终条件固定了 server mode 差异。
- 文件日志同步失败不覆盖主任务结果。stdout/stderr logger 不调用同步，以规避 Go 注释所述的同步错误。
- 信号线程使用 `if let Ok(mut g) = signal_app.lock()`；若互斥锁中毒，`Stop()` 会被静默跳过。相反，主线程多处使用 `lock().unwrap()`，锁中毒会 panic。这是当前实现事实，不应描述为可靠兜底。
- `RealLightning::RunOnceWithOptions` 固定传空附加选项向量；需要新增选项时必须同时核对 server 方法契约和 Go 对照，不能只改 trait 签名。

## 并发与资源生命周期

本文件只创建一个后台 OS 线程：当 `install_signals=true` 时，它阻塞在 `wait_for_one_of`，收到首个指定信号后尝试获取应用锁、调用一次 `Stop()`，随后线程结束。它没有 join handle、超时或重复信号循环；`run_with_factory` 返回时也不会主动等待该线程。测试通过 `install_signals=false` 避免无关后台线程，资源清理场景则注入 `SIGINT` 并确认 `Stop()` 被异步调用；另有 Unix 子进程测试验证真实 `SIGTERM` 等待。

`Arc` 让主线程和信号线程共享应用所有权，`Mutex` 保证 `GoServe`、`RunServer`/`RunOnceWithOptions` 与 `Stop` 不会同时以可变引用访问应用。代价是长时间运行方法持锁期间，信号线程只能等待锁：本文件假定下游运行方法能够返回或内部配合取消；这里本身没有无锁取消通道。

主线程每次调用前临时取锁并在表达式结束时释放。文件日志的生命周期从配置判定延续至主任务收尾，只有文件目标才显式 sync。`context::Background()` 没有父取消上下文；停止语义通过 `Lightning::Stop()` 和下游状态传播。应用、配置和 logger 引用在函数返回后按 Rust 所有权释放，但本文件没有显式调用应用级 `Close` 或等待信号线程。

## 与 Go 版本的对应关系

`main.rs` 基本逐段对应 `main.go`：配置装载、日志文件提示、`server.New`、非致命内存钩子、四种信号与 `Stop`、非 local 的 GOGC=500、`GoServe`、进度启用、server/run-once 分派、context canceled 归一化、最终消息、文件日志同步和退出条件都保留了相同顺序。

Rust 为可测试性增加了三层结构：`LightningApp` 抽象真实服务；`run_with_factory` 注入工厂和信号开关；`run`/`main` 分离返回码与进程退出。Go 则直接使用具体 `server.Lightning`、goroutine 和包级可替换变量 `exit = os.Exit`。Rust 的 `Arc<Mutex<_>>` 是 Go 指针跨 goroutine 共享的同步替代，但也带来锁中毒和持锁等待边界。

Rust 还显式调用 `fips::init_fips_only_tls_for_boringcrypto_build()`；Go 的 `fips.go` 只在 `boringcrypto` build tag 下通过空白导入触发副作用。当前 Rust 函数为空，因此只对齐非 boringcrypto 行为并保留未来接线点，不能声称 Rust 已支持 FIPS TLS。

测试对应关系如下：

- `main_test.go::TestRunMain` 过滤 `DEVEL` 和 `-test.*`，替换 exit，并在 goroutine 中等待 `main()` 返回；`main_test.rs::test_run_main` 用子进程、线程和 channel 近似同一入口契约。
- `parity_test.rs::contract_normal_run_once_success` 验证 `GoServe` 先发生且 run-once 恰好一次。
- `contract_boundary_gogc_and_log_file` 固定 local/non-local GC 行为、无状态地址的 server mode 配置错误为 2，以及版本参数早退出为 0。
- `contract_error_paths` 固定内存钩子非致命、HTTP 服务失败返回 0、导入失败返回 1。
- `contract_resource_cleanup_sync_and_cancel` 固定日志同步失败不改变结果、非 server 取消返回 0、状态地址启用进度及信号触发 `Stop()`。

## 扩展指南

- 新增启动阶段时，应先确定它在 Go 入口中的对应位置，再放入 `run_with_factory` 的同一阶段；尤其不要把可能失败的副作用放在配置早退出之前，或把 HTTP 服务移到任务启动之后。
- 扩展 Lightning 入口能力时，在 `LightningApp`、`RealLightning` 和 `parity_test.rs::MockApp` 三处同步修改，并在独立测试文件添加正常、失败及资源清理场景。不要把 Rust 测试内嵌到生产 `main.rs`。
- 改变参数、默认值或配置转换时，同步核对 `stubs.rs::config`、`Cargo.toml` 的真实依赖边界、Go `main.go`/配置实现和 `parity_test.rs` 的 flag/merge 契约。
- 接入真正的 FIPS TLS 时应修改 `fips.rs` 及 Cargo feature/dependency，并保持 `main()` 中初始化先于配置和网络构造；同时添加独立测试证明启用与未启用 feature 的差异。
- 改变信号或取消模型时，应评估 `Arc<Mutex<_>>` 的持锁时长、锁中毒、线程无法 join 和重复信号行为；至少同步真实信号子进程测试及注入信号的资源清理测试。
- 改变退出码或错误严重性前，必须保留已由 Go 和 parity 测试固定的特殊边界：`GoServe` 失败为 0、非 server 用户取消为 0、server mode 用户取消为 1、配置 `Must` 错误为 2、日志 sync 失败不覆盖主结果。
- 性能敏感的改动应特别关注 GOGC 分支和锁范围。不要在持有应用锁时加入与应用无关的慢 I/O，也不要把 local backend 误纳入 500 的默认 GC 百分比。

## 验证依据

本说明使用并交叉核对了以下证据：

- `lightning/cmd/tidb-lightning/main.rs`：完整 255 行目标源码，确认 trait、适配器、三个公开入口、所有分支、锁/线程和退出条件。
- `lightning/cmd/tidb-lightning/Cargo.toml`：确认 package 名、库/二进制目标、Go package 元数据及 progress/server 两个直接 path 依赖。
- `lightning/cmd/tidb-lightning/lib.rs` 与 `bin_main.rs`：确认 `bin_main -> lib::main -> entry::main` 的实际装配链和两个独立 Rust 测试模块。
- `lightning/cmd/tidb-lightning/fips.rs`：确认当前 FIPS 初始化函数是保留语义位置的空实现。
- `lightning/cmd/tidb-lightning/main.go`：完整 135 行 Go 对照，确认控制流顺序、GOGC 理由、HTTP 服务失败、取消、日志同步和退出语义。
- `lightning/cmd/tidb-lightning/main_test.go` 与 `main_test.rs`：确认参数过滤、可替换 exit、并发等待以及 Rust 子进程隔离方式。
- `lightning/cmd/tidb-lightning/parity_test.rs`：完整 522 行，确认正常、配置边界、错误、取消、日志同步和真实/注入信号契约，以及 `MockApp` 对 `LightningApp` 的独立测试实现。
- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning` 返回目标、入口、Go 对照和测试文件；`node --file` 读取目标及相邻 Rust/Go 文件；精确 `query run_with_factory --json` 定位 `lightning/cmd/tidb-lightning/main.rs:100`；`explore` 确认 `run_with_factory` 的五个直接调用者及五项 Lightning 方法调用汇聚关系。

目标目录没有 `doc.go`；最近的 crate 合同由 `lib.rs` 模块文档和 `Cargo.toml` 提供。本任务只新增说明文档，未运行 Cargo，也未修改 Rust、Go、Cargo 或总计划。
