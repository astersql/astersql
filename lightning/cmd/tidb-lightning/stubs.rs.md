# `lightning/cmd/tidb-lightning/stubs.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-cmd-tidb-lightning`，由 [`lib.rs`](lib.rs) 以公开模块 `stubs` 挂入 crate，再由 [`main.rs`](main.rs) 的进程入口直接调用。它不是 Lightning 导入引擎，而是入口适配层：在不引入 `kv`、`domain`、`kvproto`、`grpcio` 等完整依赖的前提下，为 arm64 可用的轻量入口提供配置解析、退出、日志同步、内存钩子、GC 参数和操作系统信号边界。

[`Cargo.toml`](Cargo.toml) 将该包标记为 Go 包 `lightning/cmd/tidb-lightning` 的 binary 移植，仅依赖 `astersql-lightning-pkg-progress` 与 `astersql-lightning-pkg-server`。本文件只直接使用后者的 `Error`、`Result`、`config` 与 `log` 类型，并把 CLI 配置投影成 server stub 所需的较小配置面。

## 核心职责

1. 用 `err_help`、`is_err_help`、`Must` 和可注入的 `exit` 保留 Go `flag.ErrHelp`/`os.Exit` 的退出码契约，同时允许测试不终止进程。
2. 用 `logger_sync`、`memory::InitMemoryHook`、`debug::SetGCPercent` 和 `os_signal::wait_for_one_of` 隔离入口周边的进程级副作用。
3. 在 `config` 子模块中保存 [`main.rs`](main.rs) 真正会读取的全局配置子集，完成默认值、命令行、极小 TOML 子集、覆盖优先级和 server-mode 前置校验。
4. 用 `to_server_global` 把本文件的 CLI 视图投影为 `astersql_lightning_pkg_server::config::GlobalConfig`，供 `server::New` 和执行配置的 `LoadFromGlobal` 使用。

“stub”表示依赖和配置面被刻意收窄，不表示入口可观察语义可以简化。完整导入、网络、存储、日志系统和完整 TOML 解析均不在本文件实现。

## 主要符号

- `err_help() -> Error` 与 `is_err_help(&Error) -> bool`：构造并识别帮助请求。识别同时检查 `class == "flag.ErrHelp"` 和错误文本，容忍错误被外层包装后类别丢失。
- `EXIT_CODE`、`EXIT_HOOK`、`set_exit_hook`、`take_exit_code`、`exit`：记录拟退出码；有 hook 时调用 hook 并返回，无 hook 时调用 `std::process::exit`。`take_exit_code` 使用交换操作实现读取即复位。
- `SYNC_HOOK`、`set_logger_sync_result`、`logger_sync`：为文件日志收尾注入成功或失败；默认成功。
- `memory::{set_fail, InitMemoryHook}`：仅模拟可失败的初始化边界，不探测真实 OS 内存信息。
- `debug::{set_gogc_override, gogc_env, SetGCPercent, current_gc_percent, reset_gc_percent}`：保存入口分支所需的 GOGC 环境与“当前 GC 百分比”状态；它不改变 Rust 内存管理器。
- `os_signal::{inject, wait_for_one_of}`：测试可注入信号；Unix 生产路径为 SIGHUP、SIGINT、SIGQUIT、SIGTERM 安装处理器并轮询原子状态；非 Unix 路径永久等待。
- `config::GlobalConfig` 及 `GlobalLightning`、`GlobalTiDB`、`GlobalMydumper`、`GlobalImporter`、`GlobalCheckpoint`、`GlobalPostRestore`、`Security`：只保存入口和 server stub 需要的字段。`GlobalLightning::File` 模拟 Go 的嵌入字段访问。
- `config::{BackendLocal, BackendTiDB, BackendImportInto}`：入口认可的三个 backend 名称。
- `config::NewGlobalConfig()`：建立 Go 对齐的默认配置，包括 checkpoint 开启、TiDB host/user/status port/log level、系统 schema 过滤列表，以及 checksum/analyze 默认策略。
- `config::LoadGlobalConfig(args, _extra_flags)`：本文件的核心解析入口；返回 `(Option<GlobalConfig>, Option<Error>)`，而不是直接退出。
- `config::Must(cfg, err)`：无错误时要求配置存在；帮助错误退出 0，其他错误打印后退出 2。测试 hook 使 `exit` 返回时，以已有配置或默认配置继续返回。
- `config::to_server_global`：只复制 server stub 所需的日志、状态地址、需求检查、TLS、脱敏与 TiDB 日志级别字段；并发度、meta schema 等字段置为零值。
- 内部辅助 `default_filter`、`timestamp_log_file_name`、`adjust_log_level`、`build_info`、`parse_flag`、`apply_toml_lite` 与公开测试辅助 `temp_log_path_prefix`。

## 执行流程

入口主链见 [`main.rs`](main.rs) 的 `run_with_factory`：

1. `LoadGlobalConfig` 从 `NewGlobalConfig` 开始，单遍扫描参数。`-h/--help` 和版本参数返回帮助错误；位置参数使扫描停止，剩余参数被忽略。
2. `parse_flag` 区分布尔、整数和字符串参数，支持 `--name=value` 与分离值形式；布尔参数接受 Go 常见大小写/数字形式，未知参数、缺值、无效整数和枚举值返回 `Error`。重复 `-f` 追加过滤器，`-c`/`--config` 最后一次出现者胜出。
3. 若指定配置文件，先读取原始字节并由 `apply_toml_lite` 应用已支持字段，同时保存到 `ConfigFileContent`；读取失败立即返回错误。该函数只识别少量 section/key，未知或格式不足的行被忽略。
4. 命令行非空值覆盖配置文件值；布尔开关仅在与 Go 逻辑相同的方向上覆写，例如 `server-mode=true` 才把字段置真，`enable-checkpoint=false` 才关闭 checkpoint。
5. 未指定日志文件时生成临时目录下的 `lightning.log.<epoch-seconds>`；空 `StatusAddr` 可由非零 `PProfPort` 推导。server mode 最终仍无监听地址则返回错误；日志级别空值变为 `info`，`warning` 规范化为 `warn`。
6. `Must` 把解析结果折叠成配置或退出意图。`run_with_factory` 随即调用 `take_exit_code`，使测试模式也能在帮助/错误后提前返回 0/2。
7. `main.rs::run` 用 `to_server_global` 构造真实 server；随后调用内存钩子、信号等待、GOGC 分支、HTTP 服务及 server/run-once 主流程。文件日志结束时调用 `logger_sync`，错误或 server-mode 取消时通过 `exit(1)` 收尾。

## 数据与状态

全局可变状态均为进程级：退出码和 GC 百分比使用 `AtomicI32`；内存失败开关使用原子布尔；退出 hook、日志同步结果、GOGC 覆写和注入信号使用 `OnceLock<Mutex<...>>` 延迟初始化。统一采用 `Ordering::SeqCst`，优先保证测试与入口线程之间可预测的观察顺序。

`EXIT_HOOK` 保存 `Arc<dyn Fn(i32) + Send + Sync>`，调用前会在锁内克隆 `Arc`，但当前表达式持锁执行到 `if let` 语句结束；hook 不应反向调用需要同一锁的 `set_exit_hook`。`Result<()>`、配置和字符串状态均通过克隆交给调用者，避免返回共享借用。

配置本身是普通拥有型结构；`ConfigFileContent` 保留文件原始字节，`Filter` 为独立 `Vec<String>`。每次 `NewGlobalConfig` 都重新建立默认过滤列表，不共享可变集合。`apply_toml_lite` 对无效数字使用 `unwrap_or(0)`，这属于当前窄解析器的容错事实，并非完整 Go TOML 解码的等价错误报告。

## 依赖与调用关系

- 上游模块：[`lib.rs`](lib.rs) 公开 `stubs`；[`main.rs`](main.rs) 导入 `config`、`debug`、`memory`、`os_signal`，并调用顶层退出与日志同步函数。
- `run_with_factory -> config::LoadGlobalConfig -> config::Must` 是配置/早退出链；`run -> config::to_server_global -> server::New` 是应用构造链。
- `run_with_factory -> memory::InitMemoryHook` 的错误只记录日志；`run_with_factory -> os_signal::wait_for_one_of -> LightningApp::Stop` 位于独立信号线程；非 local 且 `GOGC` 为空时调用 `debug::SetGCPercent(500)`。
- 文件日志路径触发 `run_with_factory -> logger_sync`；运行错误或 server-mode 取消触发 `stubs::exit(1)`。
- 下游类型来自 `astersql-lightning-pkg-server`。本文件不连接真实 KV、PD、domain 或 grpcio，也不实现 `LightningApp` 的导入行为。

RustCodeGraph 的文件节点识别出本文件 67 个符号，并能定位 `LoadGlobalConfig`、`NewGlobalConfig`、`wait_for_one_of`、`logger_sync`、`InitMemoryHook`；精确 callers/callees 查询未给出静态边，因此以上边由 [`main.rs`](main.rs) 和测试中的直接符号引用核实，不把图工具的泛化“used by”结果当成业务调用结论。

## 错误处理与边界

帮助/版本是正常早退出，`Must` 映射为退出码 0；其余参数或配置错误映射为退出码 2。`Must` 在无错误却无配置时 panic，明确要求调用者维护 `(Some(cfg), None)` 不变量。锁获取普遍使用 `unwrap`，锁中毒会 panic；Unix 不支持的信号名会 panic，注册失败由断言终止。

`LoadGlobalConfig` 会拒绝未知 flag、缺少参数、无效布尔/整数/枚举、无法读取的配置文件、未知 backend，以及 server mode 无有效状态地址。另一方面，`apply_toml_lite` 不是通用 TOML 解析器：不验证完整语法、忽略未知键、只支持标量子集，也不解析数组过滤器、全部安全字段或全部 Go 配置。这是当前迁移边界，扩展时不能把它误称为完整兼容。

内存 hook 失败由入口记录但不阻断运行；日志同步失败只写 stderr，不改变既有退出结果；GC stub 只保留分支可观测状态。非 Unix `wait_for_one_of` 没有真实信号实现，会永久阻塞。Unix 使用进程级 `signal` handler，未恢复旧 handler，且以 10ms park 轮询接收值；这是资源与平台兼容风险。

## 并发与资源生命周期

`OnceLock` 槽在首次使用时初始化并存活至进程结束，没有显式销毁。测试必须通过 `set_exit_hook(None)`、`take_exit_code`、`set_logger_sync_result(None)`、`memory::set_fail(false)`、`debug::reset_gc_percent()`、`os_signal::inject(None)` 清理进程级状态；[`parity_test.rs`](parity_test.rs) 的 `reset_test_state` 集中执行这些复位。

信号线程在 [`main.rs`](main.rs) 中持有应用的 `Arc<Mutex<_>>`，收到一个信号后获取锁并调用一次 `Stop`，随后线程结束。注入信号不会消费槽值，因此未复位时后续等待也会立即返回同一信号。真实 Unix handler 只向原子整数写值，避免在信号上下文中分配或加锁；主线程每 10ms 检查一次。若多个信号在两次检查之间到达，原子槽只保留最后一个值。

退出 hook 与其他测试桩是全局共享的；并行测试若不隔离会互相覆盖。[`main_test.rs`](main_test.rs) 因此把真实入口路径放入子进程，[`parity_test.rs`](parity_test.rs) 的真实信号测试也使用子进程，降低全局 handler 和退出钩子的竞争风险。

## 与 Go 版本的对应关系

Go 入口 [`main.go`](main.go) 直接使用 `config.LoadGlobalConfig/Must`、`memory.InitMemoryHook`、`os/signal`、`debug.SetGCPercent`、`logger.Sync` 和包级可替换变量 `exit = os.Exit`。Rust 将这些外围依赖集中到本文件，供 [`main.rs`](main.rs) 保留相同启动顺序和可观察分支。

配置结构、默认值和解析顺序主要对应 [`pkg/lightning/config/global.go`](../../../pkg/lightning/config/global.go) 的 `GlobalConfig`、`NewGlobalConfig`、`Must` 与 `LoadGlobalConfig`：配置文件先加载、CLI 再覆盖；最后一个配置路径别名胜出；server mode 必须有状态地址；旧 `pprof-port` 可推导地址；默认过滤器排除系统 schema。

差异必须明确：Rust `apply_toml_lite` 只解析入口所需的极小 TOML 子集；`build_info` 固定输出 unknown；日志时间戳只用 epoch 秒；GC 状态不作用于 Rust 分配器；内存钩子不探测 OS；server 投影丢弃大量完整配置字段；非 Unix 没有真实信号接收。这些差异说明当前文件是迁移期入口桩，而非 Go 配置/运行时包的完整移植。

Go [`main_test.go`](main_test.go) 通过替换包级 `exit` 并过滤 `DEVEL`/`-test.*` 参数测试入口；Rust [`main_test.rs`](main_test.rs) 用 hook、线程和子进程表达相同意图。Go [`pkg/lightning/config/config_test.go`](../../../pkg/lightning/config/config_test.go) 的 `TestLoadConfig` 与 `TestCreateSeveralConfigsWithDifferentFilters` 验证无效端口、版本帮助、缺失配置、server-mode 校验、字段覆盖、默认日志路径与重复过滤器；Rust [`parity_test.rs`](parity_test.rs) 覆盖对应的 flag 校验、配置合并、退出码、GOGC、日志同步、取消和信号停止路径。

## 扩展指南

- 新增 CLI flag：先在 `parse_flag` 的正确类型表中登记，再在 `LoadGlobalConfig` 的覆盖阶段赋值；若也来自文件，在 `apply_toml_lite` 增加映射。同步检查 Go `pkg/lightning/config/global.go` 的解析顺序、空值语义和枚举范围，并在独立的 [`parity_test.rs`](parity_test.rs) 增加正常、缺值、非法值及配置/CLI 优先级案例。
- 新增 server 所需字段：同时扩展本地配置结构和 `to_server_global` 投影，确认 server crate 的 canonical 类型与默认值；不要仅在 CLI 结构中保存而遗漏下游接线。
- 改动退出、日志、内存或信号行为：保持副作用可注入，确保全局状态有复位入口，并在测试中避免并发污染。不得把 Rust 测试嵌入本源文件；使用同目录独立 `*_test.rs`。
- 若要提升完整兼容，应优先替换 `apply_toml_lite`、固定 build info、GC/内存 stub 或非 Unix 信号边界，而不是在窄解析器上宣称完整支持。引入依赖前需检查 arm64 与轻依赖目标，避免重新拉入 Cargo 注释明确排除的重型子系统。
- 兼容风险集中在 Go flag 的停止规则、布尔覆盖方向、默认值、错误/退出码和配置文件容错；性能风险主要来自真实信号轮询与进程级锁。每次修改都应以 Go 对照和 parity 测试为验收基线。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning` 定位 crate 文件；`node --file lightning/cmd/tidb-lightning/stubs.rs` 完整读取 1–812 行并识别 67 个符号；`query` 定位本文件的 `LoadGlobalConfig`、`NewGlobalConfig`、`wait_for_one_of`、`logger_sync`、`InitMemoryHook`。精确 callers/callees 无静态输出，调用关系改由直接源码引用复核。
- crate 与入口：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`main.rs`](main.rs)。
- Rust 独立测试：[`parity_test.rs`](parity_test.rs)、[`main_test.rs`](main_test.rs)。覆盖真实 Unix 信号、配置错误与合并、GOGC 分支、帮助/错误退出码、非致命内存 hook、日志同步、取消、注入信号 Stop，以及入口 hook/子进程隔离。
- Go 对照：[`main.go`](main.go)、[`main_test.go`](main_test.go)、[`pkg/lightning/config/global.go`](../../../pkg/lightning/config/global.go)、[`pkg/lightning/config/config_test.go`](../../../pkg/lightning/config/config_test.go)。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收要求目标文件存在且固定二级标题恰好为 11 个；交付前另行运行任务给定命令并记录退出码。
