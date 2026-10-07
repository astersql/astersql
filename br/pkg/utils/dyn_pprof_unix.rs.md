# `br/pkg/utils/dyn_pprof_unix.rs`

## 文件定位

本文件是 `astersql-br-pkg-utils` crate 中 Unix 平台的动态 pprof/status 启动器，crate 边界由 [`br/pkg/utils/Cargo.toml`](Cargo.toml) 定义，模块由 [`br/pkg/utils/lib.rs`](lib.rs) 以 `dyn_pprof_unix` 名称公开。整个实现及其再导出都受 `cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd", unix))` 约束；非 Unix 目标由 [`dyn_pprof_other.rs`](dyn_pprof_other.rs) 提供空实现。

它不实现 HTTP 路由或性能采样，而是在进程收到 `SIGUSR1` 后调用 [`pprof.rs`](pprof.rs) 的 `StartStatusListener`，让后者绑定临时端口并启动 status/pprof 服务。当前迁移状态需要特别区分：本 crate 内已有真实实现和独立测试，但仓库中的 Rust BR CLI [`br/cmd/br/cmd.rs`](../../cmd/br/cmd.rs) 通过 `crate::stubs::*` 使用的是 [`br/cmd/br/stubs.rs`](../../cmd/br/stubs.rs) 内同名空实现，因此本文件尚未接入该 CLI 主链。

## 核心职责

- `StartDynamicPProfListener` 注册进程级 `SIGUSR1` 监听，并把阻塞的信号消费循环放到独立 OS 线程。
- `listen_for_start_signal` 过滤信号，仅在收到 `SIGUSR1` 时调用注入的启动闭包；闭包抽离使信号分派逻辑可以在独立测试中验证，而不必真的启动 HTTP 服务。
- 生产闭包固定调用 `StartStatusListener("0.0.0.0:0", tls.as_deref())`：监听所有网络接口，由操作系统选择空闲端口，并把可选 TLS 配置借用给下游。
- 信号注册失败或服务启动失败都采用“记录日志并停止/不启动”的容错策略，不把故障传播给 BR 的主业务流程。

## 主要符号

- `imp`：仅在 Unix 类目标编译的私有实现模块，封装平台依赖的 `signal-hook`、线程和信号常量。
- `listen_for_start_signal<F>(signals: Signals, start: F)`：crate 内可见的同步循环。`F: FnMut() -> Result<(), String>` 允许每次符合条件的信号触发一次可变回调；回调成功后继续等待，失败后记录错误并立即返回。
- `StartDynamicPProfListener(tls: Option<Arc<TLS>>)`：公开入口。`Arc<TLS>` 使 TLS 配置可安全移入 `'static` 后台线程；没有 TLS 时传入 `None`，下游使用明文监听。
- `SIGUSR1`：来自 `signal_hook::consts::signal` 的触发信号，对应 Go 文件中的 `startPProfSignal = syscall.SIGUSR1`，Rust 没有另设模块级常量别名。
- 文件末尾的两个 `pub use`：公开再导出 `StartDynamicPProfListener`，并以 `pub(crate)` 再导出 `listen_for_start_signal` 供同 crate 的独立测试文件访问。

本文件没有自定义结构体、枚举、trait、全局可变状态或 `impl` 块。

## 执行流程

1. 调用方把可选的共享 TLS 配置交给 `StartDynamicPProfListener`。
2. 入口用 `Signals::new([SIGUSR1])` 向 `signal-hook` 注册监听。若注册失败，写入 `failed to register SIGUSR1 listener` 警告并直接返回。
3. 注册成功后，入口用 `thread::spawn` 创建后台线程，并把 `Signals` 与 `tls` 所有权移入线程。
4. 后台线程进入 `listen_for_start_signal`；`signals.forever()` 阻塞等待信号并逐个产出信号编号。
5. 非 `SIGUSR1` 值被跳过。收到 `SIGUSR1` 时先记录 `signal received, starting pprof...`，再执行启动闭包。
6. 生产闭包调用 `StartStatusListener("0.0.0.0:0", tls.as_deref())`，并把 `SharedError` 转成 `String`，适配辅助函数的错误边界。
7. 启动成功时循环继续，后续 `SIGUSR1` 会再次尝试启动；[`pprof.rs`](pprof.rs) 的进程级 `STARTED_PPROF` 会拒绝重复绑定，因此第二次尝试通常转入失败分支。启动失败时记录 `failed to start pprof` 并终止信号线程。

## 数据与状态

本文件自身不保存长期全局状态。主要运行时状态只有：`Signals` 持有的信号注册/接收资源、后台线程拥有的 `Option<Arc<TLS>>`，以及循环中的启动闭包。

TLS 只在创建监听器时通过 `as_deref()` 临时借用为 `Option<&TLS>`；`Arc` 保证后台线程存活期间配置对象不会被释放。监听是否已经启动、实际由端口 `0` 分配出的地址以及重复启动保护均由下游 [`pprof.rs`](pprof.rs) 的 `STARTED_PPROF: OnceLock<Mutex<String>>` 管理，不在本文件重复维护。

该文件固定监听地址为 `0.0.0.0:0`。这意味着绑定到所有 IPv4 接口，并由操作系统分配端口；实际端口由 `pprof.rs::listen` 读取并记录。

## 依赖与调用关系

上游关系：

- `br/pkg/utils/lib.rs` 声明公开模块，并仅在 `cfg(all(test, unix))` 下挂载 [`dyn_pprof_unix_test.rs`](dyn_pprof_unix_test.rs)。
- RustCodeGraph 对精确符号的探索显示 `StartDynamicPProfListener -> listen_for_start_signal`，测试 `test_sigusr1_dispatches_dynamic_pprof_start -> listen_for_start_signal`。
- 仓库范围的 Rust 文本检索没有发现 `astersql_br_pkg_utils::dyn_pprof_unix::StartDynamicPProfListener` 的生产调用。`br/cmd/br/cmd.rs::startStatusServer` 中的同名调用解析到本地 `stubs::utils::StartDynamicPProfListener`，不是这里的实现；把真实 crate 接入 CLI 是后续迁移工作，不能把当前文件描述成已在 BR 启动流程中运行。

下游关系：

- `signal_hook::iterator::Signals` 负责注册与阻塞接收，`signal_hook::consts::signal::SIGUSR1` 提供平台信号值；依赖在 `Cargo.toml` 中声明为 `signal-hook = "0.3"`。
- `std::thread::spawn` 承载长生命周期阻塞循环，避免阻塞调用线程。
- `astersql_br_pkg_logutil::{log, Field}` 输出注册、触发和启动失败日志。
- `astersql_util::security::TLS` 描述可选 TLS 配置。
- `crate::pprof::StartStatusListener` 完成 TCP 绑定、TLS 包装、默认路由注册和后台 HTTP 服务。

## 错误处理与边界

- `Signals::new` 的具体错误被有意压缩为固定警告，没有返回给调用方；公开函数返回 `()`，所以调用方无法程序化获知注册失败。
- `StartStatusListener` 的 `SharedError` 在闭包中经 `to_string()` 丢失类型信息，辅助循环只记录文本。该处理与公开入口“失败不影响主流程”的职责一致，但不适合需要基于错误类型重试的扩展。
- 一次启动错误会 `return`，永久终止该监听线程；当前实现没有退避、重注册或恢复机制。
- `listen_for_start_signal` 虽会过滤非 `SIGUSR1`，但生产构造的 `Signals` 只订阅 `SIGUSR1`；过滤分支主要构成防御性边界。
- `thread::spawn` 返回的 `JoinHandle` 被丢弃，线程 panic、提前结束或资源泄漏无法由调用方观察或回收。
- 端口 `0` 避免静态端口冲突，但 `0.0.0.0` 扩大了网络可达面；是否加密完全取决于 `tls`。新增固定地址或访问控制时必须同步评估运维兼容性与暴露风险。
- cfg 表达式包含通用 `unix`，因此实际覆盖所有 Rust 认定的 Unix 目标，不仅是显式列出的 Linux、macOS 和 FreeBSD；这与 Go 构建约束中的 `unix` 意图一致。

## 并发与资源生命周期

`StartDynamicPProfListener` 在完成信号注册后立即返回，信号循环在脱离管理的 OS 线程中长期阻塞。线程拥有 `Signals`，其析构发生在线程退出时；启动闭包返回错误会结束循环并释放信号接收资源。函数没有显式取消通道、关闭方法或 join 机制，正常情况下资源生命周期等同于进程生命周期。

收到信号后，回调在信号消费线程内同步执行；它调用的 `StartStatusListener` 只负责绑定并再启动 HTTP serve 线程，因此信号循环不会承担长期网络服务。若短时间内连续收到信号，`Signals::forever()` 依次分派，回调不会并发执行。下游用 `Mutex<String>` 串行保护“是否已启动”状态；成功启动后再次收到信号会触发重复启动错误，继而结束本信号循环。

独立测试 [`dyn_pprof_unix_test.rs`](dyn_pprof_unix_test.rs) 自己创建监听线程、向当前测试进程发送真实 `SIGUSR1`，再借助 mpsc 通道和两秒超时确认回调已执行。测试回调故意返回错误，使循环可退出并被 `join`，避免测试遗留永久后台线程。

## 与 Go 版本的对应关系

[`dyn_pprof_unix.go`](dyn_pprof_unix.go) 是直接语义基准：Go 用容量为 1 的 `chan os.Signal`、`signal.Notify` 和 goroutine；Rust 用 `Signals` 与 `thread::spawn`。两者都订阅 `SIGUSR1`，收到信号后记录日志，并以 `0.0.0.0:0` 和传入 TLS 启动 status/pprof；启动失败都记录警告并退出后台循环。

差异包括：

- Go 声明 `startPProfSignal` 常量，Rust 直接使用 `SIGUSR1`。
- Go API 接收可空指针 `*TLS`，Rust API 接收 `Option<Arc<TLS>>`，以满足跨线程所有权和生命周期要求。
- Go 的 channel 由 `signal.Notify` 填充；Rust 的 `Signals::forever()` 负责迭代。两边都没有显式注销/关闭流程。
- Go 的 `StartStatusListener` 错误保留为 error 并交给结构化日志字段；Rust 在本文件边界把错误转成字符串。
- Go BR CLI 的 `br/cmd/br/cmd.go::startStatusServer` 已调用包内真实函数；当前 Rust CLI 同位置仍走本地桩，所以应用级接线尚不对等。
- 非 Unix 两端都提供空实现，但 Rust Unix 与非 Unix 函数签名目前分别是 `Option<Arc<TLS>>` 和 `Option<&TLS>`；它们位于不同模块且受互斥 cfg 保护，当前能避免同一目标上的冲突，但未来若统一再导出 API，需要先统一所有权约定。

## 扩展指南

- 修改触发信号或支持多个信号时，集中调整 `Signals::new` 与 `listen_for_start_signal` 的匹配分支，并在 [`dyn_pprof_unix_test.rs`](dyn_pprof_unix_test.rs) 增加每个信号的触发/忽略用例；不要把测试内嵌到生产文件。
- 修改监听地址、TLS 传递或默认 handler 时，接入点是 `StartDynamicPProfListener` 内的启动闭包；实际绑定、重复启动与 HTTP 生命周期仍应在 [`pprof.rs`](pprof.rs) 维护，避免两处状态漂移。
- 增加重试或优雅关闭时，需要改变 `listen_for_start_signal` 的返回/控制协议，并保留可注入闭包，使成功、失败、重试上限和取消都能由独立测试确定性验证。要特别防止重复启动造成线程或端口泄漏。
- 将功能接入 Rust BR CLI 时，应替换 `br/cmd/br/stubs.rs` 的空实现边界并在 `br/cmd/br/Cargo.toml` 建立正确依赖，而不是复制本文件逻辑；同时补充 `startStatusServer` 路径测试，证明空 `status-addr` 才注册动态监听、非空地址仍直接启动固定监听。
- 更改公开签名或 cfg 条件时，要同步检查 [`dyn_pprof_other.rs`](dyn_pprof_other.rs)、`lib.rs` 的模块声明以及所有目标平台上的类型一致性。
- 性能方面，信号路径本身成本很低；扩展时应避免在信号消费线程执行长时间阻塞工作。兼容性方面应保持 Go 的 `SIGUSR1`、失败不终止主流程和动态端口语义，除非有明确迁移决策。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/utils/dyn_pprof_unix.rs` 读取目标文件全部 71 行；精确 `explore` 验证 `StartDynamicPProfListener -> listen_for_start_signal` 及测试调用边，并显示该符号与 Go/非 Unix 同名实现。
- 源码与模块：[`dyn_pprof_unix.rs`](dyn_pprof_unix.rs)、[`lib.rs`](lib.rs)、[`pprof.rs`](pprof.rs)、[`dyn_pprof_other.rs`](dyn_pprof_other.rs)。
- crate 声明：[`Cargo.toml`](Cargo.toml)，确认 crate 名、`signal-hook = "0.3"`、日志与 util 路径依赖。
- Go 对照：[`dyn_pprof_unix.go`](dyn_pprof_unix.go) 与 [`dyn_pprof_other.go`](dyn_pprof_other.go)。
- 独立 Rust 测试：[`dyn_pprof_unix_test.rs`](dyn_pprof_unix_test.rs)，覆盖真实 `SIGUSR1` 到启动回调的分派，并用故意失败的回调结束监听线程。
- 应用接线核验：[`br/cmd/br/cmd.rs`](../../cmd/br/cmd.rs) 与 [`br/cmd/br/stubs.rs`](../../cmd/br/stubs.rs)，确认当前 CLI 调用的是本地空实现；仓库范围 `rg` 未发现本文件公开入口的其他 Rust 生产调用。
- 本任务为纯文档分析，按总计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构校验和文档差异检查。
