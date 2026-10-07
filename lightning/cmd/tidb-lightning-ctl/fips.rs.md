# `lightning/cmd/tidb-lightning-ctl/fips.rs`

## 文件定位

本文件属于 `astersql-lightning-cmd-tidb-lightning-ctl` crate 的进程装配层。crate 在 [`Cargo.toml`](Cargo.toml) 中声明为 `kind = "binary"`，库入口是 [`lib.rs`](lib.rs)，可执行壳是 [`bin_main.rs`](bin_main.rs)。`lib.rs` 通过 `#[path = "fips.rs"] pub mod fips` 暴露本模块；随后 [`main.rs`](main.rs) 的 `main_with_args` 在解析配置或执行控制命令之前调用 `fips::enable_fips_only()`。

它不是 TLS 实现、加密算法实现或运行时配置中心，而是为 Go 版本的 FIPS 构建期副作用保留的 Rust 初始化接点。当前文件已经带有 AsterSQL 处理标记与原 PingCAP Apache License 注释。

## 核心职责

当前唯一职责是提供公开函数 `enable_fips_only()`，让进程启动流程显式保留“执行 FIPS-only 初始化”的阶段。该函数目前为空，因此真实行为是 no-op：不会安装加密 provider、不会修改 TLS 全局状态、不会读取环境变量，也不会验证当前二进制是否满足 FIPS 要求。

该空实现对应的是迁移期兼容接线，而不是“Rust 已启用 FIPS”的证明。源码模块注释明确说明，真正的 TLS 限制仍待专门实现承接；默认行为只与未使用 Go `boringcrypto` 构建标签的版本一致。

## 主要符号

- `pub fn enable_fips_only()`：本文件唯一的函数和唯一业务符号；签名无参数、无返回值，函数体为空。
- 本文件没有常量、结构体、枚举、trait、`impl`、静态变量或条件编译项。
- 该函数通过公开模块 `crate::fips` 可访问，但 [`lib.rs`](lib.rs) 没有把函数直接重导出到 crate 根；调用形式是 `fips::enable_fips_only()`。

RustCodeGraph 将本文件识别为 2 个图节点（文件节点与函数节点），并把直接使用文件定位到 [`main.rs`](main.rs) 和 [`parity_test.rs`](parity_test.rs)。

## 执行流程

生产启动链如下：

1. [`bin_main.rs`](bin_main.rs) 的 `fn main()` 调用库 crate 的 `main()`。
2. [`lib.rs`](lib.rs) 的 `pub fn main()` 转发到 `entry::main()`。
3. [`main.rs`](main.rs) 的 `entry::main()` 收集命令行参数并调用 `main_with_args`。
4. `main_with_args` 首先调用 `fips::enable_fips_only()`；当前调用立即返回。
5. 启动流程随后进入 `run_main(args)`，执行配置装载、PD/TiKV 或 checkpoint 控制动作，并按结果处理退出码。

因此该钩子在 Rust 版本中对每次正常进程入口调用都是无条件执行的，并且早于配置解析和网络资源创建。测试也可以直接调用 `main_with_args` 或该公开函数；当前实现重复调用仍等价于 no-op。

## 数据与状态

本文件不定义、读取或写入任何数据。`enable_fips_only()` 不接受配置，不返回状态，也没有全局变量、缓存、原子量、锁、通道或线程局部数据。

当前可确认的不变量是：调用不会改变本模块可观察状态，且正常返回。不能从这一不变量推导 TLS 已限制到 FIPS 算法集合；相反，空函数意味着本文件自身没有实施此限制。

## 依赖与调用关系

上游生产调用者是 [`main.rs`](main.rs) 的 `main_with_args`。上游测试调用者是 [`parity_test.rs`](parity_test.rs) 的 `contract_normal`，它直接调用钩子以保护公开接点。模块由 [`lib.rs`](lib.rs) 声明，进程链再由 [`bin_main.rs`](bin_main.rs) 接入。

当前函数没有下游函数调用、模块导入或外部 crate 依赖。相邻 [`Cargo.toml`](Cargo.toml) 只列出 importer 和 server 两个路径依赖，没有 FIPS feature、TLS provider 依赖或针对本模块的 target 条件。因此，Cargo 元数据也不能为实际 FIPS 能力提供证据。

RustCodeGraph 的文件关系报告为 `fips.rs` 被 `main.rs` 与 `parity_test.rs` 使用；对精确符号执行 `callers`/`callees` 未产生额外边，和空函数体以及上述直接源码调用一致。

## 错误处理与边界

函数签名为 `() -> ()`，没有失败通道，既不传播错误也不触发降级分支。当前边界行为包括：

- 任意次数调用均立即正常返回；没有参数可校验。
- 无论平台、构建模式或是否需要 FIPS，Rust 侧都执行同一个空实现，因为文件内和 Cargo 中均无条件编译开关。
- 调用成功只表示初始化接点没有 panic，不表示 FIPS provider 已安装或合规性检查已通过。

若未来初始化可能失败，继续使用无返回值签名会迫使实现 panic、静默忽略失败或在内部终止进程，均会削弱 `main_with_args` 现有的可测试退出码边界。更安全的演进方式是让钩子返回 `Result`，并在进入 `run_main` 前明确映射错误；这需要同步评估公开 API 与启动错误文案的兼容性。

## 并发与资源生命周期

当前实现不创建线程、异步任务、锁、通道、文件句柄、网络连接或 TLS 对象，因而没有清理路径和资源泄漏风险。调用发生在 `run_main` 创建 PD client 等资源之前，也早于控制命令中的并发工作。

若未来接入进程级加密 provider，初始化通常必须满足“在任何 TLS 使用之前完成”和“全进程最多成功安装一次”的约束。实现时应显式处理并发调用与重复初始化，例如使用适合的一次性初始化原语，并记录失败后的重试语义；不能假定现有空函数已经提供幂等性之外的线程安全保证。

## 与 Go 版本的对应关系

Go 对照文件是 [`fips.go`](fips.go)：它带有 `//go:build boringcrypto`，并以 `_ "crypto/tls/fipsonly"` 空白导入触发包初始化。该文件只存在于带 `boringcrypto` 标签的 Go 构建中；副作用来自标准库包初始化，而不是由 `main.go` 显式调用函数。

Rust 没有复刻这一条件构建和空白导入机制。它改为让 [`main.rs`](main.rs) 无条件显式调用 `enable_fips_only()`，但函数体为空。因此两者当前只在“保留启动阶段的意图/接点”上对应：

- 普通非-boringcrypto Go 构建与当前 Rust 在“不额外施加 FIPS TLS 限制”这一行为上相近。
- boringcrypto Go 构建会启用 `crypto/tls/fipsonly` 的真实限制，当前 Rust 不具备等价行为。

[`parity_test.rs`](parity_test.rs) 的 `contract_normal` 仅断言该入口可调用且不 panic；它没有检查允许的 cipher suite、provider 身份或非 FIPS 算法被拒绝。因此测试名称中的 parity 不应被解读为 boringcrypto 语义已完整移植。

## 扩展指南

要补齐真实 FIPS 支持，首要修改点是 `enable_fips_only()` 及 [`Cargo.toml`](Cargo.toml) 中与平台/feature 对应的 provider 依赖和构建条件；启动接线应继续位于 [`main.rs`](main.rs) 的 `main_with_args` 开头，确保配置解析和任何网络连接之前完成初始化。

扩展时应重点确认：目标平台与构建产物如何声明 FIPS 模式、provider 安装是否只能发生一次、重复调用如何处理、初始化失败如何传到进程退出边界，以及普通构建是否保持现有行为。不要仅填充函数体便宣称与 Go boringcrypto 等价；需要以实际 TLS 握手/算法拒绝行为作为证据。

测试应放在独立文件而非 `fips.rs` 内。至少同步扩展 [`parity_test.rs`](parity_test.rs) 中的 FIPS 契约，必要时新增同目录独立测试文件，并覆盖普通构建、FIPS 构建、重复初始化、并发初始化和失败传播。若修改 Cargo feature 或依赖，还要验证各支持平台的构建矩阵及生成元数据要求。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning-ctl` 找到目标文件及直接入口/测试；`query enable_fips_only --kind function` 精确定位本函数；文件节点读取确认完整 24 行源码、唯一函数签名及 `main.rs`/`parity_test.rs` 使用关系。
- Rust 源码：[`fips.rs`](fips.rs) 的模块注释和空函数体；[`lib.rs`](lib.rs) 的公开模块声明；[`main.rs`](main.rs) 的 `main_with_args` 启动顺序；[`bin_main.rs`](bin_main.rs) 的二进制转发入口。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的库/二进制目标、porting metadata 与依赖表；其中未声明 FIPS feature 或 TLS provider。
- Go 对照：[`fips.go`](fips.go) 的 `boringcrypto` 构建标签与 `crypto/tls/fipsonly` 空白导入；[`main.go`](main.go) 的真实控制命令入口。
- 测试证据：[`parity_test.rs`](parity_test.rs) 的 `contract_normal` 直接调用 `fips::enable_fips_only()`；[`main_test.rs`](main_test.rs) 和 [`main_test.go`](main_test.go) 主要验证进程入口/退出与资源清理，没有验证 FIPS 密码学行为。
- 本任务是纯文档分析，按计划不运行 Cargo；最终用任务指定的标题计数命令验证文档恰好包含 11 个固定二级章节，并人工复核未把 no-op 描述成已启用 FIPS。
