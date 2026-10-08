# `pkg/store/store.rs`

## 文件定位

`pkg/store/store.rs` 是 `astersql-store` crate 的存储注册表与打开入口。crate 根 `pkg/store/lib.rs` 通过 `mod store; pub use store::*;` 将本文件 API 提升到 `astersql_store::*`；`pkg/store/Cargo.toml` 则表明该 crate 默认采用 Classic 内核，`nextgen` feature 同时启用 `kerneltype/nextgen` 与 `keyspace-dependency/nextgen`。

在应用主链中，`cmd/tidb-server/main.rs::registerStoresWithTiKVDriver` 先把真实 `TiKVStoreDriver` 注册为 `tikv`，把 `LocalStoreDriver` 注册为 `mocktikv` 与 `unistore`；随后 `initRegisteredStorage` 通过 `BuildStoragePath` 和 `New` 打开目标 keyspace，并把返回的 `StorageRef` 交给 SQL 侧 `kv::Storage::from_registered`。因此本文件承担的是“按 URI scheme 选择驱动、建立存储实例、管理 SYSTEM keyspace 句柄”的边界，而不是事务读写、Region 路由或 SQL 执行本身。

## 核心职责

1. `Register`、`loadDriver` 和两个 `OnceLock<RwLock<...>>` 全局槽位维护进程级驱动表及 SYSTEM keyspace 存储。
2. `New`、`newStoreWithRetry`、`newStoreWithRetryAndInterval` 解析存储 URI 的 scheme，选择驱动，并对限定类别的临时错误执行线性退避重试。
3. `Storage` 与 `Driver` 为注册表规定最小对象安全接口；`StorageRef`/`DriverRef` 使用 `Arc<dyn ...>` 让启动、会话与关闭路径共享同一对象。
4. `TiKVStoreDriver` 把 `astersql_store_driver::TiKVDriver` 接入同步注册表；`RegisteredTiKVStorage` 暴露其 keyspace、codec、cluster ID、TSO、关闭与规范 `TikvStore` 克隆。
5. `LocalStoreDriver` 为 mock/unistore 启动路径提供轻量实现，只从 URI 读取 `keyspaceName` 并构造 `BasicCodec`，不冒充真实 TiKV 客户端。
6. `MustInitStorage`、`InitStorage`、`BuildStoragePath` 和 SYSTEM 存储访问器把全局配置、Classic/NextGen 模式与存储打开流程接合起来。

## 主要符号

- 常量 `DEFAULT_MAX_RETRIES = 30`、`RETRY_INTERVAL = 500ms`：生产 `New` 的尝试上限与线性退避基数。第 `attempt` 次可重试失败后休眠 `500ms * attempt`。
- `StoreErrorKind::{Other, TxnRetryable, PdClientGetTso, PdClientGetLeader}`：重试决策所需的结构化类别。`StoreError` 还保存消息和可选的嵌套 `source`；`wrap` 允许在不丢失内层 kind 的前提下增加上下文。
- `Storage: Send + Sync`：必需方法是 `GetKeyspace` 与 `GetCodec`；`AsEtcdBackend`、`Close`、`GetClusterID`、`CurrentVersion` 有保守默认值。`CanonicalTiKVStore` 默认返回明确错误，防止本地或测试存储被静默替换成第二个 TiKV 客户端。
- `Driver: Send + Sync`：核心方法 `Open(&self, path)`；`TypeName` 主要用于启动接线验证。
- `TiKVStoreDriver`：内部用 `Mutex<TiKVDriver>` 串行调用需要可变配置的真实驱动 `Open`。成功打开后先调用 `StartGCWorker`，再包装为 `RegisteredTiKVStorage`。
- `RegisteredTiKVStorage`：缓存真实 store、keyspace 字符串与 `BasicCodec`。空 keyspace 对应 API V1，命名 keyspace 对应 API V2；由于 client-rust 0.4 公共 API 不暴露数字 keyspace ID，此处固定 `keyspace_id = 0`。
- `LocalStoreDriver`/`LocalStorage`：使用 `url::Url` 解析查询参数；除 keyspace 与 codec 外沿用 `Storage` 默认行为。
- `Register`、`RegisteredStoreTypes`、`RegisteredDriverTypeName`：分别完成合法性/重复性受锁注册、排序后的已注册类型快照、具体驱动类型查询。
- `StorageIdentity`：把共享 trait object 的数据指针转换为稳定的进程内身份值，用于验证启动链传递的是同一 `Arc`。
- `isNewStoreRetryableError` 及三个公开分类器：识别事务可重试、`NOT_BOOTSTRAPPED`、`ENTRY_NOT_FOUND`，以及带 PD TSO/Leader kind 的 `not leader` 错误。
- `MustInitStorage`、`GetSystemStorage`、`SetSystemStorage`、`InitStorage`：初始化默认存储并在 NextGen 下维护 SYSTEM keyspace 全局引用。
- `ResetStoreStateForTest`：仅供测试清空进程全局驱动表和 SYSTEM 槽位，生产代码不调用。

## 执行流程

### 注册与打开

1. `cmd/tidb-server/main.rs::registerStoresWithTiKVDriver` 完成 TLS、PD timeout 和驱动全局配置后调用 `Register`。`Register` 先用 `StoreType::Valid` 拒绝非法类型，再在同一写锁临界区检查重复并插入，避免并发重复注册的检查/写入竞态。
2. `New(path)` 委托 `newStoreWithRetry(path, 30)`；后者再委托可注入间隔的 `newStoreWithRetryAndInterval`。
3. 打开函数以 `split_once("://")` 取得 scheme 和剩余路径，要求两者均非空，并只把 scheme 转小写后转换为 `StoreType`。这里刻意不使用通用 URL 解析整个 TiKV 地址，因为逗号分隔的多 PD authority 需要原样交给具体驱动。
4. `loadDriver` 在读锁下克隆 `Arc`。未注册 scheme 立即返回 `invalid uri format` 错误；`maxRetries == 0` 则不调用驱动并返回 `Ok(None)`，与 Go `RunWithRetry(0)` 的空成功语义一致。
5. 每次 `driver.Open(path)` 成功即返回 `Some(storage)`；失败时，非可重试错误立即原样返回，可重试错误被保存为 `last_error`，随后按 `retryInterval * attempt` 休眠。全部尝试用尽后返回最后一次错误。

### TiKV 与本地驱动

`TiKVStoreDriver::Open` 持有内部 mutex，调用真实 driver 的 `Open`，随后启动 GC worker。`RegisteredTiKVStorage::new` 从规范 `TikvStore` 缓存 keyspace 并选择 codec；其 `CanonicalTiKVStore` 返回底层 store 的共享克隆，`CurrentVersion` 把版本包装类型投影为 `u64`。相对地，`LocalStoreDriver::Open` 只解析 URI 与 `keyspaceName`，返回没有集群身份、TSO 或规范 TiKV 克隆能力的 `LocalStorage`。

### 配置与 SYSTEM keyspace

`InitStorage` 读取 `config::get_global_config()`，用 `BuildStoragePath` 生成 `store://path` 或 `store://path?keyspaceName=...`，再进入 `New`。`MustInitStorage` 将失败提升为 panic；Classic 模式只返回默认存储，NextGen 模式还保证 `system_store` 有值：默认存储不是 SYSTEM 时另开 SYSTEM 存储，否则复用同一 `Arc`。服务器入口也直接使用 `BuildStoragePath`/`New` 实现等价接线，并通过 `SetSystemStorage` 保存 SYSTEM 实例。

## 数据与状态

- `store_drivers()` 懒初始化 `HashMap<StoreType, DriverRef>`。键是配置层认可的存储类型，值为共享驱动；它是进程级状态，注册结果会持续到进程结束或测试调用 `ResetStoreStateForTest`。
- `system_store()` 懒初始化 `Option<StorageRef>`，只代表 NextGen 公共元数据所需的 SYSTEM keyspace，不代表当前用户 keyspace 的默认 store。
- `StoreError` 的 `kind` 不公开写入，只能经构造器设置；`error_chain` 从外到内遍历嵌套 source，保证上下文包装不破坏 PD 错误分类。
- `RegisteredTiKVStorage` 持有规范 `TikvStore` 及派生的 keyspace/codec。多次 `CanonicalTiKVStore` 得到共享底层状态的克隆，而不是重建客户端；`pkg/store/store_test.rs::registered_tikv_storage_exposes_the_same_canonical_store_clone` 用共享 option 与连续 TSO 证明这一点。
- `LocalStorage` 只保存 `String` keyspace 和 `BasicCodec`，没有后台任务或外部连接。

## 依赖与调用关系

上游直接证据：

- `pkg/store/lib.rs` 再导出本文件全部符号。
- `cmd/tidb-server/main.rs::registerStoresWithTiKVDriver` 调用 `Register`，分别注册真实 TiKV 与两个本地 scheme；`initRegisteredStorage` 调用 `BuildStoragePath`、`New`、`SetSystemStorage`；`closeDDLOwnerMgrDomainAndStorage` 调用 `GetSystemStorage` 并关闭额外 SYSTEM 实例。
- `pkg/session/runtime/session_factory.rs` 与 `pkg/session/runtime/session.rs` 读取 `GetSystemStorage`，把 SYSTEM 存储用于跨 keyspace 会话路径。
- `pkg/store/mockstore/teststore/store.rs` 实现 `Storage` 并在需要时设置 SYSTEM 槽位，证明 trait 是测试存储与注册表的共同边界。

下游依赖：

- `astersql-store-driver` 提供 `TiKVDriver`、`TikvStore` 及读统计相关再导出类型。
- `astersql-config` 提供 `StoreType` 与全局 store/path 配置；`astersql-config-kerneltype` 决定 Classic/NextGen 分支。
- `astersql-keyspace` 提供 `System` 名称、`Codec`、`BasicCodec` 与 API 版本。
- `url` 只用于 `LocalStoreDriver` 的完整 URI/query 解析；通用注册表路径为兼容多 PD 地址采用手工 scheme 分割。
- 标准库 `OnceLock`、`RwLock`、`Mutex`、`Arc` 分别承担懒初始化、读多写少全局状态、真实驱动串行可变访问和共享所有权。

RustCodeGraph 的文件节点报告 `pkg/store/store.rs` 被 91 个文件使用，并可靠给出本文件内部主边：`New -> newStoreWithRetry -> newStoreWithRetryAndInterval -> {loadDriver, Driver::Open, isNewStoreRetryableError}`，以及 `MustInitStorage -> mustInitStorage -> InitStorage -> New`。由于索引 CLI 对大量同名 `New`/`Register` 的独立 callers 查询未能唯一消歧，跨文件直接调用点以上述模块限定源码搜索为准。

## 错误处理与边界

- URI 缺少 `://`、scheme 为空、路径为空或 scheme 未注册都在调用驱动前失败。scheme 大小写不敏感，但 path 原样传递给驱动。
- `Register` 拒绝无效 `StoreType` 和重复键；其当前锁获取使用 `unwrap`，锁中毒会 panic，而不是转换为 `StoreError`。
- `TiKVStoreDriver::Open` 会把 mutex 中毒、真实驱动打开失败、GC worker 启动失败统一映射为 `StoreErrorKind::Other`；因此当前适配层不会保留 client-rust 的细粒度重试 kind，只有显式构造为四种 `StoreErrorKind` 的错误参与结构化分类。
- `NOT_BOOTSTRAPPED` 与 `ENTRY_NOT_FOUND` 是区分大小写的消息包含判断。`not leader` 还必须同时满足：完整错误文本包含小写标记，并且错误链某层 kind 是 `PdClientGetTso` 或 `PdClientGetLeader`；普通业务消息含同样字样不得重试。
- `maxRetries` 表示最多调用 `Open` 的次数，不是失败后的额外重试次数；正数路径不可能在没有记录错误时走到循环末尾。
- `MustInitStorage` 是启动期“必须成功”接口，错误会 panic；需要恢复错误的调用者应使用 `InitStorage` 或 `New`。
- `SetSystemStorage(Some(...))` 用 `assert_eq!` 强制 keyspace 为 `SYSTEM`，错误对象会导致 panic；`None` 用于清空槽位。
- `Storage` 的默认 `Close` 是空操作，默认 cluster ID/TSO 是 `None`。扩展实现若持有真实资源，不能依赖这些默认值掩盖生命周期需求。

## 并发与资源生命周期

`Storage`、`Driver` 均要求 `Send + Sync`，共享句柄采用 `Arc`。驱动表的读取只短暂持有 `RwLock`，返回前克隆 `Arc`，因此慢速 `Open` 不会占用注册表锁；注册时写锁覆盖“是否存在”与“插入”两个动作。SYSTEM 槽位也在锁内替换、锁外使用克隆。

`TiKVStoreDriver` 的 mutex 覆盖真实 driver 的整个 `Open` 调用，保证其可变 options 操作不会并发交错；打开成功后的 `StartGCWorker` 建立后台资源，`RegisteredTiKVStorage::Close` 将关闭委托给规范 store。服务器关闭顺序先停 Domain/owner/MPP 等上层组件，关闭默认 storage 后，如果它是用户 keyspace，再通过 `GetSystemStorage` 关闭额外 SYSTEM storage（`cmd/tidb-server/main.rs::closeDDLOwnerMgrDomainAndStorage`）。

重试用当前线程的阻塞 `thread::sleep`，没有异步任务或取消通道；等待时间为线性累计。调用者若在延迟敏感线程使用 `New`，必须考虑最多 30 次尝试的阻塞成本。`LocalStorage` 无外部资源，默认 `Close` 足够；`ResetStoreStateForTest` 只清除全局引用，不主动遍历并关闭其中对象，因此测试应先自行关闭有资源的 store。

## 与 Go 版本的对应关系

核心结构与 `pkg/store/store.go` 对齐：Go 的 `storeDrivers + sync.RWMutex` 对应 Rust 的 `OnceLock<RwLock<HashMap<...>>>`；`Register` 的合法性和重复检查、`New/newStoreWithRetry/loadDriver`、四类重试条件、`MustInitStorage` 与 SYSTEM keyspace 分支、`InitStorage` 路径拼装均保留。

需要注意的实现差异：

- Go 使用 `url.Parse` 取得 scheme，并用 `util.RunWithRetry`；Rust 为保留逗号分隔多 PD 地址，只拆分 `://`，并显式实现相同的尝试次数和线性退避。Rust 额外暴露 `newStoreWithRetryAndInterval`，使测试能以零间隔覆盖相同分支。
- Go 的 `newStoreWithRetry(..., 0)` 返回 nil storage 与 nil error；Rust 用 `Result<Option<StorageRef>, StoreError>` 精确表达该内部语义，而公开 `New` 永远要求得到实际 storage。
- Go 直接使用完整 `kv.Storage`/`kv.Driver` 接口；Rust 注册表定义更窄的对象安全 `Storage`/`Driver`，真实 TiKV 能力通过 `RegisteredTiKVStorage` 委托，完整事务 API 仍在具体类型中。
- Go 通过 PingCAP errors 的 RFC code 与错误链识别 TSO leader 错误；Rust 用 `StoreErrorKind` 和显式 source 链复现判定。两者都拒绝只含 `not leader` 文本的无关错误。
- Go 的 `systemStore` 是未加锁包级变量；Rust 用 `RwLock<Option<StorageRef>>` 支持并发安全访问，并在 `SetSystemStorage` 中无条件执行 SYSTEM 断言，而 Go 的 `intest.Assert` 语义偏测试期检查。
- Rust 增加了真实 TiKV driver adapter、规范 store 克隆、cluster ID/TSO/identity/type-name 等启动接线能力；这些是 Rust client 接入所需局部接线，不改变 Go 注册/重试的主语义。

`pkg/store/store_test.go::{TestRetryOpenStore, TestRegister, TestInitStorage}` 分别是重试分类、注册边界与 Classic/NextGen 初始化的直接 Go 对照；Rust 的同名/对应测试保留这些意图，并补充零重试及规范 TiKV 客户端共享断言。

## 扩展指南

- 新增 store scheme：先在 `astersql-config::StoreType` 中建立合法类型，再实现独立的 `Driver`/`Storage`，在进程入口调用 `Register`。不要把真实 TiKV 类型接到 `LocalStoreDriver`；同步扩展 `pkg/store/store_test.rs` 的非法/重复/打开测试和服务器注册接线测试。
- 增加可重试错误：优先新增明确的 `StoreErrorKind` 或可靠的结构化来源，并在 `isNewStoreRetryableError` 接入；不要仅凭宽泛字符串判定。同步覆盖嵌套错误链、大小写、无关同文消息、立即失败与尝试次数，并核对 `pkg/store/store.go` 是否已有对应语义。
- 修改退避：保持 `maxRetries` 的“总尝试次数”含义及零次语义；使用 `newStoreWithRetryAndInterval` 写快速独立测试，同时评估阻塞线程和最坏累计等待时间。
- 扩展 `Storage`：优先给不具备能力的实现提供明确失败，而不是伪造值；若方法涉及真实资源，更新 `RegisteredTiKVStorage`、`LocalStorage`、mock/teststore 的独立测试文件及服务器关闭顺序。
- 修改 keyspace/codec：必须同时检查 `RegisteredTiKVStorage::new`、`LocalStoreDriver::Open`、`BuildStoragePath`、`MustInitStorage` 和 SYSTEM 不变量。尤其不要把当前 `keyspace_id = 0` 解释为已解析的真实 ID；client-rust 公共 API 可提供数字 ID 后才应接线。
- 修改全局状态：保持锁临界区最小且不在持有 registry 锁时执行网络操作。新增生产资源槽位时需设计明确关闭路径；测试清理逻辑继续放在独立 `*_test.rs` 或测试辅助文件，不能嵌入生产源文件。

## 验证依据

- 目标源码：`pkg/store/store.rs`（RustCodeGraph 文件节点，542 行），逐项核对常量、trait、实现、全局状态、注册、重试、错误链与初始化函数。
- crate 边界：`pkg/store/lib.rs` 的 `pub use store::*`；`pkg/store/Cargo.toml` 的 `astersql-store-driver`、config/keyspace/kerneltype/url 依赖和 `nextgen` feature。
- 应用入口与生命周期：`cmd/tidb-server/main.rs::registerStoresWithTiKVDriver`、`initRegisteredStorage`、`closeDDLOwnerMgrDomainAndStorage`。
- Rust 直接测试：`pkg/store/store_test.rs::test_retry_open_store`、`test_zero_retry_count_matches_go_nil_success`、`test_register`、`test_init_storage`、`registered_tikv_storage_exposes_the_same_canonical_store_clone`、`local_storage_rejects_canonical_tikv_clone_requests`；补充迁移回归位于 `pkg/store/migration_aster_unit_test.rs`，覆盖快速重试、错误链、路径拼装与 SYSTEM 断言。
- Go 对照：`pkg/store/store.go`；直接测试 `pkg/store/store_test.go::TestRetryOpenStore`、`TestRegister`、`TestInitStorage`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/store` 确认 Rust/Go 同路径文件；`node --file pkg/store/store.rs` 获取完整源码及“被 91 个文件使用”的文件级关系；`query` 定位 `newStoreWithRetry`、`newStoreWithRetryAndInterval`、`GetSystemStorage`、`ResetStoreStateForTest` 等精确节点。内部调用边经图查询核对；同名跨文件 callers 的歧义通过 `rg` 的模块限定直接引用搜索补足。
- 本任务为纯文档分析，没有修改 Rust/Go/Cargo，也未运行 Cargo。文档结构以任务指定的 11 个固定二级标题命令校验；内容人工复核了定位、运行流程、安全扩展点和未被实现掩盖的边界。
