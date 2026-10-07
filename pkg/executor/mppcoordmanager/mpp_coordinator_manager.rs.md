# `pkg/executor/mppcoordmanager/mpp_coordinator_manager.rs`

## 文件定位

本文件实现 `astersql-executor-mppcoordmanager` crate 的核心业务类型。crate 入口
[`lib.rs`](lib.rs) 公开 `mpp_coordinator_manager` 模块并重新导出全部符号；
[`Cargo.toml`](Cargo.toml) 用 `package.metadata.porting.go-package` 将它对应到 Go 包
`pkg/executor/mppcoordmanager`。它在概念上位于 MPP 查询执行器与 TiFlash 任务状态上报之间：用
`(MppQueryId, gather_id)` 唯一标识一次 gather 的协调器，并负责注册、查找、注销和超时回收。

当前 Rust 接线并不完整。进程启动与关闭分别从
[`cmd/tidb-server/main.rs`](../../../cmd/tidb-server/main.rs) 的
`createStoreDDLOwnerMgrAndDomain`、`closeDDLOwnerMgrDomainAndStorage` 调用全局
`InstanceMPPCoordinatorManager.Run/Stop`；[`pkg/server/server.rs`](../../server/server.rs) 的
`Server::run` 会写入 SQL 监听地址。另一方面，Rust 的
[`executor_with_retry.rs`](../internal/mpp/executor_with_retry.rs) 当前定义了另一个同名
`MppCoordinatorManager` 和另一套 `CoordinatorUniqueId`，`RpcServer` 也仅持有注入的
`Arc<dyn MppCoordinator>`。仓库生产 Rust 调用搜索没有发现目标文件的 `register`、
`unregister` 或 `report_status` 与这两条链的适配。因此，本文件目前是“生命周期和服务地址已接线，
协调器状态路由实现已存在但尚未接入生产 Rust 主链”的移植状态，不能把 Go 版完整接线当成 Rust 现状。

## 核心职责

- `MppCoordinatorManager` 以 `HashMap<CoordinatorUniqueId, Arc<dyn MppCoordinator>>`
  保存进程内协调器。键同时包含查询时间戳、本地查询号、发起服务器号和 gather 号，避免不同
  查询轮次或恢复轮次互相覆盖。
- `register`/`unregister` 实施唯一性约束和登记生命周期；重复键会返回包含四段 ID 的
  `CoordinatorError`。
- `report_status` 从请求元数据重建键，短暂持锁取得 `Arc` 后释放管理器锁，再调用协调器代码；
  未找到或协调器返回错误时，把请求的 `mpp_version` 原样带入 `MppError`。
- `run` 启动唯一的后台检测线程。线程按 `detect_frequency` 扫描，只删除同时满足“查询时间超过
  最大寿命”和 `MppCoordinator::is_closed()` 的条目，避免仍持续产出数据的长查询被时间条件单独误删。
- `init_server_address`/`server_address` 保存本节点能否提供 MPP 服务及其地址；
  `CoordinatorMetrics` 提供注册总量、当前登记量和超时删除量的原子快照。
- `Run`、`Stop`、`Register` 等 Go 风格别名以及 `CoordinatorUniqueID`、
  `MPPCoordinatorManager` 类型别名保留迁移期调用形态。

## 主要符号

- `DETECT_FREQUENCY`：默认五分钟检测周期；`detectFrequency` 是 Go 风格别名。
- `MppQueryId { query_ts, local_query_id, server_id }`：查询级身份。
  `CoordinatorUniqueId { mpp_query_id, gather_id }`：管理器的完整哈希键；
  `CoordinatorUniqueID` 是其别名。
- `ReportTaskMeta`、`ReportTaskStatusRequest`、`ReportTaskStatusResponse`、`MppError`：
  本文件内的轻量状态上报协议模型。它们尚不是 `kv::ReportStatusRequest` 或 RPC 层
  `MppTaskStatusRequest` 的直接类型别名。
- `CoordinatorError(String)`：注册冲突和协调器回调错误的字符串错误，实施 `Display` 与
  `std::error::Error`。
- `MppCoordinator` trait：被管理对象的最小接口，要求 `Send + Sync + 'static`，只暴露
  `is_closed` 与 `report_status`。
- `CoordinatorMetrics`：三个私有 `AtomicU64` 及只读访问器 `total_registered`、`active`、
  `overtime`。
- `ManagerState`：由单个 `Mutex` 保护 `server_on`、`server_address` 和协调器表。
- `BackgroundRuntime`：由另一个 `Mutex` 保护停止发送端和后台 `JoinHandle`。
- `MppCoordinatorManager`：组合共享状态、后台运行时、指标、检测周期和原子最大寿命；
  `Default` 等价于 `new(DETECT_FREQUENCY)`。
- `InstanceMPPCoordinatorManager`：通过 `LazyLock` 延迟创建的进程级单例；
  `newMPPCoordinatorManger` 保留 Go 原函数名中的 `Manger` 拼写。

## 执行流程

1. `MppCoordinatorManager::new` 创建空状态、空后台句柄、零值指标和零值
   `max_lifetime_nanos`，但不启动线程。
2. `run` 先锁定 `BackgroundRuntime`；已有句柄时直接返回。首次运行把
   `astersql_store_copr::TI_FLASH_READ_TIMEOUT_ULTRA_LONG + detect_frequency` 以饱和加法
   计算为纳秒并发布到原子字段，然后创建停止通道和线程。
3. 后台线程对停止接收端调用 `recv_timeout(frequency)`。收到停止消息即退出；超时则调用
   `detect_and_delete_shared`，传入本次启动时固定的最大寿命和当前 UNIX 纳秒时间。
4. `detect_and_delete_shared` 在状态锁内遍历注册表，以
   `query_ts.wrapping_add(maximum_lifetime)` 计算截止时间。只有 `now_timestamp > deadline`
   且协调器已关闭才加入删除列表；删除完成后释放锁，再累计 `overtime`。
5. MPP 执行侧预期通过 `register` 放入协调器。该方法在状态锁内拒绝重复键、插入 `Arc`，
   并增加 `total_registered` 与 `active`。当前生产 Rust 执行器尚未适配到这个注册入口。
6. TiFlash 状态上报预期进入 `report_status`：从 `request.meta` 构造完整键，在锁内克隆
   `Arc`，随后无锁调用 `MppCoordinator::report_status`。成功返回空响应；查找失败或回调失败
   返回带协议版本的错误响应。当前 RPC 生产路径通过另一个注入 trait 转发，尚未直接调用此函数。
7. 正常查询结束时 `unregister` 删除键；只有确实删除了条目才减少 `active`。`stop` 取出停止端
   和句柄、发送停止信号并 `join`；`Drop` 再调用 `stop`，所以显式停止后析构仍安全。

## 数据与状态

`ManagerState` 把地址配置与注册表放在同一互斥区。`server_address()` 返回克隆字符串，调用者
不会借用锁内数据；`coordinator_ids()` 同样返回键快照。`init_server_address(false, value)` 只把
`server_on` 设为 false，不清空旧地址，这是与 Go 代码一致的状态保留语义，调用者必须以布尔值判断
地址是否有效。

最大寿命用 `AtomicU64` 单独保存。`run` 采用 `Release` 写、访问器和注入清理采用 `Acquire` 读；
测试可通过 `set_max_lifetime_nanos` 注入边界值。后台线程捕获启动时的 `maximum`，因此线程运行后
再调用 setter 只影响显式 `detect_and_delete`，不会改变该线程本轮生命周期内使用的阈值。

指标也是原子值：成功注册同时增加总量和活跃量，显式注销只在键存在时减少活跃量，超时清理只增加
`overtime`。后一点与 Go 实现相同，但意味着 `active` 并不因超时删除而下降；使用该值做实时表大小
判断前应意识到它并非始终等于 `coordinator_count()`。

## 依赖与调用关系

crate 清单中唯一必选业务依赖是 `astersql-store-copr`，本文件用其
`TI_FLASH_READ_TIMEOUT_ULTRA_LONG` 计算最大寿命。`astersql-executor-metrics`、`astersql-kv`
和 `astersql-util-logutil` 在 `Cargo.toml` 中是可选依赖，但当前文件以本地协议类型、本地 trait 和
`CoordinatorMetrics` 工作，没有引用这些 crate。

已验证的上游边如下：

- `cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` →
  `InstanceMPPCoordinatorManager.Run`，在 session bootstrap 前启动清理线程。
- `cmd/tidb-server/main.rs::closeDDLOwnerMgrDomainAndStorage` →
  `InstanceMPPCoordinatorManager.Stop`，在关闭 storage 前停止线程。
- `pkg/server/server.rs::Server::run` → `init_server_address`，用实际绑定的 SQL 监听地址更新单例。
- 独立 Rust 测试直接调用注册、清理、地址和状态上报 API。

未形成的生产边也同样重要：`executor_with_retry.rs::setupMPPCoordinator` 注册的是该文件自己定义的
`CoordinatorRegistry`/`MppCoordinatorManager`；`rpc_server.rs::RpcServer::report_mpp_task_status`
调用的是构造时注入的 `Arc<dyn rpc_server::MppCoordinator>`。两者的 ID、请求和协调器 trait
均与本文件不同，仓库中没有发现适配实现把它们导向 `InstanceMPPCoordinatorManager`。

## 错误处理与边界

- 重复注册返回 `Err(CoordinatorError)`，不会覆盖原 `Arc`，也不会改变任何指标。
- 找不到协调器是预期竞态：查询可能已先注销。`report_status` 返回
  `"MppCoordinator not exists"`，并保留请求 `mpp_version`，不 panic。
- 协调器回调失败会把 `CoordinatorError` 文本放入响应；Rust 版当前不记录 Go 版的 warning 日志。
- 所有管理器 `Mutex` 获取都使用 `expect`；若持锁线程 panic 导致锁中毒，后续调用会 panic，而不是
  返回可恢复错误。后台 `join` 错误和停止消息发送错误则被忽略，使 `stop` 保持无返回值且幂等。
- `now_nanos` 在系统时间早于 UNIX epoch 时回退为 0，超出 `u64` 时封顶；最大寿命的
  `Duration` 加法饱和，纳秒转换也封顶。
- 到期判断是严格的大于号，恰好等于截止时间不会删除；即使时间已过，`is_closed() == false`
  仍保留条目。
- 截止时间刻意使用 `wrapping_add` 复现 Go `uint64` 溢出语义；靠近 `u64::MAX` 的查询时间戳
  可能回绕到较小值，并按回绕结果判断过期。

## 并发与资源生命周期

协调器表由 `Arc<Mutex<ManagerState>>` 共享给后台线程。清理期间会在状态锁内调用
`MppCoordinator::is_closed`，所以该方法应快速且不得反向获取管理器锁，否则会放大临界区或死锁。
相比之下，`report_status` 只在锁内克隆 `Arc`，随后释放锁再执行用户协调器代码；因此慢上报不会
阻塞注册、注销或地址读取，同时即使并发注销，已取得的 `Arc` 仍保证本次回调对象存活。

后台运行时使用独立互斥锁，保证同时最多安装一个检测线程。`run` 重复调用是 no-op；`stop` 在锁内
只取走资源，实际发送和 `join` 在锁外完成，避免等待线程时占用运行时锁。停止发送端存在于管理器内，
正常运行时 `recv_timeout` 的错误就是周期超时；`stop` 发送消息后线程退出。显式 `stop`、重复
`stop` 和 `Drop` 可以顺序调用。停止后句柄为空，之后再次 `run` 会创建新线程并重新计算最大寿命。

`MppCoordinator: Send + Sync + 'static` 和表中 `Arc` 允许协调器跨线程共享。原子指标使用
Acquire/Release 或 AcqRel 排序，适合作为并发快照，但多个指标之间不是事务性快照，读取者不能假定
三者来自同一瞬间。

## 与 Go 版本的对应关系

Go 对照文件是
[`mpp_coordinator_manager.go`](mpp_coordinator_manager.go)，核心映射如下：

- Go `sync.Mutex + map` 对应 Rust `Mutex<ManagerState> + HashMap`；Go
  `context.CancelFunc + WaitGroup + goroutine + ticker` 对应 Rust
  `mpsc::Sender + JoinHandle + thread::spawn + recv_timeout`。
- Go `kv.MPPQueryID`、`kv.MppCoordinator` 和 kvproto 请求/响应在 Rust 目标文件中被本地
  `MppQueryId`、`MppCoordinator`、`ReportTaskStatusRequest/Response` 复刻，尚未与 Rust
  `astersql-kv`/RPC 类型统一。这是接口迁移差异，不应误认为零成本类型别名。
- 注册冲突文本、缺协调器文本、四字段键、锁外上报、只清理已关闭超时协调器以及
  `uint64` 截止时间回绕都保持 Go 语义。
- Go 超时清理逐项增加 Prometheus overtime 指标并记录 error 日志；Rust 用原子计数批量增加，
  不记录被删除 ID。Go 上报回调失败记录 warning；Rust 只返回错误响应。
- Rust 额外把 `run`、`stop` 做成可重复调用，并通过 `Drop` 自动停止；Go 的 `Run` 每次都会建立
  新 context/goroutine，`Stop` 直接调用当前 cancel。Rust 的幂等行为有独立测试，属于安全性增强。
- Go 生产执行器直接使用这个包的全局管理器注册/注销；Rust 当前执行重试模块有独立注册表，
  所以行为实现的局部对齐不等于全链路接线已经对齐。

## 扩展指南

接入真实 Rust MPP 主链时，应先决定统一接口还是增加显式适配层，避免继续扩散两个同名管理器：

1. 若保留本文件为唯一注册表，需要把 `CoordinatorUniqueId` 与
   `executor_with_retry::CoordinatorUniqueId`、本地 `MppCoordinator` 与 kv 的协调器/状态上报器、
   `ReportTaskStatusRequest` 与 RPC/kv 请求做可审计转换，并让 RPC 构造处注入同一单例适配器。
2. 注册必须发生在协调器对外接收 ReportStatus 之前；`Execute` 失败、恢复重建、正常 `Close` 和
   `Drop` 都必须注销，保持 gather ID 与注册键同步。不要只接上报而遗漏恢复路径。
3. 若调整超时策略，修改 `run`/`detect_and_delete_shared`，并同步独立测试
   [`mpp_coordinator_manager_test.rs`](mpp_coordinator_manager_test.rs)；继续保留“已关闭”门槛、
   严格截止比较和 Go 溢出边界，除非兼容性决策明确改变它们。
4. 若接入真实 metrics/logging，应启用并使用 Cargo 中现有可选依赖，核对超时删除时是否也应降低
   active；任何改变都需与 Go 指标语义和监控查询兼容。
5. 地址读写若要脱离单锁或清除关闭后的旧地址，应同步检查
   `pkg/server/server.rs::Server::run` 和
   `pkg/server/handler/tests/http_handler_test.rs` 的读取行为。
6. 测试继续放在独立文件，不要内嵌到生产源。除现有清理测试外，建议在接线任务中增加重复注册、
   并发注销与上报、回调错误版本透传、未关闭超时不删除、线程重启，以及生产 RPC 到同一注册表的
   集成测试。兼容风险主要是 ID/协议类型转换；性能风险主要是清理时持锁调用 `is_closed` 和全表扫描。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `files --filter pkg/executor/mppcoordmanager` 确认目标 Rust、Go 对照及两套测试均已索引。
- RustCodeGraph `node --file pkg/executor/mppcoordmanager/mpp_coordinator_manager.rs`：核对全部
  474 行、类型、方法、锁范围、原子顺序、后台线程和别名。
- RustCodeGraph `query MppCoordinatorManager`、`query InstanceMPPCoordinatorManager`、
  `query report_status`，以及对 `run`、`report_status`、`setupMPPCoordinator` 的 callers/callees
  查询：确认同名类型、已知入口和图中未解析的部分。
- RustCodeGraph 读取
  `cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain`/
  `closeDDLOwnerMgrDomainAndStorage`、`pkg/server/server.rs::Server::run`、
  `pkg/server/rpc_server.rs::RpcServer::report_mpp_task_status` 和
  `pkg/executor/internal/mpp/executor_with_retry.rs::setupMPPCoordinator`；随后因调用图不能消歧
  `LazyLock` 静态引用，用精确 `rg` 搜索补充当前 Rust 生产调用集合。
- 直接读取 `pkg/executor/mppcoordmanager/Cargo.toml`，核对 crate 名、入口、Go 包元数据和依赖；
  RustCodeGraph 读取同目录 `lib.rs`，核对公开再导出与独立测试模块。
- Go 语义依据：`mpp_coordinator_manager.go` 和
  `mpp_coordinator_manager_test.go::TestDetectAndDelete`。
- Rust 边界依据：`mpp_coordinator_manager_test.rs::test_detect_and_delete`、
  `detect_and_delete_wraps_query_deadline_like_go`，以及
  `pkg/executor/test/tiflashtest/tiflash_test.rs` 中
  `mpp_manager_reports_missing_coordinator_with_request_version`、
  `mpp_manager_run_stop_is_idempotent_and_sets_lifetime`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文恰有 11 个固定二级章节，
  并人工复核所有“已接线/未接线”结论均有上述源码或调用搜索支持。
