# `br/cmd/br/fips.rs`

## 文件定位

[`fips.rs`](./fips.rs) 属于 `astersql-br-cmd-br` crate。[`lib.rs`](./lib.rs) 通过 `pub mod fips` 将它公开为 `astersql_br_cmd_br::fips`；该 crate 同时由 [`bin_main.rs`](./bin_main.rs) 包装成 `astersql-br-cmd-br` 二进制。该文件不参与命令树组装，而是 Go `boringcrypto` 构建语义在 Rust 默认构建中的最小能力查询适配层。

## 核心职责

本文件只回答一个问题：“当前 Rust BR 构建是否强制 FIPS-only TLS 语义？”。当前答案固定为否。它把“默认 Rust 构建未启用 FIPS-only”表达成可测试的显式契约，但不安装密码学 provider、不修改 TLS 配置，也不能证明运行时环境已获得 FIPS 认证。

## 主要符号

- `pub fn fips_only_enabled() -> bool`：文件中唯一的项，也是公开 API。它无参数、无副作用，并始终返回 `false`。
- 本文件没有常量、类型、trait、`impl`、宏或条件编译项。条件构建只出现在 Go 对照文件 [`fips.go`](./fips.go) 的 `//go:build boringcrypto` 中，而非当前 Rust 源文件中。

## 执行流程

1. 调用者调用 `fips_only_enabled()`。
2. 函数不读取参数、环境变量、feature 或全局状态。
3. 函数直接返回 `false`，表示该默认构建没有强制 FIPS-only TLS。

当前生产入口 [`main.rs`](./main.rs) 未调用该函数，因此启动 BR、组装子命令和执行备份恢复路径都不会经过本文件。仓库内的直接调用只出现在 [`parity_test.rs`](./parity_test.rs) 的 `contract_normal_command_tree_and_filters()` 中。

## 数据与状态

输入集为空，输出是一个布尔值。函数不保存或缓存状态，不访问命令上下文、文件系统、网络、证书或密钥。其不变量是：对当前代码的任意一次调用，结果都是 `false`，且多次调用不会改变进程状态。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](./Cargo.toml) 定义包 `astersql-br-cmd-br`、库入口 `lib.rs` 和二进制入口 `bin_main.rs`。Cargo 声明中没有 FIPS 专用 feature、TLS provider 依赖或针对本文件的条件编译。
- 上游：`lib.rs` 公开 `fips` 模块；直接代码搜索和 RustCodeGraph blast radius 均只找到 `parity_test.rs::contract_normal_command_tree_and_filters` 调用 `fips_only_enabled()`。
- 下游：`fips_only_enabled()` 的函数体仅包含布尔字面量，不调用仓库内或外部函数。
- 应用主链：`bin_main.rs::main` 转发到 `lib.rs::main`，再转发到 `main.rs::main`；这条链没有连到 `fips` 模块。因而本文件当前是可公开查询、可契约测试，但未实施进程级 FIPS 初始化的适配层。

## 错误处理与边界

函数不返回 `Result`，不会主动产生错误或 panic。重要边界是，`false` 只描述这个 Rust 适配层的当前构建契约，不检测操作系统、OpenSSL/rustls 实现、远端端点或证书是否符合 FIPS。如果上层把该函数当成完整的合规性验证，将超出其保证范围。

## 并发与资源生命周期

本文件不创建线程或异步任务，不使用锁、原子量、通道、上下文或事务，也不持有需要释放的资源。纯布尔返回使它能被多线程同时调用，各次调用之间没有顺序或可见性要求。

## 与 Go 版本的对应关系

Go 对照文件 [`fips.go`](./fips.go) 只在 `boringcrypto` 构建标签成立时参与编译，并以空白导入 `_ "crypto/tls/fipsonly"` 触发 Go TLS 的 FIPS-only 初始化副作用。Rust 文件没有复制该副作用；它只对齐未选中 Go `boringcrypto` 文件时的默认能力边界，即返回 `false`。

因此这是部分语义对齐，而不是 Go FIPS 构建路径的完整移植。对齐测试 [`parity_test.rs`](./parity_test.rs) 明确断言 `!fips_only_enabled()`，保护的是默认非 FIPS-only 契约；仓库搜索未发现 BR 中针对 FIPS 启用路径的独立 Rust 测试。

## 扩展指南

- 若要支持真正的 FIPS 构建，应先在 Cargo 和进程入口层明确构建开关与失败策略，再修改或替换 `fips_only_enabled()`；仅把返回值改为 `true` 不会安装经验证的 TLS provider，不能作为实现。
- 若引入构建时分支，要同步检查 `Cargo.toml`、`lib.rs` 和 `main.rs`，确保能力报告与实际初始化路径一致，并且在请求 FIPS 但 provider 不可用时避免静默降级。
- 测试逻辑应继续位于独立文件：默认构建契约可扩展 `parity_test.rs`；如果新增可测的 FIPS 初始化分支，应在同目录新增或扩展独立 `*_test.rs` 文件，覆盖默认路径、启用成功和不可用时的失败路径，不把测试内嵌进 `fips.rs`。
- 兼容风险主要是错报能力导致合规性判断偏差；性能风险不在当前常量返回路径中，而在未来 TLS provider 选择和初始化实现中。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件；`files --filter br/cmd/br` 确认目标文件、Go 对照文件和独立测试都在索引中。
- RustCodeGraph `explore "br/cmd/br/fips.rs FipsCheckVerify FIPS"`：返回 `fips.rs` 全文，并给出 `fips_only_enabled -> parity_test.rs::contract_normal_command_tree_and_filters` 的唯一 blast-radius 调用关系。
- RustCodeGraph `query fips_only_enabled --kind function --limit 10 --json` 和 `node --file br/cmd/br/fips.rs --offset 1 --limit 80`：确认签名、公开性、实现和文件总长度。单独的 `callers/callees` 命令在 30 秒内未返回文本，因此调用关系又用 `rg` 直接引用搜索交叉验证。
- 已读源与配置：`br/cmd/br/fips.rs`、`br/cmd/br/lib.rs`、`br/cmd/br/main.rs`、`br/cmd/br/bin_main.rs`、`br/cmd/br/Cargo.toml`。目录中不存在 `doc.go`。
- 已读对照与测试：`br/cmd/br/fips.go`、`br/cmd/br/parity_test.rs`；`rg` 未找到其他 BR Rust/Go 测试对 FIPS 的引用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的 11 章结构命令验证文档。
