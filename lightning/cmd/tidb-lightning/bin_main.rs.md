# `lightning/cmd/tidb-lightning/bin_main.rs`

## 文件定位

本文件是 Cargo 包 `astersql-lightning-cmd-tidb-lightning` 的可执行目标入口。`lightning/cmd/tidb-lightning/Cargo.toml` 的 `[[bin]]` 把同名二进制映射到 `bin_main.rs`，而 `[lib]` 把库目标映射到 `lib.rs`。因此进程启动后先进入这里的私有 `fn main()`，再跨目标调用同一包的库 crate；Cargo 包名中的连字符在 Rust 路径中转换为下划线，所以调用路径写作 `astersql_lightning_cmd_tidb_lightning`。

这是一个九行的薄包装层。完整装配链为 `bin_main.rs::main` → `lib.rs::main` → `main.rs` 中的 `entry::main`。CLI 参数解析、FIPS 初始化、Lightning 服务构造、信号处理、导入执行和退出码判定都位于下游，不属于本文件自身。

## 核心职责

本文件只承担两项职责：提供 Rust 可执行程序规定的进程入口 `fn main()`；无条件、同步地把控制权转交给库 crate 的公开 `main()`。它不读取参数、不持有配置、不解释结果，也不自行输出日志或调用 `std::process::exit`。

这层拆分使实际启动逻辑能够以库模块形式复用和测试。入口行为的扩展应优先落在 `lib.rs` 或 `main.rs` 的可测试接口中，而不是堆入这个二进制壳，否则真实二进制与直接链接库目标的测试可能走不同路径。

## 主要符号

- `fn main()`（`bin_main.rs:7`）：私有、无参数、返回单元类型的进程入口。函数体只有一次 `astersql_lightning_cmd_tidb_lightning::main()` 调用。
- `astersql_lightning_cmd_tidb_lightning::main()`（调用点 `bin_main.rs:8`，定义于 `lib.rs:41`）：库 crate 的公开入口；它继续调用 `entry::main()`。
- `entry`（`lib.rs:27-29`）：通过 `#[path = "main.rs"] pub mod entry` 把真实启动实现接入库目标。

本文件没有模块级常量、类型、trait、`impl`、局部状态、公开 API 或条件编译项。对其他 Rust 代码可见的入口是 `lib.rs::main`，不是这里的私有函数。

## 执行流程

1. Cargo 根据 `Cargo.toml` 的 `[[bin]]` 构建可执行文件；操作系统启动进程后，Rust 运行时调用 `bin_main.rs::main`。
2. `bin_main.rs::main` 立即同步调用库 crate 的 `main()`，没有前置分支或初始化。
3. `lib.rs::main` 转发到 `entry::main()`。
4. `main.rs::main` 先调用 `fips::init_fips_only_tls_for_boringcrypto_build()`，再收集除程序名外的 `std::env::args()` 并交给 `run`。
5. `main.rs::run` 通过 `run_with_factory` 构造真实 `server::Lightning`；后者完成配置装载、内存钩子、信号线程、HTTP 状态服务、server mode 或单次导入分派、取消语义、日志同步与退出码计算。
6. 下游返回非零码时，`main.rs::main` 调用 `stubs::exit(code)`；返回零时正常回到库入口和本文件，随后进程自然结束。

第 4 至 6 步只是本文件直接转交后的应用主链，用于界定入口位置；这些行为均不在 `bin_main.rs` 内实现。

## 数据与状态

本文件不创建或保存任何数据。它没有参数缓存、配置对象、全局变量、环境变量访问、锁或进程退出钩子；唯一可观察行为是把调用控制权传到库层。

下游的运行数据包括 `main.rs::main` 收集的参数、`run_with_factory` 装载的 `config::GlobalConfig`、由 `Arc<Mutex<A>>` 包装的 Lightning 应用实例，以及 `stubs.rs` 中供测试替换的退出码、信号和日志同步状态。由于本壳不截获调用结果，正常返回、panic 展开和下游退出行为都会按库入口的语义传播。

## 依赖与调用关系

上游不是仓库内的普通函数，而是 Cargo 二进制配置和 Rust 运行时。RustCodeGraph 将 `bin_main.rs::main` 识别为唯一函数，目标文件显示 `used by 0 files`；这符合私有进程入口由运行时隐式调用的边界。`Cargo.toml:16-18` 的 `[[bin]]` 是入口关系的权威证据。

唯一直接下游是 `lib.rs::main`。RustCodeGraph 的 `callees` 对常见名称 `main` 产生了全仓库歧义，并将目标项报告为无调用边，没有解析出这条跨二进制目标与库目标的调用；但 `bin_main.rs:8` 的完整限定路径、`lib.rs:41-43` 的公开定义以及同一 Cargo 包边界共同确认了该调用。再下一层是 `lib.rs::main` → `entry::main` → `run` → `run_with_factory`。

Cargo 包直接依赖 `astersql-lightning-pkg-progress` 和 `astersql-lightning-pkg-server`，但 `bin_main.rs` 没有直接引用它们。二者在 `main.rs` 的真实启动流程中分别提供进度开关及 Lightning 服务、配置、上下文和错误类型。

## 错误处理与边界

本文件没有 `Result`、`?`、匹配错误、日志、恢复逻辑或显式退出码。它既不吞掉 panic，也不改写库入口的进程行为。直接下游 `main.rs::main` 仅在 `run` 返回非零时调用 `stubs::exit`，而 `run_with_factory` 才定义实际错误边界。

下游当前的重要边界包括：配置帮助或版本路径可返回 `0`；应用构造失败返回 `1`；内存钩子失败只记录日志；`GoServe` 失败沿用 Go 的普通 `return` 语义而返回 `0`；导入错误返回 `1`；可识别的 context canceled 在非 server mode 可作为取消而返回 `0`，server mode 的未完成取消返回 `1`；文件日志同步失败不覆盖主流程结果。这里不应重复实现这些规则，否则会造成库测试入口与真实二进制不一致。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、网络连接、文件、事务或其他需清理资源；它的调用是同步转发，因此自身没有并发不变量或清理分支。

直接下游 `run_with_factory` 才用 `Arc<Mutex<A>>` 共享应用对象，并在生产模式下启动一个信号线程：等待 `SIGHUP`、`SIGINT`、`SIGTERM` 或 `SIGQUIT` 后获取锁并调用 `Stop()`。主线程同样在持锁时调用 `GoServe`、`RunServer`、`RunOnceWithOptions` 和 `TaskCanceled`。日志文件同步发生在最终退出码判断之前；同步失败只报告到标准错误。上述生命周期由 `parity_test.rs` 的信号、取消和日志同步场景验证，不应误认为由 `bin_main.rs` 直接管理。

## 与 Go 版本的对应关系

Go 对照入口是 `lightning/cmd/tidb-lightning/main.go::main`。Go 将配置读取、日志提示、`server.New`、内存钩子、信号 goroutine、GC 参数、HTTP 服务、运行模式分派、取消处理和退出码全部写在同一个 `main()` 中。Rust 为提高复用与可测试性，把同等主流程放到 `main.rs::main/run/run_with_factory`，并额外以本文件和 `lib.rs` 形成两级薄转发。

因此，本文件只与 Go `main()` 的“作为进程入口并进入统一启动链”职责对应；Go 函数中的业务装配没有丢失，而是下沉到了 Rust 库实现。`Cargo.toml` 的 `[package.metadata.porting]` 将 `go-package` 指向 `lightning/cmd/tidb-lightning`、`kind` 标为 `binary`，也明确了这种移植关系。

Go 的 `main_test.go::TestRunMain` 会过滤 `DEVEL` 和 `-test.*` 参数、替换包级 `exit`、在线程中运行 `main()` 并等待返回。Rust 的 `main_test.rs::test_run_main` 用参数过滤断言和隔离子进程保持这些约束；它调用可测试的库实现而不是私有 `bin_main.rs::main`。

## 扩展指南

- 修改 CLI 参数、配置装载、FIPS 顺序、运行模式、错误文案或退出码时，应修改 `main.rs` 中的 `main`、`run` 或 `run_with_factory`，并同步独立的 `main_test.rs` 与 `parity_test.rs`；不要把逻辑放进本文件。
- 修改 crate 模块装配或库级公共入口时，应修改 `lib.rs`，同时保持 `bin_main.rs` 只有一次转发，以确保二进制和库测试共享启动路径。
- 只有必须早于库入口、且无法安全置于库层的最低级进程初始化才适合加入这里。此类改变需要新增独立的进程级测试，验证初始化顺序、重复初始化、panic 和退出码传播。
- 若改变包名、二进制名或入口文件位置，必须同步 `Cargo.toml` 的 `[package]`/`[[bin]]` 与完整限定 crate 路径；这会影响打包脚本、运维命令和外部调用兼容性。
- 测试应继续放在独立的 `main_test.rs` 或 `parity_test.rs`，不要把 Rust 测试逻辑内嵌到生产源文件。

该壳层只有固定的一次函数调用，通常可被编译器内联。扩展时应避免在此重复解析配置、创建服务或启动后台线程，以免增加启动成本并形成双重生命周期。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning` 列出目标、库入口、实现、Go 对照和独立测试；`node --file lightning/cmd/tidb-lightning/bin_main.rs` 确认文件全貌；`node lightning/cmd/tidb-lightning/bin_main.rs::main` 确认唯一函数。`callees` 对同名 `main` 有歧义且未解析跨目标边，因此用源调用点、库定义和 Cargo 配置补证。
- 生产源码：`lightning/cmd/tidb-lightning/bin_main.rs:1-9`、`lib.rs:19-43`、`main.rs:77-255`；直接依赖声明位于 `Cargo.toml`。
- Go 对照：`lightning/cmd/tidb-lightning/main.go::main` 及包级 `exit`，用于核对配置、信号、运行模式、错误和资源收尾语义。
- 独立测试：Rust `main_test.rs::test_run_main` 验证入口测试参数过滤、退出钩子隔离和等待返回；`parity_test.rs::go_rust_public_contract_matches`、`contract_error_paths`、`contract_resource_cleanup_sync_and_cancel` 与 `real_signal_wait_matches_go` 覆盖成功、边界、错误、取消、日志同步和信号停止；Go 基线是 `main_test.go::TestRunMain`。
- `rg` 未发现任何测试或生产模块直接引用私有 `bin_main.rs::main`；它通过 Cargo/进程边界生效，库层行为由上述独立测试覆盖。本任务是纯文档分析，按计划不运行 Cargo。
