# `pkg/ddl/ingest/disk_root.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate；[`lib.rs`](lib.rs) 以 `pub mod disk_root` 暴露它，[`Cargo.toml`](Cargo.toml) 指定库入口并声明其直接依赖 `fs2`、`fail` 和 `astersql-util-dbterror`。它处于 DDL add-index/reorg 的本地 ingest 资源控制层：一部分 API 维护本地 ingest 后端的磁盘占用快照并决定是否应提前导入，另一部分 API 在分布式 read-index 执行器启动本地排序前检查文件系统余量。

它不是 DDL job、schema state 或 checkpoint 的持久化实现。当前可见的生产调用链有两条：[`BackendContext::check_flush`](backend.rs) 读取 `DiskRoot::should_import` 的缓存决策；[`ReadIndexStepExecutor::init`](../backfilling_read_index.rs) 在非云存储模式且已设置执行节点 ID 时调用 `check_local_sort_disk_space_at_path`，空间不足会变成 `ReadIndexError::LocalSortDisk` 并阻止执行器完成初始化。

按照 DDL 执行框架，它服务于需要 reorg/backfill 扫描与本地排序的 add-index 路径，而不直接推进 schema version、DDL job 状态或 rollback。磁盘检查是节点本地、启动前或运行期的保护措施，不是可恢复的 DDL checkpoint。

## 核心职责

- 用 `DiskRoot` 保存一个排序根目录、文件系统容量/已用量快照、DDL 配额及所有 ingest 资源跟踪器的聚合用量。
- 用 `ResourceTracker` 抽象单个后端当前占用的本地磁盘字节数，并通过 `add`/`remove` 管理跟踪器。
- 用 `update_usage` 刷新调用者传入的文件系统数据，并在同一次临界区内重新汇总跟踪器用量；`tracked_usage` 和 `should_import` 读取的是最近一次刷新后的快照。
- 当后端用量严格超过 DDL 配额，或文件系统使用率达到 90% 时，通过 `should_import` 请求上层提前导入以释放本地空间。
- 通过 `pre_check_usage` 和 `startup_check` 提供根目录级检查；前者真实创建/探测目录，后者只检查构造或最近刷新时缓存的容量数据。
- 通过 `check_local_sort_disk_space` 计算“文件系统至少保留 10% + 当前 DXF slot 预留量”的本地排序准入阈值，通过 `check_local_sort_disk_space_at_path` 完成真实路径探测。
- 保留 Go 浮点转整数顺序、严格不等号、wrapping 算术和 macOS 豁免等边界语义。

## 主要符号

- `CAPACITY_THRESHOLD: f64 = 0.9`：磁盘已用比例阈值。`should_import` 在 `used >= capacity * 0.9` 时触发；`min_free_disk_bytes` 将它换算成必须保留的最小空闲字节数。
- `LOCAL_SORT_HEADROOM_BYTES_PER_SLOT`：每个有效 DXF runtime slot 额外预留 2 GiB；总任务预留量不会超过 `ddl_disk_quota`。
- `DEFAULT_DDL_DISK_QUOTA`：`DiskRoot::new` 使用的固定 100 GiB 默认值。`new_with_quota` 可显式注入配额，便于调用方和测试保持确定性。
- `TRACKER_COUNT_FOR_TEST: AtomicI64`：模仿 Go `TrackerCountForTest` 的全局测试计数。每次 `add` 无条件加一、每次 `remove` 无条件减一；它不是 `BTreeMap` 实际元素数的可靠替代。
- `ResourceTracker: Send + Sync`：资源跟踪 trait，唯一方法 `disk_usage() -> u64`。实现可跨线程共享，但调用发生在 `DiskRoot` 状态锁内。
- `DiskRoot`：可克隆的磁盘根句柄。`path` 是受管目录；`state: Arc<Mutex<DiskState>>` 共享容量、占用与跟踪器；`quota` 按实例固定；`updating: Arc<AtomicBool>` 抑制重叠刷新。
- `DiskState`：内部状态，包含 `capacity`、`used`、最近聚合的 `backend_used` 和按 `i64` ID 排序的 tracker map。
- `DiskRoot::new` / `new_with_quota`：以 `capacity.wrapping_sub(available)` 初始化已用量。调用者若传入 `available > capacity`，不会报错，而会得到 wrapping 后的大数。
- `add` / `remove` / `count`：增删和统计 tracker。相同 ID 的 `add` 会替换旧值，但测试计数仍增加；不存在 ID 的 `remove` 仍减少测试计数。
- `update_usage`：用 `AtomicBool::compare_exchange` 获取单次更新资格；成功者持有状态 mutex、遍历 tracker 并以 wrapping addition 聚合，然后更新容量和已用量。
- `tracked_usage` / `usage_info`：分别读取缓存的后端用量和格式化后的三项快照。
- `should_import`：先检查 `backend_used > quota`，再检查非全零容量快照下的 90% 使用率。配额判断是严格大于；容量判断在恰好 90% 时已触发。
- `pre_check_usage`：创建目录，调用 `fs2::total_space`/`available_space`，并把失败或低空间包装为 `ErrIngestCheckEnvFailed` 文本；支持 Go 路径一致的 `mockIngestCheckEnvFailed` failpoint。
- `startup_check`：使用缓存数据计算 `capacity.wrapping_sub(used)`，要求其不小于配额；不访问文件系统，也不使用结构化 dbterror。
- `risk_of_disk_full` / `min_free_disk_bytes`：实现“available 小于最小空闲阈值即有风险”，其中最小值严格按 Go 的 `capacity - uint64(float64(capacity)*0.9)` 转换顺序计算。
- `LocalSortDiskSpaceCheck`：纯计算准入所需的输入快照，包括节点 ID、路径、空闲/总容量、runtime slots 和配额。
- `LocalSortDiskSpaceError`：确认磁盘空间不足后的结构化 Rust 错误包装；`is_ingest_check_env_failed` 固定返回 true，显示文本来自 `ErrIngestCheckEnvFailed`。
- `check_local_sort_disk_space`：纯阈值计算和错误构造，不做 I/O。
- `check_local_sort_disk_space_at_path`：创建目录、探测容量并调用纯函数；探测失败保留为普通字符串，确认低空间则转换成分类错误文本。macOS 上最终忽略纯函数结果。

## 执行流程

运行期配额决策流程：

1. 上层创建 `DiskRoot`，显式传入当前容量与空闲量；[`env.rs::initialized_disk_root`](env.rs) 当前以已初始化的 ingest 根目录和两个零值构造它。
2. 后端拥有者为任务调用 `add(id, tracker)`。tracker 进入共享 `BTreeMap`，全局测试计数加一。
3. 调用者周期性调用 `update_usage(capacity, available)`。若另一个 clone 正在更新，当前调用立即返回；否则遍历全部 tracker，缓存其 `disk_usage` 总和，再替换文件系统容量/已用量快照。
4. [`BackendContext::check_flush`](backend.rs) 调用 `should_import`。缓存后端用量严格超过 quota，或已用量达到容量 90% 时选择 `FlushDecision::Import`；否则继续周期 flush/none 分支。
5. tracker 生命周期结束时拥有者应调用 `remove(id)`；本文件不提供 RAII guard，也不会在 `DiskRoot` drop 时逐项修正测试计数。

本地排序准入流程：

1. [`ReadIndexStepExecutor::set_runtime_context`](../backfilling_read_index.rs) 提供执行节点 ID 与本任务 runtime slots；`init` 发现本地模式且 ID 非空时，从 [`env.rs`](env.rs) 取得 ingest 根目录。
2. `check_local_sort_disk_space_at_path` 确保目录存在，再用 `fs2` 获取总容量和可用空间。目录创建或探测失败直接返回普通错误字符串，使上层可将其视为可重试初始化失败。
3. `check_local_sort_disk_space` 计算 `task_headroom = min((runtime_slots as u64) * 2 GiB, ddl_disk_quota)`，所有整数运算保留 Rust/Go 对应的 wrapping 行为。
4. 计算 `free_threshold = min_free_disk_bytes(total_capacity) + task_headroom`。只有 `available_bytes > free_threshold` 才放行；相等也拒绝。
5. 拒绝时错误文本包含执行节点、排序路径、实际空闲量、阈值和操作建议，并使用 `ErrIngestCheckEnvFailed` 分类。macOS 路径包装器忽略该准入结果，但仍会执行目录创建和容量探测。

## 数据与状态

`DiskRoot::clone` 共享 `path` 的字符串副本、同一个 `DiskState` 和同一个 `updating` 原子门；`quota` 是按值复制且构造后不变。所有容量、用量及 tracker map 都在一个 `Mutex<DiskState>` 下，因此读到的是相互一致的一次快照。`BTreeMap` 让 tracker 遍历次序确定，但聚合只使用加法，顺序不改变正常范围内的结果。

`backend_used` 不是实时值。tracker 自己的原子或锁状态变化后，必须等下一次成功的 `update_usage` 才会反映到 `tracked_usage`、`usage_info` 和 `should_import`。相反，`pre_check_usage` 直接探测文件系统但不会把结果写回 `DiskState`；`check_local_sort_disk_space_at_path` 也不更新任何 `DiskRoot`。

代码有意使用 `wrapping_sub`/`wrapping_add`：容量小于 available、tracker 总和溢出、负 runtime slot 转为 `u64`、阈值相加溢出时都不会报错。正常生产输入必须满足容量和 slot 的物理约束；这些算术选择主要用于保持 Go 无符号转换/溢出形状，而不是输入校验。

两个判断表达不同策略：`should_import` 保护运行中的 backend 并可由配额或 90% 已用量触发；本地排序准入还为当前任务按 slot 增加 headroom，因而可能在磁盘尚未达到 90% 时拒绝启动。

## 依赖与调用关系

上游调用与装配：

- [`lib.rs`](lib.rs) 导出 `disk_root` 模块，并把独立测试文件 `disk_root_test.rs` 仅在 `cfg(test)` 下装配。
- [`env.rs::initialized_disk_root`](env.rs) 从全局 ingest 根目录构造零容量快照的 `DiskRoot`。该构造本身不会执行 `startup_check` 或文件系统探测。
- [`backend.rs::BackendContext::check_flush`](backend.rs) 调用 `DiskRoot::should_import`，把磁盘压力转换成 `FlushDecision::Import`。本文件不执行 flush/import。
- [`backfilling_read_index.rs::ReadIndexStepExecutor::init`](../backfilling_read_index.rs) 调用 `check_local_sort_disk_space_at_path`；失败映射为 `ReadIndexError::LocalSortDisk`，发生在 `backend_open` 和 `initialized` 置为 true 之前。
- 其他 Rust 测试和构造器把 `DiskRoot` 传入 `BackendContext`；代码搜索未发现除上述路径外的生产 `DiskRoot::new`、`update_usage`、`startup_check` 或 `pre_check_usage` 调用，因此这些 API 的完整运行期接线当前有限，不能从 Go 全局 `LitDiskRoot` 推断 Rust 已具有同等接线。

下游依赖：

- `fs2::{total_space, available_space}`：对真实路径读取文件系统容量。
- `std::fs::create_dir_all`：预检查与本地排序探测前创建目录；权限由平台默认/umask 决定。
- `astersql_util_dbterror::ErrIngestCheckEnvFailed`：把确定的环境不满足和 `pre_check_usage` 的探测失败转换为 DDL ingest 环境错误文本。
- `fail::fail_point!`：只覆盖 `pre_check_usage` 的 `mockIngestCheckEnvFailed`；Rust 路径未实现 Go 本地排序探测失败/空间不足两个 failpoint。
- 标准库 `Arc`、`Mutex`、`AtomicBool`、`AtomicI64`、`BTreeMap`：提供 clone 间共享、互斥、更新门和测试观测状态。

RustCodeGraph 将目标文件识别为 31 个符号，并报告它被 `pkg/ddl/backfilling_read_index.rs` 使用；对符号 ID 的 `callers/callees` 查询出现歧义性结果，故具体边由索引的目标源码、调用文件源码和仓库引用搜索交叉核验，不把错误的宽泛图结果作为事实。

## 错误处理与边界

- `Mutex::lock().unwrap()` 在锁被 poison 时 panic；`DiskRoot` 的公开方法没有把该状态转换为 `Result`。
- `update_usage` 的更新门在持锁期间调用外部 `ResourceTracker::disk_usage`。若 tracker panic，`updating` 可能保持 true 且 mutex 被 poison，后续刷新会被永久跳过或读操作 panic。实现者不得在 `disk_usage` 中阻塞、重入同一 `DiskRoot` 或 panic。
- `add`/`remove` 不检查 ID 是否已存在，`TRACKER_COUNT_FOR_TEST` 记录调用净次数而非 map 大小。测试清理应保证每个成功注册只配对一次删除。
- `should_import` 对 `(used, capacity) == (0, 0)` 明确返回 false，避免未初始化快照被 90% 公式误判；但后端用量若超过 quota，仍会先触发 import。
- `pre_check_usage` 把建目录、容量探测和磁盘不足都包装成 `ErrIngestCheckEnvFailed` 字符串；调用方无法仅凭 Rust error type 区分 I/O 与容量不足。macOS 只豁免容量不足，不豁免建目录或探测失败。
- `startup_check` 不执行 I/O；以零容量/零已用快照和默认 100 GiB 配额构造的实例会报告空间不足。它返回普通格式化字符串，与 `pre_check_usage` 的错误分类不同。
- 本地排序探测包装器特意让 I/O 失败保持普通字符串，而只让确认的低空间带 ingest 环境错误文本，以保留 DXF 重试语义。`LocalSortDiskSpaceError::is_ingest_check_env_failed` 是固定布尔方法，不是对 dbterror code 的动态解析。
- 准入条件是严格 `>`：可用空间恰好等于阈值时拒绝。`risk_of_disk_full` 则是严格 `<`：可用空间恰好等于 10% 阈值时不视为满盘风险。
- `current_task_runtime_slots` 为 `i32`，负数会先转换为巨大 `u64` 再乘法并被 quota 截断；调用方必须保证 slots 非负。

## 并发与资源生命周期

`DiskRoot` 设计为跨线程 clone 共享：`ResourceTracker` 要求 `Send + Sync`，状态由 mutex 保护，更新门用 `compare_exchange(false, true, AcqRel/Acquire)` 抑制重叠刷新。成功更新结束时以 `Release` 清除门。并发更新不是排队等待：竞争失败者直接返回，因此调用方不能假设每次提供的 capacity/available 都最终生效。

`update_usage` 在持有状态锁时依次调用所有 tracker。这保证 map 与聚合快照一致，但把 tracker 的执行时间放大为整个 `DiskRoot` 的读写阻塞时间；`add`、`remove`、`count`、`should_import` 和信息查询都会等待。安全扩展应先明确是否允许在锁外拍摄 `Arc` 列表；若改成锁外调用，必须处理 tracker 在快照期间被移除的语义。

典型生命周期是构造根 → 注册每个 backend tracker → 周期刷新与读取 import 决策 → backend 结束时移除 tracker。当前 API 没有自动注销 guard，clone drop 也不改变 tracker map。`TRACKER_COUNT_FOR_TEST` 使用 Relaxed 顺序，仅用于测试泄漏观测，不承担同步协议。

路径准入函数是无状态调用：每次独立建目录/探测/计算，不持有 `DiskRoot` 的 mutex。它可能与磁盘写入并发，因此结果只能代表探测瞬间；通过检查后仍可能被其他任务消耗空间。

## 与 Go 版本的对应关系

Rust `ResourceTracker`/`DiskRoot`/`DiskState` 分别对应 Go `ResourceTracker`、`DiskRoot` 接口及 `diskRootImpl` 的 map 和容量字段。`CAPACITY_THRESHOLD`、2 GiB/slot、100 GiB 默认配额、90% 边界、quota headroom 上限、严格 `available > threshold` 以及用户错误文本均按 [`disk_root.go`](disk_root.go) 保留。Rust 独立测试与 Go `disk_root_test.go` 覆盖相同的浮点舍入、等于阈值拒绝、slot 预留和 quota cap 场景。

重要差异和未接线部分：

- Go `NewDiskRootImpl(path)` 从零快照开始，`UpdateUsage()` 自行调用 `GetStorageSize` 并在探测失败时记日志；Rust 构造器和 `update_usage(capacity, available)` 由调用方注入文件系统数据，刷新本身不可能返回探测错误。
- Go 在清除 `updating` 后才获取 map 锁并汇总 tracker；Rust 从容量更新到 tracker 汇总都在同一状态锁内，并在解锁前清除原子门。两者对并发 add/remove 与下一次 update 的时序不完全相同。
- Go `ShouldImport` 从全局动态 `vardef.DDLDiskQuota` 读取配额并记录 info/warn；Rust quota 构造时固定，决策没有日志。
- Go `StartupCheck` 每次真实探测路径；Rust 只检查缓存快照。因此 Rust 调用方若需要真实启动检查，必须先提供可靠容量数据或调整 API，不能把当前方法描述成 Go 的完整等价实现。
- Go `PreCheckUsage` 用 `os.MkdirAll(path, 0700)`；Rust `create_dir_all` 不显式设置 0700。两者都在 macOS 忽略确认的低空间错误。
- Go `CheckLocalSortDiskSpace` 自行取得全局 ingest 目录、动态配额并提供两个测试 failpoint；Rust 把路径和 quota 参数显式传入，当前生产调用处固定传入 100 GiB，且没有这两个 failpoint。
- Go 的准入成功路径记录详细日志；Rust 纯函数成功时静默。
- Go `TrackerCountForTest` 同样按 Add/Remove 调用计数，因此替换/删除不存在 ID 的偏差不是 Rust 独有，但调用者仍需正确配对。

因此当前移植状态是“核心数据模型、阈值计算和错误文案已对齐，运行期探测、动态配置、日志/failpoint 及部分全局接线仍有差异”，不能标记为完整替代 Go 实现。

## 扩展指南

- 若补齐 Rust 的真实周期刷新，应在调用层或 `update_usage` 新 API 中明确 `fs2` 探测失败策略，并同步 [`backend.rs`](backend.rs) 的调用周期；不要让探测失败把旧快照悄然改成零值。新增测试应放在独立 [`disk_root_test.rs`](disk_root_test.rs)，不得嵌入生产文件。
- 若将 quota 接入动态会话/全局配置，应决定现有 `DiskRoot` 实例是否实时观察变更，并同步 `should_import`、`startup_check` 与本地排序准入三个入口，避免它们使用不同配额。兼容性风险在于变更后现有任务何时生效。
- 若修改阈值公式，必须保持 `min_free_disk_bytes` 的 Go 浮点转换次序和两个严格边界：满盘风险用 `<`，本地排序放行用 `>`。同步 Rust/Go 测试中的容量 100/101 和“恰好等于阈值”用例。
- 若增加 tracker 自动注销，优先引入拥有注册 ID 的 RAII guard，并保证 clone/drop 不会重复减少全局计数；同时覆盖替换相同 ID、删除不存在 ID、panic/early-return 清理。
- 若缩短 mutex 临界区，必须验证并发 add/remove 与刷新快照的线性化点，并避免 tracker 回调重入。性能风险主要是当前锁内逐 tracker 调用；正确性风险是锁外采样导致遗漏或重复计入。
- 若补 Go 的本地排序 failpoint 和日志，应保持错误分类：探测失败可重试且不是 `ErrIngestCheckEnvFailed`，确认低空间是致命 ingest 环境错误，macOS 仅忽略后者。
- 若把 `current_task_runtime_slots` 改为无符号或增加校验，要同步 [`ReadIndexStepExecutor::set_runtime_context`](../backfilling_read_index.rs) 的接口与测试；负值当前被 quota cap 掩盖，不能无意改变已有转换语义。
- 若接入更多 DDL 路径，先区分是根目录健康检查、运行期 import 决策还是单任务本地排序准入；三者的输入新鲜度、错误分类和生命周期不同，不应复用成一个含糊入口。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标 [`disk_root.rs`](disk_root.rs) 已索引并识别 31 个符号；`node --file` 完整读取 1–271 行，文件级关系报告直接使用者为 [`backfilling_read_index.rs`](../backfilling_read_index.rs)。另用索引读取该文件的 `ReadIndexStepExecutor::init`（255–261 行）、[`backend.rs`](backend.rs) 的 `check_flush`（175–182 行）、[`env.rs`](env.rs) 的 `initialized_disk_root`（102–105 行）和 crate [`lib.rs`](lib.rs)。精确 `callers/callees` 因 CLI 把符号 ID 当名称产生宽泛歧义结果，未将其误用为调用证据。
- Rust 直接证据：[`disk_root.rs`](disk_root.rs)、[`disk_root_test.rs`](disk_root_test.rs)、[`backend.rs`](backend.rs)、[`env.rs`](env.rs)、[`backfilling_read_index.rs`](../backfilling_read_index.rs) 与 [`lib.rs`](lib.rs)。仓库引用搜索还确认 `risk_of_disk_full` 被 [`mem_root_test.rs`](mem_root_test.rs) 复用，而其他 `DiskRoot::new` 引用位于独立测试构造中。
- crate 证据：[`Cargo.toml`](Cargo.toml) 声明包名 `astersql-ddl-ingest`、库入口 `lib.rs`、Go 包映射 `pkg/ddl/ingest`，以及目标直接使用的 `fs2`、`fail`、`astersql-util-dbterror`。大量额外依赖只在 Windows 条件表中声明，目标文件本身未引用它们。
- Go 对照证据：[`disk_root.go`](disk_root.go) 的接口、构造、刷新、阈值、预检查、启动检查与本地排序准入；[`disk_root_test.go`](disk_root_test.go) 的浮点边界、quota cap、错误文本和错误分类；[`backfilling_read_index.go`](../backfilling_read_index.go) 的 `CheckLocalSortDiskSpace` 生产调用；[`backend.go`](backend.go) 和 [`env.go`](env.go) 的全局 disk root 使用关系。
- 测试事实：Rust [`disk_root_test.rs`](disk_root_test.rs) 验证后端用量只在刷新后更新、quota/90% import 决策、Go 浮点边界、slot headroom 与错误文本，以及真实目录创建失败的 dbterror 文本；Go 测试额外验证本地排序探测失败与确认低空间的分类。测试证据说明预期，不代表未接线的 Rust 全局路径已运行。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验证使用任务指定命令检查文件存在且恰有 11 个固定二级标题；人工复核聚焦“为何存在、如何运行、状态与并发边界、Go 差异和安全扩展点”。
