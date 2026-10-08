# `pkg/util/metricsutil/common.rs`

## 文件定位

`common.rs` 是 `astersql-util-metricsutil` crate 的指标注册与 Keyspace 常量标签实现。crate 入口 `pkg/util/metricsutil/lib.rs` 以 `pub use common::*` 对外重导出本文件的公开 API；`pkg/util/metricsutil/Cargo.toml` 将 Go 对照包声明为 `pkg/util/metricsutil`，并列出配置、PD/Store 以及各子系统 metrics crate 依赖。

它实现两类入口：`RegisterMetrics` 面向普通 TiDB 进程，`RegisterMetricsForBR` 面向 BR 并在需要时向 PD 查询 Keyspace ID。但当前 Rust 进程接线尚未使用这两个真实入口：`cmd/tidb-server/main.rs:707` 解析到 `cmd/tidb-server/stubs.rs:2448` 的同名模块，`br/cmd/br/backup.rs:46` 和 `br/cmd/br/restore.rs:46` 解析到 `br/cmd/br/stubs.rs:799` 的空实现。全仓 Rust 直接搜索 `astersql_util_metricsutil::` 只发现 session 使用同 crate 的 `GetDBNames`，未发现本文件注册 API 的生产调用者。因此本文件是已实现、有独立测试的真实能力，不应误解为已接入 Rust TiDB/BR 主链。

## 核心职责

1. 维护全局 Prometheus 常量标签：合并已有标签、配置中的 Keyspace observability 标签，以及 BR 查得的 `keyspace_id`（`RegisterMetrics`、`registerMetrics`、`setKeyspaceIDConstLabel`）。
2. 统一初始化各子系统的父 collector 和 `InitMetricsVars` 绑定，使 domain、executor、session、statistics、TopSQL、TTL 等模块指向已创建的指标对象（`initParentMetricsCollectors`、`initMetrics`）。
3. 为 BR 创建带 TLS 和超时配置的 PD Keyspace 客户端，对“PD 未 bootstrap”和“Keyspace 不存在”做线性退避重试，并在成功或查询失败后关闭客户端（`RegisterMetricsForBR`、`getKeyspaceMetaWithRetry`）。
4. 通过 `PdClient`/`PdClientFactory` 和 `SetPdClientFactory` 隔离网络实现，使独立 Rust 测试可验证 PD 参数、重试和关闭语义。

## 主要符号

- 常量 `componentName` / `keyspaceIDLabel` / `pdTimeout` / `defaultMaxRetries` / `retryInterval`：分别是 PD 调用方标识、`keyspace_id` 标签键、10 秒建连超时、30 次最大尝试和 500ms 基础退避（`common.rs:47-56`）。
- `TlsConfig::{IsEnabled, ToPDSecurityOption}`：仅 `ca_path` 非空时认为 TLS 已启用，并克隆三个证书路径为本地 `SecurityOption`（`common.rs:58-80`）。
- `KeyspaceMeta`：本模块只保留 PD Keyspace 元数据中注册标签所需的 `u32 id`（`common.rs:90-94`）。
- `PdErrorKind` / `MetricsUtilError`：将 PD 错误收敛为 `NotBootstrapped`、`KeyspaceNotExist`、`Unexpected` 三类；前两类由私有 `retryable` 判定为可重试，错误的 `Display` 仅输出 `message`（`common.rs:96-138`）。
- `PdClient` / `PdClientFactory`：都要求 `Send + Sync`。前者提供同步 `LoadKeyspace` 与显式 `Close`，后者接收 component、PD 地址、TLS、超时和 `init_metrics` 选项以创建 trait object（`common.rs:140-156`）。
- `ProductionPdClientFactory` / `ProductionPdClient`：把 `NetworkPdKeyspaceClient` 包在 `Mutex<Option<_>>` 中；生产工厂调用 `NetworkPdKeyspaceClient::connect`，并用 `map_pd_keyspace_error` 保持错误分类（`common.rs:162-228`）。
- `SetPdClientFactory` / `pdClientFactory`：读写 `OnceLock<RwLock<Option<Arc<dyn PdClientFactory>>>>`；槽位为 `None` 时每次返回新的生产工厂（`common.rs:230-245`）。
- `RegisterMetrics`：NextGen 内核先从全局配置写入 `keyspace_name`，然后进入通用注册流程（`common.rs:247-254`）。
- `RegisterMetricsForBR`：空 Keyspace 直接注册；否则可先写 `keyspace_name`，再建立 PD 客户端、查 ID、写 `keyspace_id`、注册，最后关闭客户端（`common.rs:256-282`）。
- `initParentMetricsCollectors` / `initMetrics`：前者补齐 Rust 拆 crate 后的父 collector，包括两段 `unsafe` 初始化；后者按固定顺序绑定子模块变量，并用 `Once` 保证 UniStore 指标只注册一次（`common.rs:284-341`）。
- `registerMetrics`、`cloneConstLabels`、`setKeyspaceIDConstLabel`、`setConstLabels`：构成标签读取、合并、ID 写入和最终批量设置链（`common.rs:343-379`）。
- `getKeyspaceMeta` / `getKeyspaceMetaWithRetry`：前者固化生产重试参数和 `std::thread::sleep`，后者注入尝试次数、基础退避与 sleep 函数供测试验证（`common.rs:381-418`）。

## 执行流程

`RegisterMetrics` 的流程是：

1. 读取 `config::get_global_config()`。
2. 如果 `kerneltype::IsNextGen()`，先把配置中的 Keyspace 名称以 `keyspace_name` 写入 metrics common 的全局常量标签。
3. `registerMetrics` 克隆已有标签，以 `HashMap::extend` 合并 `GetKeyspaceObservabilityMetricLabels()`。同键时配置标签覆盖已有值。
4. 非空标签经 `setConstLabels` 按键排序并展平为 `[key, value, ...]`，然后交给 `metricscommon::SetConstLabels`。
5. `initMetrics` 先初始化父 collector，再按源码顺序调用 13 个子模块初始化函数；UniStore 存储下通过 `REGISTER_UNISTORE_METRICS.call_once` 额外注册。

`RegisterMetricsForBR` 的流程是：

1. Keyspace 名为空时不连 PD，直接执行通用注册。
2. 非空时，NextGen 模式先写 `keyspace_name`；只有 `tls.IsEnabled()` 为真才复制 TLS 配置，否则传空 `SecurityOption`。
3. 工厂以 component `tidb-metrics-util`、调用方 PD 地址、10 秒超时和 `init_metrics = false` 创建客户端。创建失败立即返回，没有可关闭的客户端。
4. `getKeyspaceMeta` 最多尝试 30 次。第 `attempt` 次可重试失败后睡眠 `500ms * attempt`，成功立即返回，`Unexpected` 立即失败。
5. 成功时写入十进制 `keyspace_id` 并注册指标。无论 Keyspace 查询/后续注册成功还是失败，已创建的客户端都在返回前执行 `Close`。

## 数据与状态

- 指标标签的真正全局状态由 `astersql_metrics_common::{GetConstLabels, SetConstLabels}` 持有。本文件每次取得所有权独立的 `HashMap<String, String>` 后再整体写回；`cloneConstLabels` 本身不会修改全局状态。
- 标签来源有三层：先前已设定的常量标签、全局配置的 Keyspace observability map、BR 从 PD 查得的 `keyspace_id`。`RegisterMetrics`/`RegisterMetricsForBR` 还可在 NextGen 下预先写 `keyspace_name`。
- `PD_CLIENT_FACTORY` 是惰性初始化的全局可选工厂，主要是测试注入点。`SetPdClientFactory(None)` 不存储生产工厂，而是让后续读取回退到 `ProductionPdClientFactory`。
- `REGISTER_UNISTORE_METRICS: Once` 在进程生命期内单向从未执行转为已执行，无法重置。其他子模块的重复初始化安全性由各自实现承担；Rust 测试明确连续调用两次 `registerMetrics`。
- `ProductionPdClient.client` 是 `Mutex<Option<NetworkPdKeyspaceClient>>`。`Close` 通过 `take()` 丢弃网络客户端；关闭后再调用 `LoadKeyspace` 会得到 `Unexpected("PD client is closed")`。

## 依赖与调用关系

上游关系：

- `pkg/util/metricsutil/lib.rs` 声明私有 `common` 模块并重导出全部公开符号。
- `pkg/util/metricsutil/common_test.rs` 是 `lib.rs` 通过 `#[cfg(test)] #[path = "common_test.rs"]` 接入的独立测试模块，直接使用公开符号及 crate 内可见的 `registerMetrics`/`getKeyspaceMetaWithRetry`。
- RustCodeGraph `node --file` 报告本文件“used by 26 files”，但精确 `callers/callees` 查询未返回边明细。经 Rust 直接搜索复核，真实 TiDB/BR 启动文件使用的是本地 stub，本文件的注册 API 目前没有已确认的生产 Rust 调用者。
- Go 主链已接线：`cmd/tidb-server/main.go:465` 调用 `metricsutil.RegisterMetrics`，`br/cmd/br/backup.go:31` 与 `br/cmd/br/restore.go:36` 调用 `RegisterMetricsForBR`。这些是迁移意图的对照证据，不是 Rust 已接线证据。

下游关系：

- 配置与分支：`astersql_config::get_global_config`、`GetKeyspaceObservabilityMetricLabels`、`StoreTypeUniStore.String`，以及 `astersql_config_kerneltype::IsNextGen`。
- 标签存储：`astersql_metrics_common::{GetConstLabels, SetConstLabels}`。
- PD 网络：`astersql_store::NetworkPdKeyspaceClient::connect/load_keyspace`，以及 `NetworkSecurity`、`PdKeyspaceErrorKind`。
- 指标初始化：domain、executor、infoschema、planner core、server、session、transaction isolation/info、statistics handle/cache、store coprocessor/UniStore、TopSQL reporter、TTL 的 metrics crate，与直接构造 RCCheckTS counter 所用的 `prometheus::CounterVec`。

`pkg/util/metricsutil/Cargo.toml` 还声明了 sessionctx、keyspace、util 及可选 `astersql-metrics` 等 crate，但本文件没有直接 import 它们；不应仅凭 manifest 将它们写成本文件的运行时调用边。

## 错误处理与边界

- 工厂建连、PD 查询和指标注册都以 `Result<_, MetricsUtilError>` 向上传播。当前 `initMetrics` 总是返回 `Ok(())`，子模块的初始化 API 也未向它返回可恢复错误。
- 仅 `NotBootstrapped` 和 `KeyspaceNotExist` 重试；`Unexpected` 不 sleep 且立即返回。耗尽时返回最后一个可重试错误。`max_retries == 0` 时循环不执行，返回合成的 `Unexpected("PD keyspace lookup did not run")`。
- 每次可重试失败都会 sleep，包括最后一次尝试失败之后；3 次、7ms 的测试期望 `[7, 14, 21]ms`。这是当前实现的明确边界，修改时需与 Go `util.RunWithRetry` 语义一起评估。
- TLS 启用只看 CA 路径：仅填 cert/key 仍会按非 TLS 处理，Rust 测试已锁定该语义。
- `RegisterMetricsForBR` 在客户端创建后用显式 `Close` 收尾；当前代码对查询错误保证关闭，但如果以后在创建与显式关闭之间引入 panic，该 trait 没有 `Drop` 约束来保证调用 `Close`。
- `Mutex`/`RwLock` 中毒时使用 `into_inner()` 继续操作而非传播 panic。`setConstLabels` 排序键只用于产生稳定的键值序列，不改变 map 合并优先级。
- `initParentMetricsCollectors` 中的 `unsafe` 依赖全局 `static mut` 初始化约定；RCCheckTS counter 用 `addr_of_mut!` 避免 Rust 2024 对 `static mut` 共享引用的禁止，且仅在 Option 为空时构造。

## 并发与资源生命周期

- `PdClient` 和 `PdClientFactory` 的 `Send + Sync` 边界允许工厂与客户端 trait object 跨线程共享。生产客户端用 `Mutex` 串行化 `LoadKeyspace` 与 `Close`，因此两者不会同时访问底层 Option。
- 工厂槽位由 `OnceLock` 延迟创建、`RwLock` 区分频繁读与测试写、`Arc` 延长已取出工厂的生命期。修改全局工厂不会撤回已被其他线程 clone 的 `Arc`。
- 指标常量标签采用“读出副本—本地合并—整体写回”。本文件未在读改写整个周期持有统一锁，因此多线程并发修改时的原子合并保证未被本文件证明。当前测试使用 `TEST_LOCK` 串行化会修改全局配置、标签和工厂的用例。
- `RegisterMetricsForBR` 是同步阻塞路径：PD 连接、查询和 `std::thread::sleep` 均占用当前线程。最坏重试 sleep 总量是 `500ms * (1 + ... + 30) = 232.5s`，不含每次 PD 调用时间。
- UniStore 注册的 `Once` 是进程级生命周期；PD 客户端则是单次 BR 注册调用的局部资源，查询结束即关闭。

## 与 Go 版本的对应关系

Rust `common.rs` 以 `pkg/util/metricsutil/common.go` 为直接对照，保留了主要分支和顺序：NextGen 的 `keyspace_name`、BR 的空 Keyspace 快速路径、TLS 选项、PD 超时与禁用 PD 自身 metrics、特定错误重试、`keyspace_id` 注入、observability 标签合并、子系统 `InitMetricsVars` 顺序以及 UniStore 条件注册。

主要差异如下：

- Go 直接使用 `pd.Client`、`task.TLSConfig`、`keyspacepb.KeyspaceMeta` 和 `util.RunWithRetry`；Rust 为了可测性定义本地 TLS/security/meta/error 类型、PD trait 和可注入 sleep 的重试函数。
- Go `defer pdCli.Close()` 在建客户端后立即安排关闭；Rust 在组合 `Result` 后显式 `Close` 再返回。对当前普通成功/错误路径语义一致，但 panic 安全性不同。
- Go `maps.Copy` 与 Rust `HashMap::extend` 都是后来的 observability 标签覆盖同键基础标签。Go map 迭代顺序不定；Rust 在展平前按键排序，多了确定性。
- Go 通过 `metrics.InitMetrics()` 和 `metrics.RegisterMetrics()` 初始化中央 collector。Rust 注释说明 metrics 已拆分到 owner crates，因此用 `initParentMetricsCollectors` 逐个初始化本地父 collector，并为 RCCheckTS counter 手工补齐一次性构造。
- Go 注册代码已被 TiDB 和 BR 入口调用；Rust 这两条主链当前仍是 stub。这是当前迁移状态的重要差异。

Go `common_test.go` 只覆盖 observability 标签合并与 `keyspace_id` 保留。Rust `common_test.rs` 除对齐该用例外，还覆盖 TLS 启用判定、生产工厂存在性、PD 建连参数、两类可重试错误、非预期错误立即返回、建连失败、客户端关闭及退避序列。

## 扩展指南

- 新增或改名常量标签时，优先修改 `RegisterMetrics`、`RegisterMetricsForBR`、`registerMetrics` 或 `setKeyspaceIDConstLabel` 中对应的单一合并点，保持“先克隆、再合并、后整体写回”。同步扩展独立 `common_test.rs`，并检查 Go `common.go` / `common_test.go` 是否需要对齐。
- 接入新 metrics 子 crate 时，先确定其父 collector 的所有者：父级构造应放在 `initParentMetricsCollectors`，变量绑定放在 `initMetrics`，顺序应与 Go `initMetrics` 及被依赖模块保持一致。如需新 Cargo 依赖，同步更新 `pkg/util/metricsutil/Cargo.toml`。
- 扩展 PD 语义时，不要绕过 `PdClient` / `PdClientFactory`。在 `map_pd_keyspace_error` 中明确新错误分类，在 `MetricsUtilError::retryable` 中单独决定是否重试，并用 `RecordingFactory`/`MockClientState` 增加不连网的回归测试。
- 修改重试次数或退避算法时，保留 `getKeyspaceMetaWithRetry` 的 sleep 注入点，验证尝试次数、传入 Keyspace 名、最终错误和每次 duration；特别检查最后一次失败后是否仍 sleep 这个现有行为。
- 改动客户端生命周期时，同时验证成功、查询失败、工厂失败三条路径。如将显式 `Close` 改为 RAII，需保持 Go `defer` 所覆盖的早退出语义。
- 将本实现真正接入 TiDB/BR Rust 主链是另一个接线任务：需替换 `cmd/tidb-server/stubs.rs` 和 `br/cmd/br/stubs.rs` 的同名模块，并处理 BR `task::TLSConfig` 与本地 `TlsConfig` 的类型边界。不应在单独修改本文件时声称主链已接通。
- Rust 测试必须继续保持在独立 `pkg/util/metricsutil/common_test.rs`，不要内嵌回生产源文件。涉及全局配置、工厂或常量标签的新测试应使用现有 `TEST_LOCK` 和 reset/restore guard，避免并行污染。

## 验证依据

- 目标源码：`pkg/util/metricsutil/common.rs:1-418`，通读并核对了常量、类型、trait、生产 PD 适配、标签合并、指标初始化及重试全流程。
- crate 边界：`pkg/util/metricsutil/lib.rs:1-21` 证明模块声明、重导出与独立测试接线；`pkg/util/metricsutil/Cargo.toml:1-38` 证明 crate 名、Go 包对照元数据和依赖边界。
- RustCodeGraph：`status` 报告索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/metricsutil` 确认该目录 9 个已索引 Go/Rust 文件；`node --file pkg/util/metricsutil/common.rs --offset 1 --limit 500` 返回全部 418 行、56 个符号及文件级“used by 26 files”。`query` 分别唯一定位了 Rust `getKeyspaceMetaWithRetry`，并将 Rust/Go/stub 中的同名 `RegisterMetricsForBR` 区分开。精确 ID 的 `callers/callees` 命令未输出边明细，因此本文不把文件级引用数当作具体调用边。
- Go 对照：`pkg/util/metricsutil/common.go:1-167` 核对了注册顺序、PD 选项、重试分类和标签合并；`pkg/util/metricsutil/common_test.go:25-53` 证明 Go 现有标签回归意图。
- Rust 测试：`pkg/util/metricsutil/common_test.rs:135-327` 覆盖标签合并与重复注册、TLS CA 判定、生产工厂错误、PD 选项、可重试与非预期错误、建连错误、客户端关闭及线性退避。本任务按计划不运行 Cargo，因此这些是源码证据而非本次新执行的测试结果。
- 接线复核：`cmd/tidb-server/main.rs:707` + `cmd/tidb-server/stubs.rs:2442-2454`、`br/cmd/br/{backup.rs:46,restore.rs:46}` + `br/cmd/br/stubs.rs:799-808` 证明 Rust 主链尚使用 stub；`cmd/tidb-server/main.go:465`、`br/cmd/br/backup.go:31`、`br/cmd/br/restore.go:36` 证明 Go 主链的真实调用。
- 结构验证命令为 `test -f pkg/util/metricsutil/common.rs.md && test "$(rg -c '^## (文件定位|核心职责|主要符号|执行流程|数据与状态|依赖与调用关系|错误处理与边界|并发与资源生命周期|与 Go 版本的对应关系|扩展指南|验证依据)$' pkg/util/metricsutil/common.rs.md)" -eq 11`；交付前应得到退出码 0。
