# `lightning/pkg/server/lightning.rs` 逻辑说明

## 文件定位

`lightning/pkg/server/lightning.rs` 是 `astersql-lightning-pkg-server` crate 的核心编排文件。crate 根 `lightning/pkg/server/lib.rs` 通过 `mod lightning; pub use lightning::*;` 导出这里的公开项；`lightning/pkg/server/Cargo.toml` 将该 crate 标记为 Go 包 `lightning/pkg/server` 的 library 移植，并直接依赖 checkpoints、progress、importer、importinto 四个已移植 crate。HTTP、TLS、PD、TiKV、对象存储、指标等当前迁移边界则主要由同 crate 的 `stubs.rs` 提供。

应用主入口位于 `lightning/cmd/tidb-lightning/main.rs`：`run_with_factory` 用 `server::New` 构造实例，`RealLightning` 再把 `GoServe`、`RunServer`、`RunOnceWithOptions`、`Stop`、`TaskCanceled` 转发给本文件。常驻 server mode 与一次性导入最终都汇入 `Lightning::run`，所以本文件是控制面和任务生命周期的汇合点，而不是解析数据或写入 TiKV 的算法实现。

同路径 Go 对照为 `lightning/pkg/server/lightning.go`。Rust 文件覆盖其服务生命周期、任务 HTTP API、数据源准备、资源检查、模式切换和 importer 选择；Go 文件中的 `CleanupMetas`、证书过期时间修改及私钥解析 helper 当前不在本 Rust 文件中，不能视为已经由这里移植。

## 核心职责

1. `New` 初始化日志、TLS、脱敏开关、根 context、Prometheus 工厂/注册器以及实例共享状态。
2. `GoServe`/`goServe` 组装状态 HTTP 服务，暴露 metrics、pprof、任务队列、进度、暂停/恢复和日志级别接口，并支持 Unix `SIGUSR1` 触发监听的入口。
3. `RunServer` 从 `config::List` 队列连续取任务；`RunOnceWithOptions` 接受依赖注入并调整单任务配置；二者复用 `run`。
4. `run` 建立任务级指标和 context，发布当前任务状态，完成 TLS、数据源、DB/keyspace 和 importer 前置准备，运行选定后端，并无条件清理取消句柄、importer、进度广播和指标注册。
5. `initDataSource`、`checkSystemRequirement`、`checkSchemaConflict` 在真正导入前阻止空数据源、存储错误、文件描述符不足和 checkpoint schema 冲突。
6. `LightningImporter` 以统一的 `Run/Pause/Resume/Close` 协议适配 legacy local/TiDB backend 与 import-into backend；`newImporter` 是选择和构造性预检的集中入口。
7. `SwitchMode` 为 `tidb-lightning-ctl` 等调用者把 `import`/`normal` 字符串转换为 TiKV protobuf mode，并遍历 store 执行切换。

## 主要符号

- `DELIVER_PAUSER_GLOBAL`、`deliver_pauser`、`DeliverPauser`：进程级 `OnceLock<common::Pauser>` 及公开静态外观，提供 `Pause`、`Resume`、`IsPaused`。它跨任务保存控制面的暂停状态。
- `LightningStatus`：仅含原子计数 `FinishedFileSize`、`TotalFileSize`，由 `Status` 读取，并由 import-into 的进度 adapter 更新。
- `HttpState`：路由闭包共享的快照容器，保存队列、当前任务、取消函数、当前 importer、全局配置和根 context；它不是第二套调度状态。
- `Lightning`：实例级协调器。重要字段包括根 `ctx/shutdown`、HTTP `server/serverAddr`、任务队列 `taskCfgs`、当前 `curTask/cancel/importer`、指标和 `taskCanceled`。可变资源主要用 `Mutex` 或 `Arc<Mutex<_>>` 保护。
- `initEnv`、`New`：启动期构造函数。日志文件为空时 `initEnv` 无副作用；日志初始化失败会 `exit(1)`，TLS 失败通过 fatal 路径终止。
- `GoServe`、`goServe`、`httpHandleWrapper`：状态服务入口、mux/监听器装配和统一访问日志包装。
- `RunServer`、`RunOnceWithOptions`、`run`：常驻调度入口、单次任务入口和共享核心执行流程。
- `initDataSource`、`getKeyspaceName`、`initDBAndKeyspace`：数据源、SQL DB 和 keyspace 准备链。
- `Stop`、`TaskCanceled`、`Status`、`Metrics`：停止与只读观测接口。
- `parseTaskID`、`handle_task_http` 及 `handle_*`：`/tasks`、`/pause`、`/resume` 的协议实现；`writeJSONError` 和 `writeBytesCompressed` 统一错误 JSON 与 gzip 输出。
- `handleProgressTask`、`handleProgressTable`、`handleLogLevel`：进度查询和在线日志级别控制。
- `checkSystemRequirement`、`checkSchemaConflict`、`SwitchMode`：独立的前置/运维 helper。
- `LightningImporter`、`ControllerParamLocal`、`LegacyImporter`、`ImportIntoImporter`、`newImporter`：后端抽象、参数桥、两个适配器与后端工厂。

RustCodeGraph 的 `node --symbols-only` 在该文件识别出上述模块级符号和 impl 方法（目标文件共 126 个索引符号）；文件没有 `#[cfg]` 条件实现，平台差异由 `lib.rs` 选择 `sigusr1_unix.rs` 或 `sigusr1_other.rs` 承担。

## 执行流程

生产入口的主链是：`lightning/cmd/tidb-lightning/main.rs::run_with_factory` → `server::New` → `RealLightning::GoServe` → 根据全局配置选择 `RealLightning::RunServer` 或 `RealLightning::RunOnceWithOptions` → `Lightning::run`。

`GoServe` 先注册 `handleSigUsr1` 回调。常规 `StatusAddr` 非空时调用 `goServe`；为空则不主动监听。`goServe` 注册 `/metrics`、`/debug/pprof/*`、`/tasks[/...]`、`/progress/task`、`/progress/table`、`/pause`、`/resume`、`/loglevel`，发布实际监听地址，使用全局 TLS 包装 listener，并在线程中运行 `Server::Serve`。当前 `SIGUSR1` 分支直接创建了只含空 mux 的 server，而没有复用 `goServe` 的完整业务路由，这是当前 Rust 实现的实际限制。

`RunServer` 先由 `enableServerMode` 创建队列并刷新 `HttpState`，随后循环 `Pop`。每项任务使用共享 Prometheus 工厂和注册器调用 `run`；非 context-cancel 错误会记录日志并设置全局暂停态。`RunOnceWithOptions` 则依次应用 `RunOption`，在注入外部数据存储时用 `noop://` 通过配置调整，执行 `Config::Adjust`，生成非零、正 63 位随机 `TaskID`，可注册带 IO 计数的 MySQL dialer，最后调用 `run`。

`run` 的顺序决定可观察语义：

1. 记录 build/config/环境信息，并同步全局 `WAIT_REGION_ONLINE_ATTEMPT_TIMES`。
2. 创建、注册任务 metrics，把 metrics 和 logger 放入 task context，再派生 cancel context。
3. 在 `cancelLock` 下发布 `cancel` 和 `curTask`，刷新 HTTP 快照，广播任务开始。
4. 校验 TiDB TLS。非 import-into backend 调用 `initDataSource` 并广播数据源初始进度；import-into 跳过 mydump loader。
5. `initDBAndKeyspace` 建立或复用 DB；local backend 优先采用显式 keyspace，否则查询 TiDB config。
6. 构造 `ControllerParamLocal`，由 `newImporter` 预检并选择后端；构造成功后才把 importer 放入共享状态。
7. 在 importer mutex 下运行 `Run`，随后调用 `Close`。
8. 无论成功失败都调用 cancel，清空 `cancel/importer`，刷新 HTTP 快照，广播任务结束，注销 metrics，并返回原始结果。

`initDataSource` 若没有注入 storage，会解析 `Mydumper.SourceDir` 并创建对象存储。它用 `WalkDir(ListCount: 1)` 和哨兵错误区分“至少有一个文件”“空目录”“访问失败”，随后创建 `MDLoader`，再执行系统要求和 schema 冲突检查。

## 数据与状态

状态分为三层：进程级的 `DELIVER_PAUSER_GLOBAL`；`Lightning` 实例级的队列、HTTP server、当前任务和 metrics；任务级的派生 context、cancel、配置、loader、DB 和 importer。`HttpState` 用于让 `'static` 路由闭包看到实例状态，`refreshHttpState` 在开启 server mode、发布/清除任务状态和 `Stop` 时同步关键字段。

任务列表来自 `config::List`。`handle_get_task` 只有在 `cancel.is_some()` 时才把 `curTask.TaskID` 报告为 current；排队项由 `AllIDs` 返回。POST 先继承 global config，再加载 TOML 并 `Adjust`，随后 Push。DELETE 当前任务会取走并调用 cancel；否则从队列 Remove。PATCH 只接受 `front` 或 `back`。

`LightningStatus` 使用原子计数，适合跨 adapter 读取。相比之下，`curTask` 在任务结束后没有清空，但 current 的对外判定依赖已清空的 `cancel`，因此不会继续显示为运行中；这也是扩展代码不能只凭 `curTask.is_some()` 判断活动任务的原因。

`ControllerParamLocal` 汇集 DB metadata、进度状态、外部/ checkpoint storage、DB、checkpoint 名称、重复项指示器和 keyspace。当前 `LegacyImporter::Run` 转换了 DB metadata、状态初值、checkpoint 名称和 keyspace，但为下游 controller 构造的是默认 dump storage、内存 DB、无 checkpoint storage/dup indicator 的参数；`ImportIntoImporter::Run` 同样创建新的内存 DB 交给下游，而结构体持有的 `self.db` 主要在 `Close` 中关闭。它们属于当前迁移适配事实，不能假定所有 `ControllerParamLocal` 资源已经原样传递。

## 依赖与调用关系

上游直接证据：

- `lightning/cmd/tidb-lightning/main.rs` 调用 `server::New`，并通过 `RealLightning` 转发 `GoServe`、`RunServer`、`RunOnceWithOptions`、`Stop`、`TaskCanceled`。
- `lightning/cmd/tidb-lightning-ctl/main.rs` 调用公开的 `SwitchMode`。
- `lightning/pkg/server/lib.rs` 公开再导出本文件所有 public 符号。
- `lightning/pkg/server/lightning_test.rs`、`lightning_serial_test.rs`、`lightning_server_serial_test.rs`、`parity_test.rs` 直接调用构造、生命周期、helper 和 HTTP 接口。

下游直接证据：

- progress crate：`BroadcastStartTask`、`BroadcastInitProgress`、`BroadcastEndTask`、`MarshalTaskProgress`、`MarshalTableCheckpoints`。
- importer/importinto crate：`NewImportController`、`NewImporter` 和相应 `Run/Close`；bridges 负责配置、metadata 和错误类型转换。
- checkpoints crate：`newImporter` 对 legacy backend 调用 `OpenCheckpointsDB` 做 driver/DSN 构造性预检。
- 本 crate 边界：`config::List`、`common::TLS/Pauser`、`context`、`http`、`objstore`、`mydump`、`sql`、`tikv`、`pdhttp`、`metric/promutil` 等由 `lib.rs` 聚合，其中多数当前来自 `stubs.rs`。

RustCodeGraph `files --filter lightning/pkg/server` 确认 Rust/Go 源与独立测试均在索引中，`node --file ... --symbols-only` 给出了完整符号表。但对 `GoServe`、`RunServer`、`RunOnceWithOptions`、`run`、`initDataSource`、`Stop`、`parseTaskID`、`checkSystemRequirement`、`SwitchMode`、`newImporter` 执行带目标文件约束的 `callers/callees` 均没有返回边；因此本节的具体边以精确仓库搜索和源码调用点为依据，而不把空图结果解释为无调用者。

## 错误处理与边界

启动期错误较强：`initEnv` 失败导致进程退出，TLS 构造失败走 fatal。任务期错误通过 crate 的 `Result<_, Error>` 传播，`run` 在所有结果上执行 cancel、共享状态清理、结束广播和 metrics 注销；`lightning_serial_test.rs::test_run` 明确断言数据源、未知 backend、未知 checkpoint driver 和坏 checkpoint 路径失败后 `cancel/importer` 均为空。

数据源探测把空目录映射为 `ErrEmptySourceDir`，其他遍历问题归一化或包装为 `ErrStorageUnknown`。`checkSystemRequirement` 只对 local backend 生效，按最大 `TableConcurrency` 个表的总大小、writer memory cache、range concurrency 和 region concurrency 估算文件描述符；`checkSchemaConflict` 只在 checkpoint 开启且 driver 为 MySQL 时检查保留 checkpoint 表名。

keyspace 查询没有 DB 时返回空。local backend 已显式配置时不查询；无 CONFIG 权限时记录信息并继续，其他查询错误记录 warning 并使用空 keyspace。该容错边界与 Go 的“尽量继续”意图一致，但 Rust 当前实现对权限错误不再根据 logger level 保留错误值。

HTTP helper 统一返回 JSON 错误和状态码：非法方法写 `Allow`；未开启 server mode 的 POST/PATCH 返回 501；非法 task id/TOML/config/patch verb 返回 400；不存在的任务返回 404；pause/resume 后端错误返回 500。`parseTaskID` 只去除一个前导 `/`，所以 `//42` 被视为空数字而不是任务 42。gzip 仅通过 `Accept-Encoding` 是否包含 `gzip` 判定；writer 创建使用 `unwrap`，写入/关闭错误被忽略。

`SwitchMode` 对未知 mode 立即报错；合法 mode 通过 `tikv::ForAllStores` 向所有非 Offline 过滤边界对应的 store 执行切换。`newImporter` 对未知 backend 立即报错；local/TiDB 且 checkpoint 开启时只允许 MySQL/File driver，并提前打开、关闭 checkpoint DB，以保持构造期失败时机。

## 并发与资源生命周期

HTTP server 和 `SIGUSR1` 启动路径使用 `thread::spawn`；任务本身由调用 `RunServer`/`RunOnceWithOptions` 的线程运行。`LightningImporter: Send` 并被包进 `Arc<Mutex<Box<dyn LightningImporter>>>`，HTTP pause/resume 与执行路径会竞争同一把 importer mutex。当前 `run` 在持锁状态下执行整个 `Run`，因此长期运行期间 pause/resume handler 无法取得该锁；尽管接口存在，这一锁粒度是实际可暂停性的风险点。

`cancelLock` 保护 `cancel`、`curTask`、`importer` 的关键发布/清理，`serverLock` 保护部分 server 地址读取，路由使用自己的 `HttpState` mutex。全局 region backoff 在 `unsafe` 块中写入，源码承认它是进程共享配置；若作为 library 并发运行多个任务，存在互相覆盖的风险。

`run` 注册 metrics 后必定注销；构造成功的 importer 在 `Run` 后调用 `Close`。`LegacyImporter::Run` 因下游 controller 非 `Send`，在当前线程创建、运行并关闭 controller。`ImportIntoImporter::Run` 创建并关闭 import-into importer，adapter 用原子状态回传进度；其 `Pause/Resume` 只记录“不支持”并返回成功。`Stop` 取消当前任务、设置 `taskCanceled`，关闭 HTTP server，再取消根 context。

需要注意，`LegacyImporter` 自有 pauser 与传给下游 `ControllerParam` 的新 pauser 不是同一个对象；`Pause/Resume` 修改自有/全局 pauser，但当前代码没有直接证据证明新建 controller 观察的是该对象。扩展或修复暂停功能时应把这视为必须由回归测试证明的连接点，而不是既定保证。

## 与 Go 版本的对应关系

Rust 的 `Lightning`、`initEnv/New`、`GoServe/goServe`、`RunServer`、`RunOnceWithOptions/run`、`initDataSource`、`initDBAndKeyspace`、`Stop/Status/Metrics`、任务 HTTP handlers、进度/loglevel handlers、两个检查函数、`SwitchMode`、`LightningImporter/newImporter` 均可在 `lightning.go` 找到同名或直接对应实现。主流程顺序、HTTP 路径、状态码、队列动作、open-files 估算公式和 checkpoint schema 错误分类均以 Go 为语义基准。

当前差异/迁移限制包括：Rust 的 HTTP 访问包装只记录 method、URL、status，没有 Go `loggingResponseWriter` 的非 gzip body 摘要；Rust `SIGUSR1` 分支没有复用完整 mux；Rust 的 failpoint 注入为空闭包，没有恢复 Go 的动态测试值语义；Rust TaskID 来自 UUID 截断，而 Go 使用 `crypto/rand.Int`；Rust adapters 对 DB/storage/checkpoint/pauser 的传递仍有内存替身和丢失字段；import-into pause/resume 明确不支持；Go 的 `CleanupMetas` 和证书 helper 不在本文件。这些差异都应在声称“完全对齐”前单独处理。

独立 Rust 测试与 Go 测试有清晰对应：`lightning_serial_test.rs` 对照 `lightning_serial_test.go` 的 init/run/rlimit/schema 场景；`lightning_server_serial_test.rs` 对照 `lightning_server_serial_test.go` 的 server mode、GET/DELETE、非 server mode HTTP 协议；`parity_test.rs` 额外集中验证 Rust/Go 公共合同；`lightning_test.rs` 回归了 HTTP 启动后再开启 server mode 时 `HttpState` 必须刷新。

## 扩展指南

新增 importer backend 时，最小接入面是实现 `LightningImporter`、在 `newImporter` 增加明确分支，并决定是否需要 mydump 数据源（`run` 当前只对 import-into 特判跳过）、DB/keyspace、checkpoint 构造性预检及进度 adapter。必须同步 `lightning_serial_test.rs` 的失败时机、`parity_test.rs` 的 known/unknown backend 合同，以及 HTTP pause/resume 能力测试；不能用返回 `Ok(())` 假装支持实际未接线的控制动作。

修改任务生命周期时应守住：先发布 `cancel/curTask` 再广播/执行；importer 构造成功后才发布；所有结果都清理 `cancel/importer`、结束广播和 metrics；HTTP current 判定与这些时机一致。若要允许运行中 pause/resume，需要重新设计 `Run` 长持 importer mutex 的方式，并验证 controller 使用同一 pauser。

修改 HTTP API 时，应同步 `goServe` 路由、`HttpState` 刷新点、`Allow`/状态码/JSON/gzip 协议，并扩展 `lightning_server_serial_test.rs` 的真实 TCP 测试；若修复 `SIGUSR1` 服务，应验证其 mux 与常规 `goServe` 等价。修改 task path 解析时还要保留空 id 与非法 id 的区分，以及 `//42` 只剥一个斜杠的 Go 语义。

修改数据源或资源检查时，应继续在 importer 构造前失败，保持空目录、storage error、rlimit 和 checkpoint schema 的错误类别；同步 `lightning_serial_test.rs`。修改 Go 对照逻辑时要逐项确认 Rust 是否使用真实外部资源还是 `stubs.rs`/内存对象，尤其关注 TLS、HTTP、SQL、PD/TiKV、对象存储的兼容与性能风险。

性能敏感点包括 `MDLoader` 扫描并发 `RegionConcurrency * 2`、文件描述符估算、单个 mutex 覆盖完整 importer `Run`、HTTP state 整体锁和跨所有 store 的模式切换。任何并发优化都必须同时证明状态发布顺序、取消、关闭和 Go 可观察行为不变。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标目录 24 个文件；`files --filter lightning/pkg/server` 确认目标 Rust/Go/测试文件均已索引；`node --file lightning/pkg/server/lightning.rs --offset ...` 阅读目标文件；`node --file ... --symbols-only` 核对模块级常量、类型、函数、trait 与 impl。精确 `callers/callees --file` 查询未返回方法边，已用源码调用点补证。
- 生产源码：完整阅读 `lightning/pkg/server/lightning.rs`；读取 `lightning/pkg/server/lib.rs` 确认模块再导出与平台选择；读取 `lightning/pkg/server/Cargo.toml` 确认 crate 边界和四个直接业务 crate 依赖；读取 `lightning/cmd/tidb-lightning/main.rs` 与 `lightning/cmd/tidb-lightning-ctl/main.rs` 确认上游入口。
- Go 对照：阅读 `lightning/pkg/server/lightning.go` 的对应符号与核心流程，并用符号清单确认 Rust 当前缺少的 Go helper。
- Rust 测试：阅读 `lightning/pkg/server/lightning_test.rs`、`lightning_serial_test.rs`、`lightning_server_serial_test.rs`、`parity_test.rs`；它们分别证明 HttpState 刷新、失败清理/资源检查、真实 TCP 控制面、Go 公共合同与 gzip/资源副作用。
- Go 测试：核对 `lightning/pkg/server/lightning_serial_test.go` 和 `lightning_server_serial_test.go` 的对应测试入口，包括 `TestInitEnv`、`TestRun`、`TestCheckSystemRequirement`、`TestCheckSchemaConflict`、`TestRunServer`、`TestGetDeleteTask`、`TestHTTPAPIOutsideServerMode`。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰好包含 11 个固定二级章节。
