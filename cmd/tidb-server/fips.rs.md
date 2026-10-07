# `cmd/tidb-server/fips.rs`

## 文件定位

[`fips.rs`](./fips.rs) 是 `astersql-cmd-tidb-server` crate 的进程启动安全钩子，位于命令入口与 `astersql-server` 的 TLS 实现之间。它由 [`lib.rs`](./lib.rs) 通过 `#[path = "fips.rs"] pub mod fips` 纳入库 crate；共享入口 `lib.rs::main` 先调用 `fips::enable_fips_only()`，再进入 `entry::main()`。二进制壳 [`bin_main.rs`](./bin_main.rs) 在启动分配器分析后调用这个共享入口，因此正常可执行文件会在配置解析、网络监听和后台任务启动之前执行 FIPS 检查。

[`Cargo.toml`](./Cargo.toml) 将该包声明为同时具有 `lib.rs` 库入口和 `bin_main.rs` 二进制入口的 `astersql-cmd-tidb-server` 包，并通过路径依赖 `astersql-server = { path = "../../pkg/server" }` 获得实际 crypto provider 安装能力。本文件不是独立二进制、TLS 协议实现或运行时开关解析器；它只负责把构建选择转换为启动期动作。

## 核心职责

本文件承担三项聚焦职责：

1. 用 `option_env!("ASTERSQL_FIPS_ONLY")` 在**编译期**判断该二进制是否请求 FIPS-only 启动。只判断变量是否存在，不解析其字符串值；因此即使值为空或为 `0`，只要构建环境定义了该变量，`requested` 就是 `true`。
2. 普通构建走无副作用的成功路径，使行为对应未选中 Go `boringcrypto` 构建标签的情形。
3. FIPS 构建把 provider 验证和全局安装委托给 `astersql_server::server::install_fips_crypto_provider`，并将任何错误提升为启动期 panic，确保请求 FIPS 的程序不会静默退回普通 provider。

它不自行选择密码套件，也不验证证书、创建 TLS listener 或维护 FIPS 状态；这些职责不属于该薄适配层。

## 主要符号

- `pub fn enable_fips_only()`：生产入口。它读取编译期标记，调用可测试的策略函数，并用 `unwrap_or_else` 将 `Err(String)` 转为带有 `FIPS-only initialization failed:` 前缀的 panic。函数没有返回值，成功后启动链继续执行。
- `pub fn enable_fips_only_for_build(requested: bool) -> Result<(), String>`：显式策略入口，也是独立回归测试使用的 seam。`requested == false` 时直接返回 `Ok(())`；`true` 时原样返回 `install_fips_crypto_provider()` 的结果。
- `ASTERSQL_FIPS_ONLY`：不是 Rust 常量或 Cargo feature，而是 `option_env!` 在编译时读取的环境变量名。仓库搜索未发现本文件之外的设置点或 Cargo feature 声明，因此如何在发布构建中注入它不由本文件定义。

文件没有模块级可变状态、类型、trait、`impl`、异步函数或条件编译属性。两个函数均为公开 API，但当前生产调用者是同 crate 的 `lib.rs::main`；`enable_fips_only_for_build` 还被 `parity_test.rs` 直接调用。

## 执行流程

完整启动链如下：

1. `bin_main.rs::main` 启动 `rpprof`，然后调用 `astersql_cmd_tidb_server::main()`。
2. `lib.rs::main` 首先调用 `fips::enable_fips_only()`；只有该调用正常返回，才调用 `entry::main()` 进入 TiDB Server 的配置、存储、Domain 和服务初始化。
3. `enable_fips_only` 将 `option_env!("ASTERSQL_FIPS_ONLY").is_some()` 作为 `requested` 传给 `enable_fips_only_for_build`。
4. 未请求 FIPS 时，策略函数立即返回 `Ok(())`，不访问 provider。
5. 请求 FIPS 时，策略函数调用 [`pkg/server/server.rs`](../../pkg/server/server.rs) 中的 `install_fips_crypto_provider()`。该函数取得 `rustls::crypto::aws_lc_rs::default_provider()`，先检查 `provider.fips()`，再尝试把它安装为进程默认 provider。
6. provider 已通过验证且成功安装时，启动继续；provider 不是 FIPS 构建或已有不同的全局 provider 时，错误回到 `enable_fips_only` 并触发 panic，`entry::main()` 不会执行。

RustCodeGraph 对目标文件显示 `enable_fips_only → enable_fips_only_for_build` 的直接调用边；跨 crate 的 provider 边未由图命令展开，但由目标函数源码中的全限定调用及 `cmd/tidb-server/Cargo.toml` 的路径依赖共同核实。

## 数据与状态

本文件唯一输入是构建时“环境变量是否存在”这一布尔事实，以及测试可直接传入的 `requested: bool`。唯一显式输出是 `Result<(), String>` 或生产入口的正常返回/panic；没有堆积业务数据，也没有读写配置文件、命令行参数或数据库状态。

全局状态变更发生在下游 rustls provider 注册表：`install_default()` 尝试设置进程级默认 crypto provider。这一状态不由本文件持有，且接口没有撤销句柄。`requested == false` 路径完全跳过该全局写入。`option_env!` 的结果编译进产物，程序运行后修改进程环境不会改变分支。

## 依赖与调用关系

上游关系：

- `bin_main.rs::main` 调用 `astersql_cmd_tidb_server::main`。
- `lib.rs::main` 在 `entry::main` 之前调用 `fips::enable_fips_only`，固定了安全初始化早于服务初始化的顺序。
- `parity_test.rs::contract_normal_paths` 直接调用两个公开函数以覆盖默认路径和显式请求路径。

下游关系：

- `enable_fips_only` 调用本模块的 `enable_fips_only_for_build`。
- `enable_fips_only_for_build(true)` 调用 `astersql_server::server::install_fips_crypto_provider`。
- provider 实现依赖 `rustls 0.23` 的 `crypto::aws_lc_rs::default_provider`、`CryptoProvider::fips` 和 `install_default`；`pkg/server/Cargo.toml` 声明 `rustls = "0.23"`。

该文件不参与每次 TLS 握手。它设置的是后续 rustls 配置所依赖的进程默认 provider；SQL TLS 的配置构造等逻辑在 `pkg/server/server.rs` 中继续完成。

## 错误处理与边界

`enable_fips_only_for_build(false)` 无条件成功，这是默认构建最重要的兼容边界。`true` 路径保留下游错误字符串，目前可观察到两类失败：provider 的 `fips()` 为假时返回 `the linked rustls AWS-LC provider is not FIPS validated`；`install_default()` 失败时返回 `a different rustls crypto provider is already installed`。

生产入口不把错误交给后续启动代码处理，而是 panic，消息包含统一前缀和下游原因。这是 fail-closed 设计：请求 FIPS 时，缺失验证或 provider 初始化顺序冲突都禁止服务器继续启动。相应地，调用方必须把它放在任何可能抢先安装默认 provider 的逻辑之前；当前 `lib.rs::main` 满足这个约束。

边界上需特别注意：环境变量只看“是否定义”，不存在 `true`/`false` 文本解析；本文件也没有平台检测、运行时切换或重试。重复安装是否成功由 rustls 的全局安装规则决定，不应把该函数当作可任意重复调用的幂等初始化器。

## 并发与资源生命周期

两个本地函数都是同步函数，不创建线程、异步任务、锁、通道、文件或网络资源。其关键生命周期是进程级 crypto provider：FIPS 请求路径在单线程式启动序列的最前段安装一次，安装结果供随后创建的 TLS 配置使用，并持续到进程结束；本文件没有清理阶段，也不支持运行中替换。

由于默认 provider 是全局状态，潜在竞争来自其他启动代码并发或更早调用 rustls provider 安装。下游用 `install_default()` 的失败结果拒绝覆盖已有 provider，从而避免无声替换；本文件再将此失败升级为 panic。安全扩展时应继续维持“先初始化 provider，再启动可能使用 TLS 的组件”的时序，不能将调用下沉到工作线程或首次连接路径。

## 与 Go 版本的对应关系

同目录 [`fips.go`](./fips.go) 只在 `//go:build boringcrypto` 条件成立时参与编译，并通过匿名导入 `_ "crypto/tls/fipsonly"` 利用包初始化副作用强制 TLS 的 FIPS-only 行为。没有该构建标签时，这个 Go 文件完全不进入构建。

Rust 版本保留了“由构建选择、在进程主逻辑之前生效”的意图，但机制并非逐语句翻译：它始终编译 `fips` 模块，以 `ASTERSQL_FIPS_ONLY` 是否在编译时存在模拟构建选择，并显式验证/安装 rustls AWS-LC provider。相比 Go 的匿名导入，Rust 路径公开返回错误的策略函数，生产入口则 panic；这样既可测试普通与 FIPS 请求分支，也明确提供了 fail-closed 失败行为。

现有证据不能证明发布流水线已经设置 `ASTERSQL_FIPS_ONLY` 或链接了经验证的 AWS-LC FIPS 构建；仓库内搜索没有找到对应注入点。因此文档只确认代码在标记存在时的行为，不宣称当前产物已获得 FIPS 认证。

## 扩展指南

- 若改变构建选择规则，应修改 `enable_fips_only`，并明确保持它是编译期还是运行时决策；不要把字符串值解析悄悄加入当前“存在即启用”的契约。
- 若增加 provider 选择、诊断信息或错误分类，优先在 `pkg/server/server.rs::install_fips_crypto_provider` 实现真实 provider 逻辑，本文件继续保持入口适配；同时检查 `pkg/server/Cargo.toml` 的 rustls feature/依赖是否与发布构建一致。
- 若改变默认路径、失败策略或重复安装语义，应同步独立测试 [`parity_test.rs`](./parity_test.rs) 中 `contract_normal_paths` 的 FIPS 断言。Rust 单元测试不应内嵌回 `fips.rs`，符合仓库“源文件与测试文件分离”的约束。
- 若需要更强回归，建议在独立测试中分别验证：未请求时绝不触碰 provider；请求但 provider 非 FIPS 时保留精确失败；生产入口失败时不进入 `entry::main`。全局 provider 一旦安装不可由本文件复原，测试需用进程隔离或一次性初始化避免顺序污染。
- 若调整入口组装，必须保持 `lib.rs::main` 中 FIPS 初始化先于 `entry::main`，并同步检查 `bin_main.rs`，避免库模式与二进制模式分叉。

兼容风险集中在构建标记语义和启动失败行为；安全风险集中在误用非 FIPS provider或初始化过晚；性能影响仅限进程启动时一次 provider 检查/安装，本文件不在请求热路径上。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter cmd/tidb-server` 确认目标文件、入口文件和独立测试均已索引。
- RustCodeGraph `node --file cmd/tidb-server/fips.rs --offset 1 --limit 200`：确认完整 35 行源码、两个公开函数、编译期标记与下游全限定调用。
- RustCodeGraph `query enable_fips_only --kind function`、`query enable_fips_only_for_build --kind function` 以及对应 `callers`/`callees`：确认目标符号和模块内调用边；图未展开的 crate 入口与跨 crate 边由源码和 Cargo 交叉核验。
- `cmd/tidb-server/lib.rs`、`bin_main.rs`、`Cargo.toml`：确认模块组装、启动先后顺序、库/二进制边界及 `astersql-server` 依赖。
- `pkg/server/server.rs::install_fips_crypto_provider` 与 `pkg/server/Cargo.toml`：确认 FIPS 检查、默认 provider 安装、两类错误及 rustls 版本声明。
- `cmd/tidb-server/fips.go`：确认 Go `boringcrypto` 构建约束和 `crypto/tls/fipsonly` 匿名导入语义。
- `cmd/tidb-server/parity_test.rs::contract_normal_paths`：确认独立 Rust 回归覆盖普通构建安全调用、`requested == false` 成功及显式请求可能返回“未验证/已安装”错误。仓库搜索未发现同名独立测试或其他 `cmd/tidb-server` FIPS 测试。

本任务是纯文档分析，按计划不运行 Cargo。最终结构检查应确认该文件存在且恰有上述十一个固定二级章节；人工复核重点是调用顺序、编译期/运行时边界、fail-closed 错误和未验证的发布构建声明。
