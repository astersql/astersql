# `cmd/benchkv/main.rs`

## 文件定位

源码 [`cmd/benchkv/main.rs`](./main.rs) 是 `astersql-cmd-benchkv` crate 的共享业务入口，由 `cmd/benchkv/lib.rs` 以 `#[path = "main.rs"] pub mod entry` 纳入。可执行文件 `cmd/benchkv/bin_main.rs::main` 只调用 `astersql_cmd_benchkv::main()`，crate 根的 `cmd/benchkv/lib.rs::main` 再转发到本文件的 `entry::main()`；因此命令行运行和独立 parity 测试使用的是同一套流程。

该文件是编排层，不自行实现 TiKV driver、事务、Prometheus 注册表或 HTTP 协议。上述能力来自 `cmd/benchkv/stubs.rs` 中的适配类型；其生产后端最终连接 `astersql-store-driver`、`astersql-kv`、`prometheus`、`reqwest` 和 `tiny_http`。`cmd/benchkv/Cargo.toml` 将 crate 声明为 `kind = "binary"` 的 Go 移植目标，Go 包对应 `cmd/benchkv`，实际二进制名为 `astersql-cmd-benchkv`。

## 核心职责

本文件负责五件事：

1. `default_flags` 暴露与 Go 包级 flag 一致的默认配置。
2. `main` 从进程参数构造 `Flags`，将日志级别设为 error，并选择生产依赖。
3. `init` 打开 TiKV 存储、注册三组指标、安装 `/metrics` handler，并在后台启动 `:9191` HTTP 服务。
4. `batch_rw` 把目标写入量按 worker 等分，以不重叠的 `key_<序号>` 执行事务写入并记录计数、失败数和耗时。
5. `run_with_flags` 串联初始化、压测、指标抓取、输出和响应体关闭。

它不是通用压测框架：连接 URL、指标端口、指标标签、key 格式和事务形态均固定。当前目标是保持 `cmd/benchkv/main.go` 的可观察语义，而不是改进 Go 版本的边界行为。

## 主要符号

- `pub fn default_flags() -> Flags`：返回 `Flags::default()`。真实默认值定义在 `cmd/benchkv/stubs.rs::Flags::default`：`N=1_000_000`、`C=400`、`pd=localhost:2379`、`V=5`。
- `pub fn main()`：进程级入口。调用 `stubs::args_from_env`、`stubs::parse_flags_or_exit`、`stubs::set_log_level(Error)`，最后以 `RuntimeDeps::default()` 调用 `run_with_flags`。生产解析策略对 help 退出 0、对参数错误退出 2。
- `pub fn run_with_flags(flags: Flags, deps: RuntimeDeps)`：可测试的完整运行入口。显式接收依赖以替代 Go 版本的包级可变状态。
- `pub fn init(flags: &Flags, deps: &RuntimeDeps) -> (Storage, Metrics, HttpServer)`：初始化并返回共享的存储、指标和 HTTP 句柄。测试可用 `RuntimeDeps::store_override` 绕过真实 Open。
- `pub fn batch_rw(flags: &Flags, store: &Storage, metrics: &Metrics, value: &[u8])`：并发事务核心。函数在所有 worker 线程 join 后才返回。

本文件没有自定义常量、类型、trait、`impl` 或条件编译项；五个函数都是 `pub`，但它们实际通过 crate 内的 `entry` 模块暴露，外部稳定入口仍是 crate 根 `main`。

## 执行流程

生产调用链为 `bin_main.rs::main -> lib.rs::main -> entry::main -> run_with_flags -> init/batch_rw`。

`main` 首先读取进程参数并按 Go `flag` 的兼容子集解析四个选项，然后把日志级别设为 error。`run_with_flags` 调用 `init`：若没有预置 store，就拼接 `tikv://<pd_addr>?cluster=1` 并通过 `TiKVDriver::Open` 打开；随后注册 `tikv_txn_total`、`tikv_txn_failed_total`、`tikv_txn_durations_histogram_seconds`，安装动态指标 handler，并启动监听线程。

初始化后，`run_with_flags` 将 `value_size` 转为 `usize` 并创建全零 value，记录整次压测起点，再调用 `batch_rw`。后者先计算 `base = data_cnt / worker_cnt`，为每个 worker 克隆共享句柄和值，并启动线程。worker 对自己的 `j in 0..base` 循环执行：

1. 增加总事务计数并记录单事务起点。
2. 以 `k = base * worker_index + j` 生成不重叠序号。
3. `Begin` 事务，构造 `key_<k>`，调用 `Set` 和 `Commit`。
4. Commit 出错时增加失败计数并调用 `Rollback`。
5. 无论 Commit 是否成功，都观察一次从 Begin 后到提交/回滚完成的秒级耗时。

主线程 join 全部 worker 后，`run_with_flags` GET `http://localhost:9191/metrics`，读取并打印指标正文，再打印 `elapse` 和配置中的目标总数，最后关闭响应体。

## 数据与状态

`Flags` 是本文件唯一的运行配置输入，包含 `data_cnt: i64`、`worker_cnt: i64`、`pd_addr: String`、`value_size: i64`。value 是长度为 `value_size` 的零字节数组；每个 worker 获得一份 `to_vec()` 副本，因此线程间不共享可变 value。

存储与指标通过可克隆句柄在线程间共享。生产 `Storage` 包装 `astersql_store_driver::TikvStore`；测试后端用 `Arc<Mutex<StorageInner>>` 记录 Begin、成功 Set/Commit、Rollback 和故障注入状态。`Metrics` 内部的 counter/histogram 句柄也可克隆，并把所有样本固定标注为 `type="txn"`。

key 空间的不变量是：同一次 `batch_rw` 中，worker `i` 只处理 `[base*i, base*i+base)`。当 `data_cnt >= 0` 且 worker 数为正时，不同 worker 的区间互不重叠。实际尝试数是 `base * worker_cnt`；若不能整除，余数不会写入，但最终摘要仍打印原始 `flags.data_cnt`，这与 Go 版本一致。

`init` 返回的 `HttpServer` 与后台线程持有同一克隆状态。动态 handler 在抓取时读取当前指标，避免在初始化阶段缓存全零文本。

## 依赖与调用关系

上游直接关系：

- `cmd/benchkv/bin_main.rs::main` 调用 `astersql_cmd_benchkv::main`。
- `cmd/benchkv/lib.rs::main` 调用本模块 `entry::main`。
- `cmd/benchkv/parity_test.rs` 直接导入 `default_flags`、`init`、`batch_rw` 和 `run_with_flags`，覆盖公开契约。

RustCodeGraph 对目标文件的节点读取报告 `cmd/benchkv/parity_test.rs` 是直接使用者；调用图还确认 `main -> run_with_flags`、`run_with_flags -> init/batch_rw/log_function_call_errored`、`init -> Metrics::must_register_all/HttpServer::HandleMetrics`、`batch_rw -> terror_call/fatal_process`。

下游适配集中在 `cmd/benchkv/stubs.rs`：`TiKVDriver::Open` 在生产模式调用 `astersql_store_driver::TiKVDriver::Open`；`Storage::Begin` 返回包装的 `astersql_kv::Transaction`；`Transaction::{Set,Commit,Rollback}` 转发到 canonical KV trait；`Metrics::production` 使用 `prometheus`；`HttpServer::ListenAndServe` 使用 `tiny_http`，客户端抓取由 `http_get` 的生产路径使用 `reqwest`。

`cmd/benchkv/Cargo.toml` 没有 feature 条件，本文件也没有 `cfg` 分支；生产和测试差异由 `RuntimeDeps::{default,for_test}` 运行时选择。

## 错误处理与边界

- 参数解析：`parse_flags_or_exit` 保持进程语义；help 退出 0，未知或非法 flag 输出 usage 并退出 2。独立测试还固定了遇到首个位置参数或 `--` 后停止解析，以及 Go base-0 整数语法。
- `value_size < 0`：`usize::try_from(...).expect("negative value size")` panic，对齐 Go 用负长度 `make` 的 panic。
- `worker_cnt == 0`：在计算 `data_cnt / worker_cnt` 时 panic。`worker_cnt < 0`：转 `usize` 时 panic，对齐 Go `WaitGroup.Add` 拒绝负数。
- 存储 Open 失败：`init` 通过 `must_nil` 作为致命错误传播。
- Begin 失败：生产存储调用 `fatal_process`，终止进程；测试存储调用可捕获的 `fatal`。这一区分让生产行为保持 Go `log.Fatal`，同时允许 parity 测试验证分支。
- Set 失败：只经 `terror_log(trace(...))` 记录，仍继续 Commit，完全保留 Go 现状。
- Commit 失败：增加 rollback 指标并调用 Rollback；Commit 错误本身不直接输出，Rollback 错误交给 `terror_call` 记录。
- HTTP GET 失败：`must_nil` 致命；读取 body 失败只记录；关闭 body 失败调用 `log_function_call_errored`，不改变退出状态。
- 监听失败：后台线程只记录错误。线程自身 panic 不会被主流程 join；worker panic 则由 `join` 后 `resume_unwind` 传播。

负 `data_cnt` 没有专门拒绝：在 worker 的 `0..base` 范围为空时不会产生事务。文档只记录当前事实，不把它描述为经过设计的有效输入。

## 并发与资源生命周期

`init` 为指标服务器启动一个 detached `std::thread`。生产 `ListenAndServe` 是持续接收循环，所以线程和监听 socket 通常存活到进程退出；本文件没有 shutdown 句柄，也不 join 该线程。测试 dry-run 后端只设置启动标记并立即返回。

`batch_rw` 每次调用创建 `worker_cnt` 个线程，保存全部 `JoinHandle`，最后逐个 join。这是 Go `WaitGroup.Wait` 的对应实现，保证压测写入和指标观察完成后才抓取 `/metrics`。任一 worker panic 会在 join 阶段重新抛出；由于 join 是按句柄顺序进行，即使前一个传播 panic，其余尚未 join 的线程仍会自行继续执行，但本调用不再显式等待它们。

每笔循环新建一个事务。成功 Commit 后结束；Commit 失败时显式 Rollback；Set 失败仍尝试 Commit；Begin 失败没有有效事务资源。响应体在指标读取和两次打印之后显式 `Close`，语义对应 Go 的 defer（函数返回前关闭）。本文件没有显式关闭 `Storage`，其生命周期由句柄析构和底层 driver 管理。

共享测试状态使用互斥锁，生产存储和 Prometheus 句柄依赖各自库的并发契约。key 分片避免应用层写冲突，但不保证外部已有数据或多个 benchkv 进程之间不冲突。

## 与 Go 版本的对应关系

Rust `default_flags` 与 Go `var` 中四个 flag 默认值一致；Rust `init` 对应 Go `Init`；Rust `batch_rw` 对应 Go `batchRW`；Rust `main` 加 `run_with_flags` 合起来对应 Go `main`。主要结构差异是 Rust 把 Go 包级 `store`、指标和默认 HTTP 全局状态收拢进 `RuntimeDeps`，从而让测试可以注入 stub，但 `RuntimeDeps::default` 仍明确选择真实 TiKV、Prometheus 和 HTTP 后端。

下列看似可改进的行为是刻意保留的 Go 语义：整除余数丢弃；零 worker、负 worker、负 value size 触发 panic；Begin 致命；Set 错误只记录后继续 Commit；Commit 失败才计入 `failed_total` 并回滚；摘要打印配置目标数而非实际写入数；监听、读取和关闭错误只记录。

Rust 的事务 Commit 不接收调用处创建的 context，而是由适配层用 `astersql_kv::Context::default()` 转发，外部流程仍对应 Go 的 `Commit(context.Background())`。Go 用 goroutine，Rust 用 OS 线程；Go 用 defer 关闭 HTTP body，Rust 在正常输出后显式关闭。若在读取或打印期间 panic，Rust 不具备该 defer 的异常路径清理保证，这是实现层差异。

`cmd/benchkv/parity_test.rs` 是 Rust 独立测试文件；Go 同目录没有 `main_test.go`。该 parity 测试以 Go `main.go` 为契约来源，覆盖默认值、URL、指标 bucket、key 分片、参数解析、错误分支、响应关闭以及等待 worker 等行为。

## 扩展指南

- 新增 CLI 参数时：扩展 `stubs.rs::Flags`、`try_parse_flags`、usage 与默认值；在 `main.rs` 的消费位置接线，并在 `parity_test.rs` 增加默认值、两种赋值形式和错误输入断言。Go 仍是对应语义来源时，还需同步核对 `main.go`。
- 修改事务工作负载时：主要接入点是 `batch_rw`。必须维持或明确改变 key 唯一性、实际写入数、Begin/Set/Commit/Rollback 顺序以及指标时延覆盖区间；测试应继续位于独立的 `parity_test.rs`，不要嵌入生产源文件。
- 新增指标时：在 `stubs.rs::Metrics` 的 stub 与 production 两套后端同时定义、注册和渲染，在 `init` 注册 handler 前完成组装，并测试真实 Prometheus 文本名称。注意全局注册表重复注册会 fatal。
- 使端口或 URL 可配置时：同时修改 `init` 的监听地址与 `run_with_flags` 的抓取 URL，避免服务端和客户端漂移；增加 loopback 测试，并考虑端口占用和后台线程退出策略。
- 改善余数分配、输入校验、取消机制或 worker 池会改变已由 parity 测试固定的 Go 契约，不能作为无风险重构处理。
- 性能风险主要来自每次运行创建 `worker_cnt` 个 OS 线程、每线程复制完整 value、每事务格式化 key，以及每笔写入的指标同步。优化前应分别衡量内存、线程调度和指标开销，并保持失败路径可观察性。
- 若增加可控 shutdown 或存储关闭，需要明确 HTTP 线程、所有 worker、响应体和底层 store 的关闭顺序，并新增独立生命周期测试。

## 验证依据

分析直接读取或查询了以下证据：

- `cmd/benchkv/main.rs`：五个函数的完整实现和行内不变量。
- `cmd/benchkv/bin_main.rs`、`cmd/benchkv/lib.rs`：二进制到共享入口的两级转发及测试模块接线。
- `cmd/benchkv/Cargo.toml`：crate 类型、二进制入口、Go 包映射和五项直接依赖。
- `cmd/benchkv/main.go`：`Init`、`batchRW`、`main`、flag、指标、错误分支和资源关闭的原始对应语义。
- `cmd/benchkv/parity_test.rs`：正常、边界、错误、生产后端和资源清理的独立回归证据。
- `cmd/benchkv/stubs.rs`：`Flags`、`RuntimeDeps`、`TiKVDriver`、`Storage`、`Transaction`、`Metrics`、`HttpServer` 的生产/测试后端和同步方式。
- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标及同目录六个源文件均已索引。
- RustCodeGraph `node --file cmd/benchkv/main.rs`：确认文件共 155 行并由 `cmd/benchkv/parity_test.rs` 使用。
- RustCodeGraph `query`：确认 `default_flags`、`main`、`run_with_flags`、`init`、`batch_rw` 的定义位置；`callees` 结果确认本文件内部主链以及对 `stubs.rs` 的关键调用。精确 `callers` 查询未在 60 秒内返回，因此上游关系同时由已索引文件使用关系、`bin_main.rs`、`lib.rs` 和测试导入直接核验。

本任务是纯文档分析，未运行 Cargo。结构验收应确认本文存在，且恰好含有任务规定的十一个二级标题。
