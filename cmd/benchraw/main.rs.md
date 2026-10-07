# `cmd/benchraw/main.rs`

## 文件定位

本文件是 `astersql-cmd-benchraw` crate 中可复用、可测试的命令实现层。`Cargo.toml` 把 `lib.rs` 设为库根、把 `bin_main.rs` 设为二进制入口；调用链为 `bin_main.rs::main -> astersql_cmd_benchraw::main（lib.rs）-> entry::main（本文件）`。`lib.rs` 通过 `#[path = "main.rs"] pub mod entry` 暴露本文件，并在测试构建中挂载独立的 `parity_test.rs`。

该命令用于对 TiKV RawKV 执行并发盲写压测。Cargo 元数据 `package.metadata.porting.go-package = "cmd/benchraw"`、`kind = "binary"` 表明它对齐同目录的 Go 命令 `main.go`，不是 SQL 服务主链的一部分。外部系统适配集中在 `stubs.rs`；本文件只负责编排参数、日志、pprof、缓冲区、客户端工厂、worker 和结果输出。

## 核心职责

1. `default_flags` 提供与 Go 包级 flags 一致的默认配置。
2. `main` 从进程环境取得参数，解析成功后选择生产 RawKV 客户端工厂并启动完整流程；帮助参数使解析返回 `None`，此时不运行压测。
3. `run_with_flags` 设置 Warn 日志级别，可选地后台启动 `:9191` pprof 服务，分配固定长度的全零 value，计时并调用 `batch_raw_put`，最后打印目标数据量和耗时。
4. `batch_raw_put` 拆分 PD 地址、透传 TLS 三元组、创建一个共享客户端，把连续 key 区间分给多个 OS 线程并等待全部线程完成。
5. `format_elapse_line` 固定 Go 风格摘要文本，供独立 parity 测试校验；当前生产路径在 `run_with_flags` 内直接 `writeln!`，没有调用该辅助函数。

本文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项；五个函数均为 `pub`，但它们的实际 crate 外可达路径是 `astersql_cmd_benchraw::entry::*`，库根只额外直接转发了 `main`。

## 主要符号

- `default_flags() -> Flags`：直接返回 `Flags::default()`。默认值定义于 `stubs.rs`：`N=1_000_000`、`C=100`、`pd=localhost:2379`、`V=5`，TLS 路径为空。
- `main()`：调用 `stubs::args_from_env` 和 `stubs::parse_flags`。正常参数进入 `run_with_flags(flags, default_client_factory(), true)`；`-h`/`-help` 对应的 `None` 使函数直接返回。
- `run_with_flags(flags: Flags, factory: ClientFactory, start_pprof: bool)`：完整流程的可注入入口。`factory` 让测试替换真实 TiKV，`start_pprof` 让测试控制后台监听副作用。
- `batch_raw_put(flags: &Flags, value: &[u8], factory: ClientFactory)`：写入核心。一个 `ClientFactory` 创建 `Arc<dyn RawKvClient>`；每个 worker 写 `base = data_cnt / worker_cnt` 条记录，key 为 `key_{base*i+j}`。
- `format_elapse_line(elapsed: &str, total: i64) -> String`：返回 `"\nelapse:{elapsed}, total {total}\n"`。它只被 `parity_test.rs::contract_normal_puts_and_defaults` 直接调用。

## 执行流程

生产入口按以下顺序运行：

1. `bin_main.rs::main` 转发到 `lib.rs::main`，后者再调用本文件的 `main`。
2. `main` 跳过 argv[0] 读取参数。`parse_flags` 支持 `-k v`/`-k=v`，遇到首个位置参数、`-` 或 `--` 停止解析；帮助请求打印 usage 后返回而不压测。
3. `run_with_flags` 把日志级别设为 `LogLevel::Warn`。当 `start_pprof=true` 时，分离一个线程执行 `listen_and_serve(":9191")`，监听返回的错误经 `errors_trace` 交给 `terror_log`，不阻塞主流程。
4. `flags.value_size` 用 `usize::try_from` 校验后生成全零 `Vec<u8>`；负值通过 `fatal("makeslice: len out of range")` 快速失败。计时从 value 分配完成后、写入开始前启动。
5. `batch_raw_put` 用逗号原样拆分 `pd_addr`，从三个 TLS flag 构造 `Security`，再调用注入的工厂。生产工厂位于 `stubs.rs::default_client_factory`，实际建立 `tikv_client::RawClient` 和 Tokio runtime。
6. worker 数和数据量分别经 `max(0)` 转成 `usize`。零（或负）worker 显式 fatal；随后计算整除结果 `base`。
7. 主线程把 value 复制为自有 `Vec`，每个 worker 再获得客户端 `Arc` 和 value 的独立克隆。worker `i` 遍历 `j in 0..base`，生成 `key_{base*i+j}` 并调用 `RawKvClient::Put`；错误立即走 `fatal_put_failed`。
8. 主线程逐一 `join` 全部 worker；worker panic 由 `resume_unwind` 继续向调用方传播。成功后 `run_with_flags` 打印 `elapse` 摘要。

整除分片意味着实际 Put 数是 `worker_cnt * floor(data_cnt / worker_cnt)`；不能整除时尾部余数不会写入。例如 `N=10,C=3` 只写 `key_0..key_8`。这是 `main.go::batchRawPut` 的既有行为，由 parity 测试锁定，不应在普通重构中“修正”。

## 数据与状态

`Flags`、`Security`、`ClientFactory` 和 `RawKvClient` 均来自 `stubs.rs`。本文件自身不定义全局可变状态。输入状态包括目标条数、并发数、PD 地址、value 字节数和 TLS 文件路径；每条 value 都是指定长度的零字节数组，key 是 UTF-8/ASCII 形式的 `key_<十进制序号>`。

共享状态只有由工厂返回的 `Arc<dyn RawKvClient>`。value 没有在线程间共享：主流程先拥有一个 `Vec`，每个 worker 获取完整副本，每次 Put 又克隆一份 value。因此无需为 value 加锁，但内存与复制成本约为 worker 数和 Put 次数的函数。客户端具体并发安全性由 `RawKvClient: Send + Sync` 契约保证。

时间状态用局部 `Instant` 管理，只覆盖 `batch_raw_put`（包含建连、线程创建、Put 和 join），不含参数解析、日志设置、pprof 线程启动以及 value 初始分配。打印的 `total` 是配置的 `flags.data_cnt`，即使余数被丢弃，也不是实际成功 Put 数。

## 依赖与调用关系

上游生产链是 `bin_main.rs::main -> lib.rs::main -> entry::main -> run_with_flags -> batch_raw_put`。RustCodeGraph 的 `node run_with_flags` 确认 `main` 是其直接调用者，并确认它调用 `batch_raw_put` 与 `listen_and_serve`；`node batch_raw_put` 还列出 `run_with_flags` 和三个 parity 契约分组为直接调用者。

主要下游关系如下：

- 参数与默认值：`main -> args_from_env -> parse_flags`，`default_flags -> Flags::default`。
- 运行边界：`run_with_flags -> set_log_level`；可选后台链为 `listen_and_serve -> errors_trace -> terror_log`。
- RawKV 建连：`batch_raw_put -> split_pd_addrs -> ClientFactory`。生产的 `default_client_factory -> real_new_client -> tikv_client::RawClient::new_with_config`，并由自持有的 Tokio runtime 把异步 `put` 适配成同步 `RawKvClient::Put`。
- 失败边界：建连错误调用 `fatal(e.Error())`；Put 错误调用 `fatal_put_failed`。
- 标准库资源：`Arc` 共享客户端，`thread::spawn` 创建 pprof/worker 线程，`JoinHandle::join` 等待 worker，`Instant` 计时，`stdout().lock()` 输出。

`Cargo.toml` 的直接依赖是 `log`、`pprof`、`tiny_http`、`tokio` 和带固定 tag `v0.4.2-aster.10` 的 `astersql/client-rust` `tikv-client`。其中具体使用大多封装在 `stubs.rs`；本文件直接使用标准库并调用该适配层。

## 错误处理与边界

- 参数错误由 `stubs::parse_flags` 处理：测试构建 panic，生产构建打印错误/usage 并以状态 2 退出；帮助请求是正常早退。
- 负 `value_size` 在创建客户端之前 fatal；`parity_test.rs::negative_value_size_panics_before_client_creation` 明确验证工厂未被调用。过大的非负长度仍可能因内存分配失败而终止，源码没有恢复策略。
- `worker_cnt <= 0` 被归一为 0 后 fatal。Go 的零并发在整除处失败，负并发也会快速失败；Rust 保留失败约束，但不保证运行时错误文本完全相同。
- `data_cnt < 0` 被归一为 0，因此不会产生 Put；`data_cnt < worker_cnt` 时 `base=0`，仍创建并 join 所有 worker，但没有写入。
- PD 地址按 Go `strings.Split` 语义原样按逗号切分，不 trim、不丢弃空项。TLS 三字段也原样传给工厂；生产适配器额外要求 TLS 三项要么全空、要么全部提供。
- 建连失败在创建 worker 前 fatal。任一 Put 失败会在对应 worker 中 fatal；测试构建表现为 panic，随后主线程 join 并继续传播。生产 `fatal` 直接终止整个进程。
- pprof 启动失败只记录日志，不使压测失败。stdout 写入结果被显式忽略，因此关闭管道等输出错误不会改变函数结果。
- 本流程没有重试、限速、超时、成功计数或部分失败汇总；这些不能被描述为已支持能力。

## 并发与资源生命周期

pprof 线程是分离线程：`run_with_flags` 不保存或 join 其句柄。生产 `listen_and_serve_real` 绑定 `0.0.0.0:9191` 并循环服务 pprof 路由，通常活到进程退出；绑定/接收失败才返回并记录。`start_pprof=false` 仅是可测试入口的控制参数，生产 `main` 固定传 `true`。

RawKV 客户端在启动 worker 前创建一次，由 `Arc` 分发；每个 worker 结束时释放自己的引用，`batch_raw_put` 返回时主引用也释放。生产适配器把 TiKV client 和 Tokio runtime 放在同一对象中，使 runtime 至少与共享客户端等寿。worker 句柄全部保存在预分配的 `Vec` 中，并在返回前逐个 join，因此正常返回意味着所有 Put 循环已结束。

并发写入顺序不稳定，但各 worker 的 key 区间连续且互不重叠；测试使用集合而非调用顺序断言这一不变量。没有取消协议：一个 worker 在测试模式 panic 时，主线程仍按句柄顺序 join；生产 fatal 则直接退出进程。没有显式关闭 RawKV 客户端、Tokio runtime 或 pprof 服务的方法，清理由对象析构或进程退出完成。

## 与 Go 版本的对应关系

`default_flags` 对应 `main.go:35-43` 的七个包级 flag；Rust `main + run_with_flags` 合起来对应 Go `main.go::main`；Rust `batch_raw_put` 对应 Go `batchRawPut`。两版都设置 Warn 日志、后台监听 `:9191`、构造全零 value、创建一个 RawKV 客户端、用整除分片并发写 `key_<n>`、等待 worker，再输出耗时和配置目标总数。

主要结构差异是 Rust 为可测试性显式传入 `Flags`、`ClientFactory` 和 `start_pprof`，而 Go 读取包级 flag 指针并直接调用 `rawkv.NewClient`。Go 用 goroutine/`sync.WaitGroup`，Rust 用 OS 线程/`JoinHandle`；Go Put 接收 `context.Background()`，Rust 适配层内部用 Tokio runtime 同步等待异步 client。Rust 每个 worker、每次 Put 都克隆 value，Go goroutine 共享同一只读 slice；可观察数据内容一致，但复制和内存成本不同。

Go 的 pprof 来自空导入 `net/http/pprof`；Rust `stubs.rs` 用 `tiny_http` 与 `pprof` crate 提供兼容端点，但 `/debug/pprof/trace` 明确返回 501，因此不是 Go pprof 能力的完整等价实现。Rust 的 `format_elapse_line` 是测试辅助接口；生产输出逻辑仍与 Go 格式对齐，但通过另一处格式化表达式实现。

独立 Rust 测试位于 `cmd/benchraw/parity_test.rs`。它覆盖默认值、参数停止规则、真实工厂拒绝不可达 PD、真实监听绑定失败、pprof 索引、TLS/多 PD 透传、正常与余数分片、空 value、key 区间唯一性、建连/Put/非法参数/零 worker 失败、负 value 在建连前失败，以及 worker 完成和 pprof 错误日志。Go 同目录没有 `*_test.go`；语义基准直接来自 `main.go`。

## 扩展指南

- 新增命令行参数时，应先扩展 `stubs.rs::Flags`、`Default`、`parse_flags` 和 usage，再在 `main`/`run_with_flags`/`batch_raw_put` 的正确阶段消费；同步更新 `parity_test.rs` 的默认值、两种参数格式、错误输入和 Go 对齐断言。
- 修改分片策略时，接入点是 `batch_raw_put` 中 `base` 与 key 计算。必须先决定是否有意偏离 Go 的余数丢弃语义，并更新 `contract_boundary_partition_and_flags`；还要评估脚本依赖的 key 范围和摘要中“目标总数”与“实际写数”的差异。
- 增加重试、超时或限速时，应扩展 `RawKvClient`/`ClientFactory` 边界并在独立 `parity_test.rs` 注入确定性失败，不要把测试模块或测试逻辑内嵌进本文件。注意重试会改变吞吐含义、Put 次数和失败可见性。
- 改变并发模型或共享 value 时，应保持客户端 `Send + Sync`、worker 返回前完成、key 不重叠和 panic 可见性，并测量大 value 下的复制成本；异步化还需明确 Tokio runtime 所有权，避免嵌套 runtime 或客户端先于任务析构。
- 使 pprof 端口可配置或增加关闭能力时，应同时修改 `run_with_flags` 与 `stubs.rs::listen_and_serve` 边界，并补充端口冲突、启动失败和生命周期测试。
- 若引入新的外部 Rust 依赖，必须遵守仓库规则：在独立上游仓库移植、提交并打 tag，本仓库所有 Cargo manifest 使用同一已发布 tag；不得复制到 `vendor/third_party` 或用本地 `[patch]`。

## 验证依据

- 源码：完整读取 `cmd/benchraw/main.rs`；入口与模块证据来自 `cmd/benchraw/bin_main.rs`、`cmd/benchraw/lib.rs`；外部边界来自 `cmd/benchraw/stubs.rs`；crate 边界和依赖来自 `cmd/benchraw/Cargo.toml`。
- Go 对照：读取 `cmd/benchraw/main.go` 的 flags、`batchRawPut` 和 `main`；`cmd/benchraw/BUILD.bazel` 进一步确认 Go 二进制及其依赖。目录中不存在 `doc.go` 或 Go 测试文件。
- 测试：读取独立的 `cmd/benchraw/parity_test.rs`。直接引用检索确认其导入 `default_flags`、`run_with_flags`、`batch_raw_put` 和 `format_elapse_line`，并覆盖正常、边界、错误、生产适配器及资源生命周期契约。
- RustCodeGraph：`status` 报告索引包含 7032 个 Rust 文件；`files --filter cmd/benchraw` 列出本 crate 的六个 Rust/Go 文件。`query` 精确定位 `batch_raw_put`、`format_elapse_line`、`default_flags` 和本文件的 `run_with_flags`；`node` 提供源码与调用 trail，确认 `main -> run_with_flags -> batch_raw_put`、`run_with_flags -> listen_and_serve`、三项 parity 契约分组直接调用 `batch_raw_put`，以及 `format_elapse_line` 只被正常路径契约调用。单独的 `callers batch_raw_put` 在约 60 秒内无输出，已中断；调用者结论由同一索引的 `node` trail 与源码导入/调用交叉核验。
- 结构验收使用任务规定的命令，要求目标文件存在且恰有十一个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
