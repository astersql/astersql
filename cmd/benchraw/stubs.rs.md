# [`cmd/benchraw/stubs.rs`](./stubs.rs)

## 文件定位

`stubs.rs` 是 `astersql-cmd-benchraw` crate 的外部边界适配层。`cmd/benchraw/lib.rs` 以 `pub mod stubs` 暴露本模块，二进制入口 `cmd/benchraw/bin_main.rs` 经库入口进入 `cmd/benchraw/main.rs`；后者把参数解析、日志初始化、RawKV 客户端创建、Put 调用和 pprof 监听交给本文件。它虽以 `stubs` 命名，却不只是测试桩：非测试构建中的 `default_client_factory` 会创建真实 `tikv_client::RawClient`，`listen_and_serve` 会绑定真实 HTTP socket；内存客户端和全局观测状态只在显式注入的测试路径使用。

crate 边界由 `cmd/benchraw/Cargo.toml` 定义：库入口是 `lib.rs`，二进制入口是 `bin_main.rs`，移植元数据指向 Go 包 `cmd/benchraw`。本文件直接依赖 `log`、`pprof`、带固定 tag `v0.4.2-aster.10` 的 `tikv-client`、`tiny_http` 和 Tokio 多线程运行时。

## 核心职责

- 以 `Error`、`errors_trace`、`terror_log`、`fatal` 和 `fatal_put_failed` 压缩表达 Go 的 errors、terror 与日志终止边界，并为测试保存可观察日志。
- 以 `Flags`、`parse_flags`、`print_defaults`、`print_usage` 和 `args_from_env` 实现 benchraw 实际使用的 Go `flag` 子集，而不是通用参数解析器。
- 以 `RawKvClient` 和 `ClientFactory` 隔离调用方；生产实现 `TikvRawKvClient` 桥接异步 `tikv-client`，测试实现 `StubRawKvClient` 记录 Put 并支持失败注入。
- 以 `listen_and_serve` 在测试构建中选择可观察桩、在生产构建中选择真实 `tiny_http` 服务，并提供有限的 pprof 兼容端点。
- 用 `split_pd_addrs` 保留 Go `strings.Split` 对 PD 地址字符串的原始语义，包括空项且不 trim。

## 主要符号

- `Error { msg }`：本地统一错误载体；实现 `Display` 和 `std::error::Error`，`Error()` 保留 Go 风格取消息接口。`errors_trace` 当前原样返回错误，不生成 Rust 堆栈。
- `LogLevel`、`BenchLogger`、`set_log_level`：维护模块级日志级别并把日志写到 stderr。`set_logger` 的结果被忽略，允许全局 logger 已由别处初始化；最大日志级别仍会更新。
- `Flags`：保存 `N`、`C`、`pd`、`V` 以及 TLS 三元组；`Default` 的值分别为 `1_000_000`、`100`、`localhost:2379`、`5` 和三个空字符串。
- `Security`：只保留客户端创建实际需要的 `ClusterSSLCA`、`ClusterSSLCert`、`ClusterSSLKey`。
- `PutCall` 与 `PutLog`：在 `Arc<Mutex<Vec<PutCall>>>` 中记录 key/value；`calls` 返回快照，`len` 只取数量，`clear` 清空记录，`with_shared` 共享同一底层日志。
- `RawKvClient: Send + Sync`：唯一方法为 `Put(Vec<u8>, Vec<u8>) -> Option<Error>`。`None` 表示成功，`Some(Error)` 表示失败。
- `TikvRawKvClient`：同时拥有 `tikv_client::RawClient` 与 Tokio `Runtime`；`Put` 用该运行时同步阻塞等待异步 `client.put`。
- `StubRawKvClient`：保存 PD 地址、TLS 参数、共享 Put 日志和原子失败开关；失败时不记录 Put，成功时只记录、不访问网络。
- `ClientFactory`：线程安全的共享闭包，输入地址与安全配置，输出 trait object 或 `Error`。`default_client_factory` 指向 `real_new_client`，`stub_client_factory` 指向 `new_client`。
- `listen_and_serve`、`listen_and_serve_real`、`pprof_response`：分别负责条件编译分派、真实请求循环和 URL 到响应的映射。

## 执行流程

1. `entry::main`（`cmd/benchraw/main.rs`）调用 `args_from_env`，再由 `parse_flags` 从 `argv[1..]` 构造 `Flags`；帮助参数返回 `None`，调用方随即正常返回。
2. `entry::run_with_flags` 调用 `set_log_level(Warn)`。启用 pprof 时，它另起线程调用 `listen_and_serve(":9191")`，并将返回错误经 `errors_trace` 交给 `terror_log`。
3. `entry::batch_raw_put` 用 `split_pd_addrs` 解析 PD 列表并组装 `Security`，然后调用注入的 `ClientFactory`。二进制正常路径注入 `default_client_factory`，因此进入 `real_new_client`。
4. `real_new_client` 从默认 `tikv_client::Config` 开始；TLS 三项全空时保持明文配置，任一非空时要求三项齐备，再创建 Tokio 运行时并同步建立 RawKV 客户端。
5. worker 线程共享 `Arc<dyn RawKvClient>`。每次 `TikvRawKvClient::Put` 都在所持运行时上等待真实异步 Put；测试工厂则创建 `StubRawKvClient`，把请求写入共享 `PutLog`。
6. 非测试构建中的 pprof 线程把 `:端口` 转换为 `0.0.0.0:端口`，绑定后循环接收请求，经 `pprof_response` 生成响应；CPU profile 路径按查询参数采样并编码 protobuf。

`parse_flags` 支持 `-k v` 与 `-k=v`，允许单横线和双横线前缀；遇到第一个位置参数、单独的 `-` 或 `--` 即停止解析。未知参数、坏语法、缺值或整数解析失败进入 `flag_error`。

## 数据与状态

文件内有多组进程级状态，均需视为跨调用共享：`CURRENT_LOG_LEVEL` 保存最近日志级别；`TERROR_LOGS` 保存错误文本；`NEW_CLIENT_FAIL` 保存建连故障注入；`LAST_NEW_CLIENT` 保存最近一次测试建连参数；`GLOBAL_PUT_LOG` 聚合测试默认工厂的成功 Put；`HTTP_LISTEN_ADDRS`、`HTTP_LISTEN_FAIL` 和 `HTTP_STARTED` 记录测试 HTTP 行为。

可变集合与可选值使用 `OnceLock<Mutex<_>>` 延迟初始化；Put 日志通过 `Arc<Mutex<_>>` 在客户端和 worker 间共享；布尔故障开关和启动次数使用 `AtomicBool`、`AtomicUsize`，访问均采用 `SeqCst`。这些状态没有自动按测试隔离，测试必须调用对应的 `clear_*`、`take_*` 或 `reset_http_state`，且并行测试若共享这些全局入口仍可能互相干扰。

生产 `TikvRawKvClient` 将 Tokio runtime 与 client 放在同一对象内，保证调用期间运行时存活。`Put` 接收拥有所有权的 key/value；记录桩同样存储拥有所有权的数据，避免 worker 生命周期结束后出现借用问题。

## 依赖与调用关系

上游主链由 RustCodeGraph 与源码共同确认：`cmd/benchraw/lib.rs::main` 转发到 `entry::main`；`entry::main` 调用 `args_from_env`、`parse_flags` 和 `default_client_factory`；`run_with_flags` 调用 `set_log_level`、`listen_and_serve`、`errors_trace`、`terror_log`；`batch_raw_put` 调用 `split_pd_addrs`、工厂闭包及 `RawKvClient::Put`。RustCodeGraph 的文件查询将 `main.rs` 与 `parity_test.rs` 标为本模块主要直接使用者。

下游边界如下：

- `log`：真实日志宏、全局 logger 和级别过滤。
- `tikv_client::Config`、`RawClient::new_with_config`、`RawClient::put`：生产 PD/TiKV 连接和 RawKV 写入。
- `tokio::runtime::Runtime`：把异步客户端 API 适配为当前同步 trait。
- `tiny_http::Server`：真实监听、收包和响应发送。
- `pprof::ProfilerGuard` 与 protobuf `Message::encode`：CPU profile 采集与序列化。
- Rust 标准库的环境参数、stderr/stdout、线程睡眠、锁、原子变量与进程退出。

相关测试位于独立文件 `cmd/benchraw/parity_test.rs`，由 `cmd/benchraw/lib.rs` 的 `#[cfg(test)] mod parity_test` 接入；没有把测试代码内嵌到 `stubs.rs`。

## 错误处理与边界

`fatal` 和 `flag_error` 在测试构建中 panic，便于 `catch_unwind` 断言；非测试构建分别输出消息并以状态码 1、2 退出。它们不会返回。`fatal_put_failed` 固定生成 `put failed: <底层消息>`。`terror_log(None)` 无副作用，`Some(Error)` 同时写真实 error 日志和测试缓冲区；`errors_trace` 只保留错误值，不能声称与 Go `errors.Trace` 的堆栈完全等价。

TLS 的关键不变量是“三项全空或三项全有”：只给出部分 CA/cert/key 时，`real_new_client` 在创建运行时和连接前返回明确错误。运行时创建、PD 连接、RawKV Put、HTTP bind/recv、profile 构造和编码错误都转换为文本 `Error`。HTTP 单次 `respond` 失败只记录 warn 并继续服务；bind 或 recv 失败才结束监听并返回错误。

pprof 只兼容有限端点：索引、cmdline、symbol 和 profile；trace 明确返回 501，未知路径返回 404。`profile_seconds` 对缺失或非法的 `seconds` 回退到 30；请求 profile 会让服务线程同步睡眠并采样相应秒数，因此该单线程服务在采样期间不能处理其他请求。

参数解析不是完整 Go `flag` 重实现。它只识别本命令的七个参数和帮助项；停止解析后的位置参数未返回给调用方。`split_pd_addrs` 刻意保留空字符串。`Mutex::lock().unwrap()` 遇毒化会 panic。`set_logger` 失败被忽略，这是与宿主进程全局 logger 共存的取舍。

## 并发与资源生命周期

本文件要求 `RawKvClient: Send + Sync`，使 `entry::batch_raw_put` 能把同一个 `Arc<dyn RawKvClient>` 克隆给多个 worker。生产客户端的每次同步 `Put` 都共享同一个 Tokio runtime 与 RawClient；测试日志的 Mutex 串行化记录，返回的调用顺序受线程调度影响，测试只能依赖内容集合或计数，不能依赖顺序。

`run_with_flags` 创建的 pprof 线程没有 join：与 Go 后台 goroutine 一样，它独立于压测 worker。生产监听成功后进入无限接收循环；进程退出时由操作系统回收 socket 和线程资源。测试实现不绑定端口，只递增计数、记录地址并立即返回注入错误或成功。

`cpu_profile` 持有 `ProfilerGuard` 覆盖整个睡眠区间，随后构建报告并编码；采样持续时间直接来自请求。`PutLog::calls` 在锁内克隆完整日志，读者释放锁后才处理快照；这避免长时间持锁，但日志较大时会产生与记录量成比例的内存和复制成本。

## 与 Go 版本的对应关系

Go 直接证据是 `cmd/benchraw/main.go`。对应关系为：包级 `flag.*` 默认值对应 `Flags::default`；`flag.Parse`/`flag.PrintDefaults` 对应本地解析与帮助输出；`log.SetLevel(zap.WarnLevel)` 对应 `set_log_level(LogLevel::Warn)`；`strings.Split(*pdAddr, ",")` 对应 `split_pd_addrs`；`config.Security` 对应 `Security`；`rawkv.NewClient` 对应生产 `real_new_client`；`cli.Put` 对应 `RawKvClient::Put`；后台 `http.ListenAndServe(":9191", nil)` 与 `terror.Log(errors.Trace(err))` 对应 HTTP 适配和错误记录链。

Rust 版为可测试性增加了 `ClientFactory`、内存客户端、共享日志和状态重置接口，这些不是 Go 生产 API。Rust 生产客户端还需用自持 Tokio runtime 连接同步命令代码与异步 `tikv-client`。pprof 并非 Go `net/http/pprof` 的完整实现：当前明确提供有限端点且不支持 runtime trace。错误追踪也只保留文本，不保留 Go 包装栈。

`cmd/benchraw/parity_test.rs` 锁定了与本文件直接相关的语义：默认值和两种 flag 写法；首个位置参数与 `--` 停止解析；帮助提前返回；PD/TLS 参数透传；生产工厂不会退化为内存桩；真实 HTTP bind 失败；pprof 索引；Put 成功/失败记录；错误消息；后台监听观测和 terror 日志。

## 扩展指南

新增命令参数时，应同步修改 `Flags`、`Default`、`parse_flags`、`print_defaults`，再在 `cmd/benchraw/main.rs` 接线；同时更新 `cmd/benchraw/parity_test.rs` 的默认、正常和非法输入断言，并与 `main.go` 的同名 flag 核对。不要把通用解析能力无范围地加入这里。

新增 RawKV 操作时，应先确认 `main.rs` 的真实调用需要，再最小扩展 `RawKvClient`；生产实现与 `StubRawKvClient` 必须同时实现，独立测试应继续放在 `parity_test.rs` 或同目录新的 `*_test.rs`，不能写回生产源文件。注意 trait 变更会影响工厂返回的所有 trait object，并可能增加 worker 间同步或数据复制成本。

修改 TLS 或客户端配置时，应落在 `real_new_client`，并保持 `default_client_factory` 仍只返回生产适配器；测试所需替换应通过显式 `ClientFactory` 注入。外部 `tikv-client` 版本变更必须遵守仓库规则，在上游仓库发布 tag 后统一更新 Cargo Git tag，不能引入本地 patch 或 vendor 副本。

扩展 pprof 时，应修改 `pprof_response` 的路径分派，并在 `parity_test.rs` 增加纯响应测试；若改变服务并发模型，还需验证长时间 profile 不会阻塞其他端点、线程可退出以及 socket 清理。新增共享测试状态时，要提供清理入口并避免并行测试污染。

## 验证依据

- RustCodeGraph `status`：当前索引包含 7,032 个 Rust 文件；`files --filter cmd/benchraw` 确认本 crate 的 Rust/Go 文件集合。
- RustCodeGraph `node --file cmd/benchraw/stubs.rs`：读取本文件 1–734 行，核对全部类型、函数、trait、impl、静态状态和 `cfg(test)` 分支。
- RustCodeGraph `explore "cmd/benchraw/stubs.rs symbols callers callees module role"`：确认 `fatal_put_failed <- batch_raw_put`、`default_client_factory <- entry::main`、`listen_and_serve <- run_with_flags`、`split_pd_addrs <- batch_raw_put`，以及各测试辅助符号由 `parity_test.rs` 使用。
- RustCodeGraph `node`：读取 `cmd/benchraw/main.rs`、`lib.rs`、`bin_main.rs` 与独立测试 `parity_test.rs`，核对入口、调用链、条件测试模块和契约断言。
- 配置与 Go 对照：读取 `cmd/benchraw/Cargo.toml`、根 `Cargo.toml` 的 workspace 成员以及 `cmd/benchraw/main.go`；Cargo 依赖和 Go 原始 flag、RawKV、TLS、并发与 pprof 行为均以这些文件为准。
- 本任务是纯文档分析，按计划不运行 Cargo；最终用任务规定的命令确认本文恰含十一个固定二级章节，并人工复核唯一新增生产物为 `cmd/benchraw/stubs.rs.md`。
