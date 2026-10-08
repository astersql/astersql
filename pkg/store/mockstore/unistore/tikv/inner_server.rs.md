# `pkg/store/mockstore/unistore/tikv/inner_server.rs`

## 文件定位

本文件位于内嵌 UniStore mock TiKV crate `astersql-store-mockstore-unistore-tikv` 中；crate 入口 `pkg/store/mockstore/unistore/tikv/lib.rs` 以 `pub mod inner_server` 暴露它。它不是网络服务实现，而是把“本地数据库资源如何被外层 mock TiKV 服务持有和关闭”抽象成生命周期边界，并保留与 Go `InnerServer` 相同的启动、Raft 和快照接口形状。真正的请求处理、MVCC、Region 与 RPC 聚合位于相邻的 `server.rs`、`mvcc.rs` 和 `region.rs`。

`pkg/store/mockstore/unistore/tikv/Cargo.toml` 将 `lib.rs` 声明为库入口，并用 `package.metadata.porting.go-package` 指向同路径 Go 包。目标文件本身只依赖标准库 `Arc`，没有条件编译项；Cargo 中大部分内部依赖位于 Windows 目标依赖块，不应据此推断本文件直接调用了这些 crate。

## 核心职责

- `DatabaseBundle` 隔离具体数据库资源包的关闭动作，使本模块不依赖外层 `EngineBundle` 或具体数据库类型。
- `InnerServer` 定义可放入 `Arc<dyn InnerServer>` 的线程安全生命周期及 Raft/快照入口。
- `StandAloneInnerServer<B>` 用共享所有权保存资源包，并实现 standalone 模式：`setup`、`start`、`raft`、`batch_raft`、`snapshot` 均为空操作，`stop` 是唯一产生资源状态变化的方法。
- `is_started` 明确表达当前移植语义：由于 Go standalone `Start` 不维护启动状态，查询始终返回 `false`，不能当作真实健康状态或一次性启动标记。

这里的空操作是对当前 Go standalone 实现的刻意对齐，不代表完整 TiKV 的 Raft 或快照能力已经实现。

## 主要符号

- `pub trait DatabaseBundle: Send + Sync`：资源关闭适配接口。唯一方法 `close(&self) -> Result<(), String>` 允许通过共享引用关闭底层资源并把失败向上传递。
- `pub trait InnerServer: Send + Sync`：供 `tikv::server::Server` 动态分派的服务接口。`setup` 无返回值；`start`、`stop`、`raft`、`batch_raft`、`snapshot` 都返回 `Result<(), String>`。
- `pub struct StandAloneInnerServer<B>`：泛型 standalone 实现，仅含私有字段 `bundle: Arc<B>`。字段私有保证外部只能通过生命周期接口操作资源包。
- `StandAloneInnerServer::new(bundle: Arc<B>) -> Self`：转移一个 `Arc` 句柄进入服务，不复制资源包。
- `StandAloneInnerServer::is_started(&self) -> bool`：兼容性查询，固定返回 `false`。
- `impl InnerServer for StandAloneInnerServer<B>`：要求 `B: DatabaseBundle`；除 `stop` 调用 `self.bundle.close()` 外，其余入口成功返回或不执行操作。

两个 trait 都要求 `Send + Sync`，因此具体 bundle 和 inner server 可以安全地被外层 `Arc` 跨线程共享；这只约束类型的线程安全能力，不自动提供启动/停止互斥或幂等性。

## 执行流程

1. 外层 `pkg/store/mockstore/unistore/server/server.rs::setup_stand_alone_inner_server` 创建 `Arc<EngineBundle>`，调用 `StandAloneInnerServer::new(Arc::clone(&bundle))`。
2. 外层依次调用 `setup()`，构造 `MvccStore`，将本地死锁检测角色标为 leader，再调用 `start()`。本实现的两个生命周期入口不改变状态，因而相关死锁状态由外层显式维护。
3. 构造完成的实例被擦除为 `Arc<dyn InnerServer>` 并交给 `pkg/store/mockstore/unistore/tikv/server.rs::Server::new` 保存。
4. Raft、批量 Raft或快照请求进入 `Server::raft`、`Server::batch_raft`、`Server::snapshot` 后直接转发到本 trait；standalone 实现当前直接返回 `Ok(())`。
5. `Server::stop` 首次执行时先关闭 MVCC store 和 Region manager，最后调用 `inner_server.stop()`；本实现再调用 bundle 的 `close()`，将其结果原样返回。外层 `Server` 的 `AtomicBool` 负责让整条停止流程只执行一次，本类型自身没有停止标记。

## 数据与状态

本文件唯一持久字段是 `StandAloneInnerServer::bundle: Arc<B>`。`Arc` 使外层组装代码、inner server 和可能的其他持有者共享同一个资源包实例；创建或克隆服务句柄不会复制数据库。实际实现 `pkg/store/mockstore/unistore/server/server.rs::EngineBundle` 包含 `Arc<Database>`、内存锁存储和状态时间戳，其 `DatabaseBundle::close` 通过原子标记关闭数据库。

`StandAloneInnerServer` 不保存 `started`、`stopped`、PD client、流对象或请求数据。`is_started == false` 是常量语义。重复直接调用 `StandAloneInnerServer::stop` 会重复调用 `DatabaseBundle::close`；生产组装路径的幂等性来自外层 `Server::stopped`，而不是本文件。

## 依赖与调用关系

上游关系：

- `pkg/store/mockstore/unistore/server/server.rs::setup_stand_alone_inner_server` 是实际组装入口，调用 `new`、`setup` 和 `start`。
- `pkg/store/mockstore/unistore/tikv/server.rs::Server` 持有 `Arc<dyn InnerServer>`；其 `stop`、`raft`、`batch_raft`、`snapshot` 分别调用同名 trait 方法。
- `pkg/store/mockstore/unistore/tikv/lib.rs` 公开模块，并以独立 `inner_server_test.rs` 注册单元测试。

下游关系：

- `StandAloneInnerServer::stop` 唯一下游调用是 `DatabaseBundle::close`。
- 外层 `EngineBundle` 的适配实现把该调用落实为设置底层 `Database.closed` 原子状态。
- 其余方法无下游调用、无网络 I/O，也不触碰 PD、Raft 日志或快照数据。

RustCodeGraph 确认目标文件包含 20 个索引符号，并从 `server.rs` 与外层 `server/server.rs` 的精确文件节点验证了上述组装和转发边；由于常见方法名的全局 callers/callees 查询存在同名噪声，调用关系又以限定路径搜索复核。

## 错误处理与边界

`new`、`setup` 和 `is_started` 不返回错误；空操作入口 `start`、`raft`、`batch_raft`、`snapshot` 固定返回 `Ok(())`。`stop` 不捕获、不改写 `DatabaseBundle::close` 的 `String` 错误，因此调用方能观察到底层关闭失败。

当前错误类型只是字符串，没有结构化类别或错误源链。trait 方法不接受 Go 版本中的 PD client 与 gRPC stream 参数，所以 Rust 接口不能检查流状态、处理消息或表达基于 PD 的启动失败。若直接绕过外层 `Server` 重复停止，是否安全完全取决于 bundle 的 `close` 实现；本文件没有防重入、超时、取消或 panic 恢复机制。

## 并发与资源生命周期

`DatabaseBundle` 与 `InnerServer` 的 `Send + Sync` 约束，加上 `Arc<B>`，允许同一实例被并发持有和调用。这里没有锁、任务、线程或通道；同步责任下放给具体 bundle 和外层 server。

典型生命周期是 `Arc<EngineBundle>` 创建 → inner server 克隆持有 → 外层 server 通过 trait object 持有 inner server → 首次 `Server::stop` 关闭 MVCC、Region manager 与 bundle。`Arc` 的引用计数归零只释放 Rust 对象，并不替代显式 `stop`；反过来，`stop` 调用 `close` 后其他 `Arc` 仍可能存活。`pkg/store/mockstore/unistore/tikv/main_test.rs::standalone_server_stop_is_idempotent_and_closes_resources` 证明外层停止两次只关闭 bundle 一次，并验证 Raft 类空操作在停止后仍返回成功。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/tikv/inner_server.go`：

- Rust `InnerServer` 对应 Go `InnerServer`；Rust `StandAloneInnerServer` 对应 Go 拼写为 `StandAlongInnerServer` 的结构体。
- 两侧构造器都只保存数据库 bundle；Raft、BatchRaft、Snapshot、Setup 和 Start 在 standalone 实现中均为空操作/成功返回。
- Go `Stop` 调用 `is.bundle.DB.Close()`；Rust 通过 `DatabaseBundle::close` 间接完成相同资源关闭，避免把本 crate 绑定到外层具体 DB 类型。
- Go trait 方法携带 `pd.Client` 或具体 gRPC stream，Rust 当前方法不带参数。这是接口表面上的迁移差异，也说明 Rust 的 Raft/快照入口目前只是占位。
- Go 没有 `is_started`；Rust 测试用固定 `false` 显式记录“Start 不维护状态”的兼容语义。

外层流程也相符：Go `pkg/store/mockstore/unistore/server/server.go::setupStandAlongInnerServer` 按 Setup → 建 store/设置 deadlock leader → Start → 启动死锁检测 → 构造 Server 的顺序组装；Rust 对应函数保持相同步骤意图。Go `tikv/server.go::Stop` 会等待引用计数清零并记录各关闭错误，Rust 外层使用原子幂等门并返回 inner-server 关闭错误，错误处理细节并非完全等价。

## 扩展指南

- 若要实现真实启动状态，应优先修改 `StandAloneInnerServer` 状态模型、`start`/`stop` 与 `is_started`，并明确并发调用、重复启动和关闭失败后的不变量；不能只把 `is_started` 改为常量 `true`。
- 若要接入真实 Raft 或快照处理，需要同时扩充 `InnerServer` 的请求/流参数以及 `tikv/server.rs` 的转发层，并评估与 Go gRPC stream 签名的兼容关系。目前的无参数方法没有承载数据的能力。
- 若要替换字符串错误，应同步 trait、外层 `Server` 和组装函数的错误映射，避免丢失底层关闭原因。
- 新增或改变本文件行为时，应更新独立文件 `pkg/store/mockstore/unistore/tikv/inner_server_test.rs`；涉及外层停止幂等与转发时，还应同步 `main_test.rs`/`server_test.rs`，不要把测试嵌入生产源文件。
- 保持 `DatabaseBundle` 作为抽象边界可减少 crate 耦合；引入具体 `EngineBundle` 依赖前应先验证 Cargo 目标依赖和跨平台构建影响。

## 验证依据

- 源与模块边界：`pkg/store/mockstore/unistore/tikv/inner_server.rs`、`lib.rs`、`Cargo.toml`。
- Rust 上游与下游：`pkg/store/mockstore/unistore/server/server.rs::EngineBundle::close`、`setup_stand_alone_inner_server`；`pkg/store/mockstore/unistore/tikv/server.rs::Server::{new,stop,raft,batch_raft,snapshot}`。
- Rust 独立测试：`inner_server_test.rs::start_matches_go_noop_and_stop_closes_bundle` 验证启动不改变状态、停止恰调用一次 close，以及三个占位入口成功；`main_test.rs::standalone_server_stop_is_idempotent_and_closes_resources` 验证外层幂等和资源关闭链；`server_test.rs::test_server` 验证 trait object 的实际装配方式。
- Go 对照：`pkg/store/mockstore/unistore/tikv/inner_server.go`、`tikv/server.go::{NewServer,Stop,Raft,BatchRaft,Snapshot}`、`pkg/store/mockstore/unistore/server/server.go::setupStandAlongInnerServer`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/store/mockstore/unistore/tikv/inner_server.rs` 定位目标；`node --file ...inner_server.rs` 读取全部 77 行和 20 个符号；`query InnerServer`/`query DatabaseBundle` 定位 Rust trait、方法及 Go 对照；对 `server.rs`、外层 `server/server.rs` 和测试文件使用精确文件节点验证调用与生命周期。
- 未运行 Cargo：任务是纯文档分析，计划明确排除 Cargo。结构检查用于确认目标文件存在且恰有规定的 11 个二级章节；人工复核确认没有把空操作描述为已实现的 Raft/快照功能。
