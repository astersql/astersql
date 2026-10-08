# `pkg/statistics/handle/usage/index_usage.rs`

源文件：[`index_usage.rs`](./index_usage.rs)

## 文件定位

本文件位于 `astersql-statistics-handle-usage` crate 内，并由同目录 `lib.rs` 以 `pub mod index_usage` 声明、再通过 `pub use index_usage::*` 对外导出。它提供一套 Rust 风格的索引使用量内存模型：构造单次访问样本，在会话级暂存增量，将增量合并到节点级映射，并通过 `StatsUsageImpl` 暴露创建会话收集器、启停、读取和持久层 GC 的入口。

需要区分两条相邻实现：本文件的 `IndexUsageCollector` 是基于 `Arc<Mutex<HashMap<...>>>` 的同步轻量实现；`pkg/statistics/handle/usage/indexusage/collector.rs` 则定义 Go 风格的 `Collector`、通用通道 worker、对象池和语句级去重器。仓库搜索显示目标文件的类型目前直接用于本 crate 的 `index_usage_integration_test.rs` 以及 `StatsUsageImpl` 字段，而跨包接口 `pkg/statistics/handle/types/interfaces.rs` 引用的是后者的 `SessionIndexUsageCollector`。因此不能把本文件描述成完整生产异步采集链的唯一实现。

`pkg/statistics/handle/usage/Cargo.toml` 指定 crate 根为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/statistics/handle/usage"` 标注 Go 对照包。其依赖目前全部放在 `cfg(any())` 下，不会在正常配置启用；本文件自身只使用标准库以及同 crate 的 `StatsUsageImpl`、`Error` 和存储抽象。

## 核心职责

- `IndexUsageSample` 保存某个索引最近使用时间、查询次数、KV 请求数、扫描行数和七桶访问比例直方图。
- `new_sample`/`NewSample` 将一次索引访问转换为样本，并恰好为一个比例桶加一。
- `merge_sample` 统一实现计数累加、直方图合并和“最近时间取最大值”的聚合规则。
- `IndexUsageCollector` 按 `(table_id, index_id)` 聚合节点级样本，并派生共享同一全局视图的会话收集器。
- `SessionIndexUsageCollector` 先在会话私有映射中累计，直到 `report` 或 `flush` 才将其转移到节点级映射。
- `StatsUsageImpl` 的扩展方法把索引采集器接到 usage 聚合对象上；其中 `gc_index_usage` 委托 `UsageStore`，而不是调用本文件的内存 `IndexUsageCollector::gc`。

## 主要符号

- `IndexUsageSample`：公开数据结构。`Default` 用 `UNIX_EPOCH - 62_135_596_800s` 表示 Go `time.Time{}` 的公元 1 年零值，其余计数和七个桶均为零。
- `BUCKET_BOUND` 与 `access_bucket`：内部比例分桶逻辑。桶语义依次为精确 0、`[0,1%)`、`[1%,10%)`、`[10%,20%)`、`[20%,50%)`、`[50%,100%)`、精确 100%。异常的比例大于 1，或 `NaN`，不会命中区间并回到第 0 桶。
- `new_sample(query_total, kv_req_total, row_access, table_total_rows)`：公开 Rust 风格构造器；总行数为 0 时直接归入第 6 桶，否则以浮点除法计算比例。`NewSample` 是保留 Go 命名的兼容包装。
- `IndexKey = (i64, i64)`：内部键，两个分量依次是表 ID 和索引 ID。
- `merge_sample`：内部聚合函数。三个总计和每个桶使用 `wrapping_add`，时间戳取较新者。
- `IndexUsageCollector`：可克隆的节点级收集器；克隆共享 `samples` 和 `running`，而不是复制快照。`record`、`record_counts`、`sample`、`start_worker`、`close`、`is_running`、`spawn_session`、`gc` 为其公开方法，`merge_pending` 为内部合并入口。
- `SessionIndexUsageCollector`：可克隆的会话级收集器；克隆共享同一 `pending` 映射。`update` 累计、`report` 转移并清空、`flush` 委托 `report`、`sample` 读取尚未上报的数据。
- `StatsUsageImpl::{new_session_index_usage_collector,gc_index_usage,start_worker,close,get_index_usage}`：usage 聚合对象上的公开门面方法。

## 执行流程

1. 执行器或其他调用方先用 `new_sample` 创建数据点。函数根据 `row_access / table_total_rows` 选择桶；表总行数为零时避免除零并按 100% 桶处理，同时记录 `SystemTime::now()`。
2. 调用方通过 `IndexUsageCollector::spawn_session`（或 `StatsUsageImpl::new_session_index_usage_collector`）得到会话收集器。该对象持有节点收集器的共享克隆，以及新建的会话 `pending` 映射。
3. 每次 `SessionIndexUsageCollector::update` 以表/索引二元组为键调用 `merge_sample`。同一索引的计数和桶累加，`last_used_at` 保留较晚值；此时节点级 `sample` 仍看不到增量。
4. `report` 锁住会话映射，再调用 `IndexUsageCollector::merge_pending` 锁住节点映射；它通过 `drain` 把所有条目合并并清空会话映射。`flush` 在此实现中完全等价于 `report`。
5. 节点级 `sample` 返回聚合值的克隆；缺失键返回 `IndexUsageSample::default()`。`record` 可绕过会话阶段直接合并，主要用于确定性测试或简单调用方。
6. `IndexUsageCollector::gc` 根据调用者给出的 `(table_id, index_id) -> bool` 谓词原地保留有效记录。相对地，`StatsUsageImpl::gc_index_usage` 调用 `self.store.gc_index_usage()`，两者不是同一条执行路径。
7. `start_worker` 和 `close` 仅分别写入 `running` 原子标志；本文件没有创建线程、通道或后台循环。关闭后既有样本仍可查询，方法本身也没有阻止后续 `record`、`update` 或 `report`。

## 数据与状态

节点级状态是 `HashMap<(i64, i64), IndexUsageSample>`，包在 `Arc<Mutex<_>>` 中；会话级状态使用相同映射形态并有独立的 `Arc<Mutex<_>>`。`IndexUsageCollector` 的克隆共享节点映射和运行标志；同一 `SessionIndexUsageCollector` 的克隆还共享待上报映射，所以任一克隆执行 `report` 都会排空全部共享待上报增量。

所有累计计数都显式采用 `wrapping_add`，与 Go `uint64` 溢出回绕一致，不依赖 Rust debug/release 构建的溢出策略。`last_used_at` 不相加，只保留最大值。缺失样本与真实存在但全零的样本在 `sample` 返回值上无法区分；只有会话级 `sample` 用 `Option` 区分未记录与已记录。

`record_counts` 是兼容旧四参数接口：KV 请求数固定为 0，并以 `row_access` 同时作为已扫描行数和表总行数。因此当 `row_access > 0` 时比例为 1、落在末桶；当其为 0 时也因总行数为 0 落在末桶。这一便捷方法不能表达真实表行数或 KV 请求数。

## 依赖与调用关系

下游依赖全部很薄：`new_sample` 调用内部 `access_bucket`；`record`、`update`、`merge_pending` 调用 `merge_sample`；`record_counts` 调用 `new_sample` 后再调用 `record`；`report` 调用节点收集器的 `merge_pending`；`flush` 调用 `report`。`StatsUsageImpl::gc_index_usage` 的下游是 `UsageStore::gc_index_usage`，该 trait 在 `predicate_column.rs` 定义。

上游方面，`lib.rs` 重导出本文件的公共符号，`StatsUsageImpl` 在 `predicate_column.rs` 中包含 `index_usage: IndexUsageCollector` 并在构造时初始化默认值。精确仓库引用搜索显示 `index_usage_integration_test.rs` 直接调用 `new_sample`、`record`、`spawn_session`、`update`、`flush`、`sample` 和 `gc`。除这些 crate 内测试、`StatsUsageImpl` 字段及本文件内部调用外，未找到其他 Rust 文件直接使用这组 Rust 风格收集器 API；RustCodeGraph 对常见短方法名的调用边存在名称歧义，因此此处以精确路径文本引用复核其实际接线。

相邻生产链采用 `pkg/statistics/handle/usage/indexusage/collector.rs`：它由 `pkg/statistics/handle/types/interfaces.rs` 暴露会话收集器类型，并由执行器侧索引使用上报逻辑消费。该链具有通用 `globalCollector`、异步/同步发送、对象池和 `StmtIndexUsageCollector` 去重，不能视为本文件方法的直接调用者。

## 错误处理与边界

本文件的采样、聚合和内存 GC API 不返回业务错误。所有 `Mutex::lock` 都以 `expect("index usage mutex poisoned")` 解包；若另一个线程持锁 panic 导致锁中毒，后续访问会直接 panic，而不是恢复或返回 `Result`。Go 零时间的构造也以 `expect` 断言目标平台的 `SystemTime` 能表达公元 1 年。

比例边界严格按浮点比较处理：0 和 1 各有专用桶，中间区间左闭右开；`table_total_rows == 0` 无论 `row_access` 为多少均进入 100% 桶。若 `row_access > table_total_rows`，比例大于 1 会回到第 0 桶，这是为保持 Go 当前实现行为而保留的异常输入语义，不应擅自“修正”。整数到 `f64` 的转换在极大 `u64` 值上可能丢失精度，修改桶算法时需做兼容性评估。

`StatsUsageImpl::gc_index_usage` 是唯一返回 `Result<(), crate::Error>` 的本文件入口，它原样传播存储层错误。内存 `gc` 的回调不会返回错误；若回调 panic，锁会被毒化。`sample` 的默认返回会掩盖“键不存在”，调用方如需判断存在性不能只依赖其计数字段。

## 并发与资源生命周期

`Arc<Mutex<_>>` 使节点映射和会话映射可在线程间共享，单次更新、读取、GC 和合并均在互斥锁内完成。`report` 的锁顺序固定为“会话 pending 锁 -> 节点 samples 锁”；其他方法只获取其中一个锁，当前文件内部没有反向的双锁路径。扩展代码若同时获取两把锁，必须保持相同顺序，避免引入死锁。

`running` 使用 `AtomicBool`：启动以 `Release` 写 true，关闭以 `Release` 写 false，读取以 `Acquire` 加载。但它只是生命周期标志，没有与工作线程或写入准入绑定；`start_worker` 不分配资源，`close` 不等待任务，也不释放映射。样本的生命周期由最后一个共享 `Arc` 决定；会话 `report` 用 `drain` 保证一次上报后不会重复累计。这里没有有界队列、背压或丢样逻辑，故 `report` 会同步等待互斥锁。

## 与 Go 版本的对应关系

直接 Go 对照 `pkg/statistics/handle/usage/index_usage.go` 主要是 `statsUsageImpl` 门面：`NewSessionIndexUsageCollector`、`StartWorker`、`Close` 和 `GetIndexUsage` 委托 `indexusage.Collector`；`GCIndexUsage` 通过系统会话取得最新 infoschema，再按表和索引元数据清理。Rust 本文件的同名/同义门面保持总体用途，但 `gc_index_usage` 改为委托抽象 `UsageStore`，没有在此处展开 infoschema 查询。

样本结构和聚合语义更直接对应 `pkg/statistics/handle/usage/indexusage/collector.go` 及其 Rust 移植 `indexusage/collector.rs`：七桶边界、总行数为零的末桶、计数回绕、最近时间取最大值均一致。重要差异是目标文件没有 Go 的有界通道、后台 worker、`sync.Pool` 等价物和语句级 `QueryTotal` 去重；它的 `report`/`flush` 都立即同步合并，`start_worker`/`close` 只切换标志。

`index_usage_integration_test.go::TestGCIndexUsage` 创建 10 张表、每表 10 个索引，上报后依次删除部分索引和表并验证 GC。Rust 的 `index_usage_integration_test.rs::canonical_gc_index_usage_matches_go_integration_scenario` 用整数 ID 和谓词复现相同保留矩阵，但没有真实 SQL、infoschema 或持久层交互。该 Rust 测试同时明确验证关闭后已 flush 样本仍可读。

## 扩展指南

- 新增样本字段时，应同时修改 `IndexUsageSample`、`Default`、`new_sample`、`merge_sample` 以及独立测试 `index_usage_integration_test.rs`；还要核对 Go `indexusage.Sample` 和完整 Rust 实现 `indexusage/collector.rs::Sample`，防止两套 API 漂移。
- 调整访问比例桶时，应集中修改 `BUCKET_BOUND`/`access_bucket` 并补齐 0、每个临界点、1、总行数为 0、比例大于 1 和大整数精度测试。已有 `canonical_index_usage_bucket_boundaries_match_go` 覆盖七个代表点，但没有覆盖所有左右邻域和异常比例。
- 若要把本文件接入真实异步工作线程，不应仅扩展 `running` 标志；需先决定是否复用 `usage/indexusage/collector.rs`，并明确 `report` 的非阻塞失败语义、`flush` 的同步保证、关闭时排空策略和对象复用。避免在两套实现中重复演进相同子系统。
- 修改会话共享方式或锁粒度时，要保留“clone 共享 pending”“report drain 后恰好合并一次”和锁顺序不变量，并在独立测试文件中增加并发用例；不要把测试内嵌进生产 `.rs`。
- 若改变 `StatsUsageImpl::gc_index_usage`，必须同时检查 `UsageStore::gc_index_usage` 的实现/测试以及 Go infoschema 清理语义；本文件的 `IndexUsageCollector::gc` 与存储 GC 当前是两套不同入口。
- 兼容风险主要是公开字段/方法与 Go 命名包装；性能风险主要是全局单个 `Mutex<HashMap>` 和同步 `report` 在高并发下形成争用。任何优化都应先建立并发和丢样/重复样本测试，再决定是否复用完整 collector。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/statistics/handle/usage` 确认目标、模块入口、Go 对照和测试均已索引；`node --file pkg/statistics/handle/usage/index_usage.rs` 读取完整 281 行并核对 27 个符号；对 `new_sample`、`spawn_session`、`new_session_index_usage_collector`、`gc_index_usage` 等执行了 `query`，并对目标符号尝试 `callers`/`callees`。图对常见短名称产生跨仓库歧义，因此调用接线另以精确路径 `rg` 结果复核。
- 源码与模块边界：`pkg/statistics/handle/usage/index_usage.rs`、`pkg/statistics/handle/usage/lib.rs`、`pkg/statistics/handle/usage/predicate_column.rs`、`pkg/statistics/handle/usage/Cargo.toml`。
- Go 与完整采集器对照：`pkg/statistics/handle/usage/index_usage.go`、`pkg/statistics/handle/usage/indexusage/collector.go`、`pkg/statistics/handle/usage/indexusage/collector.rs`、`pkg/statistics/handle/types/interfaces.rs`。
- 独立测试：`pkg/statistics/handle/usage/index_usage_integration_test.rs` 和 `pkg/statistics/handle/usage/index_usage_integration_test.go`。Rust 测试覆盖累计、worker 标志关闭、会话 flush、10×10 GC 场景和七个桶代表边界；它未覆盖锁中毒、并发争用、异常比例、持久层 GC 错误和真实异步 worker。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令和人工事实复核作为交付验证。
