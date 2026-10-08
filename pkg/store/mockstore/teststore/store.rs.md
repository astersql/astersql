# `pkg/store/mockstore/teststore/store.rs`

## 文件定位

本文件是 `astersql-store-mockstore-teststore` crate 的核心实现文件。crate 入口 `pkg/store/mockstore/teststore/lib.rs` 只声明 `mod store` 并公开再导出本文件的 API；`Cargo.toml` 通过 `package.metadata.porting.go-package` 明确把该 crate 对应到 Go 包 `pkg/store/mockstore/teststore`。

它位于测试基础设施与真实 Rust mockstore 之间：不自行实现 KV 后端，而是调用 `astersql_store_mockstore::NewMockStore` 创建 `MockStorage`，再用本文件的 `MockStore` 将它适配为 `astersql_store::Storage`。文件顶部注释和 Go 对照文件都说明，这一层独立于更高层 testkit，目的是让需要“未 bootstrap 存储”的代码复用 mockstore 工厂而不形成循环依赖。

Rust 侧已确认的直接使用面包括：

- `pkg/infoschema/internal/testkit.rs::TestStore::new` 调用本 crate 的 `NewMockStore`，把实际 `MockStorage` 放进 infoschema 测试存储。
- `pkg/store/mockstore/teststore/store_aster_unit_test.rs` 直接验证 `NewMockStoreWithoutBootstrap` 的经典模式、NextGen 模式、选项应用和错误传播。

## 核心职责

1. `MockStore` 保存真实 `MockStorage`，并补齐通用 `Storage` trait 所需的 keyspace 名称、codec 与关闭入口。
2. `NewMockStore` 在构造前把当前内核模式同步到 mockstore 的全局 NextGen 开关，再把底层构造结果包装为 `Arc<MockStore>`。
3. `NewMockStoreWithoutBootstrap` 在不执行数据库 bootstrap 的前提下创建存储；NextGen 模式下还把全局 keyspace 配置改为 `SYSTEM`、设置 NextGen 服务作用域，并把同一个 `Arc` 实例登记为系统存储。
4. 公开再导出底层 `StoreError`，使构造选项或真实后端的错误不必经过本层重新包装。

本文件不是内存 HashMap 替身，也不负责 schema/bootstrap、事务实现、PD/RPC 行为或 DDL checker 实现；这些行为分别位于更高层 testkit 或 `astersql-store-mockstore` 及其后端中。

## 主要符号

- `pub use astersql_store_mockstore::StoreError`：本 crate 的构造错误类型就是底层 mockstore 错误类型。
- `pub struct MockStore`：适配器类型。
  - `inner: MockStorage`：实际 mockstore 门面，持有 RPC/PD 客户端、集群、当前 keyspace、latch 配置等。
  - `keyspace: String`：构造时从 `inner.current_keyspace` 拷贝的名称；未绑定 keyspace 时为空串。
  - `codec: BasicCodec`：构造时冻结的 API 版本与 keyspace ID。
- `MockStore::new(inner: MockStorage) -> Self`：私有适配构造器。当前 keyspace 存在时拷贝其名称和 ID；否则使用空名称与 ID `0`。名称为空时选择 `ApiVersion::V1`，非空时选择 `ApiVersion::V2`。
- `MockStore::is_latch_enabled(&self) -> bool`：把 latch 状态查询委托给 `MockStorage::is_latch_enabled`，供构造选项回归测试观察。
- `impl Storage for MockStore`：
  - `GetKeyspace` 返回缓存的 keyspace 名称引用。
  - `GetCodec` 以 trait object 返回缓存的 `BasicCodec`。
  - `Close` 调用 `MockStorage::close`，并把底层错误文本转换为通用 `astersql_store::StoreError`。
  - 未覆写 `Storage` 的其他可选能力，因此沿用 trait 默认值，例如无 etcd 后端、无集群 ID/TSO，且不能提供 canonical TiKV store。
- `NewMockStore(opts) -> Result<Arc<MockStore>, StoreError>`：公开的基础工厂；同步内核模式、创建真实后端并包装共享所有权。
- `NewMockStoreWithoutBootstrap(opts) -> Result<Arc<MockStore>, StoreError>`：公开的无 bootstrap 工厂；复用 `NewMockStore`，并在 NextGen 下完成全局配置及系统存储接线。

文件没有条件编译项、模块级常量、自定义 trait 或后台任务。

## 执行流程

`NewMockStore` 的流程如下：

1. 读取 `astersql_config_kerneltype::IsNextGen()`。
2. 通过 `astersql_store_mockstore::set_next_gen` 写入 mockstore 的原子全局开关。
3. 调用底层 `astersql_store_mockstore::NewMockStore(opts)`。底层依次应用全部 `MockTiKVStoreOption`，在 NextGen 且调用者未显式指定 keyspace 时自动绑定 `SYSTEM`，再按 `StoreType` 创建 MockTiKV 或 EmbedUnistore；DDL checker 被请求但未安装 injector 时返回错误。
4. 成功后调用私有 `MockStore::new`：从 `MockStorage::current_keyspace` 派生名称、ID 和 `BasicCodec`，最后放进 `Arc` 返回。

`NewMockStoreWithoutBootstrap` 在上述流程之后继续：

1. 使用 `?` 立即传播构造错误，失败时不会执行后续全局配置或系统存储登记。
2. 经典模式直接返回创建好的 `Arc<MockStore>`。
3. NextGen 模式通过 `astersql_config::update_global` 把 `keyspace_name` 设为 `astersql_keyspace::System`，把 `instance.tidb_service_scope` 设为 `NEXT_GEN_TARGET_SCOPE`。
4. 将 `store.clone()` 擦除为 `StorageRef = Arc<dyn Storage>`，调用 `astersql_store::SetSystemStorage(Some(storage))`。原返回值和系统注册表因此共享同一个对象身份。
5. 返回原始 `Arc<MockStore>`；本函数不会 bootstrap schema 或 domain。

关闭时，`Storage::Close` 只下沉到 `MockStorage::close`，后者关闭 RPC 客户端并传播关闭错误；本适配层没有额外清理阶段。

## 数据与状态

`MockStore` 的三个字段在构造完成后不再由本文件修改。`keyspace` 和 `codec` 是从 `inner.current_keyspace` 生成的快照，而不是每次查询底层状态：

- 无当前 keyspace：`keyspace == ""`、`ApiVersion::V1`、`keyspace_id == 0`。
- 有非空名称的当前 keyspace：保存其名称和 ID，并使用 `ApiVersion::V2`。

这里实际以“名称是否为空”选择 API 版本，不是单独以 `Option` 或 ID 判断；扩展 keyspace 元数据时不能悄悄改变这一现有判定。

共享状态主要来自下游或进程全局：

- `MockStorage` 内部持有 `Arc<RPCClient>`、`Arc<PdClient>` 与 `Arc<Cluster>`，本层按值拥有这组共享句柄。
- 返回值为 `Arc<MockStore>`，既可保留具体类型能力（如 `is_latch_enabled`），也可克隆并转换成 `StorageRef`。
- mockstore 的 NextGen 标志是 `AtomicBool`；底层 `set_next_gen` 使用 Release 写，构造器使用 Acquire 读。
- `NewMockStoreWithoutBootstrap` 在 NextGen 下修改全局配置和系统存储注册表。这些副作用跨调用可见，不属于单个 `MockStore` 私有状态。

## 依赖与调用关系

上游调用关系：

- `pkg/infoschema/internal/testkit.rs::TestStore::new` → `teststore::NewMockStore` → `astersql_store_mockstore::NewMockStore`。该调用者再把底层存储与受 `Mutex` 保护的测试元数据组合起来。
- `pkg/store/mockstore/teststore/store_aster_unit_test.rs::new_mock_store_without_bootstrap_matches_go_wrapper` → `NewMockStoreWithoutBootstrap`。
- Go 侧 `pkg/testkit/mockstore.go`、`pkg/session/bootstrap_test.go`、`pkg/store/helper/helper_test.go` 以及若干 DDL/server 测试调用同路径 Go 包的 `NewMockStoreWithoutBootstrap`，证明该包装器是测试基础设施入口，而不是生产服务器的通用打开路径。

下游调用关系：

- `NewMockStore` → `astersql_config_kerneltype::IsNextGen`、`astersql_store_mockstore::set_next_gen`、`astersql_store_mockstore::NewMockStore`、`MockStore::new`。
- `MockStore::new` → `MockStorage::current_keyspace`、`BasicCodec`。
- `NewMockStoreWithoutBootstrap` → `NewMockStore`；NextGen 分支再调用 `astersql_config::update_global` 与 `astersql_store::SetSystemStorage`。
- `MockStore::Close` → `MockStorage::close` → 底层 RPC client 的 `close`。
- `MockStore::is_latch_enabled` → `MockStorage::is_latch_enabled`。

`Cargo.toml` 的直接依赖与这些边一致：config、kerneltype、DXF handle、keyspace、store 和 store-mockstore；没有额外 feature 或 dev-dependency。`lib.rs` 在 `cfg(test)` 下通过独立文件 `store_aster_unit_test.rs` 挂载单元测试，符合源文件与测试逻辑分离约束。

## 错误处理与边界

- `NewMockStore` 直接返回底层 `StoreError`。选项闭包本身不返回 `Result`，但后端创建和 DDL checker 注入可失败；例如请求 `WithDDLChecker()` 而未安装 injector 时，错误为 `DDL checker injector is not installed`。
- `NewMockStoreWithoutBootstrap` 用 `?` 原样传播上述构造错误。因此构造失败不会登记系统存储，也不会执行位于构造之后的 NextGen 全局配置更新。
- `Storage::Close` 的返回类型必须是通用 store 错误，因此这里把底层错误格式化为字符串后用 `astersql_store::StoreError::other` 包装；错误文字被保留，但底层 `StoreError` 的具体类型身份不会保留。
- 全局配置更新与系统存储登记没有本地回滚逻辑。若未来在两者之间加入可失败操作，需要明确处理部分写入；当前代码中这两个调用都没有可传播的失败返回。
- `NewMockStoreWithoutBootstrap` 名称中的 “WithoutBootstrap” 是明确边界：调用者若需要 bootstrap 后的数据库/domain，应走 testkit 的创建入口，而不是在本文件补隐式 bootstrap。
- 本适配器只实现 `Storage` 的最小覆盖面；依赖 canonical TiKV、etcd、真实 TSO 或集群 ID 的代码不能把这里的默认行为误认为真实集群能力。

## 并发与资源生命周期

`MockStore` 满足 `Storage: Send + Sync`，通过 `Arc` 在调用者和系统存储注册表之间共享。包装过程不创建线程、异步任务、通道、事务或本地锁；并发行为来自底层共享客户端和全局设施。

资源生命周期要点：

- `Arc<MockStore>` 的克隆共享同一个 `MockStorage`。NextGen 注册时转换出的 `StorageRef` 与返回值对象身份相同，独立测试用 `StorageIdentity` 验证这一不变量。
- `Close` 是显式资源释放入口，会关闭底层 RPC 客户端；仅丢弃某个 `Arc` 克隆不等价于本文件显式调用 `Close`。
- mockstore 的 NextGen 模式由进程级原子变量控制；多个并发构造如果同时改变内核测试环境，会共享该模式状态。
- NextGen 全局配置和系统存储也属于进程级状态。现有测试先 `ResetStoreStateForTest`，结束时再次重置并调用 config restore closure，说明测试必须隔离并恢复这些副作用。
- `MockStore` 中的 keyspace/codec 是构造期快照；本文件没有在后续全局配置变化时刷新它们的同步机制。

## 与 Go 版本的对应关系

Go 文件 `pkg/store/mockstore/teststore/store.go` 只公开 `NewMockStoreWithoutBootstrap(opts ...MockTiKVStoreOption) (kv.Storage, error)`：调用 `mockstore.NewMockStore`，失败即返回；NextGen 下将全局 keyspace 设为 `SYSTEM`、服务作用域设为 `NextGenTargetScope`，并登记系统存储，最后返回同一个 store。

Rust `NewMockStoreWithoutBootstrap` 保留了这条控制流和副作用顺序。主要语言/架构差异是：

- Go 的底层对象已经实现完整 `kv.Storage`；Rust 需要本文件的 `MockStore` 把 `MockStorage` 适配成当前 `astersql_store::Storage` 的最小接口。
- Rust 将可变参数表达为 `Vec<MockTiKVStoreOption>`，将接口返回值具体化为 `Arc<MockStore>`；需要 trait object 时再克隆并转换为 `StorageRef`。
- Rust 额外公开 `NewMockStore`，供无需 NextGen 全局登记的 Rust 调用者（当前直接证据为 infoschema 内部 testkit）复用同一适配过程。
- Rust 在适配层缓存 keyspace 名称和 `BasicCodec`，并显式转接 `Close`；这些是 Rust `Storage` 契约所需的局部接线，不改变 Go 包装器“不 bootstrap”的意图。

独立测试 `store_aster_unit_test.rs` 覆盖了关键对齐点：latch 选项确实落到底层；NextGen 配置值正确；登记对象与返回对象身份相同；经典模式不改配置且不登记系统存储；底层 DDL checker 构造错误未被吞掉。Go 仓库没有同目录的专用 `store_test.go`，但多个 Go 测试包直接使用该构造器，其调用面提供了入口用途证据。

## 扩展指南

- 增加通用 `Storage` 能力时，优先修改 `impl Storage for MockStore`，把行为委托给真实 `MockStorage`；不要在 teststore 中再造一套 KV/PD/RPC 状态。同步检查 `pkg/store/store.rs` 的 trait 默认语义和底层是否真正支持该能力。
- 增加构造选项时，应让 `NewMockStore` 继续原样传给 `astersql_store_mockstore::NewMockStore`，并把选项实现放在 mockstore crate。若选项改变 keyspace，检查 `MockStore::new` 的名称、ID、API 版本快照是否仍匹配 Go 行为。
- 修改 NextGen 接线时，以 `NewMockStoreWithoutBootstrap` 为唯一入口同步配置和系统存储；保持“先成功构造、后产生全局副作用”和“登记同一个实例”两个不变量。
- 修改关闭语义时，同时检查 `MockStorage::close` 与 `Storage::Close` 的错误类型转换，避免静默吞掉 RPC 关闭失败。
- 回归测试应继续放在独立的 `pkg/store/mockstore/teststore/store_aster_unit_test.rs`，不要嵌入生产源文件。至少同步覆盖经典/NextGen 两种模式、成功与构造失败、对象身份、keyspace/codec，以及新增资源的关闭行为。
- 兼容风险集中在 Go/Rust 构造顺序、全局状态恢复和 trait 默认能力；性能风险较低，但不要在 `GetKeyspace`/`GetCodec` 等热读取接口中引入全局锁或重复构造。全局模式与注册表的并发修改则需要专门的串行化测试设计。

## 验证依据

- RustCodeGraph 状态：索引可用，包含 11,467 个文件；`files --filter pkg/store/mockstore/teststore` 显示 `lib.rs`、`store.rs`、`store.go`、`store_aster_unit_test.rs` 均已索引。
- RustCodeGraph 文件节点：
  - `pkg/store/mockstore/teststore/store.rs`：完整 87 行及其 14 个符号；图报告直接使用文件为 `pkg/infoschema/internal/testkit.rs` 和 `pkg/store/mockstore/teststore/store_aster_unit_test.rs`。
  - `pkg/store/mockstore/teststore/lib.rs`：模块声明、公开再导出与独立测试挂载。
  - `pkg/infoschema/internal/testkit.rs`：`TestStore::new` 到 `NewMockStore` 的调用边。
  - `pkg/store/mockstore/mockstore.rs`：`MockStorage`、`close`、`is_latch_enabled`、原子 NextGen 开关以及真实 `NewMockStore` 的选项/后端/错误路径。
  - `pkg/store/store.rs`：`Storage` trait 与 `StorageRef` 的接口、默认能力及线程安全边界。
  - `pkg/keyspace/keyspace.rs`：`ApiVersion`、`Codec`、`BasicCodec` 与 `SYSTEM` 常量的语义。
- Cargo/模块证据：`pkg/store/mockstore/teststore/Cargo.toml`、`pkg/store/mockstore/teststore/lib.rs`。
- Go 对照：`pkg/store/mockstore/teststore/store.go`；调用面通过 `rg` 核对到 `pkg/testkit/mockstore.go`、`pkg/session/bootstrap_test.go`、`pkg/store/helper/helper_test.go`、DDL/server 测试及 `br/pkg/mock/mock_cluster.go`。
- Rust 测试：`pkg/store/mockstore/teststore/store_aster_unit_test.rs`。未发现同目录 Go 专用测试文件；相关 Go 测试通过上述调用面间接使用包装器。
- 人工复核范围：确认本文件没有条件编译、后台任务或自建后端；文档区分了已实现的最小 `Storage` 能力与 trait 默认/未支持能力，没有把 bootstrap 或真实 TiKV 能力写成本文件职责。
- 本任务为纯文档分析，按计划未运行 Cargo 或代码测试；交付验证使用任务指定的 11 章节结构检查。
