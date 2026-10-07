# `br/pkg/mock/mock_cluster.rs`

## 文件定位

[`mock_cluster.rs`](./mock_cluster.rs) 属于 Cargo workspace 成员 `astersql-br-pkg-mock`；该 crate 的入口 [`lib.rs`](./lib.rs) 以 `pub mod mock_cluster` 装载本文件，并通过 `pub use mock_cluster::*` 平铺导出其公共符号。它是 BR 测试基础设施中的 mock TiDB 集群门面，直接对照同目录 [`mock_cluster.go`](./mock_cluster.go)，不是生产环境的真实 PD、TiKV 或 TiDB Server 实现。

[`Cargo.toml`](./Cargo.toml) 将 crate 标为 `library`，`go-package = "br/pkg/mock"`，且依赖表为空。文件所需的存储、Domain、Server、PD 客户端和探针能力全部来自同 crate 的 [`stubs.rs`](./stubs.rs)，这是为避免 darwin arm64 上引入 grpcio 等重型依赖而采用的本地替身边界。

## 核心职责

- `NewCluster` 构造并 bootstrap 单 store 的内存集群，建立 `Storage`、`Domain`、`PDClient` 与 `PDHTTPClient`，但尚不启动 SQL Server。
- `Cluster::Start` 创建 TiDB 驱动和 Server 替身，等待后台 Server 发出就绪通知，再依次确认 SQL 与 HTTP 状态探针并生成可拼接数据库名的 DSN 前缀。
- `Cluster::Stop` 按 Domain、Storage、Server、pprof HTTP Server 的顺序回收资源，并复位集群在线标志。
- `getDSN`、`waitUntilServerOnline` 和 `split_after_first` 复刻 Go 版连接串生成及就绪轮询语义。
- `PPROF_ONCE` 保证进程内只执行一次 pprof 初始化；只有真正执行该闭包的 `Cluster` 实例持有 `HttpServer`。

这些能力主要服务测试套件。例如 [`br/pkg/utiltest/suite.rs`](../utiltest/suite.rs) 的 `CreateRestoreSchemaSuite` 调用 `NewCluster` 和 `Start`，并由 `Drop`/`Stop` 最终调用本文件的 `Cluster::Stop`。

## 主要符号

- `static PPROF_ONCE: Once`：进程级一次性初始化闩锁，对应 Go 的 `pprofOnce sync.Once`。
- `pub struct Cluster`：聚合 `Server`、`TiKVCluster`、`Storage`、`TiDBDriver`、`Domain`、PD 两类客户端、DSN 与 pprof `HttpServer`。除 `DSN` 外均用 `Option` 表达分阶段初始化。
- `impl Default for Cluster`：生成全部句柄为空、DSN 为空的未初始化状态。
- `pub fn NewCluster() -> Result<Cluster>`：bootstrap 构造入口；可传播 mock store 或 session bootstrap 错误。
- `pub fn Cluster::Start(&mut self) -> Result<()>`：运行阶段入口；要求 `Storage` 已存在，成功后填入 `TiDBDriver`、`Server` 和 `DSN`。
- `pub fn Cluster::Stop(&mut self)`：显式 teardown；逐项判断可选句柄后关闭。
- `pub type ConfigOverrider = Box<dyn FnMut(&mut MysqlConfig)>`：可变配置闭包；`getDSN` 接受 `Vec<Option<_>>`，以保留 Go 可传 `nil` overrider 的行为。
- `default_dsn_config` / `getDSN`：默认生成 `root@tcp(127.0.0.1:4001)/`，然后按顺序应用覆写。
- `waitUntilServerOnline`：SQL 与 HTTP 两阶段就绪轮询，返回首个 `/` 及其之前的 DSN 前缀。
- `split_after_first`：本文件唯一私有函数，等价于 `strings.SplitAfter(s, sep)[0]`。

本文件没有 trait、宏或条件编译项；测试通过 [`lib.rs`](./lib.rs) 中的 `#[cfg(test)]` 独立装载 [`mock_cluster_test.rs`](./mock_cluster_test.rs) 和 [`parity_test.rs`](./parity_test.rs)，未把测试内嵌进生产源文件。

## 执行流程

1. `NewCluster` 从 `Cluster::default` 开始。首次调用经 `PPROF_ONCE.call_once` 创建地址为 `0.0.0.0:12235` 的 `HttpServer` 替身并调用 `ListenAndServe`；后续调用不会执行闭包，也不会继承第一次的句柄。
2. `NewMockStoreWithoutBootstrap` 创建共享的 `TiKVCluster`；inspector 闭包先调用 `BootstrapWithSingleStore`，再克隆捕获 bootstrap 后的集群状态。返回的 `Storage` 同时写入 `Cluster::Storage`。
3. `DisableStats4Test` 关闭测试统计路径；`BootstrapSession(&storage)` 创建 `Domain`。随后经 `Storage::GetRegionCache().PDClient()` 和 `GetPDHTTPClient()` 保存两类 PD 客户端。
4. `Start` 先设置全局 `RUN_IN_GO_TEST`，通过 `make_run_in_go_test_chan` 建立单次就绪通知通道；若 `Storage` 为 `None`，立即返回 `Error("nil storage")`。
5. `Start` 创建 `TiDBDriver`，把 SQL/Status 端口都设为 `0`、Store 设为 TiKV、开启 status，并以当前纳秒时间生成 `/tmp/tidb-mock-<n>.sock`。`NewServer` 在替身层把零端口转换为测试端口。
6. Server 克隆被移入独立线程执行 `Run(None)`；主线程阻塞接收就绪通知。线程中的 `Run` 错误会 panic。收到通知后，`set_cluster_online(true)` 使默认 SQL/HTTP 探针成功，再由 `waitUntilServerOnline` 写入 `DSN`。
7. `waitUntilServerOnline` 用覆写后的地址构造 DSN。SQL 阶段每轮先 `sleep_retry`，再调用 `sql_open`；成功即关闭占位连接，耗尽则 panic。HTTP 阶段访问 `http://127.0.0.1:<status_port>/status`，成功读取响应体后退出。最后以 `split_after_first` 返回形如 `root@tcp(addr)/` 的前缀。
8. `Stop` 关闭已存在的资源，调用 `view_Stop`，最后 `set_cluster_online(false)`；调用方应让 `Start` 与 `Stop` 成对出现。

## 数据与状态

`Cluster` 是分阶段状态容器：`NewCluster` 成功后应至少有 `Cluster`、`Storage`、`Domain`、`PDClient` 和 `PDHTTPCli`；`Start` 成功后再增加 `TiDBDriver`、`Server` 与非空 `DSN`。`HttpServer` 是例外：它只属于第一次实际执行 `PPROF_ONCE` 闭包的实例，因此后续实例的该字段可以为 `None`。

主要共享状态位于 [`stubs.rs`](./stubs.rs)：`RUN_IN_GO_TEST` 是 `AtomicBool`；Server 的 `running`/`closed`、Domain/Storage/HttpServer 的 `closed` 均为 `Arc<AtomicBool>`；Server 就绪状态由 `Mutex<bool> + Condvar` 保存；就绪发送端位于全局 `Mutex<Option<Sender<()>>>`。SQL/HTTP 探针、重试次数、休眠控制与 `CLUSTER_ONLINE` 是线程局部状态，因此注入钩子只影响当前测试线程。

`Cluster` 本身没有 `Drop` 实现，也不会把已关闭句柄清空。关闭后的可观察状态保留在 stubs 的原子标志中；重复调用 `Stop` 在当前替身实现下只会重复写关闭标志，但接口没有声明一般意义上的幂等保证。

## 依赖与调用关系

RustCodeGraph 对 `mock_cluster.rs::NewCluster` 的下游边给出 `DisableStats4Test`、`BootstrapWithSingleStore`、`NewMockStoreWithoutBootstrap`、`BootstrapSession`；对 `Start` 给出 `make_run_in_go_test_chan`、`NewTiDBDriver`、`NewServer`、`set_cluster_online` 与本文件的 `waitUntilServerOnline`；对后者给出 `getDSN`、`split_after_first`、`retry_time`、`sleep_retry`、`sql_open`、`http_get`。

模块边界为：

`br/pkg/utiltest::CreateRestoreSchemaSuite` → `NewCluster` → `stubs` 的 store/session bootstrap → `Cluster::Start` → `stubs::Server::Run` 与 SQL/HTTP 探针 → 测试使用 `Domain`/Server → `Cluster::Stop`。

直接外部调用证据来自 [`br/pkg/utiltest/suite.rs`](../utiltest/suite.rs)，其 Cargo manifest 以路径依赖引用 `astersql-br-pkg-mock`。包内调用者是 [`mock_cluster_test.rs`](./mock_cluster_test.rs) 和 [`parity_test.rs`](./parity_test.rs)。`lib.rs` 的平铺再导出让调用方使用 `astersql_br_pkg_mock::{Cluster, NewCluster}`，不必引用文件模块路径。

## 错误处理与边界

- `NewCluster` 对 store 创建与 `BootstrapSession` 的错误用 `Error::Trace` 传播，不会留下一个部分初始化的成功返回值；但在 session bootstrap 失败前已经创建的局部 Storage 由 Rust 所有权正常释放，stubs 不代表真实外部进程清理语义。
- `Start` 仅显式检查 `Storage`；随后对刚写入的 `TiDBDriver` 使用 `unwrap`，其不变量由同一函数内的赋值保证。`NewServer` 错误经 `Error::Trace` 返回。
- 后台 `Server::Run` 失败直接 panic，不能通过 `Start` 的 `Result` 返回给调用线程；接收就绪通知的结果被忽略，如果发送端断开，流程仍会继续探针阶段。
- SQL 探针耗尽 `retry_time` 会 panic，与 Go 的 `log.Panic` 对齐。SQL 成功只表示 `sql_open` 返回成功；当前 stubs 不建立真实网络连接。
- HTTP 循环刻意复刻 Go 的 `for retry = range retryTime` 终值：自然耗尽时 `retry == retry_time - 1`，因此后置的 `if retry == retry_time` 不成立，函数仍返回 DSN。该兼容边界由 `exhausted_http_retries_return_dsn_like_go_source` 固化，不能在本文件中单独“修正”；若 Go 行为变化，应连同对照测试一起更新。
- `SystemTime::duration_since(UNIX_EPOCH)` 失败时退回零时长，socket 名可能退化为固定后缀；它只降低冲突概率，不提供严格唯一性。
- `Stop` 忽略 Storage 和 HttpServer 的关闭错误，以保证后续清理继续执行；`Domain::Close`、`Server::Close` 与 `view_Stop` 本身不返回错误。

## 并发与资源生命周期

`PPROF_ONCE` 是进程级同步原语，但 `HttpServer` 句柄被闭包写入首次调用的局部 `cluster`，不是所有实例共享字段。Rust 测试 `pprof_server_is_not_shared_by_later_clusters` 明确断言两个连续实例不会都持有该句柄。

`Start` 每次都会替换全局 Go-test 就绪发送端并启动一个 Server 线程；当前接口没有防止同一实例重复 `Start`，也没有保存 `JoinHandle`。线程依赖 `Server::Close` 翻转原子状态退出，因此漏调 `Stop` 会让后台线程和在线标志存活到后续测试。并行启动多个 Cluster 还会共享 `RUN_IN_GO_TEST` 和全局通知发送端，存在相互覆盖风险；现有 API 最安全的用法是单测试内串行执行 `NewCluster → Start → Stop`。

探针钩子与在线标志是 thread-local，而 Server 在新线程运行；就绪通知走进程级 Mutex 中的 mpsc sender。`Start` 在调用线程设置在线标志后执行探针，测试应在同一线程设置/复位 `set_retry_time`、`set_skip_sleep`、`set_sql_open` 和 `set_http_get`。[`mock_cluster_test.rs`](./mock_cluster_test.rs) 与 [`parity_test.rs`](./parity_test.rs) 都通过 `reset_test_hooks` 避免状态污染。

## 与 Go 版本的对应关系

Rust `Cluster` 的 `Option<T>` 对应 Go 结构体中的嵌入指针/interface 零值；Rust 的显式字段名保留了 Go 风格大写，以便机械移植调用点。`NewCluster` 的 bootstrap 顺序、取 PD 客户端的路径、`Start` 的零端口配置、后台运行与 channel 等待、`Stop` 的关闭顺序，以及 DSN 默认值均对照 [`mock_cluster.go`](./mock_cluster.go)。

当前 Rust 不是重型 Go 依赖的等价运行时：`HttpServer::ListenAndServe` 不 bind，`NewServer` 使用固定测试端口，`sql_open`/`http_get` 默认只检查线程局部 online 标志，`DisableStats4Test` 和 `view_Stop` 是空操作，PD 客户端也只是带 id 的占位结构。因此它验证生命周期与接口契约，不证明真实 TiDB/PD/TiKV 的网络、调度或存储行为。

Rust 额外增加了可注入、线程局部的重试和探针钩子，以便独立测试成功、重试及 panic 边界；同时用纳秒 `SystemTime` 对应 Go 的 `time.Now().UnixNano()`。HTTP 耗尽后仍返回 DSN 是有测试保护的 Go 源码精确对齐行为。

## 扩展指南

- 新增 Cluster 生命周期字段时，先在 `Cluster::default` 给出明确空态，再分别确定由 `NewCluster`、`Start` 还是 `Stop` 负责建立与释放；同步更新 [`stubs.rs`](./stubs.rs) 的可观察状态。
- 修改 bootstrap 链时，应保持 `BootstrapWithSingleStore` 在 `BootstrapSession` 之前，并在 [`mock_cluster_test.rs`](./mock_cluster_test.rs) 独立断言 store 数、bootstrap 标志及新增句柄。
- 修改 Server 配置或就绪协议时，重点检查全局 channel 被并发/重复 `Start` 覆盖的风险、后台线程退出条件和 `Stop` 是否能释放新增资源。
- 修改 DSN 或重试语义时，同时对照 Go `getDSN`/`waitUntilServerOnline`，扩展 [`parity_test.rs`](./parity_test.rs) 的成功、SQL 耗尽、HTTP 耗尽和返回前缀用例；不要把测试逻辑放回生产 `.rs` 文件。
- 若目标从“接口/生命周期替身”升级到真实网络集群，应在 Cargo 边界显式引入对应依赖并重新评估 macOS arm64/grpcio 约束，不能只替换一个探针函数就宣称与 Go 完全等价。
- 性能风险主要在固定 10ms × 100 次的轮询上；测试可通过 stubs 钩子缩短，但生产对照常量与 Go 行为应保持同步。

## 验证依据

- 源码与模块：[`mock_cluster.rs`](./mock_cluster.rs)、[`lib.rs`](./lib.rs)、[`stubs.rs`](./stubs.rs)。
- crate 边界：[`Cargo.toml`](./Cargo.toml)；workspace 根 `Cargo.toml` 将 `br/pkg/mock` 列为成员，`br/pkg/utiltest/Cargo.toml` 以路径依赖使用它。
- Go 对照：[`mock_cluster.go`](./mock_cluster.go) 和 [`mock_cluster_test.go`](./mock_cluster_test.go)。
- Rust 独立测试：[`mock_cluster_test.rs`](./mock_cluster_test.rs) 覆盖主生命周期和 pprof Once 归属；[`parity_test.rs`](./parity_test.rs) 覆盖 DSN、SQL 重试、SQL panic、HTTP 耗尽兼容语义和资源关闭。
- 上游调用：[`br/pkg/utiltest/suite.rs`](../utiltest/suite.rs) 的 `CreateRestoreSchemaSuite` 与 `TestRestoreSchemaSuite::{Stop, Drop}`。
- RustCodeGraph：索引状态为 7032 个 Rust 文件；`query NewCluster --kind function` 定位 Rust/Go 对照符号；`callees` 核验了 `NewCluster`、`Start`、`Stop`、`getDSN`、`waitUntilServerOnline` 的直接下游边。宽泛 `explore` 与 `callers` 查询未在时限内返回，所以上游事实改由索引搜索候选后使用 `rg` 核对直接引用，没有据此推断未验证的调用者。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查目标文件存在且恰含规定的十一个二级章节，并人工复核以上路径和符号可追溯。
