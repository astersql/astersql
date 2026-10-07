# `pkg/ingestor/ingestctrl/engine_mgr.rs`

源码：[engine_mgr.rs](./engine_mgr.rs)

## 文件定位

本文件属于 Cargo crate `astersql-ingestor-ingestctrl`（`pkg/ingestor/ingestctrl/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `lib.rs` 以 `pub mod engine_mgr` 暴露。它位于 Rust 本地导入后端的控制面：上层 `local.rs::Backend` 持有 `Arc<EngineManager>`，将打开、关闭、重置、清理、Writer 创建、磁盘/内存统计以及外部 Engine 查询委托给这里；真正的有序 KV、导入状态和 Region 切分逻辑位于同 crate 的 `engine.rs::Engine`。

本文件不是完整存储引擎实现。它维护本地与外部 Engine 注册表、排序目录、锁状态入口和 TSO 分配，并把具体操作下沉给 `Engine`、`Writer`、`StoreHelper` 与 `ExternalEngine`。Cargo 清单表明当前无条件直接依赖包括 `astersql-ingestor-engineapi`；大量完整 TiDB/Lightning 依赖仅列在 `cfg(windows)` 目标段，阅读时不能据此推断本文件已拥有 Go 版 Pebble 后端的全部能力。

## 核心职责

1. `newEngineManager` 调用 `prepareSortDir` 建立本地排序根目录，按 `BackendConfig::duplicate_detection` 选择 `DupDetectKeyAdapter` 或 `NoopKeyAdapter`，并初始化两类 Engine 注册表和管理器级重复 KV 缓冲。
2. `openEngine`、`closeEngine`、`resetEngine`、`cleanupEngine` 和 `close` 组织本地 Engine 生命周期；`registerExternalEngine` 及外部查询方法管理实现 `ExternalEngine` trait 的对象。
3. `rLockEngine`、`lockEngine`、`tryRLockAllEngines`、`lockAllEnginesUnless` 把注册表查找与 `engine.rs` 中基于原子状态字的读锁/独占状态机组合起来。
4. `allocateTSIfNotExists` 通过 `StoreHelper::GetTS` 取得 PD 风格物理/逻辑时间，按 `(physical << 18) | logical` 合成时间戳写入 `EngineMeta::ts`。
5. `localWriter`、`engineFileSizes`、`totalMemoryConsume`、重复数据与 key adapter 访问器，为 `local.rs::Backend` 的写入、配额和重复检测控制面提供统一入口。

当前实现有明确边界：`flushEngine`/`flushAllEngines` 仅获取并释放读锁，不执行真实持久化；本地 `Engine` 的数据主体是 `engine.rs` 中的内存 `BTreeMap`，本文件也没有 Go 版的 Pebble DB、membuf pool、日志器或 checkpoint 状态。它们应视为迁移差距，而不是隐含支持。

## 主要符号

- `RUN_IN_TEST`、`IN_MEM_TEST`：全局原子测试开关。当前文件只读取 `IN_MEM_TEST`，使 `new/open/prepare` 路径跳过真实目录创建；`RUN_IN_TEST` 在本文件中未被读取。
- `StoreHelper: Send + Sync`：存储侧窄接口。`GetTS` 为 Engine 元数据分配 TSO，`GetTiKVCodec` 向上层透传 TiKV codec 名称。`engine_mgr_test.rs::TestStoreHelper` 是直接测试实现。
- `ExternalEngine: Send + Sync`：外部排序 Engine 契约。必须实现 ID、总量/已导入统计、冲突信息、键范围、Region 切分键和关闭；`LoadIngestData` 默认返回 `InvalidArgument`，`SetWorkerPool` 默认无操作，`GetTotalLoadedKVsCount` 默认取 `KVStatistics().1`，`ConflictFiles` 默认空列表。
- `EngineManager`：核心状态容器。公开字段仅有 `config`；其余字段分别保存 `store_helper`、本地 `engines`、`external_engines`、共享 `duplicate_data`、选定的 `key_adapter` 与幂等关闭标志 `closed`。
- `newEngineManager`、`prepareSortDir`：构造和目录前置检查。空路径直接返回 `InvalidArgument`；非内存测试模式用 `fs::create_dir_all`，因此已有目录是合法输入。
- 生命周期方法：`openEngine`、`registerExternalEngine`、`closeEngine`、`resetEngine`、`cleanupEngine`、`cleanupAllLocalEngines`、`close`。
- 锁与 flush 方法：`rLockEngine`、`lockEngine`、`tryRLockAllEngines`、`lockAllEnginesUnless`、`flushEngine`、`flushAllEngines`。
- 数据面辅助：`localWriter`、`engineFileSizes`、`getImportedKVCount`、三项外部 Engine 查询、`totalMemoryConsume`、`getDuplicateData`、`getKeyAdapter`、`GetTiKVCodec`。

## 执行流程

构造流程从 `local.rs::NewBackend` 开始：配置先经 `BackendConfig::adjust` 归一化，再调用 `newEngineManager`。构造器准备排序根目录，选择重复检测 key adapter，创建空注册表和共享缓冲，最后由 `Backend` 以 `Arc` 持有。

本地打开流程如下：`local.rs::Backend::OpenEngine` 从当前配置取 Region 大小/键数阈值并调用 `openEngine`；后者先检查取消令牌和管理器关闭状态，再持有本地注册表互斥锁。若 ID 已存在，直接克隆并返回同一个 `Arc<Engine>`，保持 Go `LoadOrStore` 的幂等语义；否则创建 `<local_store_dir>/<engine_id>`，构造 `Engine`，以 `IMPORT_MUTEX_STATE_OPEN` 加锁，分配 TSO，解锁后登记。`engine_mgr_test.rs::open_engine_is_idempotent_for_an_existing_id` 验证相同 ID 两次打开返回同一对象。

写入与导入时，`Backend::LocalWriter` 经 `localWriter` 获得绑定 Engine 的 `Writer`；`Backend::ImportEngine` 则先检查是否为外部 Engine，否则调用 `lockEngine(..., IMPORT_MUTEX_STATE_IMPORT)`，再在 `local.rs` 中完成 `finishWrite`、Region 分裂、导入与统计校验，最后无论闭包成功与否都显式 `unlock`。

关闭单个本地 Engine 时，`closeEngine` 先获取 CLOSE 独占状态，再依次 `finishWrite`、`Close`、`unlock`。`clean=true` 才执行目录清理并从注册表移除；`Backend::CloseEngine` 当前传入 `false`。重置流程获取 IMPORT 锁，未知 ID 按 Go 约定直接成功；已知 Engine 被关闭、解锁并移出注册表，然后用旧 Engine 计算出的切分键数量重新打开。`allocate_ts=false` 会把重新打开时刚分配的 TSO 清零。

清理流程与普通关闭不同：`cleanupEngine` 先从本地注册表移除，再关闭并删除其目录；同 ID 外部 Engine 也会从外部注册表移除并关闭。`cleanupAllLocalEngines` 先拍摄 ID 列表，再逐个清理且忽略错误。管理器级 `close` 用 `closed.swap` 保证幂等，清理全部本地 Engine、尽力关闭并清空全部外部 Engine，最后无条件尝试删除排序根目录。

## 数据与状态

- 两个注册表均为 `Mutex<HashMap<EngineId, ...>>`。本地值为 `Arc<Engine>`，外部值为 `Arc<dyn ExternalEngine>`；返回句柄前会克隆 `Arc`，因此注册表锁不跨越调用方的后续工作。
- `duplicate_data: Arc<Mutex<Vec<KvPair>>>` 是管理器级共享冲突缓冲，`getDuplicateData` 与 `getKeyAdapter` 被 `local.rs::Backend::GetDupeController` 使用。它不同于 Go 的持久化 `duplicateDB`。
- `closed: AtomicBool` 只阻止新的 `openEngine` 并使 `close` 幂等；其他查询/清理方法没有统一的 closed 前置检查。
- `Engine` 自身的 `state: AtomicU32` 表达读锁计数和 OPEN/CLOSE/IMPORT 独占状态。管理器方法负责找到对象并调用 `tryRLock` 或会等待的 `lockUnless`；具体状态常量和 CAS 循环定义在 `engine.rs`。
- TSO 在 `EngineMeta::ts` 中以 `AtomicU64` 保存。0 表示尚未分配；非零值不会被再次获取。物理或逻辑分量为负时，`allocateTSIfNotExists` 返回 `InvalidData`。
- `engineFileSizes` 和 `totalMemoryConsume` 都是遍历当前本地注册表所得的即时汇总；互斥锁毒化时分别降级为空列表和 0，而非返回错误。

## 依赖与调用关系

上游主调用者是 `pkg/ingestor/ingestctrl/local.rs::Backend`。RustCodeGraph 和源码核对得到的直接边包括：`NewBackend -> newEngineManager`，`Backend::OpenEngine -> openEngine`，`CloseEngine -> closeEngine`，`CleanupEngine -> cleanupEngine`，`LocalWriter -> localWriter`，`UnsafeImportAndReset`/`ResetEngineSkipAllocTS -> resetEngine`，以及 `Backend::Close`/`CloseEngineMgr -> EngineManager::close`。`Backend::ImportEngine` 还直接使用 `getExternalEngine` 或 `lockEngine` 进入外部/本地导入分支。

下游依赖集中在同 crate：

- `engine.rs::Engine` 提供锁状态、写入终结、关闭/目录清理、统计、切分键和内存计量；`Writer::new` 创建批量 Writer。
- `iterator.rs::{DupDetectKeyAdapter, NoopKeyAdapter, KeyAdapter}` 决定重复键编码策略。
- `local.rs::BackendConfig` 提供目录、重复检测和 Region 分裂阈值。
- `lib.rs` 提供 `CancellationToken`、`EngineId`、`Error`、`Result`、`ConflictInfo`、`EngineFileSize`、`KeyRange` 与 `KvPair`。
- `astersql-ingestor-engineapi` 仅出现在 `ExternalEngine::LoadIngestData` 的上下文、同步发送端、数据批和错误类型中；`import_pipeline.rs::ExternalEngineSource` 和 `local.rs::import_external_engine` 消费该 trait 的数据与统计接口。

RustCodeGraph 对同名符号会同时返回 Go 与 Rust 定义，因此调用边需要路径消歧。例如 Rust `resetEngine` 的下游是本文件的 `lockEngine`/`openEngine` 以及 trait `Close`/`GetRegionSplitKeys`，不能混入 Go `openEngineDB`、Pebble 或 globalsort `Reset` 边。

## 错误处理与边界

- 所有文件系统错误通过 `From<std::io::Error>` 转换成 crate 的 `Error::Io`；注册表锁毒化在会修改关键状态的方法中返回 `Error::Poisoned`。
- `openEngine` 在任何目录或状态修改前检查取消；但在等待注册表锁后不会再次检查。管理器已关闭时返回 `Error::Closed`，已有 ID 则幂等返回，不重新应用新的切分阈值。
- `closeEngine` 对未知 ID 返回 `NotFound`。若 `finishWrite` 或 `Close` 失败，当前 `?` 提前返回会跳过后续 `unlock`；这是当前源码事实，扩展错误路径时必须特别保护锁释放。
- `resetEngine` 对未知本地 ID返回成功；当前没有 Go 版“若为 globalsort 外部 Engine 则调用 Reset”的分支。它在 `old.Close()?` 失败时同样可能保留独占状态；成功移除旧对象后若重新打开失败，旧注册项不会恢复。
- `cleanupEngine` 先从注册表移除再调用 `Close`/`Cleanup`。关闭失败会留下已摘除但可能尚未清理的数据；外部关闭失败时对象也已经从注册表移除。
- `cleanupAllLocalEngines`、`close` 故意吞掉逐 Engine 错误，且本文件没有 logger，失败不可从返回值观察。`close` 最后删除根目录的错误也被忽略。
- `getExternalEngineKVStatistics` 当前返回 `ExternalEngine::KVStatistics`；Go 同名方法返回 `ImportedStatistics`。Rust 单测只覆盖未注册时的 `None`，没有锁定已注册时的统计口径，因此这是需要调用方确认的兼容差异。
- `ExternalEngine::LoadIngestData` 的默认实现是显式错误，不是空数据成功；未覆盖该方法的实现者不能参与需要流式加载的导入路径。

## 并发与资源生命周期

`EngineManager` 可在线程间共享：两个 trait 都要求 `Send + Sync`，Engine 以 `Arc` 保存，注册表和重复缓冲由 `Mutex` 保护，关闭标志及 Engine 元数据使用原子量。注册表锁只保护映射本身，Engine 操作通常在克隆句柄后进行；例外是 `openEngine`，它在创建目录、分配 TSO并登记的整个过程中持有本地注册表锁，以保证同 ID 打开的唯一性。

Engine 操作锁不是 Rust guard，而是 `engine.rs` 的原子状态协议，调用者必须成对执行 `rUnlock`/`unlock`。本文件正常路径中显式成对释放，但使用 `?` 的关闭/重置错误路径并非 RAII，存在状态未释放风险。`tryRLockAllEngines` 与 `lockAllEnginesUnless` 只返回成功锁定的子集，调用方负责逐项解锁；`flushAllEngines` 已做到这一点。

`close` 与其他操作没有一把覆盖全生命周期的全局互斥锁。它先原子标记 closed，随后分别清理注册表；已经取得 `Arc<Engine>` 的并发调用仍可能继续，因此安全性依赖 Engine 自身 closed/state 检查。外部 Engine 关闭发生在持有 `external_engines` 互斥锁期间，若实现的 `Close` 回调管理器会有重入死锁风险，新增实现时应避免这种反向调用。

目录生命周期为：构造时创建根目录，打开时创建 ID 子目录，`cleanupEngine` 删除 Engine 及其重复检测附属目录，管理器 `close` 删除整个根目录。`IN_MEM_TEST` 仅跳过创建，不改变清理调用；`remove_dir_all` 对不存在路径的错误被忽略。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/ingestctrl/engine_mgr.go`，测试对照为 `engine_mgr_test.go`。Rust 保留了 Go 的 `StoreHelper`、管理器双注册表、锁辅助、Engine 生命周期、TSO 分配、统计访问和排序目录准备等总体职责；Rust 测试 `TestEngineManager`、`TestGetExternalEngineKVStatistics`、`TestCleanupAllLocalEnginesLogsErrorOnly` 的命名也指向相应 Go 测试意图。

已对齐的关键语义包括：同 ID open 幂等、未知 reset 返回成功、未知 cleanup 不报错、TSO 由物理/逻辑部分合成、清理后注册表视图为空，以及 codec 由 StoreHelper 透传。Rust 还用独立测试 `open_engine_is_idempotent_for_an_existing_id` 和 `reset_missing_engine_is_a_noop` 显式固定前两项。

尚未完整对齐之处必须保留在迁移视野中：

- Go 用 `sync.Map`、Pebble DB、SST 目录和 Engine 元数据落盘；Rust 当前为 Mutex 注册表加内存 Engine，`flush*` 是占位逻辑。
- Go 重复检测使用 Pebble `duplicateDB`，并有 `membuf.Pool`、allocator 追踪、logger；Rust 使用共享 `Vec<KvPair>`，没有 buffer pool 或日志聚合。
- Go `resetEngine` 可重置 globalsort 外部 Engine并复用本地 Engine 对象/DB；Rust未知本地 ID直接 no-op，已知 ID移除后新建对象，而且用旧切分键数量近似重建参数。
- Go `getExternalEngineKVStatistics` 调用 `ImportedStatistics` 并对缺失返回 `(0, 0)`；Rust 调用 `KVStatistics` 并以 `Option` 表达缺失。
- Go close 按 checkpoint 与目录是否为空决定保留数据，并检查未收集重复项；Rust `BackendConfig` 无 checkpoint 字段，`close` 无条件尝试删除根目录。
- Go 的全量 flush 并发执行真实 flush 且汇总错误；Rust 仅锁/解锁并始终成功，除非单 Engine 不存在。

## 扩展指南

新增或修正本地生命周期逻辑时，优先在本文件的对应管理方法接线，并把具体 KV/锁状态行为留在 `engine.rs`。若引入新的注册表状态，必须明确它与 `closed`、Engine 原子 state 以及本地/外部同 ID 共存的关系。任何可能失败的加锁后流程应改用可自动释放的 guard，或保证每个提前返回分支先解锁；重点审查 `closeEngine` 和 `resetEngine`。

实现真实 flush 应替换 `flushEngine`/`flushAllEngines` 的占位体，保持“对象存在检查、关闭对象处理、全部 Engine 错误汇总和锁必释放”的 Go 契约，并在独立的 `engine_mgr_test.rs` 增加失败与并发覆盖，而不是把测试写进生产文件。

扩展外部 Engine 时，实现方至少应审视所有必需 trait 方法以及 `LoadIngestData`、`SetWorkerPool` 的默认行为；若业务需要加载数据，不能依赖默认错误实现。调整统计 API 前应先决定 `getExternalEngineKVStatistics` 的目标究竟是总量还是已导入量，并同步 `local.rs`、`import_pipeline.rs`、globalsort 实现和 Rust/Go 对照测试。

若补齐 Go 的持久化重复检测、buffer pool 或 checkpoint 保留策略，它们不应被伪装成当前 `Vec<KvPair>`/无条件删目录的小改动；需要同步扩展 `BackendConfig`、构造失败清理、close 顺序和资源泄漏测试。性能上尤其关注 `openEngine` 持有全局注册表锁期间执行 I/O/TSO 请求、遍历统计的 O(n) 成本，以及外部 `Close` 在表锁内执行的阻塞风险。

相关测试应继续放在独立文件 `pkg/ingestor/ingestctrl/engine_mgr_test.rs`。Go 对照行为则在 `engine_mgr_test.go`；涉及 Engine 状态机和 Writer 的边界还应同步检查同目录 `engine_test.rs`，但不要把测试内嵌到 `engine_mgr.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 68 个文件；`node --file pkg/ingestor/ingestctrl/engine_mgr.rs --offset 1 --limit 520` 返回目标文件完整 419 行；`query EngineManager --kind struct` 区分出 Rust `EngineManager`（第 74 行）与 Go `engineManager`（第 62 行）。
- RustCodeGraph 调用/被调用查询：`explore "engine_mgr.rs EngineManager"` 给出 `local.rs::NewBackend -> newEngineManager`、`local.rs` 各 Backend 门面对生命周期/统计方法的调用，以及 `resetEngine -> lockEngine/openEngine/GetRegionSplitKeys/Close`、`closeEngine -> lockEngine/Close` 等边；精确 `callees openEngine|closeEngine|resetEngine` 进一步按路径分离了 Go/Rust 同名定义。通用名 `close` 的图结果歧义过大，因此最终关系用 `local.rs` 直接调用点复核。
- 已读生产路径：`engine_mgr.rs`、`Cargo.toml`、`lib.rs`、`local.rs`、`engine.rs`；本包不存在 `doc.go`，所以以 `lib.rs` 的 crate/module 文档作为最近的模块契约。
- 已读对照与测试：`engine_mgr.go`、`engine_mgr_test.rs`、`engine_mgr_test.go`。Rust 测试覆盖 TSO/open/close/reset/cleanup、重复 open 幂等、未知 reset、未知外部统计和批量清理；Go 测试补充了原始日志/错误聚合意图。
- 本任务是纯文档分析；按总计划不运行 Cargo。结构验收以固定 11 个二级标题、文件存在和人工事实复核为准。
