# `br/pkg/task/operator/crr_checkpoint.rs`

## 文件定位

本文件位于 `astersql-br-pkg-task-operator` crate，模块由 `br/pkg/task/operator/lib.rs` 以 `pub mod crr_checkpoint` 挂载并通过 `pub use crr_checkpoint::*` 平铺导出。它是 Go 文件 `br/pkg/task/operator/crr_checkpoint.go` 的 Rust 对照，负责把 CRR（cross-region replication）checkpoint 所需的外部存储、连接管理器、keyspace-aware etcd 元数据连接、对象同步判定器和断点状态存储组装成一个服务，并返回一次性清理闭包。

当前迁移状态必须分层理解：`dialEtcdWithCfgAndFactory` 已调用 `astersql-metaservice` 的真实 `DialEtcdClient`；但 `CRRService`、`CRRDeps`、`NewCRRService`、`ExternalStorage`、`NewMgr` 等仍来自 `br/pkg/task/operator/stubs.rs`。特别是该桩 `NewCRRService` 只校验任务名并保存配置，没有接入 `br/pkg/stream/crr/service/service.rs` 中真实的 checkpoint 计算循环。RustCodeGraph 显示本文件目前只被 `br/pkg/task/operator/crr_checkpoint_test.rs` 直接引用；因此它是可导出的迁移接线层，不能据此宣称 Rust CLI 已运行完整 CRR 服务。

## 核心职责

- `NewCRRCheckpointService` 按固定顺序创建并校验上游、下游存储，创建 `ConnMgr` 和 etcd 客户端，选择对象同步检查策略，创建下游 resume-state 存储，最后构造 `CRRService`。
- `checkCRRExternalStorage` 通过 `LockFile` 判断一个外部存储是否为日志备份目录；上游校验失败是硬错误，下游校验失败在主入口中仅输出告警。
- `buildObjectSyncChecker` 在“检查下游同名对象是否存在”和“使用上游存储原生同步状态能力”之间选择，能力不足时显式失败。
- `storageResumeStateStore` 把 `PersistentState` 以 JSON 整体读写到下游存储的 `crr-checkpoint/resume-state.json`。
- `newEtcdClientConfig`、`dialEtcdWithCfgAndFactory` 将 BR 公共配置转换为 metaservice 拨号参数，并确保 keyspace 元数据组的命名空间语义。
- `cleanupFunc` 与 `closeEtcdClient` 约束资源退出路径：成功构造后由调用方执行一次清理；etcd 关闭失败只记录、不阻断后续清理。

## 主要符号

- `pub type cleanupFunc = Box<dyn FnOnce() + Send>`：一次性资源释放函数。`FnOnce` 反映其拥有存储、etcd 和 manager 的捕获值，不应重复调用。
- `const etcdGRPCBackOffMaxDelay`：3 秒的 etcd gRPC 最大退避，与 Go 常量一致。
- `NewCRRCheckpointService(&dyn Glue, CRRCheckpointConfig) -> Result<(CRRService, cleanupFunc)>`：本文件总入口。它公开返回桩 `CRRService` 与清理闭包，而不是 `br/pkg/stream/crr/service::Service`。
- `checkCRRExternalStorage(&dyn ExternalStorage, &str) -> Result<()>`：检查 `LockFile`，错误信息保留 `source` 标签和存储 URI。
- `buildObjectSyncChecker(Arc<dyn ExternalStorage>, Arc<dyn ExternalStorage>, bool)`：选择 `NewExistenceSyncChecker(downstream)` 或 `upstream.as_object_sync_checker()`。
- `storageResumeStateStore { storage, path }`：下游 JSON 状态存储；`path` 公开是为了与 Go 路径及测试断言对齐。
- `buildResumeStateStore`：使用 `GetStatusFileName()` 初始化状态键。
- `EtcdClient`：包装 `NamespacedEtcdClient`，并用共享 `AtomicBool` 暴露可测试的关闭状态。
- `EtcdBackoffConfig`、`EtcdClientConfig`：保存从 Go etcd 配置映射而来的退避、keepalive、TLS、endpoint 和超时字段；`DialOptionsLen` 是无 grpcio 对象环境中的契约计数。
- `etcdGRPCBackoffConfig`、`etcdKeepaliveParams`、`newEtcdClientConfig`：分别生成退避、keepalive 和完整拨号配置。
- `dialEtcdWithCfg`：生产便捷入口，使用默认 metaservice `Context`；`dialEtcdWithCfgAndFactory` 允许测试注入 `PdClientFactory`。

## 执行流程

`NewCRRCheckpointService` 的正常流程如下：

1. 用 `GetStorage(cfg.UpstreamStorage, &cfg.Config)` 打开上游；随后 `checkCRRExternalStorage(..., "upstream")` 要求存在 `LockFile`，否则关闭上游并返回错误。
2. 打开下游；若打开失败，仅关闭已经成功打开的上游。下游缺少 `LockFile` 时打印告警但继续，这与 Go 允许复制目标暂时不完整的分支一致。
3. 持有 `DIAL_HOOKS` 互斥锁，优先调用测试注入的 `new_mgr`，否则调用 `NewMgr`。失败时关闭上下游。随后调用 `GetKeepalive` 保留 Go 侧配置读取行为。
4. `dialEtcdWithCfg` 先生成 TLS/超时/keepalive 配置，再由 `astersql_metaservice::DialEtcdClient` 根据 `KeyspaceName`、PD 地址及安全配置解析真正的 namespaced metadata endpoint。
5. `buildObjectSyncChecker` 根据 `CheckSyncedFromDownstreamStorage` 选择同步判定方式。失败时关闭下游、上游、etcd、manager。
6. `buildResumeStateStore(downstreamStorage.clone())` 把续跑状态放在副本侧；随后把任务名、轮询间隔、元数据读取并发度和重试间隔传给 `NewCRRService`。
7. 服务构造成功后，返回捕获全部资源的 `cleanup`。其执行顺序固定为下游、上游、etcd、manager。

状态存储的读取流程是“存在性检查 → 整体读取 → JSON 反序列化”；对象不存在返回 `Ok(None)`。保存流程是“JSON 序列化 → 整体覆盖写”。在真实服务语义中，`br/pkg/stream/crr/service/service.rs` 会在首次循环加载状态，并仅在 checkpoint 前进时排队保存；当前 operator 桩服务尚未消费这里传入的 `State`。

## 数据与状态

- `CRRCheckpointConfig` 定义于 `br/pkg/task/operator/config.rs`，包含公共 `Config`、`CRRServiceConfig`、上下游 URI 和同步检查开关；其 `ParseFromFlags` 强制任务名、上游和下游均非空。
- `PersistentState` 当前定义于 `operator/stubs.rs`，JSON 字段为 `last_checkpoint`、`synced_ts` 和可省略的 `synced_by_store`。`storageResumeStateStore` 不做局部更新、版本转换或并发合并。
- 状态对象键由 `GetStatusFileName()` 提供，当前固定为 `crr-checkpoint/resume-state.json`。更名会破坏跨进程或跨版本续跑兼容性。
- `EtcdClient.closed` 是 `Arc<AtomicBool>`；克隆后的句柄观察同一关闭状态。`metadata` 持有真实 namespaced etcd 连接。
- `EtcdClientConfig` 的稳定值为：自动同步 30 秒、拨号超时 5 秒、最大退避 3 秒、`PermitWithoutStream = true`、等价拨号选项数 4；endpoints 来源于 `Config.PD`。
- 外部存储、同步检查器和状态存储都通过 `Arc` 共享。主入口克隆 storage 交给依赖对象，同时把原始 `Arc` 移入 cleanup，因而资源真正何时析构还取决于其他克隆的生命周期。

## 依赖与调用关系

上游模块关系为 `br/pkg/task/operator/lib.rs` 声明并再导出本模块；RustCodeGraph 对 `NewCRRCheckpointService` 的 Rust 调用者只找到 `test_new_crr_checkpoint_service_rejects_non_log_backup_upstream`，未发现生产 Rust 调用者。Go 图中同名入口也主要由同目录测试直接覆盖。

本文件的直接内部依赖集中在：

- `br/pkg/task/operator/config.rs`：`CRRCheckpointConfig` 及 CLI 参数校验。
- `br/pkg/task/operator/stubs.rs`：存储、Glue、manager、同步检查、resume-state trait，以及当前的桩 `CRRService`/`NewCRRService`。
- `pkg/metaservice` crate（Cargo 名 `astersql-metaservice`）：`Context`、`PdClientFactory`、`DialEtcdClient`、`NamespacedEtcdClient`。
- `serde_json`：resume-state 的整体 JSON 编解码。

`br/pkg/task/operator/Cargo.toml` 将本目录定义为独立 library crate，并直接依赖路径 crate `astersql-metaservice`、`astersql-objstore` 以及 `serde`/`serde_json` 等；没有为本文件声明条件 feature。语义上的下游真实实现位于 `br/pkg/stream/crr/service/service.rs`：那里 `ResumeStateStore::LoadState/SaveState` 被服务循环用于恢复和推进状态，但 operator 当前使用的是另一套本地桩 trait，二者尚未直接接线。

## 错误处理与边界

- 上游获取或 `LockFile` 校验失败立即返回；已经打开的上游在校验失败时关闭。下游获取失败也会关闭上游。
- 下游缺 `LockFile` 只是 `eprintln!` 告警；这不是“下游必然有效”的保证。真正读写 resume-state 时仍可能因存储错误失败。
- `FileExists` 本身失败时，`checkCRRExternalStorage` 用 `Error::Annotatef` 加入 lock 文件和上下游来源；文件不存在则使用 `berrors::ErrInvalidArgument` 并包含 URI。
- 未启用下游存在性模式且上游没有 `ObjectSyncChecker` 能力时必须失败，不能把未知状态当作已同步。
- `LoadState` 区分检查、读取、解码三类错误上下文；不存在是合法的无状态。损坏 JSON 不会静默回退为空状态。
- `SaveState` 区分编码与写入错误；没有临时文件、CAS 或原子 rename 保证，原子性取决于 `ExternalStorage::WriteFile` 实现。
- TLS 启用时，`newEtcdClientConfig` 先调用 `ToTLSConfig` 验证证书配置；随后 metaservice 的 `security.etcd_tls()` 或拨号失败均向上传播。
- `closeEtcdClient` 吞掉关闭错误并记录，确保 cleanup 继续关闭 manager；相对地，直接调用 `EtcdClient::Close` 会返回错误，并且 metadata 关闭失败时不会把 `closed` 标志置为真。
- `NewCRRService` 当前桩只拒绝空任务名。Go 版还向真实服务注入 PD、Watcher、Upstream、Sync、State；Rust 当前缺失这些真实运行边界，应作为迁移缺口而非正常简化。

## 并发与资源生命周期

入口本身同步执行。唯一显式全局锁是 `DIAL_HOOKS: Mutex<DialHooks>`；代码在持锁期间调用 hook 或 `NewMgr`，因此注入实现若重入同一 hooks 锁可能死锁，且慢拨号会串行化其他使用该全局 hook 的操作。

`EtcdClient` 的关闭标志使用 `SeqCst`，为测试和跨线程观察提供最强顺序；真实连接由 `NamespacedEtcdClient::close()` 管理。`cleanupFunc` 是 `Send + FnOnce`，可移交其他线程执行但只能消费一次。成功路径不会自动在 `CRRService` drop 时清理这些资源，调用方必须保存并执行 cleanup；当前返回类型也未用 RAII guard 强制这一点。

失败路径逐阶段释放已经获得的资源。大多数后段失败与成功 cleanup 使用“下游 → 上游 → etcd → manager”；较早的 manager 创建失败代码按“上游 → 下游”关闭，与成功路径次序不同，但两者都覆盖当时已创建的资源。存储的 `Arc` 克隆可能仍被 checker/state 对象持有，实际底层关闭语义由 `ExternalStorage::Close` 的实现决定。

真正的 CRR 服务在 `br/pkg/stream/crr/service/service.rs` 内用多个 `Mutex` 保护 calculator、初始化标志和待保存状态，并在取消/重试/关机时协调 flush；这些并发行为尚不属于本文件返回的桩 `CRRService`。

## 与 Go 版本的对应关系

保持一致的部分包括：上下游打开与 lock 校验策略、下游校验仅告警、同步检查器选择、resume-state 文件名和 JSON 整体读写、3 秒最大退避、30 秒自动同步、5 秒拨号超时、keepalive 参数及 `PermitWithoutStream`、四项拨号选项契约，以及清理闭包覆盖的四类资源。

关键差异如下：

- Go `NewCRRCheckpointService` 接收 `context.Context`，并创建真实 `streamhelper.CliEnv`、metadata watcher 和 `service.Service`；Rust 入口没有调用方 context，默认创建 metaservice `Context`，且最终调用 `operator/stubs.rs::NewCRRService`。
- Go `service.Deps` 包含 `PD` 与 `Watcher`；Rust 本地 `CRRDeps` 只有 `Upstream`、`Sync`、`State`，而桩构造函数甚至只消费配置中的任务名。
- Go 可在 `checkSyncedFromDownstreamStorage == false` 时传入 `nil` downstream，因为该参数不使用；Rust 类型要求 `Arc<dyn ExternalStorage>`，测试以占位 `MemStorage` 表达同一分支。
- Go etcd config 保存原始 context 和四个实际 gRPC dial options；Rust 用 `DialOptionsLen = 4` 记录契约，并将字段映射给 `astersql-metaservice::EtcdDialConfig`。
- Go 测试启动嵌入式 etcd 验证 keyspace metadata group；Rust 对等测试 `crr_dial_writes_to_keyspace_metadata_group` 标为 `#[ignore]`，需要外部 `ASTER_ETCD_TEST_ENDPOINT`。
- Go `storageResumeStateStore` 方法接收 context；Rust 本地桩 trait 不接收 context，因此存储调用无法响应请求级取消。

## 扩展指南

- 若完成生产接线，优先修改 `NewCRRCheckpointService` 的返回类型与依赖组装，使其适配 `br/pkg/stream/crr/service/service.rs::{Deps, Service, New}`；不要继续扩充本地桩来模拟计算循环。需要同时解决 `Box`/`Arc` trait 对象、context 传递、PD reader 与 watcher 适配。
- 新增同步判断策略应集中在 `buildObjectSyncChecker`，保持“能力未知即错误”的安全默认值，并在 `br/pkg/task/operator/crr_checkpoint_test.rs` 独立增加策略选择、错误传播和下游未就绪用例。
- 修改 resume-state schema 或路径时，应在 `storageResumeStateStore::{LoadState, SaveState}` 增加向后兼容读取/迁移，且同步真实服务的 `PersistentState` 定义；避免原地破坏旧 JSON。测试应覆盖不存在、损坏 JSON、旧版本状态和写失败。
- 调整拨号配置应修改 `newEtcdClientConfig`/`dialEtcdWithCfgAndFactory`，同步 Go 对照和 keyspace namespace 测试。TLS、超时、keepalive 与 metadata group 路由属于兼容性边界。
- 改动资源创建次序时，逐个审计 `NewCRRCheckpointService` 的所有提前返回，确保每个已创建资源只关闭一次且后续资源仍能清理。建议用独立测试注入各阶段失败并记录关闭次序。
- 不要把测试写回生产源文件；本仓库对应独立测试为 `br/pkg/task/operator/crr_checkpoint_test.rs`，Go 语义基准为 `br/pkg/task/operator/crr_checkpoint_test.go`。涉及真实服务交互时还应扩展 `br/pkg/stream/crr/service/*_test.rs`。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边，其中 Rust 文件 7,032 个；目标文件被完整索引为 382 行、25 个符号。
- RustCodeGraph 源码与调用图：`br/pkg/task/operator/crr_checkpoint.rs`；`NewCRRCheckpointService -> NewCRRService`；Rust 直接调用者为 `br/pkg/task/operator/crr_checkpoint_test.rs`；`lib.rs` 负责模块声明和再导出。
- crate 与模块证据：`br/pkg/task/operator/Cargo.toml`、`br/pkg/task/operator/lib.rs`、`br/pkg/task/operator/config.rs`、`br/pkg/task/operator/stubs.rs`。
- Go 对照：`br/pkg/task/operator/crr_checkpoint.go`、`br/pkg/task/operator/crr_checkpoint_test.go`。
- Rust 独立测试：`br/pkg/task/operator/crr_checkpoint_test.rs`，覆盖 etcd 配置常量、缺失必填 flag、非日志备份上游、上下游 lock 标签、两种同步 checker、无能力错误，以及需外部 etcd 的 keyspace metadata group 路由。
- 真实服务语义对照：`br/pkg/stream/crr/service/service.rs`，用于确认 resume-state 在完整计算循环中的加载、推进保存和关机 flush 角色；本文件当前未直接调用该实现。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查，并人工核对上述路径和符号。
