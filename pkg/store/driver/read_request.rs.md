# `pkg/store/driver/read_request.rs`

## 文件定位

本文件属于 `astersql-store-driver` crate；crate 边界由 [`pkg/store/driver/Cargo.toml`](Cargo.toml) 声明，模块由 [`pkg/store/driver/lib.rs`](lib.rs) 中的私有 `mod read_request` 装入。它不是通用读请求实现，而是 TiKV client-rust 读 RPC 的进程级测试故障注入适配层：把 Go 测试沿用的 failpoint 名 `tikvclient/mockBatchClientSendDelay` 转换成 client-rust `RpcInterceptor::delay` 返回的延迟。

模块本身不导出到 crate 外；唯一的 crate 内入口 `install_read_queue_delay` 是 `pub(crate)`。仓内直接调用者是 [`ClientSnapshot::new`](kv_adapter.rs)；因此正常路径是在第一次构造 client-rust 快照时安装拦截器，随后该进程中的相应 TiKV RPC 都可观察同一 failpoint。

## 核心职责

- `ReadQueueDelay` 识别 client-rust 交给拦截器的动态请求类型，仅对 `kvrpcpb::GetRequest`、`BatchGetRequest` 和 `ScanRequest` 注入延迟。
- `RpcInterceptor::delay` 每次请求都读取 `astersql_testkit_testfailpoint` 中的 `tikvclient/mockBatchClientSendDelay`，把可解析的毫秒字符串转换为 `Duration`。
- `install_read_queue_delay` 通过 `OnceLock<InterceptorGuard>` 保证全进程最多注册一次，并永久持有 guard，避免拦截器在安装函数返回时被注销。
- 文件只负责 client-rust KV 读请求。标准 coprocessor unary 请求的同名 failpoint 处理位于 [`pkg/store/copr/network_backend.rs`](../copr/network_backend.rs)，不能把那条路径归因于本文件。

## 主要符号

- `struct ReadQueueDelay`：无字段的私有标记类型；自身不保存配置或计时状态，行为完全由每次调用时的请求动态类型和 failpoint 当前值决定。
- `impl RpcInterceptor for ReadQueueDelay`：仅覆盖 `delay(&self, request: &dyn Any) -> Duration`，沿用 trait 的 `before`、`after` 等默认实现。
- `ReadQueueDelay::delay`：先用 `Any::is::<T>()` 做三个 protobuf 请求类型的白名单判断；未命中立即返回 `Duration::ZERO`。命中后调用 `eval_string`，再执行 `parse()`；缺少 failpoint 或解析失败均经 `unwrap_or_default()` 降级为零毫秒。
- `install_read_queue_delay()`：crate 内安装入口。函数内静态 `GUARD` 的类型是 `OnceLock<InterceptorGuard>`，初始化闭包以 `Arc` 注册一个 `ReadQueueDelay`。

本文件没有模块级常量、枚举、公开 trait、条件编译项或内嵌测试。

## 执行流程

1. [`ClientSnapshot::new`](kv_adapter.rs) 在取得 client runtime 和创建底层 snapshot 之前调用 `install_read_queue_delay`。
2. 首次调用通过 `register(Arc::new(ReadQueueDelay))` 把拦截器加入 client-rust 的进程级拦截器表，并把返回的 `InterceptorGuard` 保存到静态 `OnceLock`；后续调用只取得已存在的 guard。
3. client-rust `KvRpcClient::dispatch_with_timeout` 为一次实际请求取得拦截器快照，对每个 hook 调用 `delay(request.as_any())`，并以饱和加法合并延迟。
4. 对 `GetRequest`、`BatchGetRequest` 或 `ScanRequest`，`ReadQueueDelay::delay` 在发送当次请求前读取 failpoint 毫秒值；其他请求返回零。
5. client-rust 在同一个 `tokio::time::timeout(timeout, ...)` 作用域内先 sleep 再 dispatch。因此当注入延迟大于读超时，结果是 deadline exceeded，调用链可触发副本重试或默认 timeout 回退；延迟不是发送完成后的额外等待。
6. failpoint guard 在测试中被释放/禁用后，拦截器仍然注册，但后续求值得到零延迟。

## 数据与状态

本文件唯一的持久状态是函数内静态 `GUARD: OnceLock<InterceptorGuard>`。`ReadQueueDelay` 本身是零大小类型，没有每个 snapshot 或每个请求的字段。由于 guard 被静态持有，注册生命周期等同于进程生命周期。

输入 `request: &dyn Any` 只用于运行时类型判断，不会被修改或保存。failpoint 值是字符串；成功解析时由返回类型推断为毫秒整数并交给 `Duration::from_millis`。输出仅是一个 `Duration`，真正的 sleep、timeout 和错误构造发生在 tag `v0.4.2-aster.10` 的 client-rust `src/store/client.rs::KvRpcClient::dispatch_with_timeout` 中。

重要不变量是：非白名单请求永不读取该 failpoint；白名单请求在 failpoint 不存在、值无法解析或值为 `0` 时都不增加延迟；重复创建 snapshot 不会重复注册相同 hook。

## 依赖与调用关系

上游直接调用边为 `kv_adapter.rs::ClientSnapshot::new -> read_request.rs::install_read_queue_delay`。再往上，`ClientSnapshot` 实现 `kv::Snapshot`/`kv::Getter` 等适配接口，把 AsterSQL 的点查、批量点查和扫描落到 client-rust snapshot；本文件只在其构造阶段接线，不参与键编码、结果转换或重试决策。

下游关系为 `install_read_queue_delay -> tikv_client::rpc_interceptor::register`，以及 `ReadQueueDelay::delay -> astersql_testkit_testfailpoint::eval_string`。[`Cargo.toml`](Cargo.toml) 直接声明了本地 `astersql-testkit-testfailpoint` 和固定 tag 的 `tikv-client = v0.4.2-aster.10`，且 `default-features = false`。[`Cargo.lock`](../../../Cargo.lock) 将该 tag 锁到提交 `710d2187e79e1a3d22035b0e68d35a8f267ee835`；该版本的 `rpc_interceptor.rs` 证明 `register` 返回 RAII guard，`store/client.rs` 证明延迟在 transport dispatch 的 timeout 内执行。

测试与行为证据包括：

- [`pkg/session/runtime_pessimistic_test.rs`](../../session/runtime_pessimistic_test.rs) 启用同名 failpoint 后验证短读超时会增加 `Get` RPC 次数。
- [`tests/realtikvtest/sessiontest/session_fail_test.rs`](../../../tests/realtikvtest/sessiontest/session_fail_test.rs) 的 `test_tikv_client_read_timeout`/`check_read_timeout` 覆盖 point get、batch point get、coprocessor 及 stale read 的超时重试统计；真实三副本用例标记为需要外部集群。
- 同文件的 coprocessor 断言同时依赖 `pkg/store/copr/network_backend.rs` 的独立实现，不是本拦截器单独提供的覆盖面。

## 错误处理与边界

该实现刻意采用无错误的测试注入接口：failpoint 未启用时 `eval_string` 返回空，格式错误时 `parse().ok()` 丢弃解析错误，二者均返回零延迟，不影响生产读路径。非 `GetRequest`、`BatchGetRequest`、`ScanRequest` 请求同样直接返回零。安装函数没有返回值，`OnceLock::get_or_init` 和 `register` 在当前 client-rust API 下也不暴露可恢复错误。

延迟值由 `Duration::from_millis` 表示；多个已注册拦截器的延迟在 client-rust 中以 `saturating_add` 合并，避免相加溢出。本文件不捕获 transport deadline、重试耗尽、Region 错误或业务错误，这些由 client-rust 和上层 AsterSQL 调用链处理。

覆盖边界必须保持清楚：这里不延迟 coprocessor protobuf request、写请求、事务 prewrite/commit、PD 请求或其他 RPC；若测试希望覆盖这些类别，应在其实际 transport 边界实现并验证，而不是泛化本白名单后假定语义相同。

## 并发与资源生命周期

`OnceLock` 为并发首次安装提供一次性初始化保证；多个线程同时创建第一个 `ClientSnapshot` 时只有一个线程注册 hook。client-rust 的注册表由 `RwLock<Vec<(u64, Arc<dyn RpcInterceptor>)>>` 保护，请求发送前复制 `Arc` 快照，因此 `ReadQueueDelay` 满足 `RpcInterceptor: Send + Sync + 'static` 的并发要求。

保存 `InterceptorGuard` 至关重要：client-rust 在 guard 的 `Drop` 中按注册 ID 删除 hook。静态 `GUARD` 不在正常运行中释放，所以 hook 不会因局部变量退出而消失。测试侧 failpoint guard 的生命周期只控制 `eval_string` 能否读到值，不控制拦截器注册生命周期。

本实现没有显式线程、异步任务、channel、锁等待、网络连接或事务资源；实际异步 sleep 和 transport timeout 由 client-rust runtime 管理。每次请求都会重新读取 failpoint，这允许测试在已创建 snapshot 后动态启停注入，但也意味着该全局名字会影响进程内所有命中类型的并发请求。

## 与 Go 版本的对应关系

`pkg/store/driver` 下没有与 `read_request.rs` 同路径的 Go 生产文件。Rust 文件移植的是 Go/client-go 测试环境中已存在的 failpoint 契约，而不是逐函数翻译本目录某个 `.go` 文件。

可核对的 Go 语义位于 [`tests/realtikvtest/sessiontest/session_fail_test.go`](../../../tests/realtikvtest/sessiontest/session_fail_test.go) 的 `TestTiKVClientReadTimeout`：测试以 `return(100)` 启用 `tikvclient/mockBatchClientSendDelay`，把读 timeout 设为 1ms，并期望 point get、batch point get 和 coprocessor 读取因三副本尝试加回退而出现相应 RPC 次数。Rust 的 [`session_fail_test.rs`](../../../tests/realtikvtest/sessiontest/session_fail_test.rs) 保留相同 failpoint 名、三副本前提、读类型与断言意图。

Rust 与 Go 的实现位置不同：Go 的延迟注入由 Go TiKV client 的 failpoint 提供；Rust 侧必须显式借助 AsterSQL fork 的 client-rust `RpcInterceptor` 注册机制。Rust 又把 client-rust KV 请求与 coprocessor transport 分开接线，所以 `read_request.rs` 只覆盖前三种 kvrpcpb 读请求，而 `network_backend.rs` 覆盖 coprocessor unary 请求。文档没有证据表明其他 Go client failpoint 行为也已由本文件移植。

## 扩展指南

- 增加新的 client-rust 读请求类型时，修改 `ReadQueueDelay::delay` 的白名单，并先确认实际请求传给 `request.as_any()` 的具体 protobuf 类型；不要只依据 SQL 算子名称推断。
- 调整 failpoint 名、值格式或延迟单位时，必须同步 Go `TestTiKVClientReadTimeout` 的兼容契约、Rust `session_fail_test.rs` 和 `runtime_pessimistic_test.rs`。单位变化尤其会改变 timeout/retry 行为和测试时长。
- 改变安装时机时，应重点检查 `ClientSnapshot::new` 之前是否可能已经发出目标 RPC，以及静态全局 hook 对多 store、多测试并发的影响。若改成可卸载或每实例 hook，必须重新设计 guard 所有权并增加独立测试。
- 若扩展到 coprocessor 或写请求，应在对应 transport 边界单独实现和测试，避免同一 failpoint 被两层同时应用而产生双倍延迟。
- 本文件目前没有同目录独立测试。按仓库规则，若修改运行时代码，应在独立 `*_test.rs` 文件中补充：三种命中类型、至少一种非命中类型、未设置/非法/零/正数 failpoint，以及并发重复安装只注册一次；不要把测试内嵌回 `read_request.rs`。
- 兼容风险集中在固定的 client-rust interceptor API 与请求具体类型；性能风险是全局拦截器对每个 RPC 的动态类型检查及 failpoint 查询，当前开销固定且不持锁等待于本文件，但扩大白名单或增加复杂解析前应测量热点影响。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 `pkg/store/driver/read_request.rs` 已索引；`files --filter` 报告该文件含 9 个图符号。
- RustCodeGraph `node --file pkg/store/driver/read_request.rs --offset 1 --limit 260`：读取了 29 行完整源码；`query ReadQueueDelay --kind struct` 和 `query install_read_queue_delay --kind function` 均唯一命中本文件。精确 `callers` 查询在 30 秒内无输出，已终止，调用边随后以仓库 `rg` 核验。
- 读过的仓库文件：`pkg/store/driver/read_request.rs`、`lib.rs`、`Cargo.toml`、`kv_adapter.rs`，`pkg/session/runtime_pessimistic_test.rs`，`pkg/store/copr/network_backend.rs`，`tests/realtikvtest/sessiontest/session_fail_test.rs` 及其 Go 对照 `session_fail_test.go`。
- 直接引用搜索只找到生产调用 `kv_adapter.rs::ClientSnapshot::new -> install_read_queue_delay`；同名 failpoint 的其他生产读取点位于 coprocessor backend 和 session 的内存 explain 模拟路径，文中均未混作本文件调用者。
- 依赖源码核验使用 `Cargo.lock` 锁定的 client-rust `710d218…`：`src/rpc_interceptor.rs` 的 trait、注册表和 guard drop 语义，以及 `src/store/client.rs::KvRpcClient::dispatch_with_timeout` 的 hook、delay、sleep 和 timeout 顺序。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另以任务指定命令验证目标文档存在且固定二级标题恰好为 11 个，并人工复核唯一产物、链接和范围陈述。
