# `pkg/store/mockstore/unistore/server/server.rs`

## 文件定位

本文件是嵌入式 unistore 的建服与装配层，属于 Cargo crate `astersql-store-mockstore-unistore-server`。crate 入口 `pkg/store/mockstore/unistore/server/lib.rs` 公开 `server` 模块并再导出本文件的公开项；上层 `pkg/store/mockstore/unistore/mock.rs::New` 调用 `server::new_mock`，将这里返回的 `tikv::Server`、`MockRegionManager` 和 `MockPd` 继续包装成进程内 RPC 客户端、PD 门面与 Cluster。

它位于配置、锁存储和 tikv 行为实现之间：从 `Config`/`Engine` 生成数据库选项，创建初始 Store/Region 元数据，再把 Region 管理、MVCC 存储和内部服务组合为可供 mockstore 使用的 `Server`。它不实现 SQL、RPC 请求处理或 MVCC 算法；这些行为分别留在上层 mockstore 和 `pkg/store/mockstore/unistore/tikv/`。

## 核心职责

1. `create_db` 将 Go/Badger 风格的 `Engine` 配置映射为 Rust `DatabaseOptions`，处理目录创建、压缩级别、缓存、刷盘和 compaction-filter 开关。
2. `new_mock` 创建内置 `MockRegionManager`/`MockPd`，分配 Store、Region、Peer 三个 ID，并引导一个覆盖全键空间的 Region；这是当前仓库上层嵌入式 unistore 的实际入口。
3. `new` 通过抽象 `PdClient` 获取 TSO 与三个 ID，创建 `StandAloneRegionManager`；该路径允许接入外部 PD 能力，但明确拒绝 RaftStore。
4. `setup_stand_alone_inner_server` 统一组装 `EngineBundle`、`StandAloneInnerServer`、`MvccStore` 和最终 `Server`，并维护当前实现可观察的死锁检测启动标志。
5. `MockRegionAdapter` 将 `MockRegionManager` 适配为 tikv 层的 `RegionManager` trait，并缓存带共享 latch 的 `RegionContext`。

## 主要符号

- `SUB_PATH_RAFT` / `SUB_PATH_KV`：数据库子目录名。`create_db` 对 `raft` 直接返回不支持；实际建服均打开 `kv`。
- `LOCK_STORE_ARENA_SIZE`：传给 `MemStore::NewMemStore` 的 8 MiB arena 大小。`BADGER_LEVEL_COUNT` 固定为 7，用于对齐 Go 的 Badger 默认压缩层数。
- `ServerError`：装配阶段的统一错误，区分不支持能力、无效配置、IO、PD、Region 与内部服务启动错误；`From<RegionError>` 使 Region 引导错误可用 `?` 传播。
- `PdClient`：本文件所需的最小 PD 接口，仅含 `get_ts` 和 `alloc_id`；`MockPd` 的实现直接委托其固有方法。
- `ValueLogWriteOptions`、`TableBuilderOptions`、`DatabaseOptions`：`Engine` 配置转换后的只读选项快照，可由 `Database::options` 查看。
- `Database`：轻量数据库句柄，保存选项及 `closed`、`deadlock_leader`、`deadlock_detection_started` 三个原子标志。当前 Rust 文件不实现实际 Badger 数据读写。
- `EngineBundle`：聚合 `Arc<Database>`、`Arc<MemStore>` 和打包后的 `state_ts`；其 `DatabaseBundle::close` 负责标记数据库关闭。
- `create_db(sub_path, safe_point, config)`：校验并转换引擎配置，必要时创建数据库目录，返回共享 `Database`。
- `get_region_options(config)`：把 Server 地址、PD 地址和 Region 大小复制到 `RegionOptions`。
- `initial_metadata`：生成一个 Store 和一个空起止键、epoch 均为 1、含单 Peer 的根 Region。
- `MockRegionAdapter`：私有适配器；`context` 按 Region ID 懒创建并缓存 `RegionContext`，其 `RegionManager` 实现处理上下文校验、Store 查询、分裂和关闭。
- `setup_stand_alone_inner_server`：私有公共装配路径；先 setup、设置 leader 标志、start，再设置检测已启动标志，最后构造 `Server`。
- `new_mock(config, cluster_id)` 与 `new(config, pd_client)`：两个公开建服入口。前者返回 Server、mock Region 管理器与 MockPd；后者只返回 Server。

## 执行流程

`new_mock` 的主流程如下：

1. 调用 `mock_region::get_ts`，按 `(physical as u64) << 18` 加 `logical as u64` 生成 `state_ts`；使用 `wrapping_add` 明确保留 Go `uint64` 溢出语义。
2. 创建初值为 0 的 `SafePoint`，以 `SUB_PATH_KV` 调用 `create_db`。因为传入了 safe point，生成选项的 `compaction_filter_enabled` 为真。
3. 以数据库、8 MiB `MemStore` 和 `state_ts` 创建 `EngineBundle`。
4. 创建 `MockRegionManager`，一次分配三个连续 ID；`initial_metadata` 将它们依次用作 Store、Region 和 Peer ID，然后 `bootstrap` 注册单 Store/根 Region。
5. 从同一 manager 创建 `MockPd`，并用 `MockRegionAdapter` 把 manager 转为 `Arc<dyn RegionManager>`。
6. 进入 `setup_stand_alone_inner_server`，最后同时返回 Server、原始 manager 和 PD，供上层 RPC/Cluster/PD 包装共享。

`new` 前三步相同，但 TSO 和 ID 均来自传入的 `PdClient`。数据库 bundle 创建后若 `config.Server.Raft` 为真立即返回 `Unsupported("raftstore")`；否则依次调用三次 `alloc_id`，创建 `StandAloneRegionManager` 后进入统一装配路径。注意数据库在 Raft 检查前已经打开，因此该错误路径可能已经创建 `DBPath/kv` 目录。

`create_db` 先拒绝 raft 子路径，再要求 `Engine.Compression` 至少有七项。它保留调用者给出的压缩数组长度，只解析前七项，额外项维持 `CompressionType::None`；随后将 Engine 字段逐项写入 `DatabaseOptions`。非 volatile 模式会创建数据目录和值目录，当前两者均为 `DBPath/sub_path`。

## 数据与状态

- 数据库配置在 `DatabaseOptions` 中按值保存，外部只能通过 `Database::options` 借用查看；运行状态由三个 `AtomicBool` 保存。
- `EngineBundle.state_ts` 是从 PD/Mock 时间戳打包出的启动状态时间戳。它在本文件中只被保存，供 bundle 的下游实现使用；本文件没有推进它。
- 初始 Region 的 `start_key` 和 `end_key` 都为空，表示覆盖完整用户键空间；`conf_ver`、`version` 均为 1，且只有一个 Peer。
- `MockRegionAdapter.contexts` 是 `RwLock<HashMap<u64, Arc<RegionContext>>>`。同一 Region ID 命中缓存时复用上下文；所有上下文共享适配器的同一 `Arc<Latches>`。
- mock Region 分裂后，适配器删除原 Region ID 以及返回的变化 Region ID 对应缓存，确保后续请求基于最新元数据重建上下文。
- `SafePoint`、数据库、锁存储、Region manager 与 Server 均通过 `Arc` 共享；建服成功后的拥有关系不依赖局部变量生命周期。

## 依赖与调用关系

上游直接调用边是 `pkg/store/mockstore/unistore/mock.rs::New -> server::new_mock`。crate 入口 `server/lib.rs` 再导出本文件，因此也可由 facade 路径访问公开 API。独立测试直接调用 `create_db`、`new_mock` 和 `new`。

本文件的主要下游调用边包括：

- 配置：`Config`、`Engine`、`ParseCompression` 来自 `../config` crate。
- 锁存储：`MemStore::NewMemStore` 来自 `../lockstore` crate。
- 生命周期：`StandAloneInnerServer::new/setup/start` 与 `DatabaseBundle` 来自 `tikv/inner_server.rs`。
- Region：`MockRegionManager::{new,alloc_ids,bootstrap,validate_context,split_keys,close}`、`MockPd::new` 及 `StandAloneRegionManager::new`。
- 存储与服务：`SafePoint::new`、`MvccStore::new`、`Server::new`。最终 `tikv::Server::stop` 会依次关闭 MVCC store、Region manager 和 inner server，后者再调用 `EngineBundle::close`。

`Cargo.toml` 的启用依赖只有 config、lockstore 和 tikv 三个本地 crate；pd 与 tikv-mvcc 被放在 `target.'cfg(any())'.dependencies`，当前构建条件恒假，说明本文件当前不直接链接那两个历史拆分 crate。

## 错误处理与边界

- `create_db(SUB_PATH_RAFT, ...)` 和 `new` 的 RaftStore 配置均返回 `ServerError::Unsupported`；Raft 服务不是降级运行，而是明确拒绝。
- 压缩配置少于七层返回 `InvalidConfig`，避免像 Go 循环那样发生越界；多于七层时保留长度并让额外层为 `None`，由 `migration_aster_unit_test.rs` 固定这一迁移语义。
- 非 volatile 模式下目录创建错误转换为 `ServerError::Io`。这里只验证目录可创建，不打开真实 Badger 文件或验证容量、权限以外的引擎行为。
- PD 的 TSO 或任一次 ID 分配失败都转换为 `ServerError::Pd`；已经创建的目录/bundle 没有显式回滚，但未返回部分构造的 Server。
- Region 引导、上下文校验和分裂错误通过 `RegionError` 或 `ServerError::Region` 保留。`MockRegionAdapter::split_region` 会先检查目标 Region 存在。
- `RwLock::read/write().unwrap()` 会在锁中毒时 panic，而不是返回 `ServerError`；这是当前内部 mock 实现的边界。
- `setup_stand_alone_inner_server` 只有 `inner_server.start` 的字符串错误会变成 `ServerError::Start`；当前 `StandAloneInnerServer::start` 实际为空实现并返回成功。

## 并发与资源生命周期

共享状态通过 `Arc` 管理。数据库的关闭、leader 和检测启动状态使用 Acquire/Release 原子读写；`tikv::Server` 自身用原子停止标志保证 `stop` 幂等，测试验证连续停止两次成功。

`MockRegionAdapter` 用读写锁保护 RegionContext 缓存，采用“先读、未命中再写并通过 `entry` 插入”的双阶段方式，因此并发首次访问同一 ID 仍只留下一个共享上下文。所有缓存上下文共用 `Latches`；实际请求写互斥由 `tikv::Server::with_latches` 获取和释放这些 latch。

成功启动时的顺序是：创建 inner server 并 `setup`，创建 MVCC store，将数据库标记为 deadlock leader，启动 inner server，再标记 deadlock detection started，最后发布 `Server`。停止顺序在 `tikv/server.rs::Server::stop` 中是 MVCC、RegionManager、InnerServer；inner server 通过 `EngineBundle::close` 标记数据库关闭。当前两个 deadlock 标志是状态模拟，没有在本文件中启动后台线程；也没有 `Drop` 自动停止，调用方应显式调用 `Server::stop`。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/store/mockstore/unistore/server/server.go`：Rust 的 `new_mock`、`new`、`get_region_options`、`setup_stand_alone_inner_server` 和 `create_db` 分别对应 Go 的 `NewMock`、`New`、`getRegionOptions`、`setupStandAlongInnerServer` 和 `createDB`。

保持一致的关键点包括：TSO 的 physical 左移 18 位后与 logical 相加、8 MiB lock store、拒绝 raft 引擎/RaftStore、managed transaction、4 MiB value-log 写缓冲、三个 value-log 文件、七层 Badger 压缩配置、safe point 存在时启用 compaction filter，以及 standalone 启动前成为死锁检测 leader。

当前 Rust 的重要结构差异也必须视为现状而非完整 Go 等价：

- Rust `Database` 只是选项与状态句柄，不是 `badger.DB`；`MvccStore` 是内存实现。
- Go `NewMock` 把 bundle、PD client 和完整 config 传入 Region/MVCC 构造；Rust 将初始元数据直接引导到 `MockRegionManager`，最终 `Server` 构造参数更少。
- Go 的 deadlock detector 执行 `ChangeRole`/`StartDeadlockDetection`；Rust 这里只设置两个可观察原子标志。
- Go 的 `StandAlongInnerServer.Setup/Start` 接收 PD client；Rust trait 方法无参数且当前实现为空操作。
- Go `New` 的 standalone RegionManager 保留 PD client 与 bundle；Rust 当前只持有 Store/Region/`RegionOptions`，三个初始 ID由本文件提前分配。

## 扩展指南

- 新增或修改引擎选项时，应同时更新 `DatabaseOptions`、`create_db` 的字段映射，并在 `migration_aster_unit_test.rs` 增加精确断言；涉及 Go 迁移语义时先对照 `server.go::createDB`，不要只让 Rust 测试通过。
- 要支持 Raft，不能仅删除 `Unsupported` 分支；需要独立实现 raft database、Raft RegionManager/InnerServer 及其启动停止语义，并覆盖 `SUB_PATH_RAFT` 与 `config.Server.Raft` 两条路径。
- 修改初始拓扑或 ID 分配时，集中调整 `initial_metadata`、`new_mock`/`new`，同步验证 Store/Region/Peer 唯一性、epoch 与 PD 连续分配行为。
- 修改 mock Region 校验或分裂时，关注 `MockRegionAdapter::{context,split_region}` 的缓存失效和 latch 复用；测试应放在独立的 `server_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。
- 引入真实死锁检测或后台任务时，应明确线程/任务句柄、失败回滚、stop/join 顺序与重复停止行为，不能继续只依赖 `Database` 的布尔标志。
- 兼容性风险主要是 Go 行为差异（TSO 溢出、压缩数组形状、错误时机）；性能风险主要在全局共享 latch、`changed.contains` 的线性缓存失效和 RegionContext 缓存增长。改动后应优先补充针对这些边界的独立测试。

## 验证依据

- 目标源码：`pkg/store/mockstore/unistore/server/server.rs`，核对了全部常量、类型、trait、函数与 impl。
- crate 边界：`pkg/store/mockstore/unistore/server/Cargo.toml` 与 `pkg/store/mockstore/unistore/server/lib.rs`。
- 上游入口：`pkg/store/mockstore/unistore/mock.rs::New` 对 `server::new_mock` 的直接调用。
- 下游实现：`pkg/store/mockstore/unistore/tikv/{inner_server.rs,server.rs,region.rs,mock_region.rs,mvcc.rs}`，用于核对装配、停止顺序、Region 校验/分裂、latch 与 safe point 语义。
- Go 对照：`pkg/store/mockstore/unistore/server/server.go`。
- 独立测试：`server_test.rs::new_composes_timestamp_with_go_uint64_wrapping`，以及 `migration_aster_unit_test.rs` 中压缩选项形状、mock 引导/幂等停止、外部 PD ID 与生命周期测试。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件、目标文件包含 58 个符号；`files --filter pkg/store/mockstore/unistore/server` 确认相关 Rust/Go 文件；`node --file .../server.rs --offset 1 --limit 500` 读取完整 460 行；`query` 定位 `server.rs::create_db`、`server.rs::setup_stand_alone_inner_server` 和 `server.rs::MockRegionAdapter`。精确 `callers/callees` 未返回可用边，跨文件直接调用以源码检索和相邻实现核实。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用固定 11 章节的结构命令，并人工检查本文能回答文件存在原因、运行路径和安全扩展位置。
