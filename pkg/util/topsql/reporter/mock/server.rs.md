# `pkg/util/topsql/reporter/mock/server.rs`

源文件：[server.rs](server.rs)。本文描述的是当前 Rust 实现；它是 TopSQL reporter 测试使用的真实回环 gRPC 服务，不是生产部署中的 TopSQL Agent。

## 文件定位

该文件属于 Cargo 包 `astersql-util-topsql-reporter-mock`。包入口 [lib.rs](lib.rs) 将构建期生成的 `tipb` protobuf/tonic 类型和 `server` 模块公开；[Cargo.toml](Cargo.toml) 声明 `tonic`、`tokio`、`tokio-stream`、`prost` 等依赖，并用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/util/topsql/reporter/mock`。

在完整链路中，reporter 或 single-target data sink 作为 gRPC 客户端，把 TopSQL 记录、TopRU 记录及 SQL/Plan 元数据发往 `StartMockAgentServer` 返回的回环地址；测试再通过 `mockAgentServer` 的查询接口观察接收结果。上层使用证据包括 [single_target_3_aster_unit_test.rs](../single_target_3_aster_unit_test.rs) 的地址切换测试和 [topsql_test.rs](../../topsql_test.rs) 的 reporter 端到端测试。

本文件没有条件编译项；测试装配由 [lib.rs](lib.rs) 中的 `#[cfg(test)]` 完成。同目录不存在 `doc.go`，包级行为以源文件、Cargo 声明和测试为准。

## 核心职责

- `StartMockAgentServer` 在 `127.0.0.1:0` 上绑定操作系统分配的端口，注册 tonic 生成的 `TopSqlAgentServer`，异步启动服务并返回可观测、可关闭的句柄。
- `AgentService` 实现 `TopSqlAgent` 的四个客户端流 RPC：`report_top_sql_records`、`report_top_ru_records`、`report_sql_meta` 和 `report_plan_meta`。
- `State` 在线程间保存 Hang 窗口、按 digest 去重/覆盖的元数据，以及按 RPC 流分批保存的 TopSQL/TopRU 记录。
- `mockAgentServer` 向测试提供计数、限时等待、按 digest 查询、批次取出、地址读取和停止服务等控制面 API。

它刻意提供“可制造慢下游”的测试能力：`HangFromNow` 修改时间窗，四个 RPC 在每次读取下一条流消息之前都执行 `State::mayHang`。

## 主要符号

### 内部类型

- `HangWindow { beginTime, endTime }`：两个 `Instant` 构成半开语义的等待窗口。`mayHang` 实际判断为 `now < endTime && now > beginTime`，边界点本身不等待。
- `State`：共享数据容器。`hang` 使用 `RwLock`；`sqlMetas`、`planMetas`、`records`、`ruRecords` 分别使用独立 `Mutex`，避免所有观测数据共用一把锁。
- `AgentService { state: Arc<State> }`：tonic 服务实现；可克隆，并与外部控制句柄共享同一个 `State`。

### 公开入口与控制 API

- `pub async fn StartMockAgentServer() -> io::Result<mockAgentServer>`：可能因监听器绑定或读取本地地址失败而返回 `io::Error`。
- `mockAgentServer::HangFromNow(Duration)`：从调用时刻起设置新的 Hang 窗口。
- `RecordsCnt`、`RURecordsCnt`、`SQLMetaCnt`：分别返回已完整接收的 TopSQL 批次数、TopRU 批次数和当前 SQL digest 数。
- `WaitCollectCnt`、`WaitCollectCntOfSQLMeta`：等待相对旧计数的增量达到目标；函数没有成功/超时返回值。
- `GetSQLMetaByDigestBlocking`、`GetPlanMetaByDigestBlocking`：限时轮询 digest，返回 `(值, 是否存在)`。
- `GetLatestRecords`、`GetLatestRURecords`：清空对应的全部批次，只返回清空前最后一批；没有批次时返回 `None`。
- `GetTotalSQLMetas`：克隆当前 SQL meta map 的所有值，返回顺序不稳定。
- `Address`：克隆实际监听地址；`Stop`：至多发送一次 oneshot 关闭信号。
- `Drop for mockAgentServer`：句柄离开作用域时调用 `Stop`，为未显式停止的测试提供兜底清理。

`mockAgentServer` 虽是公开类型，但沿用 Go 命名，所以文件级 `#![allow(non_snake_case, non_camel_case_types)]` 保留了迁移 API 的拼写。

## 执行流程

1. `StartMockAgentServer` 创建动态回环监听器，取得实际 `ip:port`，建立 `Arc<State>`、`AgentService` 和 oneshot 停止通道。
2. 函数用 `tokio::spawn` 驱动 tonic server。`serve_with_incoming_shutdown` 从 `TcpListenerStream` 接受连接，并在 oneshot 接收端完成时结束。
3. 客户端打开四类 RPC 中的一条流。RPC 循环先调用 `mayHang`，再调用 `Streaming::message`：收到消息则收集或写入 map，收到流结束则退出，收到 tonic 状态错误则立即向客户端传播。
4. TopSQL/TopRU 记录先在该 RPC 的局部 `Vec` 中积累；只有流正常结束，整个 `Vec` 才作为一个批次写入共享状态。SQL/Plan meta 则逐条写入，以 digest 为键覆盖已有值。
5. 测试线程通过计数或阻塞查询等待异步接收结果。`WaitCollectCnt*` 和 `Get*ByDigestBlocking` 每 1 ms 轮询一次，直到条件成立或超过超时。
6. `GetLatest*` 通过 `std::mem::take` 原子地替换共享批次容器，然后在锁外选择最后一批；较早批次也随本次读取被丢弃。
7. 显式 `Stop` 或 `Drop` 消耗 `grpcServer: Option<oneshot::Sender<()>>`，触发 tonic 的优雅关闭 future；重复 `Stop` 不再发送信号。

## 数据与状态

- `sqlMetas: HashMap<Vec<u8>, tipb::SqlMeta>` 保留完整 SQL meta。键是原始 digest 字节，因此不要求 UTF-8；[migration_aster_unit_test.rs](migration_aster_unit_test.rs) 用 `[0xff, 0x00]` 验证了该边界。
- `planMetas: HashMap<Vec<u8>, String>` 只保留 `normalized_plan`，不会保留完整 `PlanMeta`。相同 digest 的后到消息覆盖先到消息。
- `records` 与 `ruRecords` 是二维向量：外层元素对应一次正常结束的流式 RPC，内层是该流收到的消息。空流也会形成空批次。
- `State::new` 预分配两个 meta map 的容量 5000；记录向量不预分配。每个 RPC 的局部批次以容量 10 创建，但可按需增长，不构成数量上限。
- `GetTotalSQLMetas` 返回快照而非活动引用；因为 `HashMap` 无稳定迭代顺序，调用方只能把它当集合或自行排序。
- 等待接口使用 `saturating_sub(old)`。当调用方给出的旧计数大于当前计数（例如批次已被 `GetLatest*` 清空）时，增量保持 0，不发生无符号下溢。

## 依赖与调用关系

### 上游

- [migration_aster_unit_test.rs](migration_aster_unit_test.rs) 直接创建服务器，通过生成的 `TopSqlAgentClient` 上报四类流，并读取全部控制 API。
- [single_target_3_aster_unit_test.rs](../single_target_3_aster_unit_test.rs) 把 `Address` 注入 `MutableReceiverAddress`，验证 single-target sink 向当前地址发送 TopSQL/SQL meta/Plan meta，以及切换到第二台 mock server 后重新发送。
- [topsql_test.rs](../../topsql_test.rs) 将 mock 地址接到 `NewSingleTargetDataSinkWithReceiver`，验证 `RemoteTopSQLReporter` 生成的记录与元数据能从本服务读回。
- Go 侧对应调用者包括 [single_target_test.go](../single_target_test.go) 和 [topsql_test.go](../../topsql_test.go)，用于核对迁移意图。

RustCodeGraph 已索引本文件并列出 27 个符号；对 `StartMockAgentServer`、`GetLatestRecords`、`HangFromNow`、`report_top_sql_records` 的精确查询确认了签名。当前索引的 `callers`/`callees` 对这些 Rust 符号没有返回边，因此上述上游关系由索引文件上下文和 `rg` 的直接引用结果补证，而不是推测出的调用图。

### 下游

- `crate::tipb::top_sql_agent_server::{TopSqlAgent, TopSqlAgentServer}` 提供服务 trait 和注册包装器；消息类型均来自 `crate::tipb`。
- `tonic::{Request, Response, Status, Streaming}` 定义 RPC 边界；`tonic::transport::Server` 负责服务生命周期。
- `tokio::net::TcpListener`、`tokio::spawn`、`tokio::time::sleep_until` 和 `tokio::sync::oneshot` 负责监听、任务调度、异步 Hang 与关闭信号。
- `tokio_stream::wrappers::TcpListenerStream` 把 Tokio 监听器适配为 tonic incoming stream。
- 标准库 `Arc`、`Mutex`、`RwLock`、`HashMap`、`Instant` 和 `Duration` 承载共享状态与时间控制。

## 错误处理与边界

- 启动阶段只把监听/地址错误作为 `io::Result` 返回。后台 serve 错误无法再返回给启动者，只以 `eprintln!` 记录；测试若需要断言后台失败，当前 API 不提供错误通道。
- RPC 中 `stream.message().await?` 将解码、传输或取消错误原样转成 `tonic::Status` 返回。TopSQL/TopRU 的局部批次仅在正常 EOF 后提交，因此中途错误会丢弃该流已读记录；SQL/Plan meta 是逐条提交，中途错误会保留此前成功写入的条目。
- 所有标准锁都用 `expect("... lock poisoned")` 获取。持锁线程 panic 会使后续访问 panic，这是测试辅助设施的 fail-fast 选择，不是可恢复错误。
- 超时判断使用 `elapsed() > timeout`，而非 `>=`。零超时时仍会先查一次状态：已有 digest 可立即成功，不存在则在第一次检查后返回默认值/空字符串与 `false`。
- `WaitCollectCnt*` 超时与成功均返回 `()`，调用方必须随后读取计数或数据才能区分结果；不能仅凭函数返回认定收集成功。
- `mayHang` 在读取消息前等待，但不取消正在等待的 sleep。关闭信号停止服务的具体完成时刻受 tonic 正在处理的 RPC 与 Hang 窗口影响。
- `Stop` 只发关闭信号，不等待后台任务 join；`Address` 在停止后仍返回原字符串，但该地址不再保证可连接。

## 并发与资源生命周期

`State` 由 tonic 服务任务和测试控制句柄通过 `Arc` 共享。Hang 窗口读取时只短暂持有 `RwLock`，复制 `HangWindow` 后才 `await`，因此不会跨异步挂起持锁；RPC 的 `Mutex` 也只在同步插入或批次提交期间持有，没有跨 `.await` 持锁。

四类数据使用不同锁，因此 SQL meta 写入不会被记录批次读取直接阻塞。同一种数据仍是串行临界区；`GetLatest*` 与 RPC 批次提交之间的竞争由互斥锁线性化：批次要么包含在本次清空中，要么留给下一次读取。

计数/查询等待函数是同步阻塞轮询，内部调用 `std::thread::sleep`。它们适合测试线程或 Tokio 多线程 runtime；若在单线程异步执行器的唯一工作线程中直接调用，可能阻塞处理 RPC 的任务。扩展时若需要生产级等待语义，应考虑 `Notify`/channel 等异步通知，而不是提高轮询频率。

服务器后台任务的拥有关系是：返回句柄拥有 oneshot 发送端，spawn 任务拥有接收端、listener 和 service。`Stop` 取走发送端，`Drop` 再次调用时成为空操作；发送失败也被忽略，因为这表示接收端/服务任务已经结束。当前实现不保存 `JoinHandle`，所以不能等待资源完全释放或读取 serve 的最终结果。

## 与 Go 版本的对应关系

直接对照文件为 [server.go](server.go)。主要一致点如下：

- 都绑定 `127.0.0.1:0`，注册 `TopSQLAgent`，后台启动 gRPC 服务，并向测试暴露实际地址。
- 四个 RPC 都在每次接收前执行 Hang 检查；记录按完整流形成批次，SQL/Plan meta 按 digest 覆盖写入。
- `WaitCollectCnt*` 和 `Get*ByDigestBlocking` 都采用 1 ms 轮询与有界超时；`GetLatest*` 都清空全部已存批次但只返回最后一批。
- SQL meta 返回完整消息；Plan meta 只保存规范化文本；`GetTotalSQLMetas` 返回当前 map 内容的快照。

实现层差异如下：

- Go 将所有业务数据放在嵌入式单一 `sync.Mutex` 下，Hang 时间使用两个 `atomic.Pointer[time.Time]`；Rust 为 Hang 使用 `RwLock`，为四类数据使用独立 `Mutex`。
- Go `Stop` 调用 `grpc.Server.Stop()`，语义是立即停止；Rust 向 `serve_with_incoming_shutdown` 发送 oneshot，走 tonic 的 shutdown future，并额外由 `Drop` 自动触发。
- Go digest map 的键为由字节转成的 `string`；Rust 直接使用 `Vec<u8>`，避免文本编码假设，但保持按原始字节相等判定的效果。
- Go 的 RPC 接收错误同样立即返回；Rust TopSQL/TopRU 的局部批次提交策略与 Go 一致，元数据逐条落盘策略也一致。
- Rust `WaitCollectCnt` 使用 `saturating_sub`；Go 直接做 `len-old` 的有符号减法。正常调用（`old` 来自此前计数）语义一致，异常的过大 `old` 在 Rust 中更明确地保持未达标。

## 扩展指南

- 新增 RPC 时，应在生成的 `TopSqlAgent` trait 实现中增加对应方法，在 `State` 中选择独立锁保护的数据结构，并在 [migration_aster_unit_test.rs](migration_aster_unit_test.rs) 增加通过真实 tonic client 的测试。不要把 Rust 测试嵌回生产源文件。
- 修改批次语义时，应同时检查 `report_top_sql_records`/`report_top_ru_records` 与 `GetLatestRecords`/`GetLatestRURecords`，并同步 Go 对照 [server.go](server.go) 和上层 single-target/reporter 测试。尤其要明确：空流是否计为批次、流错误是否保留部分数据、一次读取是否丢弃旧批次。
- 修改 digest 存储时，必须保留任意字节 digest 的兼容性，并测试重复 digest 覆盖、空 digest、非 UTF-8 digest 与查询超时。SQL 与 Plan 当前返回类型不同，不能未经调用方迁移就统一。
- 修改 Hang 时，应维持“不跨 await 持同步锁”的不变量，并在多线程 Tokio 测试中验证窗口边界、并发流和停止期间行为。
- 若需要可靠确认停止完成，应在 `mockAgentServer` 中保存后台 `JoinHandle` 并设计异步 stop/join API；这会改变现有同步 `Stop` 接口和 Go 对齐关系，需要同步全部调用者。
- 性能风险主要来自每条 meta 都获取一次标准互斥锁、同步等待接口的 1 ms 轮询，以及无界记录/meta 容器。该服务定位为测试 mock；若扩大到压力测试，应先定义容量、背压和清理策略。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/topsql/reporter/mock` 确认 `server.rs`、Go 对照、crate 入口和独立测试均已索引。
- RustCodeGraph `node --file pkg/util/topsql/reporter/mock/server.rs --offset 1 --limit 500`：读取了目标文件全部 359 行及其 27 个符号。
- RustCodeGraph 精确 `query`：核对了 `StartMockAgentServer`、`GetLatestRecords`、`HangFromNow`、`report_top_sql_records` 的 Rust/Go 候选和签名；精确 `callers`/`callees` 未返回 Rust 图边，此限制已在“依赖与调用关系”中披露。
- 已读实现/配置：[Cargo.toml](Cargo.toml)、[lib.rs](lib.rs)、[server.go](server.go)。`Cargo.toml` 不在 RustCodeGraph 的 Rust/Go 索引范围内，按计划直接读取。
- 已读 Rust 测试：[migration_aster_unit_test.rs](migration_aster_unit_test.rs)、[single_target_3_aster_unit_test.rs](../single_target_3_aster_unit_test.rs)、[topsql_test.rs](../../topsql_test.rs)。它们分别证明四流收集与 Hang、单目标地址切换、完整 reporter 发送链。
- 已读 Go 测试：[single_target_test.go](../single_target_test.go)；并通过 `rg` 确认 [topsql_test.go](../../topsql_test.go) 对启动、地址、等待、批次和 digest 查询接口的使用。
- 本任务是纯文档分析，未运行 Cargo 或代码测试。最终结构以任务规定的 11 个固定二级标题命令验证。
