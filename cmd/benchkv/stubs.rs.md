# `cmd/benchkv/stubs.rs`

源文件：[`stubs.rs`](./stubs.rs)  
所属 crate：`astersql-cmd-benchkv`（[`Cargo.toml`](./Cargo.toml)）

## 文件定位

`stubs.rs` 是 `benchkv` 命令的运行时边界适配层，而不是单纯的空实现集合。crate 根在 [`lib.rs`](./lib.rs) 中以 `pub mod stubs` 暴露本模块；[`main.rs`](./main.rs) 的 `main`、`run_with_flags`、`init` 和 `batch_rw` 通过它访问命令行参数、TiKV 事务、Prometheus 指标、HTTP 服务/客户端以及 Go 风格错误处理。

该文件同时提供两套后端：`RuntimeDeps::default()` 组合真实 `astersql_store_driver::TiKVDriver`、Prometheus 默认注册表、`tiny_http` 服务和 `reqwest` 客户端；`RuntimeDeps::for_test()` 组合内存事务状态、内存指标及 `dry_run` HTTP 服务。生产入口不会静默使用测试桩，测试也无需连接 PD/TiKV 或固定端口。

`Cargo.toml` 将本目录声明为 `kind = "binary"` 的迁移 crate，库入口为 `lib.rs`，二进制包装为 `bin_main.rs`；本文件直接依赖 `astersql-kv`、`astersql-store-driver`、`prometheus`、`reqwest`（blocking）和 `tiny_http`。

## 核心职责

1. 以 `Error`、`trace`、`must_nil`、`terror_log`、`terror_call`、`fatal`/`fatal_process` 模拟 Go `errors`、`terror` 和日志调用在 benchkv 范围内可观察到的控制流。
2. 以 `Flags`、`try_parse_flags`、`parse_flags_or_exit` 实现 Go `flag` 的本命令子集，保留默认值、停止解析规则、base-0 整数语法和进程退出码。
3. 以 `TiKVDriver`、`Storage`、`Transaction` 统一真实与测试事务表面，维持 `Open → Begin → Set → Commit/Rollback` 调用顺序。
4. 以 `Metrics`、`CounterVec`、`HistogramVec` 维护事务尝试、回滚和耗时指标，并在真实后端注册/导出 Prometheus collector，在测试后端生成可断言文本。
5. 以 `HttpServer`、`HttpResponse`、`read_all`、`http_get` 封装 `/metrics` 服务、抓取和响应体关闭生命周期。
6. 以 `RuntimeDeps` 把上述边界显式注入 `main.rs`，避免测试依赖 Go 版本的包级全局状态。

## 主要符号

- 错误与日志：`Error { msg }` 只保留 benchkv 消费的错误文本；`must_nil` 在测试友好路径 panic，`fatal_process` 在真实事务 `Begin` 失败时退出码 1；`logged`/`take_logs`/`clear_logs` 保存测试可见日志；`LogLevel` 当前只有 `Error`。
- 参数：`Flags { data_cnt, worker_cnt, pd_addr, value_size }` 默认分别为 `1_000_000`、`400`、`localhost:2379`、`5`。`FlagParseError` 区分帮助与解析错误；`parse_go_int` 处理符号、二/八/十/十六进制和 Go 合法下划线。
- 存储：`TiKVDriver` 由私有 `DriverMode::{Stub, Production}` 决定打开方式。`StorageBackend` 保存 `Arc<Mutex<StorageInner>>` 或真实 `TikvStore`；`StorageInner` 记录 begin、成功 set、成功 commit、rollback 及故障注入状态。
- 事务：`TransactionBackend::{Stub, Production, Invalid}`；测试事务只缓存一个 `pending` 键值对，提交成功后才写入共享统计，真实事务委托 `astersql_kv::Transaction`。
- 指标：`CounterVec`/`Counter` 按 label 键计数；`HistogramVec`/`Histogram` 保存原始观测并可同时写真实 Prometheus；`Metrics` 汇总三组指标和一次性注册状态。
- HTTP：`HttpServer` 保存监听地址、启动状态、路由、注入错误和动态指标句柄；`HttpBody` 区分内存正文与被互斥锁保护的网络响应；`HttpResponse::Close` 负责释放网络响应并记录关闭状态。
- 聚合入口：`RuntimeDeps::{default, for_test}` 是生产/测试后端的唯一明确选择点；`duration_seconds` 对齐 Go `Duration.Seconds()`。`TXN_SEQ`/`reset_txn_seq` 当前未进入主流程，仅保留测试复位入口。

文件没有条件编译项；测试模块由 `lib.rs` 上的 `#[cfg(test)]` 独立加载 [`parity_test.rs`](./parity_test.rs)，符合测试逻辑不内嵌生产源文件的约束。

## 执行流程

生产主链为：`lib.rs::main → main.rs::main → args_from_env → parse_flags_or_exit → set_log_level → run_with_flags`。`run_with_flags` 调用 `main.rs::init`，后者从 `RuntimeDeps` 取得依赖，若没有 `store_override`，则构造 `tikv://<pd>?cluster=1` 并调用 `TiKVDriver::Open`；随后 `Metrics::must_register_all`、`HttpServer::HandleMetrics`，并在线程中调用 `ListenAndServe(":9191")`。

写入主链为 `main.rs::batch_rw → Storage::Begin → Transaction::Set → Transaction::Commit`。每个 worker 处理 `base = data_cnt / worker_cnt` 个互不重叠的 `key_<n>`；提交失败时增加失败指标并通过 `terror_call` 调用 `Rollback`。每次尝试都会增加总事务计数并记录从 Begin 后到提交/回滚结束的秒数。

收尾主链为 `http_get("http://localhost:9191/metrics") → read_all → HttpResponse::Close`。真实 HTTP 服务收到 `/metrics` 时动态调用 `Metrics::render`；其他路径返回 404。测试 HTTP 优先动态渲染已注册 handler，其次读取 `/metrics` 缓存路由，最后直接渲染传入指标。

测试事务的关键顺序是：`Begin` 先增加尝试数并分配提交序号；`Set` 仅覆盖当前事务的单个 `pending`；成功 `Commit` 才把该键值加入 `StorageInner::sets` 并增加 commits；`Rollback` 丢弃 pending 并增加 rollbacks。故提交失败不会留下成功写入。

## 数据与状态

- `StorageInner` 通过 `Arc<Mutex<_>>` 在克隆的 `Storage` 和各 worker 事务之间共享。成功写入以 `(Vec<u8>, Vec<u8>)` 保存；故障注入包括全部 Begin 失败、全部 Set 失败和每第 N 次 Commit 失败。
- `next_commit_id` 在 Begin 成功路径分配，`fail_commit_every` 使用一基序号判断 `(commit_id + 1) % n == 0`。生产存储的测试统计访问器返回零/空集合，不伪造真实后端内部统计。
- 指标 label 以逗号拼接为内存键；当前 benchkv 只使用 `type="txn"`。直方图保留原始 `f64` 秒值，测试渲染时再计算累计 bucket、`+Inf`、sum 和 count。
- `Metrics::registered` 用 `AtomicBool` 防止同一对象重复注册；生产 collector 仍进入 Prometheus 全局注册表，因此跨对象重复名称也会由注册库报错并转为 fatal。
- `HttpServer` 的地址、路由、错误和指标对象通过 `Mutex` 共享，启动状态及是否挂载动态指标使用 `AtomicBool`。`HttpResponse` 的网络响应放在 `Mutex<Option<_>>` 中，关闭时 `take()`，之后再次读取返回 `response body is closed`。
- 全局日志池和日志级别由 `OnceLock<Mutex<_>>` 延迟初始化；这些状态服务于 parity 断言，需要测试通过 `clear_logs` 隔离场景。

## 依赖与调用关系

上游直接调用者集中在 [`main.rs`](./main.rs)：`main` 使用参数与生产 `RuntimeDeps`；`init` 使用 `TiKVDriver::Open`、指标注册和 HTTP 启动；`batch_rw` 使用事务、指标和错误包装；`run_with_flags` 使用 HTTP 抓取、读取和关闭。独立 [`parity_test.rs`](./parity_test.rs) 直接调用测试构造器、故障注入器和状态访问器验证契约。RustCodeGraph 的文件关系显示 `stubs.rs` 在本 crate 中由 `lib.rs` 声明，主要流程调用边包括 `run_with_flags → http_get/read_all/log_function_call_errored`、`init → Open/must_register_all/HandleMetrics/ListenAndServe`、`batch_rw → Begin/Set/Commit/Rollback`。

下游真实依赖如下：

- `TiKVDriver::Open` 调用 `astersql_store_driver::TiKVDriver::Open`，`Storage::Begin` 通过 `astersql_kv::Storage` trait，事务调用 `astersql_kv::Transaction` 的 `Set`、`Commit(Context::default())`、`Rollback`。
- `Metrics::production` 构造 `prometheus::{CounterVec, HistogramVec}`；注册使用全局 `prometheus::register`，渲染使用 `prometheus::gather` 和 `TextEncoder`。
- `HttpServer::ListenAndServe` 使用 `tiny_http` 同步接收循环；生产 `http_get` 使用 `reqwest::blocking::get`。
- 标准库负责线程共享状态、原子标记、I/O、环境参数和持续时间转换。

`Cargo.toml` 没有 feature 开关，所以生产/测试后端不是编译期裁剪，而是由 `RuntimeDeps` 和私有 mode/backend 枚举在运行时明确选择。

## 错误处理与边界

- 空 TiKV path 直接返回 `empty tikv path`；真实 driver、事务、HTTP 和编码错误都降格为本地字符串 `Error`，因此这里不保留类型化错误码或堆栈。
- `parse_flags_or_exit` 对帮助打印 usage 并退出 0，对未知 flag、缺值或非法整数退出 2；`parse_flags` 则 panic，供测试捕获。解析在 `--`、单独 `-` 或第一个位置参数处停止。
- base-0 整数接受 `0b`、`0o`、`0x`、传统前导零八进制、正负号及受约束的下划线，并检查 `i64` 溢出。该实现仅覆盖 benchkv 的四个 flag，不是完整 Go `flag` 包替代品。
- `worker_cnt == 0` 在整除时 panic；负 worker 在转 `usize` 时 panic；负 `value_size` 在 `run_with_flags` 分配缓冲区时 panic，均由 parity test 固定为 Go 对齐行为。`data_cnt` 不能整除 worker 时余数不补写。
- Begin 失败是致命错误：测试后端 panic，生产后端进程退出 1。Set 失败只记录日志并继续 Commit；Commit 失败增加失败指标并尝试 Rollback；监听、读取、关闭和回滚错误只记录日志，不覆盖主流程错误。
- 测试故障注入仅允许内存存储；对生产存储调用注入器会 panic。`Metrics::must_register_all` 的重复调用也会 fatal。
- 测试 `http_get` 只接受包含 `localhost:9191` 或 `127.0.0.1:9191` 的 URL；真实后端不施加该限制。真实服务是无限接收循环，没有本文件提供的优雅停机接口。

## 并发与资源生命周期

`main.rs::init` 克隆 `HttpServer` 后启动后台线程，测试 dry-run 线程只设置 started 并返回；生产线程在 `tiny_http` 接收循环中长期存活。该线程没有 join handle 或 shutdown channel，生命周期随进程结束；扩展嵌入式使用场景时必须补充显式停止协议。

`batch_rw` 为每个 worker 克隆 `Storage`、`Metrics` 和 value，最后 join 所有线程；任一 worker panic 会由 `resume_unwind` 传播。测试存储和指标的互斥锁保证计数与集合更新串行一致；原子量使用 `SeqCst`，强调可观察顺序而非极致性能。锁中毒统一通过 `unwrap` 转为 panic。

事务没有 `Drop` 自动回滚：只有 Commit 返回错误的显式分支会调用 Rollback；Begin/Set/Commit 的其他 panic 路径不会由本适配层补偿。网络响应也没有自定义 `Drop` 契约，主流程必须显式 `Close`；关闭会取走 `reqwest::blocking::Response`，测试响应则仅翻转原子标记并返回预置错误。

Prometheus 生产 collector 注册到进程全局注册表，无法由 `RuntimeDeps` 自动注销；因此真实后端适合单次命令进程，重复在同进程初始化会触发重复注册错误。测试后端用对象内的注册位和内存集合隔离大多数场景。

## 与 Go 版本的对应关系

直接对照文件是 [`main.go`](./main.go)。`Flags::default` 对应 Go 包级 `flag.Int/String`；`TiKVDriver::Open`、`Metrics::must_register_all`、`HttpServer::HandleMetrics/ListenAndServe` 合起来对应 `Init`；Rust `main.rs::batch_rw` 加本文件事务/指标接口对应 Go `batchRW`；HTTP 抓取、`ReadAll`、延迟关闭和日志包装对应 Go `main` 的收尾。

关键一致性包括：TiKV URL 为 `tikv://<pd>?cluster=1`；指标名最终为 `tikv_txn_total`、`tikv_txn_failed_total`、`tikv_txn_durations_histogram_seconds`，label 为 `type`；桶为 `ExponentialBuckets(0.0005, 2, 13)`；端口为 `:9191`；key 为 `key_<base*i+j>`；整除余数被舍弃；Set 错误只记日志、Commit 错误回滚。

有意的 Rust 适配差异是：Go 使用包级全局存储/指标/HTTP，Rust 用 `RuntimeDeps` 显式组合；测试 fatal 使用可捕获 panic，生产 Begin fatal 使用真实退出；测试后端自行渲染 Prometheus 文本，生产后端使用 prometheus crate；Go goroutine 对应 Rust 后台线程，WaitGroup 对应 join handles。这些差异保留了测试关注的外部行为，但不宣称复刻 Go 库的全部内部语义。

## 扩展指南

- 新增 CLI 参数时，在 `Flags`、`Default`、`try_parse_flags`、`print_defaults_to` 同步实现，并在独立 `parity_test.rs` 增加默认值、`-x value`/`-x=value`、非法值和停止解析测试；同时核对 Go `main.go`。
- 扩大 KV 操作时优先扩展 `StorageBackend`/`TransactionBackend` 两个分支，保证生产委托 canonical trait、测试记录真实可观察结果。若需要单事务多次 Set，必须把当前单个 `pending` 改为有序集合，并新增提交失败不落盘、回滚清空及覆盖语义测试。
- 新增指标时在 `Metrics::{default, production, must_register_all, render}` 四处成对维护，避免测试文本与生产 collector 漂移；新增 label 时不要继续依赖可能碰撞的逗号拼接键，宜改为结构化 label key。
- 新增 HTTP 路由或停机能力时修改 `HttpServer` 的真实接收循环和 dry-run 行为，并在 `parity_test.rs` 增加网络回环及资源清理测试。当前无限循环意味着库式重复启动/停止不是已支持能力。
- 修改错误策略时先判断 Go 分支是 fatal、返回错误还是只记录日志；不要把 Set、Commit、Close、Listen 的不同严重级别合并。生产进程退出行为需用进程级测试验证，不能只用 `catch_unwind`。
- 测试继续放在独立 [`parity_test.rs`](./parity_test.rs)，不要把 `#[cfg(test)] mod tests` 塞入本生产文件。性能风险主要来自每次指标更新和内存状态更新的互斥锁、每 worker 复制 value，以及 blocking HTTP；兼容风险主要是 Go CLI、指标文本/名称和错误控制流漂移。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；`files --filter cmd/benchkv` 确认本 crate 的 `bin_main.rs`、`lib.rs`、`main.rs`、`stubs.rs`、`parity_test.rs` 和 Go 对照文件均已索引。
- RustCodeGraph `node --file cmd/benchkv/stubs.rs` 阅读了全文件 1–1363 行；`explore "cmd/benchkv/stubs.rs symbols callers callees role in benchkv"` 核对了主要符号与调用路径。精确查询确认 `RuntimeDeps` 位于本文件并关联 `main.rs::init/run_with_flags`，`http_get` 位于本文件；通用方法名因跨仓库重名较多，调用边以带文件上下文的 explore 结果和直接入口源码交叉核验。
- 已读生产入口：[`lib.rs`](./lib.rs)、[`main.rs`](./main.rs)、[`Cargo.toml`](./Cargo.toml)；已读 Go 对照：[`main.go`](./main.go)；已读独立 Rust 测试：[`parity_test.rs`](./parity_test.rs)。同目录不存在 `doc.go`，也未发现其他 benchkv Rust 测试模块。
- `parity_test.rs` 覆盖默认值、URL、无冲突 key、整除余数、Go 整数语法、零/负 worker、负 value size、生产后端选择、真实 loopback metrics、Begin/Set/Commit 故障、回滚指标、监听/关闭日志以及 worker join；它验证适配契约而非真实 TiKV 性能。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰含上述 11 个固定二级标题，并人工复核所有“已支持”结论均能回指源码、Cargo、Go 对照或独立测试。
