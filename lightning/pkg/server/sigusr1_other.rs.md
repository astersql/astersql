# `lightning/pkg/server/sigusr1_other.rs`

## 文件定位

[`sigusr1_other.rs`](sigusr1_other.rs) 是 `astersql-lightning-pkg-server` crate 的非 Unix 平台适配文件。crate 入口 [`lib.rs`](lib.rs) 在 `cfg(not(unix))` 下把它装配为 `sigusr1` 模块，并通过 `pub use sigusr1::*` 向 crate 内外提供与 Unix 实现同名的 API；Unix 平台则选择 [`sigusr1_unix.rs`](sigusr1_unix.rs)。因此本文件不是信号处理器的简化实现，而是“目标平台不存在 SIGUSR1 控制通道”时的显式空实现。

crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定。该 manifest 没有为此文件定义 feature；平台选择完全来自 `lib.rs` 的 `cfg(unix)` / `cfg(not(unix))`。生产代码不会在同一次构建中同时编译两种实现。

## 核心职责

本文件只承担两项职责：

1. 在非 Unix 目标上保留 `handleSigUsr1` 这一统一入口，使 [`Lightning::GoServe`](lightning.rs) 不必包含平台条件分支。
2. 明确丢弃调用方提供的回调，不注册信号、不启动线程，也不触发任何副作用。这与非 Unix Go 对照文件 [`sigusr1_other.go`](sigusr1_other.go) 的空函数语义一致。

它不负责启动 HTTP 状态服务。上层 `Lightning::GoServe` 传入的闭包只有在 Unix 信号实现收到 SIGUSR1 时才可能打开随机端口；选择本文件时，该备用启动路径不可用，后续仍会根据既有 `StatusAddr` 决定是否直接调用 `goServe`。

## 主要符号

- `pub fn handleSigUsr1<F>(_handler: F) where F: Fn()`：文件内唯一的生产符号，也是公开 API。泛型参数接受任意实现 `Fn()` 的值；参数名以下划线开头，表达有意不使用。
- 与 [`sigusr1_unix.rs`](sigusr1_unix.rs) 的同名函数相比，此签名没有 `Send + 'static` 约束。原因是空实现既不保存回调也不跨线程移动它；独立测试用借用的 `Rc<Cell<_>>` 验证了这一平台特性。
- 文件没有模块级常量、类型、trait、`impl`、静态状态或内部辅助函数。除由 `lib.rs` 施加的平台条件外，文件内部没有额外条件编译项。

## 执行流程

生产路径如下：

1. 非 Unix 构建中，`lib.rs` 选择 `sigusr1_other.rs` 并再导出 `handleSigUsr1`。
2. [`Lightning::GoServe`](lightning.rs) 调用该函数，传入一个用于延迟启动或报告 HTTP 状态服务地址的闭包。
3. `handleSigUsr1` 接收回调后直接返回；函数体为空，回调从未执行。
4. 回调值在函数返回时被丢弃。控制流返回 `GoServe`，后者继续读取配置中的 `StatusAddr`：为空则返回 `Ok(())`，非空则按正常配置启动服务。

因此，“注册成功”不是本实现的后置条件；唯一可观察结果是立即、正常返回，且回调的副作用为零。

## 数据与状态

本文件不持有任何持久状态。`_handler: F` 按值传入，只在当前栈帧中短暂拥有，随后被销毁。函数不读取或修改 `Lightning`、状态地址、服务器地址及全局信号状态，也不分配通道、文件描述符、锁或任务。

泛型边界 `F: Fn()` 只声明回调可被不可变调用；由于函数不实际调用它，这一边界主要用于保持 API 意图并与 Unix 版本的回调形状对齐。非 Unix 版本刻意不要求 `Send` 或 `'static`，所以调用者可传入带非线程安全或短生命周期借用的闭包。

## 依赖与调用关系

- 上游装配：[`lib.rs`](lib.rs) 第 40–43 行在 `cfg(not(unix))` 下选择本文件并公开再导出符号。
- 生产调用者：[`Lightning::GoServe`](lightning.rs) 调用 `handleSigUsr1`，其闭包负责在尚无状态地址时监听随机端口，否则记录已启动地址。非 Unix 实现不会进入该闭包。
- 测试调用者：[`sigusr1_other_test.rs`](sigusr1_other_test.rs) 通过 `#[path = "sigusr1_other.rs"]` 直接包含本文件，因此即使测试宿主是 Unix，也能独立验证空实现契约。
- 下游调用：函数体为空，没有被调用函数、系统调用或外部 crate 依赖。虽然 crate 的 [`Cargo.toml`](Cargo.toml) 声明了 `libc`，该依赖服务于 Unix 对应实现，本文件未引用它。

RustCodeGraph 能识别本文件中的函数、`lib.rs` 平台装配以及两个直接引用测试文件，但其按同名符号查询会同时命中 Go/Rust 和 Unix/非 Unix 四个实现，且没有可靠解析经 `pub use` 到 `Lightning::GoServe` 的精确调用边；该生产边由 `lightning.rs` 的导入和调用源码直接核验。

## 错误处理与边界

`handleSigUsr1` 没有返回值或错误类型，函数体也没有可能显式失败、阻塞或 panic 的操作。调用方不能从返回值区分平台，也不应把返回视为已注册信号监听器。

边界条件包括：

- 只适用于 Rust `cfg(not(unix))` 目标；Unix 构建中的生产模块不会选择本文件。
- 回调永远不执行，即使其捕获状态、包含潜在 panic 或具有其他副作用。
- 传入回调会立即被丢弃；其捕获资源也随闭包析构正常释放。
- Go 文件使用 `!linux && !darwin && !freebsd && !unix` 构建表达式，Rust 使用更概括的 `not(unix)`。二者目的相同，但具体目标集合由各自工具链的平台分类决定，扩展新平台时必须分别核对。

## 并发与资源生命周期

本实现没有并发行为：不生成线程，不安装操作系统信号处理器，不创建通道或 socket，也不访问锁和原子变量。函数执行期间仅拥有传入闭包，返回时结束其生命周期。

这些事实与 Unix 对应实现形成明确边界：[`sigusr1_unix.rs`](sigusr1_unix.rs) 使用全局 `OnceLock<Dispatcher>`、非阻塞 `UnixStream`、`sigaction` 和后台线程保存并调度 `Send + 'static` 回调；本文件不复用其中任何资源。维护者不应把 Unix 实现的多回调、重复通知或进程期全局资源语义推断到非 Unix 平台。

## 与 Go 版本的对应关系

直接对照 [`sigusr1_other.go`](sigusr1_other.go)：Go 的 `handleSigUsr1(handler func())` 同样为空，注释也明确说明非 Unix 平台不存在 SIGUSR1。因此 Rust 版本保持了关键行为：统一名称、接受回调、无调用、无返回错误、无副作用。

Rust 移植中的显式差异是泛型闭包类型 `F: Fn()`，而 Go 使用 `func()`。这个差异不改变运行行为，却让 Rust 非 Unix 入口无需承担 Unix 线程所需的 `Send + 'static` 限制。[`sigusr1_other_test.rs`](sigusr1_other_test.rs) 通过捕获借用的 `Rc<Cell<i32>>` 证明该差异是有意且可用的；调用后值仍为 `0`，同时证明回调没有执行。Go 目录下没有针对该空实现的独立测试文件，当前 Go 语义证据来自实现本身及 [`lightning.go`](lightning.go) 的统一调用点。

## 扩展指南

- 若只是修改 Unix 的信号分发、日志或多处理器行为，应改 [`sigusr1_unix.rs`](sigusr1_unix.rs) 及独立的 [`sigusr1_unix_test.rs`](sigusr1_unix_test.rs)，不要把 Unix 资源管理引入本文件。
- 若要为某个非 Unix 平台提供等价控制事件，应先在 `lib.rs` 增加精确的目标条件并创建独立平台实现，而不是悄悄改变所有 `cfg(not(unix))` 平台的空操作契约。
- 若改变公共回调签名，必须同步检查 `Lightning::GoServe`、Unix/非 Unix 两个实现、Go 对照语义以及两个独立 Rust 测试文件。尤其要避免无必要地给非 Unix 版本增加 `Send + 'static`，否则会破坏当前已测试的借用闭包兼容性。
- 本仓库要求 Rust 源文件与测试逻辑分离；新增回归覆盖应继续放在同目录的 `sigusr1_other_test.rs`，不应在生产文件内添加 `#[cfg(test)]` 测试模块。
- 此路径是平台兼容边界而非可选运行开关。新增功能需要同时评估：无信号平台应继续静默禁用、返回显式能力信息，还是提供另一种触发机制；该产品语义不能仅凭本文件推断。

兼容风险主要是目标平台选择和闭包 trait 约束；正确性风险是意外执行回调或让上层误判监听已启用；当前空实现没有性能热点。

## 验证依据

- RustCodeGraph `status`：本地索引包含 11,467 个文件，目标目录及目标文件已索引。
- RustCodeGraph `files --filter lightning/pkg/server`：确认 `sigusr1_other.rs`、`lib.rs`、Unix 对应实现及独立测试均位于同一 crate。
- RustCodeGraph `node --file lightning/pkg/server/sigusr1_other.rs`：核对完整 27 行源码，确认唯一函数、签名与空函数体。
- RustCodeGraph `query handleSigUsr1 --kind function --json`：确认 Go/Rust、Unix/非 Unix 四个同名平台实现；`node` 查询进一步核对 `lib.rs`、`lightning.rs`、`sigusr1_unix.rs` 和相关测试。
- 源码与配置复核：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`lightning.rs`](lightning.rs)、[`sigusr1_other.go`](sigusr1_other.go)、[`sigusr1_unix.go`](sigusr1_unix.go)、[`sigusr1_unix.rs`](sigusr1_unix.rs)。目录中没有 `doc.go`，故以 crate 入口 `lib.rs` 的模块文档作为最近的包级契约。
- 测试复核：[`sigusr1_other_test.rs`](sigusr1_other_test.rs) 验证非 `Send` 借用回调被接受且不执行；[`sigusr1_unix_test.rs`](sigusr1_unix_test.rs) 仅作为平台差异证据，验证 Unix 实现会在 SIGUSR1 到来时通知所有已注册处理器。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；完成判定依赖源码事实审阅及固定章节结构验证。
