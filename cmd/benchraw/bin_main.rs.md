# `cmd/benchraw/bin_main.rs`

## 文件定位

源文件：[bin_main.rs](bin_main.rs)。它是 Cargo 包 `astersql-cmd-benchraw` 的二进制入口文件。`cmd/benchraw/Cargo.toml` 的 `[[bin]]` 将二进制 `astersql-cmd-benchraw` 明确映射到该文件，同时 `[lib]` 将同一包的库入口映射到 `cmd/benchraw/lib.rs`。因此它位于操作系统启动进程与可复用库逻辑之间，只承担入口适配，不是 RawKV 压测的实现文件。

该文件不是门面式 re-export，也不是未接线桩：它定义了 Cargo 二进制所需的私有 `fn main()`，并立即调用同包库 crate 的 `astersql_cmd_benchraw::main()`。真实命令逻辑继续位于 `cmd/benchraw/lib.rs` 暴露的 `entry` 模块，即 `cmd/benchraw/main.rs`。

## 核心职责

本文件只有一个职责：把 Cargo/进程入口转发到共享库入口。这个分层避免二进制包装层复制参数解析、客户端创建、并发写入或输出逻辑，也使 `cmd/benchraw/parity_test.rs` 能直接通过库模块测试命令行为，而无须启动子进程。

它刻意不负责参数校验、日志初始化、Tokio runtime、错误转换或退出码映射。所有可观察业务行为均由 `astersql_cmd_benchraw::main()` 及其下游实现决定；本文件存在的价值是满足 Rust 可执行目标要求，并稳定库/二进制边界。

## 主要符号

- `fn main()`（`cmd/benchraw/bin_main.rs:8`）：文件内唯一符号，也是唯一的进程入口。它没有参数、没有返回值、不是 `pub`，函数体只调用 `astersql_cmd_benchraw::main()`。
- `astersql_cmd_benchraw::main()`（定义于 `cmd/benchraw/lib.rs:31`）：包的公开共享入口，继续转发到 `entry::main()`。
- `entry::main()`（定义于 `cmd/benchraw/main.rs:38`）：读取进程参数、解析 flags，并以生产客户端工厂和启用 pprof 的配置调用 `run_with_flags`。

本文件没有模块级常量、类型、trait、`impl`、条件编译项或公开 API。`Cargo.toml` 也没有为该二进制声明额外 feature；其依赖来自同包库目标。

## 执行流程

1. Cargo 根据 `cmd/benchraw/Cargo.toml` 的 `[[bin]] path = "bin_main.rs"` 构建并以本文件的 `main` 作为进程入口。
2. `bin_main.rs::main` 同步调用库 crate 的 `astersql_cmd_benchraw::main()`。
3. `cmd/benchraw/lib.rs::main` 同步调用 `entry::main()`；`entry` 通过 `#[path = "main.rs"]` 指向 `cmd/benchraw/main.rs`。
4. `entry::main` 从环境取得参数，调用 `stubs::parse_flags`；帮助请求返回 `None` 时正常返回，否则进入 `run_with_flags(flags, default_client_factory(), true)`。
5. `run_with_flags` 设置 Warn 日志级别，按需启动 `:9191` pprof 后台线程，创建指定大小的 value，然后调用 `batch_raw_put`。
6. `batch_raw_put` 创建共享 RawKV 客户端，按 worker 切分连续 key 区间并启动线程；所有 worker join 后，`run_with_flags` 输出耗时和请求总数，控制流逐层返回到本文件并结束进程。

其中第 4 至 6 步是下游行为说明，不在本文件内实现；对应源码分别是 `cmd/benchraw/main.rs:38`、`:50` 和 `:79`。

## 数据与状态

本文件不定义、持有或转换任何数据。它没有静态变量、堆分配、缓存、命令参数对象或可变状态；调用时也不向库入口传参。

下游状态由 `cmd/benchraw/main.rs` 和 `cmd/benchraw/stubs.rs` 管理：`Flags` 承载数据量、worker 数、PD 地址、value 大小及 TLS 路径；RawKV 客户端通过 `Arc` 在 worker 间共享；每个 worker 拥有 value 副本。上述状态不会在本包装层形成第二份表示，因此新增 flag 或运行状态通常不应修改 `bin_main.rs`。

## 依赖与调用关系

直接依赖只有同包库 crate `astersql_cmd_benchraw`。Rust 将 Cargo 包名中的连字符转换为 crate 路径中的下划线，因此 `astersql-cmd-benchraw` 对应源码中的 `astersql_cmd_benchraw`。

调用链为：

`bin_main.rs::main` → `lib.rs::main` → `main.rs::entry::main` → `run_with_flags` → `batch_raw_put`。

RustCodeGraph 对 `run_with_flags` 记录了到 `batch_raw_put` 和 `stubs::listen_and_serve` 的被调边，并记录 `entry::main` 是其调用者；对 `batch_raw_put` 记录了 `stubs::split_pd_addrs`、`stubs::fatal_put_failed` 等下游边，以及 `run_with_flags` 和多个 parity 契约函数的调用边。索引对本文件这一级极薄的跨 crate 转发未生成边，因此该直接边以 `bin_main.rs:9`、`lib.rs:31-32` 和 Cargo 目标声明共同核验。

`Cargo.toml` 的运行依赖包括 `tikv-client`、`tiny_http`、`pprof`、`tokio` 和 `log`，但本文件不直接引用它们；它们由共享库的 `entry`/`stubs` 实现使用。修改这些依赖不应把初始化逻辑复制进本文件。

## 错误处理与边界

本文件没有 `Result` 返回、`match`、恢复逻辑或显式退出码处理。下游正常返回时进程正常结束；下游 panic 时这里不捕获，panic 会越过入口终止进程。这一透明传播是当前包装层的重要边界。

具体错误策略位于下游：参数帮助会使 `entry::main` 提前返回；非法参数、负 value 大小、零 worker、建连失败和 Put 失败走快速失败路径；pprof 监听失败则由后台路径记录而不阻塞压测。`cmd/benchraw/parity_test.rs` 分别验证这些契约。不要在本文件添加统一吞错或静默默认值，否则会改变现有可观察失败行为，并绕过库级测试覆盖的入口。

## 并发与资源生命周期

本文件自身不创建线程、runtime、锁、通道、事务、文件句柄或网络资源。其同步调用会一直占用主线程，直到共享库入口完成或异常终止。

下游 `run_with_flags` 会分离一个 pprof 服务线程；该线程不在主流程结束前 join。`batch_raw_put` 为每个 worker 创建线程，在线程间用 `Arc` 共享客户端，并在返回前 join 全部 worker。RawKV 客户端、value 副本和 worker handle 都由下游作用域管理；本文件不介入清理。`contract_resource_cleanup_and_pprof` 证明 worker 在入口返回前完成，同时 pprof 启动错误只进入日志通道。

## 与 Go 版本的对应关系

Go 对照文件 `cmd/benchraw/main.go` 将进程入口和实现都放在同一个 `package main` 文件中：其 `main()` 解析参数、设置 Warn 日志、启动 pprof、分配 value、执行 `batchRawPut` 并打印摘要。Rust 迁移版为了复用和独立测试，把同一行为拆成三层：本文件的二进制入口、`lib.rs` 的共享入口、`main.rs` 的实际实现。

因此，本文件与 Go `main()` 的对应关系是“入口身份相同、实现位置不同”。行为对齐不能只检查这 3 行函数体，而应沿转发链检查 `entry::main`、`run_with_flags` 和 `batch_raw_put`。独立测试 `cmd/benchraw/parity_test.rs` 锁定了 Go 默认 flags、连续 key 分片、不能整除时丢弃余数、零长度 value、参数停止规则、建连/Put 失败、worker 等待以及 pprof 日志副作用。当前没有专门只测试 `bin_main.rs` 转发动作的子进程测试。

## 扩展指南

- 新增或修改命令参数：修改 `cmd/benchraw/stubs.rs` 的 `Flags`/解析逻辑以及 `cmd/benchraw/main.rs` 的消费点，并在独立的 `cmd/benchraw/parity_test.rs` 增补 Go/Rust 对齐断言；通常不要改本文件。
- 修改 RawKV 压测流程、并发策略或输出：修改 `entry::run_with_flags`、`entry::batch_raw_put` 或其边界适配器，并同步 parity 测试。需特别评估余数丢弃、key 唯一性、错误快速失败、线程 join 和输出兼容性。
- 修改启动策略：只有当需要在进入共享库前进行进程级操作（例如明确的退出码映射）时才考虑修改 `bin_main.rs::main`。此类变化应优先把可测试逻辑放进库，并新增独立集成/子进程测试，避免把测试逻辑内嵌到源文件。
- 调整 crate/二进制名称或路径：同时核对 `cmd/benchraw/Cargo.toml` 的 `[lib]`、`[[bin]]` 和本文件的 crate 路径，防止 Cargo 接线与源码调用失配。

扩展时的主要兼容风险是改变 Go 既有 CLI/失败语义；主要性能风险位于下游线程数、value 克隆和客户端调用，而不在本包装函数。保持本文件极薄可让这些风险继续由库级测试覆盖。

## 验证依据

- 源文件：`cmd/benchraw/bin_main.rs`，确认唯一符号及直接转发调用。
- Cargo 边界：`cmd/benchraw/Cargo.toml`，确认包名、库路径、二进制名称/路径、迁移元数据和依赖集合。
- 模块入口：`cmd/benchraw/lib.rs`，确认 `entry` 指向 `main.rs`，公共 `main` 转发到 `entry::main`，测试模块独立位于 `parity_test.rs`。
- 实际实现：`cmd/benchraw/main.rs`，确认 `main`、`run_with_flags`、`batch_raw_put` 的启动顺序、并发与错误边界。
- Go 对照：`cmd/benchraw/main.go`，确认原始 `main`/`batchRawPut` 行为及默认参数。
- 独立测试：`cmd/benchraw/parity_test.rs`，确认默认值、参数解析、分片边界、错误传播、生产适配器和资源生命周期契约；同目录不存在 `doc.go` 或同名 `bin_main` 测试。
- RustCodeGraph：索引状态为 7,032 个 Rust 文件；`node --file cmd/benchraw/bin_main.rs` 确认文件内容，`query` 确认 `run_with_flags`/`batch_raw_put` 符号，`callers`/`callees` 确认实际实现层的关键调用边。包装层跨 crate 边未被图索引识别，已由源码和 Cargo 声明交叉验证，而未把缺失图边解释成“未接线”。
- 结构验收使用任务指定命令，要求目标文件存在且上述固定二级标题恰好为 11 个；本任务为纯文档分析，按计划不运行 Cargo。
