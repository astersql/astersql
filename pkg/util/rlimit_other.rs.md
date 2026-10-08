# `pkg/util/rlimit_other.rs`

## 文件定位

该文件是 `astersql-util` crate 在非 Windows 平台上的进程文件描述符软上限查询实现。文件级 `#![cfg(not(windows))]` 保证其中代码只在非 Windows 目标参与编译；模块由 [`pkg/util/lib.rs`](lib.rs) 的 `pub mod rlimit_other` 暴露。crate 边界由 [`pkg/util/Cargo.toml`](Cargo.toml) 定义，实际使用的直接外部依赖是 `libc`（调用 `getrlimit`、读取 `rlimit`）与 `log`（失败告警）。

当前 Rust 仓库内没有发现生产代码调用 `rlimit_other::GenRLimit`；Rust 侧直接消费点均为测试。完整应用中对应能力目前可从 Go 同名实现的两个生产调用点观察：表导入在 `pkg/executor/importer/import.go` 中把结果写入 `BackendConfig.MaxOpenFiles`，DDL ingest 在 `pkg/ddl/ingest/env.go` 中把结果保存到 `litRLimit` 并记录初始化日志。因此，本文件已经提供与 Go 对齐的公共查询能力，但不能据现有证据声称 Rust 主链已经接线。

## 核心职责

- `GenRLimit` 查询当前进程的 `RLIMIT_NOFILE` 软限制，即 `libc::rlimit.rlim_cur`，而不是硬限制 `rlim_max`。
- 系统调用失败时不向调用者传播错误，而是记录包含 `source` 与操作系统错误的警告，并返回兼容默认值 `1024`。
- `gen_rlimit_with` 把“取得限制”和“发出告警”注入为闭包，使成功、失败与回退行为可以在独立测试文件中确定性验证，而无需修改真实进程资源限制。

该文件只读取限制，不提升、降低或持有任何资源配额，也不负责根据限制创建连接池或文件句柄池。

## 主要符号

- `pub fn GenRLimit(source: &str) -> u64`：非 Windows 公共入口。`source` 仅用于失败日志中的来源标签；成功路径不读取它。函数用 `libc::getrlimit(libc::RLIMIT_NOFILE, ...)` 获取限制，再委托 `gen_rlimit_with` 统一选择成功值或回退值。名称保留 Go 风格，crate 根的 `#![allow(non_snake_case)]` 允许这种命名。
- `pub(crate) fn gen_rlimit_with(source: &str, getrlimit: impl FnOnce() -> io::Result<libc::rlimit>, warn: impl FnOnce(&str, &io::Error)) -> u64`：crate 内可见的策略函数。成功时返回 `rlim_cur as u64`；失败时恰好调用一次 `warn(source, &err)`，随后返回 `1024`。两个闭包均为 `FnOnce`，符合每次查询最多执行一次系统读取和一次失败通知的控制流。
- 文件没有模块级常量、结构体、枚举、trait 或 `impl`；`1024` 是两个失败分支中的字面兼容值，而非可配置状态。

## 执行流程

1. 调用者以用途标签调用 `GenRLimit(source)`。
2. `GenRLimit` 构造读取闭包：先分配未初始化的 `MaybeUninit<libc::rlimit>`，再以 `RLIMIT_NOFILE` 调用 `libc::getrlimit`。
3. 若系统调用返回非零值，读取闭包立即用 `io::Error::last_os_error()` 捕获线程当前的 OS 错误并返回 `Err`；此时不会对未初始化内存执行 `assume_init`。
4. 若系统调用返回零值，读取闭包才执行 `assume_init`，把完整的 `libc::rlimit` 交给 `gen_rlimit_with`。
5. `gen_rlimit_with` 在 `Ok` 分支提取软限制 `rlim_cur` 并转成 `u64`；`rlim_max` 不参与结果计算。
6. 在 `Err` 分支，告警闭包以 `log::warn!` 输出来源、错误和 `default=1024`，随后返回 `1024`。

## 数据与状态

函数输入只有借用字符串 `source`，输出是按值返回的 `u64`。临时 `libc::rlimit` 仅存活于一次调用栈中；成功结果只读取 `rlim_cur`。文件不含全局变量、缓存、配置、锁或可变静态状态，多次调用会各自重新读取当前进程限制。

`MaybeUninit` 用于满足 C ABI 的输出指针要求。安全不变量是：只有 `getrlimit` 返回零时才能把缓冲区视为已初始化。当前代码在非零返回时先返回 `Err`，在零返回后才执行 `assume_init`，保持了该不变量。

## 依赖与调用关系

- 模块装配：`pkg/util/lib.rs` 公开 `rlimit_other`，并在 `cfg(test)` 下从独立文件 `pkg/util/rlimit_other_test.rs` 挂载单元测试。
- 内部调用边：RustCodeGraph 显示 `rlimit_other.rs::GenRLimit -> rlimit_other.rs::gen_rlimit_with`。
- 测试调用边：RustCodeGraph 显示 `gen_rlimit_preserves_success_and_error_fallback -> gen_rlimit_with`；`pkg/util/cpu_posix_1_aster_unit_test.rs` 还直接调用公共入口验证真实查询返回正数。
- 下游系统接口：`libc::getrlimit`、`libc::RLIMIT_NOFILE` 与 `libc::rlimit`；标准库 `io::Error::last_os_error` 捕获失败原因；`log::warn!` 发出告警。
- 生产接线现状：仓库搜索未找到生产 Rust 调用者。Go 对照入口则由 `pkg/executor/importer/import.go` 和 `pkg/ddl/ingest/env.go` 使用；这些调用点说明该数值在完整 TiDB 应用中的用途，但不是 Rust 调用边。
- 平台对偶：Windows Rust 实现在 `pkg/util/rlimit_windows.rs`，固定返回 `1024`；本文件的 `cfg(not(windows))` 决定非 Windows 分支。

## 错误处理与边界

`GenRLimit` 的 API 不返回 `Result`。任何 `getrlimit` 失败都降级为告警加 `1024`，所以调用者无法区分“系统软限制恰为 1024”和“查询失败后回退为 1024”；这是与 Go 实现一致的兼容约定。错误必须在系统调用后立即通过 `last_os_error` 捕获，以免其他 OS 调用覆盖错误码。

成功路径原样返回软限制，没有在本文件中设置最小值、最大值或非零校验。测试只证明注入值 `4096` 原样返回、`EINVAL` 触发来源/错误告警并回退，以及真实环境查询结果大于零；它没有证明所有平台上软限制必然为正，也没有验证极端 `rlim_t` 数值的跨平台转换。

日志是失败路径的唯一诊断信号。修改回退值、日志内容或错误传播方式会改变 Go 对齐语义及调用者容量决策，应视为兼容性变更。

## 并发与资源生命周期

本文件没有共享 Rust 状态，函数仅使用栈上临时值，因此自身不需要锁或通道；并发调用彼此独立。它读取的是进程级资源限制，若其他线程或外部控制面同时修改该限制，不同调用可能观察到不同结果，文件不提供快照一致性或缓存保证。

系统调用同步完成，不创建异步任务、线程或文件描述符。`MaybeUninit<libc::rlimit>` 在调用返回后立即销毁；函数不取得任何需要显式释放的资源。注入闭包采用 `FnOnce`，其捕获资源也在一次调用完成后按 Rust 所有权规则释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/rlimit_other.go`。两者均只在非 Windows 平台生效，读取 `RLIMIT_NOFILE`，成功返回软限制，失败记录带 `source` 的警告并返回 `1024`。Rust 的 `rlim_cur` 对应 Go 的 `syscall.Rlimit.Cur`，Rust 的 `io::Error::last_os_error` 对应 Go `syscall.Getrlimit` 返回的 `error`。

实现结构的主要差异是 Rust 将策略拆成可注入的 `gen_rlimit_with` 以支持独立单元测试，并用 `MaybeUninit` 明确表示 C 函数写入前的未初始化状态；Go 使用已零初始化的 `syscall.Rlimit` 局部变量。Go 使用结构化 `logutil.BgLogger`/`zap` 字段，Rust 使用 `log::warn!` 格式化文本，但都保留来源、错误和默认值信息。

Windows 对照由 `pkg/util/rlimit_windows.go` 与 `pkg/util/rlimit_windows.rs` 提供，均固定返回 `1024`。未发现 Go 侧专门的 rlimit 单元测试；Rust 的专用测试因此是当前最直接的成功/错误分支证据。

## 扩展指南

- 若增加新的资源类型查询，优先抽取明确的资源选择参数或新函数，不要悄然改变 `GenRLimit` 只代表 `RLIMIT_NOFILE` 的既有契约。
- 若改变失败回退或日志策略，应同步修改 `gen_rlimit_with`、`GenRLimit` 的告警闭包以及独立测试 `pkg/util/rlimit_other_test.rs`，并核对 Go 同路径实现是否仍需保持一致。
- 若把该能力接入 Rust 导入或 DDL 主链，应在真实消费者处新增独立测试，验证返回值如何约束文件句柄/后端配置；不要把生产测试逻辑嵌入本源文件。
- 若修改 `unsafe` 区域，必须继续保证非零返回时不读取输出缓冲区，并优先围绕系统调用建立可测试的安全封装。
- 若调整平台条件，应同时检查 `pkg/util/rlimit_windows.rs` 和 `pkg/util/lib.rs` 的模块装配，避免同一目标缺失实现或产生名称冲突。
- 性能上一次调用对应一次同步系统调用；若未来引入缓存，需要明确进程限制动态变化时的失效策略，而不能假定限制永久不变。

## 验证依据

- 源码与装配：`pkg/util/rlimit_other.rs`、`pkg/util/lib.rs`、`pkg/util/Cargo.toml`、`pkg/util/rlimit_windows.rs`。
- Rust 测试：`pkg/util/rlimit_other_test.rs` 验证成功值、失败回退及告警内容；`pkg/util/cpu_posix_1_aster_unit_test.rs::printer_and_rlimit_are_operational` 验证公共入口可执行并在当前环境返回正数。
- Go 对照与生产用途：`pkg/util/rlimit_other.go`、`pkg/util/rlimit_windows.go`、`pkg/executor/importer/import.go`、`pkg/ddl/ingest/env.go`。
- RustCodeGraph：`query GenRLimit --kind function` 与 `node rlimit_other.rs::GenRLimit` 确认公共符号和到 helper 的调用；`node rlimit_other.rs::gen_rlimit_with` 确认签名、分支及来自公共入口和专用测试的调用。精确 `callers/callees` 子命令对同名符号存在结果消歧限制，因此生产调用者结论另以仓库范围 `rg` 核对。
- 人工边界复核：确认文件仅查询软限制、不写资源限制；失败不传播、固定回退 `1024`；无共享状态、异步任务或资源持有；当前没有生产 Rust 调用点。

