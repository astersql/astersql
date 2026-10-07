# `cmd/tidb-server/main.rs`

## 文件定位

`main.rs` 是 `astersql-cmd-tidb-server` crate 的可测试进程编排层。真正的二进制入口在 `cmd/tidb-server/bin_main.rs::main`：它先启动 `rpprof` 分配采样，再调用 `cmd/tidb-server/lib.rs::main`；后者执行 FIPS 钩子并转发到本文件的 `entry::main`。因此，本文件不是操作系统直接调用的最外层壳，而是复用在二进制和 crate 内测试之间的共享启动实现。

crate 边界由 `cmd/tidb-server/Cargo.toml` 明确：包同时导出 `lib.rs` 和 `bin_main.rs`，`nextgen` feature 向 `astersql-session`、`astersql-store` 透传。直接依赖包括 canonical 的 `astersql-config`、`astersql-domain`、`astersql-server`、`astersql-session`、`astersql-store`、`astersql-store-driver`、`astersql-metaservice`、`tikv-client`，同时仍通过 `crate::stubs` 提供配置、旧 Domain 外壳、信号、指标及多个迁移期适配面（`main.rs:32-55`）。这意味着它是 Go `cmd/tidb-server/main.go` 的 Rust 对齐入口，也是新旧运行时的接线边界，不能把所有 `stubs::*` 调用理解为已落地的独立生产子系统。

## 核心职责

1. `initFlagSetWithArgs` 注册并解析 TiDB CLI，把分散的 flag 收敛为进程级 `FlagValues` 快照；`overrideConfig` 只把用户显式传入的 flag 覆盖到配置文件结果上（`main.rs:291-550,1172-1413`）。
2. `run_main_inner` 按固定顺序完成配置/部署模式校验、存储驱动注册、运行时全局变量投影、观测组件初始化、storage/Domain/server 创建、信号处理和优雅停机（`main.rs:619-847`）。
3. `registerStoresWithTiKVDriver`、`initRegisteredStorage` 和 `createStoreDDLOwnerMgrAndDomain` 把最终配置接到 store registry，处理 nextgen SYSTEM keyspace、PD kernel 类型校验、DDL owner 与 session bootstrap（`main.rs:957-1065`）。
4. `createServer` 把 registry storage 转成 canonical TiKV 或本地 mock session runtime，启动 Starter 资源组与 TTL 管理器，并组装 canonical MySQL/PostgreSQL/status listener（`main.rs:1838-1995,2353-2395`）。
5. `cleanup` 及其辅助函数维护停机顺序，避免 server、后台任务、Domain、DDL owner 和 storage 交叉访问已释放资源（`main.rs:2037-2091`）。

## 主要符号

- CLI 常量 `nmVersion` 至 `nmStarterParams` 保留 Go flag 字符串；退出码 `exitCodeOK`、`exitCodeErr`、`exitCodeInt` 分别为 0、1、`128 + SIGINT`（`main.rs:60-117`）。
- `FlagValues` 集中保存解析后的所有入口参数；`FLAGS: Mutex<Option<FlagValues>>`、`flags`、`set_flags` 允许启动期写入以及测试内重置。`starter_additional_params` 的独立 getter/setter 是少数只消费该字段的控制面路径（`main.rs:125-268`）。
- `main`、`run_main`、`run_main_inner` 分别对应库入口、返回退出码的测试入口、完整编排实现。只有 `run_main_inner(..., true)` 才可能调用 `process::exit`；当前 `entry::main` 调用的 `run_main` 使用 `false`（`main.rs:602-619`）。
- `overrideConfig` 是 CLI 到 `config::Config` 的增量映射点，包含 advertise address 推导、PostgreSQL 可选端口、Starter 双套 TLS 配对、初始化模式互斥、keyspace/standby 参数等约束（`main.rs:1172-1413`）。
- `validateVersionConfigPolicy`、`deriveRuntimeVersionsFromBuildInfo`、`initVersions`、`mustInitVersions` 负责 classic/nextgen 的版本策略与 MySQL 对外版本派生（`main.rs:1415-1475`）。
- `setGlobalVars` 将最终配置写入原子变量、sysvar、事务/内存/计划缓存、TiKV client 和执行器追踪器，并展开 socket 的 `{Port}` 后回写全局配置（`main.rs:1478-1804`）。
- `registerStoresWithTiKVDriver` 是可注入测试后端的注册边界；`createStoreDDLOwnerMgrAndDomain` 是 storage、DDL owner、旧 Domain 的聚合创建边界（`main.rs:957-1065`）。
- `canonicalServerConfig` 校验端口范围和 SQL/status TLS cert-key 配对，再投影 canonical listener 配置；`assembleCanonicalServer` 接入 session driver 和 connection domain（`main.rs:1927-1995`）。
- `starterParams`、`parseStarterAdditionalParams`、`applyStarterAdditionalParams`、`createMgrClientForStarter` 严格解析 `k=v` 参数并建立 Starter manager client（`main.rs:2175-2320`）。
- 内部辅助符号只有 `initRegisteredStorage`、`prometheus_default`、`hostname` 模块和 `assembleCanonicalServer` 等少数项不是公开 API；大量 `pub` 主要服务 crate 测试和迁移期对齐，不表示稳定的跨 crate API。

## 执行流程

`bin_main.rs::main -> lib.rs::main -> entry::main -> run_main -> run_main_inner`。主流程如下：

1. 解析 flags；`collect-log` 直接调用 `redact::DeRedactFile` 后返回，`--help` 也在全局初始化前返回。
2. `InitializeConfig(..., overrideConfig, ...)` 合并配置；nextgen 写入 deploy mode；`-V` 初始化版本并提前返回。随后拒绝 nextgen 缺少 keyspace/standby，以及 classic 携带 keyspace/standby/activate 的组合。
3. standby 模式创建 manager client、等待激活、重新执行配置校验，并取得 activation metadata。
4. 注册 USR1、三类 store 和 metrics；按条件初始化临时目录、日志、内存 hook、扩展、语句摘要、CPU profiler、TiFlash autoscaler、failpoint、全局 sysvar、SEM、CPU affinity、cgroup、tracing 和 metrics。
5. 启动 executor/resource manager，通过 `createStoreDDLOwnerMgrAndDomain` 得到 storage 与旧 Domain；配置 repository 和可选外部 workload manager；`createServer` 建立 canonical server。Starter 激活模式执行完整清理后立即返回。
6. 正常模式安装信号闭包。闭包依次 `Server::Close`、停止 resource manager、`cleanup`、停止 profiler/executor，并写入信号对应的退出码。随后设置 TopSQL、阻塞在 `Server::Run`，等待信号路径完成。
7. 退出前关闭外部 workload manager并刷日志；日志刷盘失败覆盖为通用错误码。关键调用边由 RustCodeGraph 确认：`run_main_inner -> createStoreDDLOwnerMgrAndDomain/createServer/cleanup`，且 `createServer -> canonicalServerConfig/assembleCanonicalServer/startStarterResourceGroupController`。

## 数据与状态

- `FLAGS` 是 `Mutex<Option<FlagValues>>` 的进程级快照。读取会 clone，未初始化则回退 `Default`；锁中毒会 `unwrap` panic。它适合启动期和串行化测试，不是热更新配置机制（`main.rs:247-268`）。
- 真正运行配置由 `config::GetGlobalConfig`/`UpdateGlobal` 管理。`overrideConfig` 只处理 `FlagSet::Visit` 标记的显式项，防止 CLI 默认值覆盖配置文件；`setGlobalVars` 再把其投影到大量全局原子量、sysvar 和单例。
- storage registry 同时注册 `tikv`、`mocktikv`、`unistore`。nextgen 用户 keyspace 会另开并保存 SYSTEM storage；关闭用户 keyspace 时 `closeDDLOwnerMgrDomainAndStorage` 也会回收它（`main.rs:1010-1035,2037-2059`）。
- server 实际跨越两套 Domain：`stubs::domain::Domain` 保留 Go 对齐的外围 handle；canonical `astersql_domain::Domain` 驱动真实 session/listener/TTL。`createServer` 将二者装配进同一 `server::Server` 适配器（`main.rs:1838-1942`）。
- `exit_code: Arc<Mutex<i32>>` 在主线程与信号闭包之间共享；`activationMetadata`、`external_mgr`、standby controller 只活到一次启动生命周期。
- `gracefulCloseConnectionsTimeout` 固定 15 秒；Starter 强制关闭将 drain 时间改为 0，取消阶段固定 1 秒（`main.rs:2061-2082`）。

## 依赖与调用关系

上游调用关系只有 crate 壳层和测试：`bin_main.rs` 调用 `lib.rs::main`，后者调用本文件 `entry::main`；RustCodeGraph 将 `main.rs` 的文件级使用者识别为 `main_test.rs`，而 `parity_test.rs` 也经 `crate::entry` 验证公开契约。生产调用不是其它业务模块主动进入本文件，而是进程启动时自顶向下驱动。

主要下游分为四组：

- 配置/兼容层：`crate::stubs::{config, flag, deploymode, kerneltype, variable, vardef, ...}`；这些适配 Go 风格 API 和尚未完全 canonical 化的外围组件。
- 存储链：`astersql_store` registry -> `astersql_store_driver::TiKVDriver` -> `tikv-client`；PD 状态用于 kernel 匹配，nextgen 另维护 SYSTEM storage。
- SQL 服务链：`astersql_session::CanonicalSessionFactory` 或 `CreateAnalyzeSession` -> `ConcreteSessionDriver` -> `astersql_server::CanonicalServer` -> `server::Server` 适配器。
- 生命周期链：signal handler -> server close -> resource manager stop -> `cleanup` -> Domain/DDL owner/storage close；同时关闭 plugin、repository、TopSQL、磁盘临时目录、语句摘要和 cgroup monitor。

`Cargo.toml` 注释也明确“local stubs continue to cover server/session/domain/config/signal/metrics”，而 production TiKV path 已连到 canonical store registry；扩展时应按这个边界判断新逻辑属于 canonical crate 还是入口适配层。

## 错误处理与边界

- 返回 `Result` 的初始化步骤多数经 `must_nil_result`/fatal 转成启动失败；外围能力有意降级：外部 workload manager 初始化/关闭与语句摘要持久化失败只记录告警或错误，Prometheus push 失败会继续重试。
- 配置硬边界包括：端口必须落在 `u16`；SQL/status TLS 证书与私钥必须成对；secure/insecure 初始化互斥；Starter 参数不得有空项、空 key/value、重复/未知 key，布尔值仅接受 Go `ParseBool` 等价形式；manager notifier 必须获得地址或 namespace 以及 pod 三元组。
- `parseDuration` 对裸数字追加秒单位重试；无效值 fatal。`checkTempStorageQuota` 仅在 quota 非负时比较磁盘容量。CPU 列表解析或 affinity 设置失败阻塞启动。
- `syncLog` 只忽略包含 `/dev/stdout` 的同步错误，其他错误令最终退出码变为 1。SIGINT 返回 `128 + SIGINT`，其它信号返回 0。
- `collect-log`、help、version、keyspace activate 是提前返回路径。classic/nextgen 配置组合不合法目前打印错误但返回 0，这是与 Go `os.Exit(0)` 对齐的既有行为，不应擅自“修正”。
- `createServer` 任一 canonical storage/domain/listener/TTL/资源组装配失败，都会先关闭已创建的 storage/Domain 再 fatal，避免半初始化泄漏。

## 并发与资源生命周期

- `pushMetric` 启动永久指标线程；测试编译时 `prometheusPushClient` 在两次 push 后退出。`setupMetrics` 另起系统时间回拨监控线程。
- `ASTERSQL_TIDB_SERVER_IMMEDIATE_EXIT` 仅用于测试/即时模式：线程延迟 10ms 投递 SIGTERM 并关闭 server，使阻塞的 `Run` 返回（`main.rs:818-827`）。
- 信号闭包通过 `Arc` clone 持有 server/storage/Domain/exit code。主线程在 `Server::Run` 返回后调用 `signal::wait_exited`，保证清理完成后再刷日志和返回。
- 创建顺序是不变量：注册 store -> 投影全局配置 -> storage -> DDL owner -> Domain -> server；关闭顺序大体相反，并在底层资源前停止会继续发 SQL/消费事件的 auto analyze、plugin、repository、TopSQL。
- canonical listener 测试验证 MySQL/status 监听真实启动，且关闭事件顺序为 canonical server -> canonical Domain -> 旧 Domain -> storage（`main_test.rs::canonical_listener_starts_on_port_zero_and_preserves_cleanup_order`）。
- `FLAGS` 和 exit code 使用互斥锁；大部分其它全局值由 stubs/canonical crate 的原子量或内部同步保证。本文件没有事务锁或异步 runtime，后台工作主要以线程和各子系统自身的 Run/Stop 生命周期表达。

## 与 Go 版本的对应关系

Rust 主流程、flag 名称、初始化顺序、清理顺序以及多数 helper 都逐名对应 `cmd/tidb-server/main.go`：`main`、`overrideConfig`、`setGlobalVars`、`createStoreDDLOwnerMgrAndDomain`、`createServer`、`cleanup`、Starter 参数与 observability 逻辑均可直接对照。

已验证的差异与迁移状态：

- Go 使用包级 flag 指针和直接 `os.Exit`；Rust 用 `FlagValues` + `Mutex`，提供 `run_main` 返回退出码以支持同进程测试。
- Go 在 `createStoreDDLOwnerMgrAndDomain` 内创建并注入 external workload manager；Rust 先 bootstrap storage/旧 Domain，再在 `run_main_inner` 的 Starter 分支创建可选 manager。Rust 当前 manager 仍来自 stubs，失败策略为非阻塞告警。
- Go `setCPUAffinity` 会在绑核数较少时调整 `GOMAXPROCS`；Rust 只记录信息，不改变线程池并行度（`main.rs:914-947`）。
- Rust 的 TiKV store、canonical session/domain/server/listener、TTL 和 Starter resource-group provider 已接真实 crate；其余大量 Go API 仍由 `stubs.rs` 模拟。非 TiKV 本地模式使用 `CreateAnalyzeSession` 的进程内事务 mock，避免再开外部 client。
- Rust 新增可选 PostgreSQL listener 配置投影和 canonical listener 装配；Go 对照的 `createServer` 仍直接 `server.NewTiDBDriver/NewServer`。
- Go 的 pyroscope 还设置 mutex/block profile 与鉴权字段；Rust 当前只按 `PYROSCOPE_SERVER_ADDRESS` 启动 stub/适配接口。Go metrics 还设置 mutex profile fraction，Rust 未复刻该调用。

独立 Rust 测试在 `cmd/tidb-server/main_test.rs`，Go 对照测试在 `cmd/tidb-server/main_test.go`；`cmd/tidb-server/parity_test.rs` 额外守护公开契约、正常/边界/错误路径和资源清理。

## 扩展指南

- 新增 CLI 参数时，应同时更新 flag 名常量、`FlagValues`/`Default`、`initFlagSetWithArgs` 注册与提取、`overrideConfig` 的“仅显式覆盖”分支，并在独立 `main_test.rs` 增加默认值、显式覆盖和非法组合测试；不要把测试内嵌进 `main.rs`。
- 新增 listener 配置应进入 `canonicalServerConfig`，覆盖端口范围、TLS 成对、PROXY/socket 等边界，并扩展 `canonical_listener_config_maps_ports_socket_proxy_and_tls`。真实监听行为应在 `canonical_listener_starts_on_port_zero_and_preserves_cleanup_order` 同类测试中验证。
- 新存储或 keyspace 行为应接在 `registerStoresWithTiKVDriver`、`initRegisteredStorage`、`createStoreDDLOwnerMgrAndDomain`，同时核对 SYSTEM storage 的创建/关闭对称性和 PD kernel 校验；不可绕过带 tag 的 `tikv-client` 依赖。
- 新常驻后台组件必须明确启动点、失败策略和 `cleanup`/提前返回路径的关闭点，并在 `parity_test.rs::contract_resource_cleanup` 验证顺序，尤其覆盖 server 创建失败与 keyspace activate。
- 新 Starter 控制面字段应扩展 `starterParams` 和严格解析器，拒绝未知/重复/空值，并同步 Go `main.go`/`main_test.go` 的语义；观测字段合并应继续保持显式配置覆盖默认 `keyspace_name`。
- 将 stubs 能力迁移到 canonical crate 时，优先在真实 crate 实现并仅在本文件替换接线；保留 Go 行为与错误边界，不因文档或测试方便而简化生产逻辑。

## 验证依据

- RustCodeGraph 状态：索引包含 7032 个 Rust 文件；文件查询显示 `cmd/tidb-server/main.rs` 有 128 个符号。读取了完整文件范围，并查询 `run_main_inner`、`createStoreDDLOwnerMgrAndDomain`、`createServer`、`cleanup` 等节点。
- RustCodeGraph 关键调用边：`run_main_inner -> initFlagSetWithArgs/overrideConfig/registerStores/createStoreDDLOwnerMgrAndDomain/createServer/setupMetrics/setupTracing/cleanup/createMgrClientForStarter`；`createServer -> canonicalServerConfig/assembleCanonicalServer/startStarterResourceGroupController/closeDDLOwnerMgrDomainAndStorage`；`cleanup -> closeDDLOwnerMgrDomainAndStorage/closeStmtSummary`。
- 已读生产路径：`cmd/tidb-server/main.rs`、`cmd/tidb-server/lib.rs`、`cmd/tidb-server/bin_main.rs`、`cmd/tidb-server/Cargo.toml`、`cmd/tidb-server/main.go`。
- 已读测试路径：`cmd/tidb-server/main_test.rs`、`cmd/tidb-server/parity_test.rs`、`cmd/tidb-server/main_test.go`。关键测试覆盖真实 TiKV driver 接线、canonical listener 配置与启动、退出码、配置覆盖、全局变量、deploy/version、Starter manager/observability/布尔解析、错误路径和资源关闭顺序。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定的 11 标题检查；文档中的迁移边界以 Cargo 依赖、源码调用和独立测试为依据，未把 `stubs.rs` 的适配行为表述为全部 canonical 子系统已完成。
