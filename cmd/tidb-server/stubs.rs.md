# `cmd/tidb-server/stubs.rs`

## 文件定位

`cmd/tidb-server/stubs.rs` 属于 `astersql-cmd-tidb-server` crate，由 [`cmd/tidb-server/lib.rs`](lib.rs) 以 `pub mod stubs` 公开，并由 [`cmd/tidb-server/main.rs`](main.rs) 集中导入。它不是一个独立可执行入口，而是 Rust 版 `tidb-server` 启动代码的本地边界适配层：为尚未全部接入真实 Rust 子系统的 server、session、domain、配置、信号、指标和资源管理接口提供可编排、可观测的最小实现。

crate 在 [`cmd/tidb-server/Cargo.toml`](Cargo.toml) 中声明为二进制迁移包，实际二进制入口是 `bin_main.rs`，库入口依次转发到 `fips::enable_fips_only()` 与 `entry::main()`。`stubs.rs` 处于入口与下层 crate 之间：大多数模块只模拟调用边界，但存储、server、session 和信号的部分路径已经桥接到 `astersql-store`、`astersql-server`、`astersql-session`、`astersql-store-driver` 与 `astersql-util-signal`。因此不能把文件名中的 “stubs” 理解为全部无效，也不能把任一 `Ok(())` 当作真实子系统已经完整实现。

## 核心职责

该文件的职责可以归为六组：

1. 提供统一的轻量错误协议：`Error`、`Result<T>`、`must_nil`、`must_nil_result`、`fatal` 与 `terror_log`，让从 Go 迁移来的启动代码保留“初始化失败即终止、清理失败只记录”的控制流。
2. 提供测试观测面：`EVENTS`、`LOGS`、`record_event`、`take_events` 和 `take_logs` 将原本会触发网络、后台服务或日志系统的边界调用压缩为有序事件，供独立测试核对启动及清理顺序。
3. 镜像入口所需的配置和全局状态：`config`、`vardef`、`variable`、`kerneltype`、`deploymode`、`mysql` 等模块保存默认值、原子开关、系统变量和版本派生逻辑。
4. 模拟外部生命周期：`signal`、`server`、`domain`、`ddl`、`executor`、`resourcemanager`、`repository`、`topsql`、`disk`、`cpuprofile` 等模块为启动、运行、信号退出和资源清理提供确定性的状态转换。
5. 连接已经存在的 canonical Rust 实现：`kv::Storage` 可持有 `astersql_store::StorageRef`，把 `CanonicalTiKVStore`、`CurrentVersion`、`GetClusterID` 和 `Close` 委托给注册存储；非测试构建中的 `signal` 将操作系统信号映射到本地 `Signal`；`main.rs` 还会将这里的配置投影到 canonical server/session/store 类型。
6. 隔离进程级测试状态：`test_guard` 串行化会修改静态状态的测试，`reset_all_for_test` 清空事件、日志、信号通道、store 注册表、部署模式、全局配置、系统变量和版本字段。

## 主要符号

- `Error` / `Result<T>`：只保存消息文本的统一错误类型。`Display` 与 `std::error::Error` 使其可进入 Rust 错误链；保留 Go 风格 `Error()` 是为了兼容迁移调用点。
- `record_event` / `take_events`、`log_info` / `take_logs`：分别操作 `OnceLock<Mutex<Vec<String>>>`。`take_*` 使用 `std::mem::take`，读取后即清空，是测试断言“一轮流程”的基础。
- `AtomicBoolVar` / `AtomicF64Var`：可克隆的共享值包装。前者使用 `Arc<AtomicBool>` 与 `SeqCst`；后者因标准库没有原子 `f64`，使用 `Arc<Mutex<f64>>`。
- `flag::FlagSet`：保存 `Bool/String/Int/Uint` 四类 flag、访问集合和位置参数；`Parse` 支持 `-x value`、`-x=value`、布尔省略值及 `--` 截断，未知 flag、缺少参数或非法值返回 `Error`。
- `Signal` 与 `signal::{SetupSignalHandler, deliver, wait_exited, reset}`：把有限的 POSIX 信号域转换成可注入枚举，并通过 `mpsc` 通知主线程退出。
- `config::Config`：入口所需的配置树；`InitializeConfig` 创建默认配置、调用 `overrideConfig`、按需执行 `Valid`，最后替换全局快照。`ResolveKeyspaceObservability` 将激活 metadata 投影到 metric、slow-log 与 statement-log 字段，缺失 required source 时失败。
- `mysql::{NormalizeTiDBReleaseVersionForNextGen, BuildTiDBXReleaseVersion, BuildTiDBXServerVersion}`：完成 nextgen/TiDBX 版本标准化和格式校验；年份、月份或语义版本格式非法时返回明确错误。
- `kv::Storage` / `kvstore`：表示本地或 canonical 注册存储。`Storage::Close` 设置共享关闭标志、唤醒等待者，并在存在 `StorageRef` 时继续关闭真实注册存储。
- `server::{Server, StandbyController}`、`domain::Domain`：承载 listener/domain 生命周期的入口侧视图；具体启动与清理动作通过状态位和事件暴露给测试，canonical server 则由 `main.rs::assembleCanonicalServer` 装入包装对象。
- `parse_go_duration`：覆盖入口配置会使用的 `ns/us/µs/ms/s/m/h` 单位，拒绝空串、未知单位和负时长；它只复刻当前调用面，不是完整的 Go `time.ParseDuration`。
- `args_from_env`、`test_guard`、`reset_all_for_test`：分别提供 argv 兼容、全局测试互斥和测试基线恢复。

文件还包含大量窄边界模块。可按用途阅读：配置与兼容层（`util`、`naming`、`versioninfo`、`variable`、`redact`、`logutil`、`log`、`keyspace`）；核心启动资源（`driver`、`mockstore`、`standby`、`tidbmanager`、`session`、`ddl`、`extworkload`）；后台能力（`metricsutil`、`systimemon`、`extension`、`stmtsummaryv2`、`tiflashcompute`、`tikv`）；全局开关与跟踪器（`statistics`、`plannercore`、`privileges`、`domainutil`、`kvcache`、`transaction`、`parsertypes`、`deadlockhistory`、`txninfo`、`chunk`）。这些模块均只应按其实际函数体认定能力。

## 执行流程

该文件自身没有顶层业务循环；它由 `main.rs::run_main_inner` 按以下顺序驱动：

1. `args_from_env` 提供进程参数，`flag::FlagSet::Parse` 生成访问过的 flag 与剩余参数；`collect-log` 分支只调用 `redact::DeRedactFile` 后返回。
2. `config::InitializeConfig` 创建配置、调用 `main.rs::overrideConfig` 叠加显式 CLI 值，并更新全局快照；nextgen 路径随后通过 `deploymode`、`kerneltype` 和 `mysql` 派生部署/版本语义。
3. 入口调用 `signal::SetupUSR1Handler`、注册 store、准备 metrics/temp dir/log/memory/extensions/profiler，再由 `setGlobalVars` 写入 `vardef`、`variable`、`statistics`、`plannercore` 等全局落点。
4. `executor` 和 `resourcemanager` 先启动；`main.rs::createStoreDDLOwnerMgrAndDomain` 再通过 store、PD 状态、DDL owner、session bootstrap 与 `domain` 建立核心资源；最后创建 canonical server 并包装成 `server::Server`。
5. `signal::SetupSignalHandler` 保存清理闭包。收到信号后，闭包依次关闭 server、停止资源管理器、调用 `main.rs::cleanup`、停止 profiler 与 executor，并通过通道唤醒 `wait_exited`。
6. 清理阶段先停止自动分析和连接，再停止 plugin/repository/TopSQL/statement summary，随后关闭 domain、DDL owner、MPP 组件、主 storage，以及用户 keyspace 场景下的 system storage。`syncLog` 的失败会把退出码折叠为通用错误。

测试可设置 `ASTERSQL_TIDB_SERVER_IMMEDIATE_EXIT`，让入口在线程中投递 `SIGTERM`，从而在不常驻的情况下覆盖 `Server::Run` 到信号清理的完整路径。

## 数据与状态

本文件大量使用进程级共享状态，主要分成以下几类：

- `OnceLock<Mutex<T>>`：用于惰性初始化且需要测试重置的复杂值，例如事件、日志、全局 `Config`、系统变量表、版本字符串和 repair table list。
- 原子类型：`AtomicBool`、`AtomicI32`、`AtomicU32`、`AtomicU64` 保存 kernel/deploy 开关、配额、超时和运行时布尔位；多数读写采用 `Ordering::SeqCst`，以换取测试与启动期最直观的跨线程可见性。
- `Arc` 共享对象：`kv::Storage` 的关闭状态、`domain::Domain` 状态和 server/standby 状态可被主线程与信号处理闭包共同持有。
- `mpsc` 信号通道：`EXIT_TX`/`EXIT_RX` 只承载一次“退出处理已发生”的通知；`wait_exited` 会取走 receiver，因此同一轮安装只应等待一次。
- 观测数组：`EVENTS` 和 `LOGS` 是断言接口而不是审计日志；`take_*` 会清空内容，生产逻辑不能依赖其持久性。

`config::GetGlobalConfig` 返回克隆快照，而 `UpdateGlobal` 在锁内原地修改。部分字段（例如 `AtomicBoolVar`）在 clone 后仍共享底层原子值，这与普通值字段的复制语义不同，扩展配置时必须先判断调用者需要“快照”还是“共享”。

## 依赖与调用关系

上游主要有三类：

- [`cmd/tidb-server/main.rs`](main.rs) 是生产入口调用者，直接导入几乎所有 stub 模块，并在 `run_main_inner`、`setGlobalVars`、`createStoreDDLOwnerMgrAndDomain`、`createServer`、`setupMetrics` 和 `cleanup` 中使用它们。
- [`cmd/tidb-server/main_test.rs`](main_test.rs) 调用 `test_guard`、`reset_all_for_test`、`take_events` 等接口，验证真实 store driver 接线、canonical listener 配置、启动/清理顺序、部署模式、版本与 keyspace observability。
- [`cmd/tidb-server/parity_test.rs`](parity_test.rs) 将 Go/Rust 公共契约、边界错误、资源清理与 Prometheus push 周期集中做对齐验证。

下游依赖由 [`cmd/tidb-server/Cargo.toml`](Cargo.toml) 界定。`stubs.rs` 直接或间接触达 `astersql-store` 与 `astersql-util-signal`，而 `main.rs` 使用 `astersql-config`、`astersql-domain`、`astersql-server`、`astersql-session` 和 `astersql-store-driver` 把部分 stub 数据投影到 canonical 实现。`nextgen` feature 同时开启 `astersql-session/nextgen` 与 `astersql-store/nextgen`；`kerneltype` 通过 `cfg!(feature = "nextgen")` 建立默认内核类型。

关键调用边是：`lib::main -> entry::main -> run_main_inner -> stubs`；配置边为 `flag::FlagSet -> config::InitializeConfig -> main::overrideConfig -> config::GLOBAL`；服务边为 `kvstore/store registry -> kv::Storage -> session/domain -> canonical server`；退出边为 `signal::deliver -> registered handler -> main::cleanup -> Storage::Close -> signal::wait_exited`。

## 错误处理与边界

启动期硬失败通常经 `must_nil_result` 或 `must_nil` 转为 `fatal` 的 panic，这对应 Go 入口中 `terror.MustNil`/fatal 的“不能继续启动”语义。清理期则用 `terror_log` 降级为 warning，避免次要关闭错误覆盖原始退出原因。

需要特别保留的边界包括：未知 flag、缺参数、非法布尔/整数；负数或未知单位 duration；带空白的 service scope；不合法的部署模式；nextgen 版本缺少 `v`、不是三段版本、年份或月份越界；required observability metadata 缺失；不存在的 redact 输入；未由 canonical registry 支撑却请求 `CanonicalTiKVStore`；端口和 TLS 成对关系等。部分完整约束位于 `main.rs` 而不在 stub 中，例如 classic/nextgen keyspace 门禁、SQL/status 端口到 `u16` 的转换和证书/私钥必须成对。

许多 API 固定返回 `Ok(())` 或常量，例如 `Config::Valid`、tracer 创建、内存总量、metrics 注册与 extension setup。这些仅说明调用面可被编排和测试，不证明真实 I/O、容量检测、指标注册或安全初始化已经落地。扩展文档或代码时应明确区分：纯事件桩、带状态的模拟、以及委托 canonical crate 的适配器。

## 并发与资源生命周期

`signal::HANDLER` 存储 `FnMut(Signal) + Send`，`deliver` 在锁内调用回调，随后通过 channel 发送完成通知；新增回调逻辑不得再次获取同一 handler 锁，否则可能自锁。`wait_exited` 会从全局槽取走 receiver 并阻塞接收；未先调用 `SetupSignalHandler` 时它直接返回。

`kv::Storage::Close` 通过 `Arc<(Mutex<bool>, Condvar)>` 标记关闭并唤醒等待者，再关闭 canonical storage。`Domain`、`Server`、standby controller 和若干后台 manager 使用 `Arc<AtomicBool>` 或事件记录表示共享生命周期。`main.rs` 的强制关闭与正常关闭都依赖固定顺序：先阻止新流量和后台调度，再释放 domain/DDL/MPP，最后关闭存储与日志。

测试之间必须持有 `test_guard`，并在每轮前调用 `reset_all_for_test`。原因是许多 `OnceLock` 无法销毁，只能清空其内部可变值；若并行测试跳过该协议，事件、deploy mode、配置、版本或 store 注册状态可能互相污染。`setupMetrics` 和立即退出路径会创建线程，新增测试还应确保线程不会在 guard 释放后继续修改全局状态。

## 与 Go 版本的对应关系

直接对照文件是 [`cmd/tidb-server/main.go`](main.go)。Rust `main.rs` 保留了 Go 的 `initFlagSet`、`overrideConfig`、`setGlobalVars`、`registerStores`、`createStoreDDLOwnerMgrAndDomain`、`createServer`、`setupMetrics`、`cleanup` 等职责分层；`stubs.rs` 则把 Go 侧来自众多 package 的依赖压缩到一个本地模块集合中。因此，它没有单一同名 Go 文件，而是对应 `main.go` 的导入边界和全局包变量。

语义对齐点包括：flag 名和默认值、只有显式访问的 flag 才覆盖配置、裸数字 lease 追加 `s` 重试、`SIGINT` 返回 `128 + SIGINT`、启动时先注册存储再 bootstrap domain、退出时先停服务再关存储、日志 sync 失败改为非零退出、nextgen 版本转换以及 startup/cleanup 的顺序。Go 测试 [`cmd/tidb-server/main_test.go`](main_test.go) 覆盖 `runMain`、signal exit code、配置覆盖、全局变量、deploy mode、manager 身份、版本与 observability；Rust 的 `main_test.rs` 与 `parity_test.rs` 延续并扩展这些意图。

当前差异是 Rust 为可测试性引入了显式 `FlagValues`、全局互斥锁、事件日志和可注入信号，并只实现入口会触达的外部能力；Go 版本直接调用真实 package 和 goroutine。Rust 已对 canonical storage/listener/session 做局部真实接线，但其他 `Ok(())`/固定值模块仍是迁移占位，不能据此宣称与 Go 的底层行为完全等价。

## 扩展指南

新增入口边界时，先判断是否已有 canonical crate API：若存在，应在此处只做形状适配和错误转换，并在 `main.rs` 接入真实对象；若尚不存在，stub 必须记录可断言事件或维护必要状态，同时在注释和文档中明确缺失的真实副作用。不要通过继续扩大固定成功返回来掩盖生产依赖。

修改配置时，应同步检查 `config::Config::default`、`main.rs::FlagValues`/`overrideConfig`、`setGlobalVars`、`variable::default_sysvars` 和 `reset_all_for_test`。新增生命周期组件时，应同时定义启动位置、清理逆序、失败回滚与测试重置。新增共享状态优先使用现有 `OnceLock + Mutex` 或原子模式，并说明 clone 后是否共享；不要让测试直接依赖私有静态变量。

测试逻辑必须继续放在独立文件，不嵌入 `stubs.rs`。最接近的验证位置是 `cmd/tidb-server/main_test.rs`；Go/Rust 契约和资源顺序适合放在 `cmd/tidb-server/parity_test.rs`，并参考 `cmd/tidb-server/main_test.go` 的原始意图。涉及真实 store/listener 时，应扩展已有 canonical wiring 测试，而不是仅断言 `record_event`。兼容风险集中在 Go 风格 API 名、默认配置和错误文本；性能风险集中在全局 `SeqCst` 原子、粗粒度 mutex 与后台线程，虽然它们当前主要位于启动/测试路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter cmd/tidb-server` 确认 `stubs.rs`、`main.rs`、`main_test.rs` 和 `parity_test.rs` 均已索引；`node --file cmd/tidb-server/stubs.rs` 核对了 3032 行源码、453 个符号及 15 个使用文件；`node --file cmd/tidb-server/main.rs` 核对了 `run_main_inner`、canonical server 装配和 cleanup 调用链。通用名称的 `callers` 查询未返回可消歧结果，因此调用者关系另由精确引用搜索核实。
- 源码：`cmd/tidb-server/stubs.rs` 的错误/事件层、flag、signal、config、version、storage、server/domain、后台模块及测试重置；`cmd/tidb-server/lib.rs` 的模块装配；`cmd/tidb-server/main.rs` 的生产入口、启动顺序和清理顺序。
- crate 边界：`cmd/tidb-server/Cargo.toml` 的 binary metadata、`nextgen` feature，以及 config/domain/server/session/store/store-driver/signal 依赖。
- Go 对照：`cmd/tidb-server/main.go` 中同职责函数和 `cmd/tidb-server/main_test.go` 中对应测试意图。
- 独立 Rust 测试：`cmd/tidb-server/main_test.rs` 与 `cmd/tidb-server/parity_test.rs`；同目录不存在 `stubs_test.rs`，测试通过 crate 公开模块直接使用 stub 状态与事件接口。
- 本说明只描述当前源码能够证明的行为；没有运行 Cargo，符合本纯文档任务的验证约束。
