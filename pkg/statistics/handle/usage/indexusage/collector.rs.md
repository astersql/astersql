# `pkg/statistics/handle/usage/indexusage/collector.rs`

## 文件定位

本文件是独立 crate `astersql-statistics-handle-usage-indexusage` 的核心实现，crate 入口 `pkg/statistics/handle/usage/indexusage/lib.rs` 将其全部公开项重新导出。它位于统计信息 handle 的 usage 子系统内，负责把执行期间产生的“表 ID + 索引 ID”访问样本从语句级、会话级逐步汇总为节点级内存视图；文件本身不负责从执行器计算 KV 请求数或扫描行数，也不负责把结果持久化到系统表。

`pkg/statistics/handle/usage/indexusage/Cargo.toml` 表明该 crate 的运行时依赖只有通用异步增量框架 `astersql-statistics-handle-usage-collector`、元数据模型 `astersql-meta-model` 和通道实现 `crossbeam-channel`（后者由通用 collector 使用）。`pkg/statistics/handle/types/interfaces.rs` 又把本文件的 `Sample` 和 `SessionIndexUsageCollector` 暴露为统计 handle 接口类型，说明它处在执行侧采样与统计 handle 查询/清理边界之间。仓库另有 `pkg/statistics/handle/usage/index_usage.rs` 的 snake_case 内存实现；两者是不同模块，本文只描述本 crate 中的 Go 风格 API，不能把另一实现的即时合并行为当作本文件现状。

## 核心职责

1. `NewSample` 根据一次索引访问的查询数、KV 请求数、访问行数和表总行数生成 `Sample`，并把访问比例计入七桶直方图中的恰好一个桶。
2. `updateByKey` 以 `GlobalIndexID` 为键累加计数和直方图，并保留较新的 `LastUsedAt`；该函数同时服务于会话内累加和节点级合并。
3. `Collector` 管理节点级 `index_usage` 视图，借助通用 `globalCollector` 在后台接收会话增量，并提供查询、worker 生命周期控制及失效索引 GC。
4. `SessionIndexUsageCollector` 在会话本地缓存增量，通过非阻塞 `Report` 或同步 `Flush` 交给节点 collector。
5. `StmtIndexUsageCollector` 在单条语句内按索引去重，只让同一 `(TableID, IndexID)` 第一次更新增加 `QueryTotal`；其他计数仍逐次累加。
6. `INDEX_USAGE_POOL` 回收已清空的 `HashMap` 分配，对齐 Go `sync.Pool`，降低频繁创建会话增量的分配成本。

## 主要符号

- `GlobalIndexID { TableID, IndexID }`：公开、可复制和哈希的复合键。表 ID 与索引 ID 缺一不可，分区或物理表 ID 的选择由上游决定。
- `Sample`：公开聚合值。`LastUsedAt` 是最后使用时间；`QueryTotal`、`KvReqTotal`、`RowAccessTotal` 为累计计数；`PercentageAccess: [u64; 7]` 保存七个比例区间的命中次数。
- `impl Default for Sample`：计数和桶清零，并把时间设为 Go `time.Time{}` 对应的公元 1 年，而不是 Unix epoch。该转换由 `migration_aster_unit_test.rs::default_sample_uses_go_zero_time` 验证。
- `BUCKET_BOUND` 与 `getIndexUsageAccessBucket`：内部比例边界及分桶函数。七桶分别表示 `0`、`(0,1%)`、`[1%,10%)`、`[10%,20%)`、`[20%,50%)`、`[50%,100%)`、`100%`；源码的比较式决定边界值落入右侧桶。
- `NewSample(queryTotal, kvReqTotal, rowAccess, tableTotalRows)`：公开采样构造器。`tableTotalRows == 0` 时直接进入最后一桶；否则使用浮点比值分桶并记录当前系统时间。
- `IndexUsageMap` / `IndexUsageDelta`：私有别名，分别是 `HashMap<GlobalIndexID, Sample>` 和 `Arc<Mutex<...>>`。后者允许会话句柄、通道消息和合并 worker 安全共享一次增量。
- `takeIndexUsageMap` / `takeIndexUsageDelta`：从全局池取空映射并包装；池为空时创建默认 `HashMap`。
- `updateByKey`：私有统一累加点。所有 `u64` 使用 `wrapping_add`，明确复现 Go 无符号整数溢出回绕语义；时间取最大值。
- `mergeDelta`：私有 worker 回调。在节点写锁内 drain 会话增量，随后把保留容量的空映射放回池。
- `Collector` / `NewCollector`：公开节点级采集器及构造器。构造器创建节点映射，并把捕获该映射的 `mergeDelta` 闭包交给 `collector::NewGlobalCollector`。
- `Collector::{GetIndexUsage, SpawnSessionCollector, StartWorker, Close, GCIndexUsage}`：分别负责只读查询、创建会话收集器、启动/关闭 worker、按 `TableInfo.Indices` 清理不存在的表或索引。
- `SessionIndexUsageCollector::{Update, Report, Flush}`：会话级累计与两种上报入口。类型可克隆；克隆体共享同一个 `SessionIndexUsageState`，并非复制一份独立增量。
- `StmtIndexUsageCollector` / `NewStmtIndexUsageCollector` / `Update` / `Reset`：语句级去重包装、构造、更新及复用入口。`Reset` 清空去重集合，让下一条语句可再次为相同索引增加一次查询数。

## 执行流程

典型流程如下：

1. 节点初始化调用 `NewCollector`。它从对象池取得节点映射，创建通用 `globalCollector<IndexUsageDelta>`，并注册 `mergeDelta` 回调；之后调用者必须通过 `StartWorker` 启动后台消费线程。
2. 会话通过 `Collector::SpawnSessionCollector` 获取会话 collector。该对象持有一张待上报映射，以及通用 collector 派生的 `sessionCollector` 通道句柄。
3. 执行器或其适配层为一次索引访问调用 `NewSample`。构造器计算 `rowAccess / tableTotalRows`，在七桶直方图中加一个命中；总行数为零时按满比例桶处理。
4. 如果需要语句级查询去重，上游先用 `NewStmtIndexUsageCollector` 包装会话 collector，再调用语句 collector 的 `Update`。它在 `recorded_index` 锁内检查复合键：首次出现强制 `QueryTotal = 1`，重复出现强制为 `0`，然后把完整样本交给会话 `Update`。KV 请求数、访问行数和比例桶不会被去重。
5. 会话 `Update` 调用 `updateByKey`，把样本合入本会话的待发送映射。此时节点级 `GetIndexUsage` 尚不可见。
6. `Report` 为空时直接返回；非空时克隆当前 delta 并调用 `SendDelta`。仅当通用 collector 接受消息时，才切换到新的空 delta；通道满或发送失败时原增量继续留在会话侧，供后续 `Report`/`Flush` 重试。
7. `Flush` 同样跳过空增量，但通过 `SendDeltaSync` 阻塞发送，随后切换到新 delta。通用 collector 的 worker 收到消息后执行 `mergeDelta`，在节点写锁下逐项累加。
8. 查询方用 `GetIndexUsage` 取得快照；不存在的键返回 `Sample::default()`，API 不区分“从未记录”与“记录值恰为零”。
9. 元数据变更后，`GCIndexUsage` 在节点写锁内逐项调用 `tableMetaLookup`：表不存在则删除；表存在但 `Indices` 中找不到索引 ID 也删除。节点关闭时调用 `Close`，通用 collector 会等待 worker 结束并在退出前排空通道中的残留消息。

RustCodeGraph 直接确认了下游调用边：`NewCollector → NewGlobalCollector / takeIndexUsageMap / mergeDelta`、`NewSample → getIndexUsageAccessBucket`、`mergeDelta → updateByKey`，以及 `Report`、`Flush → takeIndexUsageDelta`。全局 callers 查询在本次分析中超时，因此跨 crate 上游接线以 Cargo 依赖、`lib.rs` 再导出、`interfaces.rs` 接口再导出和精确文本检索为依据；未把无法确认的生产调用者写成已接线事实。

## 数据与状态

节点状态存于 `Collector.index_usage: Arc<RwLock<IndexUsageMap>>`。查询只持读锁；worker 合并和 GC 持写锁，因此二者与查询互斥程度不同，但映射始终由锁保护。节点 map 与会话 delta 均可能复用池中分配；只有已经 drain 并与会话状态分离的空 map 才会返回 `INDEX_USAGE_POOL`。

会话状态是 `Arc<Mutex<SessionIndexUsageState>>`，内部同时保存当前 delta 和通用会话通道句柄。所有克隆的 `SessionIndexUsageCollector` 共享这两个对象，因此同一会话 collector 的克隆之间顺序一致且不会各自重复上报。`Update` 获取外层状态锁后再取 delta 锁；`Report`/`Flush` 使用相同锁顺序，避免本文件内部锁顺序反转。

语句状态是独立的 `Mutex<HashSet<GlobalIndexID>>`。它只影响 `QueryTotal`，不会改变样本的其他字段。`Reset` 是跨语句复用时的语义边界；遗漏 Reset 会让后续语句对已经出现的索引继续按重复项处理。

所有聚合计数都按 `u64` 回绕累加，不做饱和、报错或溢出告警。`LastUsedAt` 取所有样本的最大时间。对象池本身是 `LazyLock<Mutex<Vec<IndexUsageMap>>>`，没有容量上限；其生命周期与进程相同。

## 依赖与调用关系

- 向下依赖 `pkg/statistics/handle/usage/collector/collector.rs`：`NewGlobalCollector` 创建有界普通/高优先级通道；`SpawnSession` 派生发送端；`SendDelta` 尝试非阻塞发送，`SendDeltaSync` 使用高优先级同步发送；`StartWorker`/`Close` 管理线程。普通和高优先级通道默认容量均为 10。
- 向下依赖 `astersql-meta-model` 经 `indexusage/lib.rs` 暴露的 `model::TableInfo`；本文件只在 `GCIndexUsage` 中读取 `TableInfo.Indices[*].ID`，避免直接依赖 session context 或 info schema。
- crate 边界由 `indexusage/Cargo.toml` 和 `lib.rs` 确认；`lib.rs` 将 `collector::*` 全量重新导出，并仅把测试放在独立的 `collector_test.rs` 与 `migration_aster_unit_test.rs` 中。
- 向上接口证据位于 `pkg/statistics/handle/types/interfaces.rs`：它再导出 `Sample as IndexUsageSample` 和 `SessionIndexUsageCollector`，并在 `IndexUsage` trait 中声明会话创建、GC、worker 启停和查询能力。
- Cargo 依赖清单显示 `pkg/statistics/handle/types`、`pkg/session`、`pkg/executor/internal/exec` 与 `pkg/statistics/handle/usage` 声明了对该 crate 的依赖；但精确 Rust 源检索只确认了 types 接口层的直接再导出以及本 crate 测试调用。其他依赖声明不能单独证明相应生产路径已调用本文件 API。
- `pkg/executor/internal/exec/indexusage.rs` 定义执行器侧 reporter 抽象，但当前所见源码使用其自身的样本/trait 类型；本文不声称它已经直接调用本文件的 `NewSample` 或 `StmtIndexUsageCollector`。

## 错误处理与边界

本文件的公开 API 不返回 `Result`，同步失败主要通过默认值、布尔返回的内部处理或 panic 表现：

- `GetIndexUsage` 对缺失键返回默认样本，调用方无法据此判断键是否存在。
- `Report` 只有在 `SendDelta` 成功时才更换 delta；拒绝的消息不会丢失。`migration_aster_unit_test.rs::report_rejection_keeps_pending_delta` 用未启动 worker 的 10 容量通道填满场景验证这一点。
- `Flush` 忽略 `SendDeltaSync` 的布尔结果，并无条件更换 delta。若 collector 已关闭或同步发送失败，待发数据可能不再由当前会话保留；调用方应在关闭节点 collector 前完成 Flush。
- 所有 `Mutex`/`RwLock` 获取均使用 `unwrap()`；任一持锁线程 panic 导致锁中毒后，后续访问会继续 panic，而非恢复或返回错误。
- `GCIndexUsage` 约定 `(exists == true)` 时必须同时返回 `Some(TableInfo)`；违反约定会在 `expect` 处 panic。`exists == false` 时忽略 `Option` 并删除记录。
- `Sample::default` 要求平台 `SystemTime` 能表示公元 1 年；否则初始化时 panic。现有迁移测试验证预期值，但本任务按要求未运行 Cargo。
- 分桶函数只显式处理 `0..=1` 的正常比例。`rowAccess > tableTotalRows`、NaN 或其他异常输入不会命中中间/末桶，保留初始桶 0；调用者不应把这解释为经过校验的合法比例。
- `tableTotalRows == 0` 无论 `rowAccess` 为多少均进入最后一桶，这是与 Go 对照实现一致的兼容行为，不代表实际进行了全表扫描。

## 并发与资源生命周期

节点 collector 的 worker 由调用者显式启动和关闭。`StartWorker` 委托通用 collector 创建线程；通用实现允许每次调用再创建一个 worker，因此上游应把启动视为生命周期动作而非无条件重复调用。`Close` 在通用实现中由 `Once` 保证只执行一次，设置关闭标志、唤醒阻塞操作、join 所有 worker，并在 worker 退出前 drain 两个通道；关闭后再次启动会直接返回。

会话 `Report` 是面向低延迟路径的非阻塞操作。普通通道满时它保留增量；通用会话 collector 距上次成功发送超过默认 5 分钟后，`SendDelta` 会转入同步高优先级发送。`Flush` 总是走同步高优先级通道，因此可能阻塞到 worker 接收或 collector 关闭。

worker 调用 `mergeDelta` 时先获取节点写锁，再获取 delta 锁。会话一旦成功发送就立即换用新 delta，不会继续修改已入队对象；因此被 worker drain 的 map 可以安全清空并回池。`StmtIndexUsageCollector::Update` 用独立互斥锁覆盖“检查/插入去重键 + 下游会话更新”的整个区间，以支持同一语句的多个执行 worker 同时结束并上报。

测试证据 `collector_test.rs::test_flush_concurrent_index_collector` 和 `migration_aster_unit_test.rs::concurrent_flush_matches_serial_aggregation` 使用 64 个会话、每会话 100000 次操作比较并发与串行聚合；它们验证设计意图，但本次纯文档任务没有执行测试。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/usage/indexusage/collector.go`，主要结构一一对应：

- Go `GlobalIndexID`、`Sample`、`bucketBound`、`getIndexUsageAccessBucket`、`NewSample` 对应同名 Rust 符号；字段含义、七桶边界和零总行数末桶规则一致。
- Go `indexUsage.updateByKey`/`merge` 对应 Rust `updateByKey`/`mergeDelta`。Rust 显式使用 `wrapping_add`，复现 Go `uint64` 溢出回绕；两者都取较新的时间。
- Go `sync.Pool` 对应 Rust `INDEX_USAGE_POOL`。Rust 以 `Mutex<Vec<HashMap>>` 实现，复用目标相同，但 Rust 池无 Go runtime 可随 GC 丢弃池项的语义。
- Go `Collector` 内嵌 `sync.RWMutex` 并持有普通 map；Rust 用 `Arc<RwLock<HashMap>>`，以便 `NewGlobalCollector` 的 `'static` 合并闭包共享目标。
- Go 会话 collector 是可变指针对象；Rust 为了支持克隆和跨线程共享，把整个会话状态放入 `Arc<Mutex<_>>`。其成功 Report 后换 map、失败保留和 Flush 同步发送的语义保持一致。
- Go 语句 collector 的 `sync.Mutex + map` 对应 Rust `Mutex<HashSet>`；两者均把首次样本的 `QueryTotal` 强制设为 1、后续设为 0。Rust 额外提供明确的 `Reset` 测试以证明复用边界。
- Go 缺失索引返回 `Sample{}`，其中时间为 Go 零时间；Rust 因 `SystemTime::default` 不具备该语义而手工回退 62135596800 秒。
- Rust 独立测试 `collector_test.rs` 复刻 Go `collector_test.go` 的分桶、累计、并发 Flush 和语句去重；`migration_aster_unit_test.rs` 补充 Go 零时间、Report 拒绝保留、Reset 和元数据 GC。Rust 稳定测试框架不会执行 Go benchmark 的原生等价物，因此其并行 benchmark 只保留为 `dead_code` 负载辅助函数。

## 扩展指南

- 新增或改变采样字段时，必须同步修改 `Sample::default`、`NewSample`、`updateByKey`、Go `Sample`/聚合逻辑以及两个独立 Rust 测试文件；尤其要决定新字段是求和、取最大值还是去重，不能只在构造器赋值。
- 调整比例桶时，应同时修改 `BUCKET_BOUND`、`PercentageAccess` 数组长度、`NewSample` 和 Go 对照，并扩展 `collector_test.rs::test_get_bucket` 与 `migration_aster_unit_test.rs::bucket_and_new_sample_match_go_boundaries` 的每个开闭区间边界。还应明确处理大于 100%、NaN 等异常比例，避免沿用桶 0 的隐式结果而不知情。
- 改变 Report/Flush 可靠性时，接入点是 `SessionIndexUsageCollector::{Report, Flush}` 和通用 `usage/collector` 的 `SendDelta*`。必须保留“非阻塞拒绝不丢增量”不变量，并为关闭竞态、通道满和同步发送失败添加独立测试；测试不能内嵌在生产源文件中。
- 改变对象池策略时，重点审查 `takeIndexUsageMap`、`takeIndexUsageDelta`、`mergeDelta`，确保只有已从会话状态切走且已 drain 的 map 被回收，并评估无界池对长期内存占用的影响。
- 扩展 GC 条件时修改 `Collector::GCIndexUsage`，保持回调依赖最小化；同步更新 `migration_aster_unit_test.rs::gc_removes_missing_tables_and_indexes`，覆盖表缺失、索引缺失、元数据回调契约以及查询并发。
- 在生产路径接入本 crate 前，应从 `pkg/statistics/handle/types/interfaces.rs::IndexUsage` 与执行器 reporter 边界核实类型转换；当前 Cargo 依赖并不等于所有声明者都已真实调用本实现。
- 性能风险集中在节点写锁、每次会话 Update 的两层互斥锁、语句去重锁和全局池锁。增加高频字段或更细粒度事件时，应使用与现有 64 会话负载相当的并发测试/基准验证，而不能通过删除锁或缩减 Go 语义来换取测试通过。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件，目标目录六个文件均已索引；`node --file pkg/statistics/handle/usage/indexusage/collector.rs` 读取到完整 338 行与 26 个符号。
- RustCodeGraph 精确符号查询：确认目标定义 `NewSample`（第 96 行）、`NewCollector`（第 178 行）、`GetIndexUsage`（第 193 行）、`GCIndexUsage`（第 228 行）、`NewStmtIndexUsageCollector`（第 312 行）及私有 `mergeDelta`、`updateByKey`、`getIndexUsageAccessBucket`。
- RustCodeGraph callees：`NewCollector → NewGlobalCollector/takeIndexUsageMap/mergeDelta`；`NewSample → getIndexUsageAccessBucket`；`mergeDelta → updateByKey`；`SpawnSessionCollector`、`Report`、`Flush → takeIndexUsageDelta`。callers 查询曾超过 60 秒未返回，已停止该查询，并用下列本地直接证据限定上游结论。
- 源码与边界：`pkg/statistics/handle/usage/indexusage/collector.rs`、`lib.rs`、`Cargo.toml`；通用异步实现 `pkg/statistics/handle/usage/collector/collector.rs`；接口再导出 `pkg/statistics/handle/types/interfaces.rs`；声明依赖者的 Cargo manifests。
- Go 对照：`pkg/statistics/handle/usage/indexusage/collector.go` 与 `collector_test.go`。
- 独立 Rust 测试：`pkg/statistics/handle/usage/indexusage/collector_test.rs`、`migration_aster_unit_test.rs`。测试覆盖桶边界、字段累加、异步可见性、通道拒绝保留、64 会话并发一致性、语句去重/Reset、Go 零时间及 GC。
- 接线限制核验：精确 `rg` 只确认 `pkg/statistics/handle/types/interfaces.rs` 对公开类型的生产再导出；`pkg/session` 和 `pkg/executor/internal/exec` 虽有 Cargo 依赖声明，但未据此推断其已调用本文件 API。
- 本任务是纯文档分析，遵循计划不运行 Cargo。交付前只执行任务指定的 11 章节结构验证，并人工复核标题、符号、调用边、边界和扩展建议均能回指上述文件。
