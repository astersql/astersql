# `lightning/cmd/tidb-lightning/fips.rs`

源文件：[`fips.rs`](./fips.rs)

## 文件定位

该文件属于 `astersql-lightning-cmd-tidb-lightning` crate，是 Lightning 命令行进程的 FIPS/TLS 初始化接线点。`lib.rs` 通过 `#[path = "fips.rs"] pub mod fips` 将它公开为 crate 模块；`main.rs::main` 在解析命令行参数和执行导入流程之前调用这里的唯一函数。crate 的二进制入口 `bin_main.rs::main` 只转发到库入口，因此生产启动链为 `bin_main.rs::main -> lib.rs::main -> main.rs::main -> fips::init_fips_only_tls_for_boringcrypto_build`。

`Cargo.toml` 将这个 crate 标记为 `kind = "binary"`，并声明库入口 `lib.rs` 与二进制入口 `bin_main.rs`。当前依赖只有 Lightning 的 `progress` 和 `server` 两个本地 crate，没有 FIPS TLS 提供方依赖，也没有声明与 `boringcrypto` 对应的 Cargo feature；因此本文件当前是迁移锚点，不是可工作的 FIPS 实现。

## 核心职责

当前职责只有两项：

1. 为 Rust 进程保留一个稳定、可无条件调用的 FIPS 初始化位置，使入口层无需在以后接入 TLS 提供方时改变启动顺序。
2. 记录它与 Go 条件编译文件 `fips.go` 的语义关系，并明确 Rust 版本目前只对应 Go 的非 `boringcrypto` 构建行为。

函数体有意为空，不会启用 FIPS 模式、注册 TLS provider、改变进程全局策略或验证运行环境。源码注释所说的 “future cargo feature” 是扩展方向，不代表仓库已经支持该 feature。

## 主要符号

- `pub fn init_fips_only_tls_for_boringcrypto_build()`：文件内唯一的生产符号，也是公开 API。它不接收参数、不返回结果，函数体为空。公开可见性使 `main.rs` 和 crate 内的 parity 测试都能通过 `crate::fips` 调用它。
- 本文件没有常量、类型、trait、`impl`、静态变量或条件编译属性。唯一与条件编译有关的内容是模块注释对 Go `//go:build boringcrypto` 的说明；Rust 源码本身在当前 crate 构建中始终被 `lib.rs` 挂载。

## 执行流程

1. `bin_main.rs::main` 调用 `astersql_lightning_cmd_tidb_lightning::main()`。
2. `lib.rs::main` 转发到 `entry::main()`，其中 `entry` 对应 `main.rs`。
3. `main.rs::main` 首先调用 `fips::init_fips_only_tls_for_boringcrypto_build()`；这发生在读取 `std::env::args()` 和进入 `run(...)` 之前。
4. 当前函数立即返回且没有副作用，随后入口才解析参数、构造 Lightning 应用并选择单次导入或服务模式。
5. `parity_test.rs::contract_normal_run_once_success` 也在运行可注入的成功路径前直接调用该函数，用于保证入口始终可调用，并保留未来接线时的顺序契约。

因此，这个函数位于进程初始化的最前端，但目前不会改变后续控制流。它也不会被 `run(...)` 或 `run_with_factory(...)` 间接调用；绕过 `main.rs::main` 直接测试这些主体函数时，测试需要自行决定是否调用该初始化挂点。

## 数据与状态

该模块不定义、读取或写入任何业务数据与进程状态。函数没有参数、返回值或局部变量，也不访问环境变量、配置、文件、网络、TLS 会话或全局单例。

未来若在此注册进程级 TLS provider，状态模型会从“纯空操作”变为“一次性全局初始化”。届时需要明确重复调用策略、初始化完成标记以及 provider 是否必须先于任何 TLS 客户端或服务端创建；这些约束在当前代码中都不存在。

## 依赖与调用关系

上游直接关系：

- `lib.rs` 公开声明 `fips` 模块。
- `main.rs::main` 是生产调用者，并保证调用发生在 CLI 主体之前。
- `parity_test.rs::contract_normal_run_once_success` 是测试调用者，验证空入口可调用且不阻断正常单次运行场景。
- `bin_main.rs::main` 与 `lib.rs::main` 是生产调用链上的间接上游。

下游关系：

- 当前函数没有函数调用、导入项或 crate 依赖，RustCodeGraph 因而没有可追踪的下游调用边。
- `Cargo.toml` 没有 FIPS 专用依赖或 feature。`progress`、`server` 依赖服务于同 crate 的其他入口逻辑，不是本文件的直接依赖。
- Go 对照文件 `fips.go` 的下游是空白导入 `crypto/tls/fipsonly`，其初始化副作用由 Go 包加载机制触发；Rust 端尚无等价依赖。

RustCodeGraph 对精确符号给出的直接调用轨迹是 `main`（`main.rs:77`）和 `contract_normal_run_once_success`（`parity_test.rs:164`），与源码引用搜索一致。

## 错误处理与边界

当前函数不返回 `Result`、不会 panic，也没有任何可失败操作，因此不存在本地错误传播或恢复分支。它的关键边界反而是能力边界：名称提到 `boringcrypto`，不表示 Rust 二进制已经启用 BoringCrypto 或满足 FIPS 要求；当前实现对所有 Rust 构建都是同一个空操作。

若未来初始化 API 可能失败，不能静默丢弃错误。需要先决定进程安全策略：在 CLI 解析前失败并终止，或把错误显式返回给 `main.rs::main` 转换为非零退出码。同时必须避免出现“函数返回成功但实际 provider 未注册”的降级路径，因为这会让名称承诺与运行状态不一致。

## 并发与资源生命周期

当前实现不创建线程、任务、通道、锁、文件描述符、网络连接或堆资源，也没有清理阶段。生产入口在启动信号处理线程、HTTP 服务和导入任务之前同步调用它，所以现有顺序天然适合作为进程级安全 provider 的早期初始化点。

未来若接入全局 provider，应把初始化设计为进程级、线程安全且可判定重复调用的操作。`parity_test.rs` 会直接调用该函数，而测试进程内可能执行多个场景，因此新增实现不能在重复调用时产生竞态或不受控的全局污染；必要时应采用明确的一次性初始化原语，并为测试隔离策略单独建模。

## 与 Go 版本的对应关系

Go 文件 `fips.go` 带有 `//go:build boringcrypto`，只在该构建标签成立时进入 `package main`，并以 `_ "crypto/tls/fipsonly"` 空白导入依赖其包初始化副作用。Go 的普通构建不会编译这个文件，`main.go::main` 也没有显式 FIPS 初始化调用。

Rust 没有复制这种条件编译：`lib.rs` 始终挂载 `fips.rs`，`main.rs::main` 始终显式调用函数，而函数始终为空。因此当前可观察行为只与 Go 的非 `boringcrypto` 构建相近；它保留了一个未来承载 Go 空白导入副作用的显式位置，但尚未实现 Go `boringcrypto` 分支的安全能力。`parity_test.rs` 只证明该挂点可调用并与正常启动流程兼容，不证明 TLS 算法限制、provider 注册或 FIPS 合规性。

## 扩展指南

若要真正支持 FIPS 构建，最可能需要同步修改以下位置：

1. 在 `Cargo.toml` 中增加明确、可审计的 feature 与已发布的 TLS/provider 依赖，并定义支持的平台和构建组合。
2. 在 `fips.rs::init_fips_only_tls_for_boringcrypto_build` 中实现 provider 初始化、重复调用规则和错误语义；不要把初始化散落到 `main.rs` 的后续业务流程。
3. 在 `main.rs::main` 中根据函数的新返回类型处理启动失败，同时保持它在参数解析、网络和并发资源创建之前执行。
4. 在独立测试文件中增加测试，不要把测试写进 `fips.rs`。可扩展现有 `parity_test.rs` 验证调用顺序与失败传播，并为 feature 启用/禁用组合增加构建或集成测试，验证真实 provider 状态而非只验证函数可调用。
5. 对照 `fips.go` 和目标 Go 工具链的 `boringcrypto` 行为，记录 Rust 与 Go 在算法限制、证书验证、客户端/服务端覆盖范围和启动失败策略上的兼容差异。

主要风险是错误地把“存在初始化函数”当成“已经合规”；其次是全局 provider 被其他 TLS 对象抢先初始化、重复初始化竞态，以及 feature 组合造成普通构建行为变化。初始化本身应只执行一次，性能影响应集中在启动阶段，不能进入每次请求或每个连接的热路径。

## 验证依据

- `lightning/cmd/tidb-lightning/fips.rs`：确认唯一公开函数、空函数体、无条件编译项及迁移说明。
- `lightning/cmd/tidb-lightning/lib.rs`：确认 `pub mod fips`、入口模块装配和测试模块位置。
- `lightning/cmd/tidb-lightning/main.rs`：确认生产调用位于 `main` 第一项操作，之后才读取参数并进入 `run`。
- `lightning/cmd/tidb-lightning/bin_main.rs`：确认二进制入口只转发到库入口。
- `lightning/cmd/tidb-lightning/Cargo.toml`：确认 crate 边界、二进制性质、入口文件以及当前没有 FIPS feature/依赖。
- `lightning/cmd/tidb-lightning/fips.go` 与 `main.go`：确认 Go 仅通过 `boringcrypto` 条件文件空白导入 `crypto/tls/fipsonly`，普通 `main` 无显式调用。
- `lightning/cmd/tidb-lightning/parity_test.rs::contract_normal_run_once_success`：确认现有独立 Rust 测试只覆盖初始化挂点可调用和正常运行场景兼容性。
- RustCodeGraph：索引状态为 7,032 个 Rust 文件；精确查询定位函数于 `fips.rs:25`，`node` 调用轨迹列出生产调用者 `main.rs::main` 和测试调用者 `parity_test.rs::contract_normal_run_once_success`。独立 `callers/callees` 查询在 30 秒内未返回输出，因此调用边另以 `node` 轨迹和源码引用搜索交叉核验。
- 引用搜索：`rg` 只在 `main.rs`、`parity_test.rs` 和目标定义中发现 Rust 函数名；同目录没有同名独立测试文件，相关测试位于 `parity_test.rs`，`main_test.rs` 仅说明其 `run` 路径位于 FIPS 初始化之后。

本任务是纯文档分析，未运行 Cargo。结构检查应确认本文存在，并且固定的十一个二级标题各出现一次。
