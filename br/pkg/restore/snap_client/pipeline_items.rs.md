# `br/pkg/restore/snap_client/pipeline_items.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-snap-client` crate，由同目录 `lib.rs` 以 `pub mod pipeline_items` 挂载，并通过 `pub use pipeline_items::*` 重导出公开符号。`Cargo.toml` 把该 crate 定位为 Go 包 `br/pkg/restore/snap_client` 的 Rust 迁移单元；当前依赖面主要是本地 `stubs.rs`、`client.rs` 与 `systable_restore.rs`，并没有直接连接真实 TiKV/Domain 的完整 Go 依赖。

它同时承载四组相关但可分开理解的能力：恢复后的临时系统表替换、多阶段表级并发流水线、统计元数据批量写入，以及供 `tikv_sender.rs` 使用的 `PhysicalTable` 数据形状。RustCodeGraph 索引已覆盖该文件的 559 行和 51 个符号。

## 核心职责

- `SnapClient::replaceTables` 以固定顺序完成恢复临时表的识别、`mysql.user` 兼容处理、统计表 schema 补齐、原子重命名和权限缓存通知。
- `PipelineConcurrentBuilder` 把按注册顺序排列的表处理阶段连成有界通道，每阶段内并发，阶段间保留顺序依赖，任一处失败时协作取消。
- `statsMetaItemBuffer` 把每个物理表的行数变更聚合成最多 3000 项的批次，并以固定次数重试写入 `StatsHandler`。
- `calculateRowCountForPhysicalTable` 与三个 `updateStatsMetaFor*` 函数从备份文件元数据中只累加 record-key SST，区分普通表、分区和逻辑表总计。

这些是当前 Rust 实现的真实边界。Go 文件中的进度统计、checksum 任务注册、JSON/外部存储统计加载和 TiFlash 就绪等待并未在本 Rust 文件中完整移植。

## 主要符号

- 常量：`defaultChannelSize = 1024` 是每个阶段间有界通道容量；`defaultChecksumConcurrency = 64` 保留 Go 默认值，但当前 Rust 本文件没有使用它注册 checksum 阶段；`statsMetaItemBufferSize = 3000` 是统计元更新触发刷新的阈值。
- `PhysicalTable { NewPhysicalID, OldPhysicalID, RewriteRules, Files }`：把新旧物理 ID、键重写规则和 SST 文件绑在一起。直接生产消费者是 `tikv_sender.rs::getSortedPhysicalTables`，而非本文件内的流水线。
- `ExhaustErrors(&Mutex<Vec<Error>>) -> Vec<Error>`：持锁后用 `std::mem::take` 原子地拿走当前全部错误，同时把共享容器留为空 `Vec`。索引未发现本函数的外部 Rust 调用者。
- `SnapClient::{filterAndValidateTemporaryTables, updateTemporaryUserTable, moveRenamedTable, replaceTables}`：实现临时表筛选与替换链。`replaceTables` 是该链的公开编排入口。
- `PipelineTask`：调用方提供的阶段描述，包含标签、并发度、逐表 `process` 和阶段收尾 `end`；两个回调都必须是 `Send + Sync` 的 `Arc<dyn Fn...>`。
- `PipelineContext { loadStatsPhysical, loadSysTablePhysical, tasks }`：Rust 版只携带过滤开关和已组装任务，不是 Go 版那个包含 checksum/统计/TiFlash 开关、并发度和外部客户端的完整上下文。
- `PipelineConcurrentBuilder`：`RegisterPipelineTask` 只追加阶段；`StartPipelineTask` 负责过滤输入、启动 source/stage/sink 线程、保留第一个错误并 join 所有线程。内部 `pipelineFunction` 是不对外暴露的存储形式。
- `statsMetaItemBuffer`：用 `Mutex<Vec<model::MetaUpdate>>` 保护批次。`TryUpdateMetas` 追加一项并在满 3000 时写入；`UpdateMetasRest` 用于阶段结束时刷新尾批。
- `calculateRowCountForPhysicalTable`、`updateStatsMetaForNonPartitionTable`、`updateStatsMetaForPartitionTable`、`updateStatsMetaForTable`：完成行数计算、新旧分区 ID 映射和普通/分区分派。

## 执行流程

### 临时表替换

1. `replaceTables` 用两个 load 开关创建 `TemporaryTableChecker`，再把 `CheckTemporaryTables` 作为策略传入 `filterAndValidateTemporaryTables`。
2. 筛选器遍历 `CreatedTable`，以旧库名和旧表名查询目标库；命中时把表名放入 `HashMap<db, HashMap<table, ()>>` 并计数。
3. 若结果为空立即返回 `0`；否则先由 `updateTemporaryUserTable` 删除临时 `mysql.user` 中不再兼容的 resource-group 字段/数据。
4. `updateStatsTableSchema` 通过注入的 `InfoSchema` 和 `execution` 回调补齐统计表 schema。
5. `moveRenamedTable` 生成成对的 rename SQL，并在 `SnapClient.db` 存在时执行；随后 `notifyUpdateAllUsersPrivilege` 根据表集合触发权限刷新。

### 并发流水线

1. `SnapClient::RestorePipeline` 将 `PipelineContext.tasks` 依次注册到 builder，然后调用 `StartPipelineTask`。它当前不会自动调用 `replaceTables`。
2. `StartPipelineTask` 先移除已由 physical restore 处理的统计临时表或可重命名系统临时表。
3. source 线程逐表写入容量 1024 的首通道。通道满时用 `try_send` + `yield_now` 重试，并每次检查取消标记。
4. 每个注册阶段拥有一个调度线程和 `max(concurrency, 1)` 个 worker。worker 从被 `Arc<Mutex<Receiver>>` 串行保护的接收端取表，执行 `processFn`，成功后把同一 `CreatedTable` 移交下一阶段。
5. 阶段等待所有 worker 结束，且只在未取消时调用一次 `endFn`。最终 sink 排空末端通道，使上游不会因无消费者阻塞。
6. join 完毕后先返回记录的首个阶段错误，其次返回 `Context::Err()`，否则成功。

### 统计元数据

1. `updateStatsMetaForTable` 按旧表是否存在 `Partition` 选择普通表或分区表路径。
2. 行数计算仅累加 `tablecodec::IsRecordKey(StartKey)` 的 `TotalKvs`，故索引 SST 不会重复计数。
3. 普通表用旧逻辑表 ID 取文件，但把结果写到新表 ID。分区表跳过旧逻辑表 ID，先统计每个旧物理分区，再按分区名查新 ID；零行分区不产生单独更新，最后总是追加逻辑表总计。
4. 缓冲区未满时不 I/O；满 3000 或显式调用 `UpdateMetasRest` 时，整批传给 `saveMetaToStorageWithRetry`。

## 数据与状态

- 流水线中的单元是拥有所有权的 `CreatedTable`；阶段回调只收到 `&CreatedTable`，因此不能像 Go 测试那样在第一阶段原地改写表 ID。
- 阶段列表保持注册顺序；每个表只有在前一阶段 `processFn` 成功后才会进入下一阶段。同一阶段内的表完成顺序不确定。
- `cancelled: AtomicBool` 是全流水线的单向终止标记；`first_error: Mutex<Option<Error>>` 只保留首个可见错误。这与 `ExhaustErrors` 的“拿走全部已收集错误”工具函数是两套独立机制。
- `statsMetaItemBuffer.metaUpdates` 受 mutex 保护，允许多个统计阶段 worker 共享。每项同时把 `Count` 和 `ModifyCount` 设为恢复计算值，`PhysicalID` 必须是下游新 ID。
- 临时表集合使用单元值 `()` 表示 set；外层键是最终数据库名，内层键是表名。

## 依赖与调用关系

- 上游：`lib.rs` 挂载并重导出本模块；`tikv_sender.rs::getSortedPhysicalTables` 构造 `PhysicalTable`；`export_test.rs` 通过测试导出包装 `replaceTables`；`pipeline_items_test.rs` 直接调用 builder 和统计元更新函数。RustCodeGraph `explore` 未显示 `RestorePipeline` 的生产 Rust 调用者，所以不应把它描述成已接入 Rust 恢复主链。
- 系统表下游：`TemporaryTableChecker`、`IsStatsTemporaryTable`、`IsRenameableSysTemporaryTable`、`updateStatsTableSchema`、`GenerateMoveRenamedTableSQLPair`、`removeUserResourceGroup` 和 `notifyUpdateAllUsersPrivilege` 均来自 `systable_restore.rs`。
- 模型下游：`CreatedTable`、`Context`、`Error`、`StatsHandler`、`RewriteRules`、`backuppb::File`、`model::MetaUpdate` 以及 `tablecodec` 由 `stubs.rs` 提供。`Cargo.toml` 的注释也明确说明当前是本地 trait/stub 路径，不包含 arm64 Darwin 上的 kv/domain/kvproto/grpcio 真实链。
- 统计写入：`StatsHandler::SaveMetaToStorage("br restore", false, updates)` 是唯一持久化出口；分区新 ID 通过 `GetPartitionByName` 按名称查找。
- Go 主链：同路径 `pipeline_items.go::RestorePipeline` 由 Go 恢复工作流调用，它自行执行表替换、计算进度，并注册 checksum/统计/TiFlash 阶段；这些调用边不能自动套用到当前 Rust 实现。

## 错误处理与边界

- `replaceTables` 用 `?` 严格串联各步：任一步失败都不继续后续 rename/通知。但 `moveRenamedTable` 在 `self.db == None` 时安静成功，这是当前桔环境语义，与 Go 必须有 DB session 不完全等价。
- `filterAndValidateTemporaryTables` 的 `checksum` 分支目前只有注释，不执行校验，也没有 Go 的 `kvClient`/`checksumConcurrency` 参数。因此 `checksum=true` 不能被解读为已保证数据正确。
- 流水线将 `concurrency == 0` 提升为 1，避免无 worker 死锁。收通道用 2ms timeout 定期重查取消；发通道满时主动 yield，没有指数退避。
- worker panic 会被 join 检测并转为 `Error::new("pipeline worker panicked")`。source、stage 调度线程或 sink 本身的 panic join 结果被丢弃，没有同等的显式错误转换。
- 只记录首个 process/end/panic 错误，后续错误不会聚合。`Context::Err()` 只在未已返回阶段错误时成为结果。
- `calculateRowCountForPhysicalTable` 在 `u64` 中求和后用 `as i64` 转换，没有溢出/越界检查。普通表缺失文件 map 项时按空列表和零行处理；分区名无法映射则立即返回错误。
- 批量写入最多尝试 8 次，失败间固定 sleep 500ms，不接收 `Context`，所以不能在等待期间响应取消。尝试耗尽后返回最后一个错误。
- `Mutex::lock().unwrap()` 意味着锁中毒会 panic，而非返回 `Result`。

## 并发与资源生命周期

`StartPipelineTask` 拥有所有新建线程的 join handle，在返回前依次等待 source、每个 stage 调度线程和 sink；阶段调度线程又在内部等待全部 worker。因此正常和已捕获错误路径不会留下脱管线程。

每个阶段的原始 `next_tx` 在 worker 启动后立即丢弃；最后一个 worker 结束并释放 sender 时，下游才看到 disconnected，这就是正常完成信号。错误路径不依赖通道完整排空，而是设置 `cancelled` 使 source、worker 和 sink 尽快退出。Acquire/Release 排序只用于取消标记的可见性，具体错误值另由 mutex 保护。

同一阶段的 receiver 包在 mutex 内，故“取一个任务”是串行短临界区，而 `processFn` 在锁外并行。有界通道约束在途表数，但满通道处理会忙等待/yield，扩展时需评估 CPU 开销和取消响应。

`statsMetaItemBuffer` 在持锁期间只做追加或整个 `Vec` 交换，真实存储调用在释放锁之后执行，避免慢 I/O 阻塞其他生产者。但多个达到阈值的 worker 可同时执行不同批次的 `SaveMetaToStorage`，具体存储实现必须满足 `StatsHandler: Send + Sync`。

## 与 Go 版本的对应关系

当前 Rust 保留了 Go 的核心名称、通道容量、流水线阶段模型、临时表处理顺序、统计元 3000 项批次和 8 次/500ms 重试。`pipeline_items_test.rs` 也对齐了 Go 的两个并发场景、统计元计数和四组表替换开关组合。

需要明确保留的差异如下：

- Go `ExhaustErrors` 非阻塞地排空一个永不关闭的 error channel；Rust 版排空 `Mutex<Vec<Error>>`，只对“调用时已收集的项”有相似语义。
- Go 临时表筛选在 `checksum=true` 时用 worker pool 真正执行 `execAndValidateChecksum`；Rust 版该分支是明示的 slim/mock 空实现。
- Go `RestorePipeline` 从开关构建 checksum、update/load stats 和 TiFlash 阶段，也处理表替换和 progress；Rust 版只执行调用方已填充的 `tasks`，本文件没有 `registerValidateChecksum`、`registerUpdateMetaAndLoadStats` 或 `registerWaitTiFlashReady` 对应实现。
- Go 用 `errgroup.WithContext` 连锁取消；Rust 用原子标记、2ms 轮询和 OS 线程。Go 阶段 worker pool 并发度传 0 时的精确行为不在本文件中定义；Rust 明确至少为 1。
- Go `processFn` 接收可变指针，第一个对照测试会直接改表 ID；Rust 回调只收不可变引用，Rust 测试改用第二阶段计算 `ID + 10000` 来验证总和。
- Go 统计写重试接收 context 并使用 `utils.WithRetry`；Rust 是不可取消的固定 sleep 循环。Go 的统计阶段还会尝试加载 JSON/外部统计文件，失败或无文件时才回退到 meta 计数；Rust 本文件只实现 meta 计数底层函数。
- Go `replaceTables` 从 `SnapClient` 内部获取 domain/info schema/session 和权限通知器；Rust 要求调用方显式注入 `InfoSchema`、SQL execution 和 notifier，便于在桔环境中测试。

## 扩展指南

- 新增逐表阶段时，通过 `PipelineTask`/`RegisterPipelineTask` 接入，把阶段收尾操作（例如刷新尾批）放在 `endFn`，不要依赖表的处理顺序。同步在独立 `pipeline_items_test.rs` 增加成功、中途失败、取消和收尾断言，不应把测试内嵌到源文件。
- 若对齐 Go `RestorePipeline`，应先补齐 `PipelineContext` 的配置/客户端语义，再移植三个 register 函数和 progress/summary，同时把 `replaceTables` 放回流水线前置阶段。不能只注册空回调来宣称行为对齐。
- 若补齐 checksum，首先替换 `filterAndValidateTemporaryTables` 的空分支，传入可取消上下文、KV 客户端和 checksum 并发度，并保留“任一 checksum 失败则不替换表”的 Go 不变量。
- 调整并发模型时，必须保持：有界背压、前阶段成功后才传递、错误导致全链取消、失败阶段不调用 `endFn`、正常阶段只收尾一次，以及返回前回收所有线程。优化 `try_send` 忙等待时需用有界压力测试证明吞吐和取消不退化。
- 扩展统计元处理时，继续排除 index-key SST，保持分区按名称映射新 ID，并评估 `u64 -> i64` 溢出、零行分区、全局索引文件和并发批次写入顺序。测试应扩展 `test_update_stats_meta` 和重试失败注入场景。
- 修改临时系统表清单或 schema 兼容逻辑时，同步检查 `systable_restore.rs` 和 `pipeline_items_test.rs` 的四组 `replaceTables` 场景，特别是 `mysql.user` resource group 清理、stats 表升/降级和权限通知时机。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/restore/snap_client` 确认源、Go 对照和独立测试文件；`node --file br/pkg/restore/snap_client/pipeline_items.rs --offset 1 --limit 1200` 读取了全部 559 行；`explore "pipeline_items.rs: identify all structs, traits, functions, callers and callees in snapshot restore pipeline"` 核对了 `replaceTables`、builder、统计元函数的内部调用边和测试调用者。精确 `callers RestorePipeline` 在本地索引上长时无输出后被终止，故上游结论又用限定路径的 `rg` 交叉检查。
- Rust 源码：`br/pkg/restore/snap_client/pipeline_items.rs`；crate 边界与挂载：`br/pkg/restore/snap_client/Cargo.toml`、`br/pkg/restore/snap_client/lib.rs`；直接模型/trait 定义：`br/pkg/restore/snap_client/stubs.rs`；`PhysicalTable` 消费者：`br/pkg/restore/snap_client/tikv_sender.rs`。
- Go 对照：`br/pkg/restore/snap_client/pipeline_items.go`，重点核对 `RestorePipeline`、`PipelineConcurrentHandler`、三个 register 函数、统计元批次和 `replaceTables`。
- 独立测试：`br/pkg/restore/snap_client/pipeline_items_test.rs` 的 `test_pipeline_concurrent_handler_1/2`、`test_update_stats_meta`、`test_update_stats_meta_retries_transient_storage_errors`、`test_replace_tables*`；Go 意图对照为 `br/pkg/restore/snap_client/pipeline_items_test.go` 的 `TestPipelineConcurrentHandler1/2`、`TestUpdateStatsMeta` 和 `TestReplaceTables*`。
- 本任务是只读分析加 Markdown 产物，按任务约定不运行 Cargo。文档结构用任务文件指定的 11 章节 shell 命令验证。
