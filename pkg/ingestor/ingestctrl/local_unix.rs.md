# `pkg/ingestor/ingestctrl/local_unix.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate。`pkg/ingestor/ingestctrl/Cargo.toml` 将 `lib.rs` 指定为 crate 入口，`pkg/ingestor/ingestctrl/lib.rs` 通过 `pub mod local_unix` 对外公开此模块。它是 local backend 创建前的 Unix 进程资源门禁：读取 `RLIMIT_NOFILE`，并在当前 soft limit 不足时尝试提升，避免并发写 SST 和打开 Engine 文件时过早耗尽文件描述符。

直接 Rust 生产接线是 `pkg/ingestor/ingestctrl/local.rs::NewLocalBackend`：配置调整后，它以 `config.max_open_files` 调用 `VerifyRLimit`，成功后才创建 `EngineManager`。`GetSystemRLimit` 是公开查询 API，但全仓 Rust 搜索未发现当前生产调用者；Go 版 Lightning 仍使用对应 API 计算每表最大打开文件数。

## 核心职责

1. 用 `read_limit` 将 libc ABI 的 `getrlimit(RLIMIT_NOFILE, ...)` 转换为 crate 统一的 `Result<RawRLimit>`。
2. 用 `GetSystemRLimit` 只读返回当前进程的 soft limit，不修改系统状态。
3. 用 `VerifyRLimit`/`verify_rlimit_with` 将请求值限制在 `maxRLimit`，必要时同时提高 soft limit 和过低的 hard limit，并二次读取验证操作真正生效。
4. 把 FFI 和 OS 错误统一映射为 `crate::Error::Io`，让 local backend 构造链可以用 `?` 直接阻止不满足要求的启动。

本文件不计算业务侧所需的文件数，也不打开、关闭或管理 SST/Engine 文件；上游必须先给出估算值。

## 主要符号

- `pub const maxRLimit: RlimT = 1_000_000`：单次校验允许请求的上限，与 Go `maxRLimit` 一致。
- `pub(crate) struct RawRLimit { current: u64, maximum: u64 }`：`#[repr(C)]` 的 FFI 数据布局，`current` 表示 soft limit，`maximum` 表示 hard limit。只在 crate 内可见，并派生 `Clone`/`Copy`/`Default`以支持调用与独立测试。
- `RLIMIT_NOFILE`：传给 C ABI 的资源编号；`target_os = "macos"` 时为 `8`，其他目标为 `7`。
- `unsafe extern "C" { getrlimit, setrlimit }`：原始系统调用声明。本文件未通过第三方 libc crate 引入它们。
- `fn read_limit() -> Result<RawRLimit>`：内部读取适配器，是 `GetSystemRLimit` 和 `VerifyRLimit` 的共用下游。
- `pub fn GetSystemRLimit() -> Result<RlimT>`：公开只读 API，仅返回 `RawRLimit.current`。
- `pub(crate) fn verify_rlimit_with(...) -> Result<()>`：可注入读/写闭包的核心算法。它把决策逻辑与真实 FFI 分离，是 `local_unix_test.rs` 的直接测试面。
- `pub fn VerifyRLimit(estimateMaxFiles: RlimT) -> Result<()>`：生产入口，向 `verify_rlimit_with` 注入 `read_limit` 和调用 `setrlimit` 的写入闭包。

## 执行流程

`GetSystemRLimit` 的路径很短：初始化默认 `RawRLimit`，传可写指针给 `getrlimit`；返回值非零时取 `last_os_error`，否则取 `current` 作为结果。

`VerifyRLimit` 的核心流程在 `verify_rlimit_with` 中：

1. 以 `estimateMaxFiles.min(maxRLimit)` 得到 `requested`，因此过大输入不会请求超过 1,000,000 的限制。
2. 第一次读取 soft/hard limit。若 `current >= requested`，立即成功，不写入也不二次读取。
3. 保存原 soft limit 为 `previous`，将 `current` 设为 `requested`，并将 `maximum` 设为旧 hard limit 与 `requested` 的较大值。因此 hard limit 已足够时保持不变，不足时与 soft limit 一起请求提升。
4. 调用注入的 `set_limit`。写入失败立即返回带 `previous` 与 `requested` 的 `Error::Io`，不再执行第二次读取。
5. 写入成功后再读一次实际 soft limit。读取错误直接传播；若实际值仍小于 `requested`，返回包含 `ulimit -n <requested>` 手工修复建议的错误。
6. 只有复读值达标才返回 `Ok(())`。

生产包装 `VerifyRLimit` 将第 2/5 步接到 `getrlimit`，将第 4 步接到 `setrlimit`。

## 数据与状态

模块自身不保存长期状态。`RawRLimit` 是单次调用栈上的快照，唯一可观察的持久变化是 `setrlimit` 对当前进程资源限制的修改。`RlimT` 来自 `pkg/ingestor/ingestctrl/local_unix_generic.rs`，当前是 `u64` 别名；`RawRLimit` 字段也固定为 `u64`。

`maxRLimit` 是编译期常量，不是系统当前 hard limit。算法可以请求将 hard limit 提到 `requested`，但能否成功由 OS、进程权限和启动环境决定。二次读取是最终事实来源，不把“`setrlimit` 返回成功”单独当作已生效证据。

## 依赖与调用关系

直接依赖为 crate 内的 `local_unix_generic::RlimT`、`Error` 与 `Result`，以及操作系统 C ABI 中的 `getrlimit`/`setrlimit`。`Cargo.toml` 没有为此文件引入独立 libc 依赖或 feature。

已验证的 Rust 链路是：

`local::NewLocalBackend`
→ `local_unix::VerifyRLimit(config.max_open_files as u64)`
→ `verify_rlimit_with`
→ `read_limit` / 注入的 `setrlimit` 闭包
→ OS `getrlimit` / `setrlimit`。

`NewLocalBackend` 使用 `?` 传播失败，所以资源限制未达标时不会继续创建 `EngineManager`。`local_unix_test.rs` 作为 crate 根的 `#[cfg(test)] mod local_unix_test` 编译，直接调用 crate-visible 的 `verify_rlimit_with`。

RustCodeGraph 对精确 `callers/callees` 查询未返回这组 FFI/同名符号边；上述生产链因此又用已索引的 `local.rs` 文件节点与全仓精确搜索核对。搜索还显示 Rust Lightning 的 `lightning/pkg/server/lightning.rs` 调用的是它自身 `stubs.rs` 中的 `ingestctrl::VerifyRLimit`，不是本文件定义，不应记为本模块的直接 Rust 调用边。

## 错误处理与边界

- `getrlimit` 或 `setrlimit` 返回非零时，以 `std::io::Error::last_os_error()` 取得 errno 语义，最终转成 `Error::Io(String)`。
- 首次读取失败时不执行写入；写入失败时不执行复读；复读失败保留其原始 crate 错误。这些顺序均有 `local_unix_test.rs` 独立用例锁定。
- 请求值为 `0` 时，正常情况会因当前 soft limit 已大于等于它而直接成功；代码没有单独拒绝零。
- 大于 1,000,000 的请求会被截断，错误文本与手工命令也使用截断后的 `requested`。
- hard limit 较高时不降低；较低时则尝试与 soft limit 一起提高。非特权进程可能因提高 hard limit 被 OS 拒绝，这是显式错误，不会降级成警告后继续。
- `RawRLimit` 的 `#[repr(C)]` 与字段类型是 unsafe FFI 的核心边界。当前仅对 macOS 与“非 macOS”选择了资源编号，模块本身没有 `cfg(unix)` 门禁；扩展新平台时必须重新核对 ABI、符号可用性和常量值，不能仅假设所有非 macOS 都一致。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道，也不持有文件描述符。每次调用只在当前栈上保留 `RawRLimit` 快照，返回后即销毁。

`RLIMIT_NOFILE` 是进程级状态，因此多线程可同时观察或修改它。`verify_rlimit_with` 没有串行化“读—改—复读”序列，也没有 compare-and-swap 保证；复读只确认当时观察到的 soft limit 不低于请求值。若新增并发调用者或其他 rlimit 修改器，需评估竞争下的最终状态，不要把函数成功解读为永久保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/ingestctrl/local_unix.go`。Rust `maxRLimit`、`GetSystemRLimit` 和 `VerifyRLimit` 保留了 Go 同名符号与主要分支：截断过大请求、首次读取、充足时短路、提升过低 hard limit、设置 soft limit、复读验证，以及失效时给出 `ulimit -n` 建议。

实现机制存在三个可见差异：

1. Go 使用 `syscall.Rlimit`/`syscall.Getrlimit`/`syscall.Setrlimit`；Rust 自行定义 `#[repr(C)] RawRLimit` 并声明 C ABI。
2. Go 通过 `GetRlimitValue` 和 `SetRlimitError` failpoint 操纵生产函数；Rust 把算法抽成闭包注入的 `verify_rlimit_with`，由 `local_unix_test.rs` 以确定性 fake 覆盖相同边界。
3. Go 成功提升后记录 old/new rlimit 日志；当前 Rust 成功路径不记日志。这是可观测性差异，不改变返回值判定。

Go 上游 `lightning/pkg/server/lightning.go::checkSystemRequirement` 按数据规模、MemCache 和并发度估算需要的 fd 并调用 `VerifyRLimit`；`lightning/pkg/importer/import.go` 调用 `GetSystemRLimit` 推导每表 `maxOpenFiles`。当前本 Rust crate 的直接生产接线是 `NewLocalBackend` 使用已有 `config.max_open_files`，`GetSystemRLimit` 则无 Rust 生产调用者；因此文档不声称两边上游装配已完全一致。

## 扩展指南

- 调整阈值、hard-limit 策略、错误文本或复读规则时，修改 `verify_rlimit_with`，并同步独立测试 `pkg/ingestor/ingestctrl/local_unix_test.rs`。不要把测试内嵌到生产文件。
- 修改系统调用层时，保持 `read_limit` 和 `VerifyRLimit` 的生产注入边界，并同时核对 macOS 与其他目标的 `RLIMIT_NOFILE`、`rlimit` 布局、链接符号和权限语义。unsafe 声明或结构字段的任何变更都有 ABI 正确性风险。
- 若增加新 Unix 变体，应先决定是否用 `cfg` 拆分资源编号/结构布局，并与 `local_freebsd.rs`、`local_unix_generic.rs`、`local_windows.rs` 的平台分工一起审查，不要默认 `not(target_os = "macos")` 已证明所有平台可用。
- 若恢复 Go 等价的成功日志，应在确认复读达标后记录，并评估日志依赖、敏感度和重复调用的噪声；不要在 `setrlimit` 返回成功但复读未达标时记录为成功。
- 若要让 Rust Lightning 直接使用本 crate，需改造它当前的 `stubs.rs` 边界与 Cargo 接线；不应仅因同名 `VerifyRLimit` 就假定已连通。

兼容性风险主要是错误类别/文本与 Go 语义漂移，正确性风险主要是 FFI ABI、平台常量和并发修改资源限制，性能开销仅为常数次系统调用，通常低于后续 Engine/SST 工作。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，目标目录与目标文件已索引。
- RustCodeGraph `files --filter pkg/ingestor/ingestctrl` 与 `node --file pkg/ingestor/ingestctrl/local_unix.rs`：核对 102 行完整源码、条件编译项、FFI 声明和两个使用文件。
- RustCodeGraph `query GetSystemRLimit` / `query VerifyRLimit` / `query verify_rlimit_with`：核对同名 Go/Rust/Windows 符号与唯一内部核心函数。精确 `callers/callees` 未返回边，文档已明确保留该限制，没有把缺失图边写成已验证事实。
- RustCodeGraph 文件节点：`pkg/ingestor/ingestctrl/local_unix_test.rs`、`pkg/ingestor/ingestctrl/lib.rs`、`pkg/ingestor/ingestctrl/local.rs`、`pkg/ingestor/ingestctrl/local_unix_generic.rs`。
- crate 边界：`pkg/ingestor/ingestctrl/Cargo.toml`；Go 对照：`pkg/ingestor/ingestctrl/local_unix.go`、`lightning/pkg/server/lightning.go`、`lightning/pkg/importer/import.go`。
- Rust 独立测试 `pkg/ingestor/ingestctrl/local_unix_test.rs` 覆盖：设置失败不复读、限制已足够时短路、请求截断并提高 hard limit、保留已足够的 hard limit、复读错误传播、设置未生效时的手工修复提示。
- 人工复核结论：该文件存在是为了在 local backend 打开大量 Engine/SST 文件前建立进程级 fd 容量门禁；运行时最多执行两次读取和一次写入；安全扩展必须同步审查 FFI/平台语义、`NewLocalBackend` 调用链、Go 对照和独立 Rust 测试。
