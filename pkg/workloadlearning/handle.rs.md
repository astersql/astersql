# `pkg/workloadlearning/handle.rs`

## 文件定位

`handle.rs` 属于 `astersql-workloadlearning` crate；`pkg/workloadlearning/Cargo.toml` 将 `lib.rs` 设为库入口，而 `lib.rs` 通过 `mod handle` 加载本文件并用 `pub use handle::*` 重导出其公开项。该 crate 当前只直接依赖 `serde` 和 `serde_json`。

本文件是 Rust 工作负载学习原型中的分析与持久化边界：它从最近七天的语句统计抽象 `StatementRecord`，解析执行计划，按表 ID 汇总 `TableReadCostMetrics`，计算归一化读代价，再通过 `WorkloadStore` 保存新版本。仓库内 Rust 引用搜索只找到 `handle_test.rs` 和 `cache_test.rs` 对这些入口的调用，尚未找到服务器启动链或调度器对 `NewWorkloadLearningHandle` / `HandleTableReadCost` 的生产接线；因此它目前是 crate 可导出的能力，而不是已验证会由完整应用自动运行的后台任务。

与 Go 实现不同，Rust 文件没有直接依赖 TiDB 系统会话池、受限 SQL 执行器、事务和 protobuf 解码器，而是把这些环境能力压缩到 `WorkloadStore` trait，并使用 JSON 形式的 `ExplainOperator`。这一差异决定了文档所述运行链以当前 Rust 代码为准，不能将 Go 的完整数据库接线视为 Rust 已支持能力。

## 核心职责

1. `Handle::HandleTableReadCost` 编排一次完整分析：调用 `analyzeBasedOnStatementStats`，再调用 `SaveTableReadCostMetrics`，最终返回内存中的表 ID 到指标映射。
2. `Handle::analyzeBasedOnStatementStats` 选取 `SystemTime::now()` 前七天到当前时间的窗口，经 `findClosestSnapshotIDByTime` 获取两端快照，加载语句，并对可解析计划进行指标提取和累加。
3. `extractScanAndMemoryFromBinaryPlan` 与 `extractMetricsFromOperatorTree` 把 JSON 执行计划转换为逐扫描算子的表读指标，覆盖 `IndexLookUp`、`IndexReader`、`PointGet`、`BatchPointGet`、非 TiFlash 的 `TableReader` 和 `IndexMerge`。
4. `AccumulateMetricsGroupByTableID` 通过 `InfoSchema::TableID` 把库表名解析为稳定表 ID，将扫描时间和内存按语句频率放大，并合并同表指标。
5. `SaveTableReadCostMetrics` 将每表指标序列化为 JSON，按最多 1000 行一批交给 `WorkloadStore::save_metrics`，版本取最新版本加一。
6. `DBNameExtractor` 保留了 Go AST visitor 的最小语义：遍历 `Node` 时收集小写 schema 名；它不参与上述表读代价主链。

## 主要符号

- 常量 `batchInsertSize = 1000` 控制保存批大小；`defaultPointGetMemUsage = 1` 为无法获得真实内存数据的点查算子提供默认值。公开常量 `feedbackCategory` 和 `tableReadCost` 与 Go 分类字符串一致，但当前 Rust 保存接口只提交 `(table_id, JSON)`，本文件没有把这两个字符串传给存储层。
- `StatementRecord { digest, sql, binary_plan, frequency }` 表示一个统计窗口内的语句记录。主链实际读取 `binary_plan` 和 `frequency`；`digest`、`sql` 仅作为记录数据保留，Rust 遇到计划解析错误时也没有用它们记录日志。
- `WorkloadStore: Send + Sync` 是环境适配面，定义版本读取、指标读写、快照定位、语句加载和表 ID 查询。`Handle` 以 `Arc<dyn WorkloadStore>` 持有它，因此 Handle 可共享底层存储，但 trait 的具体同步和事务保证由实现者负责。
- `InfoSchema::TableID` 是库表名解析接口。对所有 `WorkloadStore` 自动实现该 trait，并转发到 `table_id`，使聚合函数可以单独用轻量替身测试。
- `Handle` 与构造函数 `NewWorkloadLearningHandle` 构成公开编排 API。`HandleTableReadCost`、`analyzeBasedOnStatementStats` 和 `SaveTableReadCostMetrics` 均以 `Result<..., String>` 报告失败。
- `AccessObject`、`ExplainOperator` 是 JSON 计划模型。`#[serde(default)]` 允许缺失动态分区、执行信息、访问对象或子节点字段；缺失字段是否可接受由后续算子分支决定。
- `extractScanAndMemoryFromBinaryPlan` 只对一个 JSON 根节点反序列化并递归。`extractMetricsFromOperatorTree` 是算子分派核心；辅助函数分别处理算子类型、访问对象、表/索引扫描后代、IndexMerge 部分扫描和执行时间。
- `Node` 与 `DBNameExtractor` 是本地化 AST 抽象。`Enter` 对 `TableName` 的 schema 做 ASCII 小写并通过 `HashSet` 去重，始终返回 `false`；`Leave` 始终返回 `true`。

## 执行流程

一次 `HandleTableReadCost` 的流程如下：

1. `analyzeBasedOnStatementStats` 固定以当前系统时间为结束点、向前 7×24 小时为起点。它分别调用 `findClosestSnapshotIDByTime`；该函数本身只是 `WorkloadStore::closest_snapshot_id` 的转发。
2. `WorkloadStore::load_statements(start_snapshot, end_snapshot)` 返回窗口中的记录。每条记录调用 `extractScanAndMemoryFromBinaryPlan`；反序列化或算子提取失败会被本层静默跳过，其他记录继续处理。
3. 计划被反序列化为单棵 `ExplainOperator` 树。每个节点先以 `extractOperatorTypeFromName` 解析严格的 `Type_ID` 名称，再按类型提取当前节点指标，最后递归所有子节点。
4. `IndexLookUp` / `IndexReader` 从后代 `IndexRangeScan` 或 `IndexFullScan` 获取库表名，但使用父算子的执行时间和内存。`TableReader` 类似地从后代 `TableFullScan` / `TableRangeScan` 获取库表名；若子树含 `task_type` 为 `mpp` 的节点，则跳过该 TableReader。
5. `PointGet` / `BatchPointGet` 使用首个访问对象、当前节点执行时间和 1 字节默认内存。`IndexMerge` 查找一层全部为索引扫描的后代，逐个取得扫描时间和表名，再把父节点内存按扫描项数整除分摊。
6. 不识别的算子本身不产出指标，但仍递归其子节点，因此包装算子不会阻断后代扫描发现。某个受支持算子发生结构或时间解析错误时，错误向上传播并使整条语句被分析层跳过。
7. `AccumulateMetricsGroupByTableID` 对每个提取项查询表 ID。查不到表 ID 的项被跳过；频率小于零时扫描时间按零倍计算，而内存仍以原始负频率相乘，`ReadFrequency` 也保留负值。正常输入预期频率非负，调用者若可能产生负差值应在存储适配层校验。
8. 聚合完成后，代码分别求所有表扫描纳秒总和和内存总和。每表 `TableReadCost` 等于扫描时间占比加内存占比；任一总量为零时，对应分量为 0。单表同时拥有非零扫描和内存时，代价为 2.0，这一点由 Rust 端到端测试确认。
9. `SaveTableReadCostMetrics` 先读取 `latest_version` 并饱和加一，序列化全部指标，再按 `rows.chunks(1000)` 依次保存。所有批次成功后主入口返回聚合映射；任一存储或序列化错误会终止本轮并返回错误。

## 数据与状态

`Handle` 自身只有不可变的 `Arc<dyn WorkloadStore>`，没有缓存分析结果。一次调用的可变状态集中在局部 `HashMap<i64, TableReadCostMetrics>`：键是解析后的表 ID，值记录库表标识、累计扫描时间、累计内存、累计频率和最终代价。

`TableReadCostMetrics` 定义在 `metrics.rs`。库表名使用 `CIStr` 同时保存原始字符串 `O` 与小写字符串 `L`；本文件解析表 ID 时使用 `L`。扫描时间是 `Duration`，JSON 中由 `metrics.rs::duration_nanos` 序列化为纳秒 `u64`；内存和频率是 `i64`，代价是 `f64`。

聚合使用饱和算术保护部分溢出：扫描时间通过 `Duration::saturating_mul` 和 `saturating_add`，内存通过 `i64::saturating_mul` 和 `saturating_add`。总扫描时间转为 `u128` 求和；总内存仍以普通 `i64::sum` 汇总，在极端数据下可能溢出。版本号采用 `latest_version().saturating_add(1)`，到达 `u64::MAX` 后不会前进；存储实现需决定是否允许覆盖同版本。

访问对象可直接携带 `database/table`，也可通过 `dynamic` 表示动态分区对象。`extractTableNameFromAccessObject` 优先使用顶层任一非空字段；否则返回首个 table 非空的动态对象。一次算子只选择一个访问对象或动态对象，不会为同一访问对象中的多个动态表分别建指标。

## 依赖与调用关系

crate 内部依赖关系是 `handle.rs -> metrics.rs::TableReadCostMetrics/CIStr`；`cache.rs` 反向依赖本文件的 `WorkloadStore`，从存储加载 `SaveTableReadCostMetrics` 所写版本。`lib.rs` 重导出三者，使外部 crate 理论上可直接使用这些类型，但当前仓库搜索未发现生产 Rust 调用点。

RustCodeGraph 给出的关键边包括：

- `extractScanAndMemoryFromBinaryPlan -> extractMetricsFromOperatorTree`；后者也递归调用自身。
- `extractMetricsFromOperatorTree -> metric/checkTiFlashOperator/extractTableNameFromAccessObject/extractTableNameFromChildrenTableScan/extractTableNameFromIndexScan/extractPartialMetricsFromChildrenIndexMerge/extractOperatorTypeFromName/extractScanTimeFromExecutionInfo`。
- `analyzeBasedOnStatementStats -> AccumulateMetricsGroupByTableID`；后者调用 `InfoSchema::TableID`。
- 源码直接显示 `HandleTableReadCost -> analyzeBasedOnStatementStats -> SaveTableReadCostMetrics`，以及分析函数调用快照、语句加载和计划解析接口。

外部库只用于 JSON：`serde` 为计划与指标模型派生序列化/反序列化，`serde_json` 解析计划并序列化落盘指标。时间、集合和共享所有权均来自标准库。

## 错误处理与边界

公开存储与分析 API 统一使用 `Result<_, String>`，丢失了结构化错误类别。快照定位、语句加载、读取版本、指标序列化和任一批保存失败都会由 `?` 传播到 `HandleTableReadCost`。保存不是事务接口：如果前几批成功而后续批失败，调用者会收到错误，但本文件没有回滚已写批次。

计划层的关键失败条件包括：JSON 不合法；名称不符合严格的单下划线 `Type_ID` 格式；支持的扫描算子没有库表访问对象；IndexMerge 找不到索引扫描；执行信息含非空但不可解析的 `time:` 字段。执行信息查找顺序是 root basic、首个 root group、cop；前一来源解析失败后仍尝试后一来源，获得非零时长即成功。三类信息均为空时返回零时长；存在错误且最终仍为零时返回最后一次错误。

`parseDuration` 模拟 Go 正时长格式，支持 `ns`、`us`、`µs`、`ms`、`s`、`m`、`h` 以及复合片段和小数，`0` 单独有效。它不支持负号，使用 `f64` 累积后四舍五入到纳秒，并拒绝非有限值及大于 `u64::MAX` 纳秒的值。

需要特别注意的当前边界：计划解析只接受本地 JSON `ExplainOperator`，不接受 Go 所用的压缩 `BINARY_PLAN` protobuf；只解析一个根，不覆盖 Go 的 CTE 与 subquery 列表；TiFlash 检查递归有效，而同路径 Go 函数遗漏了递归结果返回，Rust 行为更符合函数意图；未知表 ID 和整条坏计划均被跳过且无日志。

## 并发与资源生命周期

`WorkloadStore` 要求 `Send + Sync`，并被 `Arc` 持有，所以多个 Handle 或线程可以共享同一存储实现。文件内部不创建线程、任务或通道，也不持有锁；并发控制全部留给具体 `WorkloadStore`。`MemoryStore` 测试替身通过互斥锁实现这一要求，但这不能证明生产存储的隔离级别。

每轮分析先完整加载 `Vec<StatementRecord>`，再顺序解析；指标行也先完整序列化为 `Vec<(i64, String)>`，然后分批写入。`batchInsertSize` 限制单次保存行数，不限制总内存。计划树通过同步递归遍历，极深的恶意 JSON 树存在栈深风险。

版本分配是“读取最新版本再加一”，没有比较交换或事务包裹。并发运行两轮分析时，两者可能选择同一新版本；原子性、冲突处理和批次可见性必须由 `WorkloadStore` 实现保证。传入 `SaveTableReadCostMetrics` 的起止时间目前参数名前带下划线且未使用，没有持久化作业生命周期信息。

## 与 Go 版本的对应关系

核心算法基本对应 `pkg/workloadlearning/handle.go`：七天窗口、快照区间语句、算子类型分派、频率放大、按表 ID 聚合、扫描占比加内存占比、1000 行批写，以及 `DBNameExtractor` 的 visitor 控制值均保留。`handle_test.rs` 也复刻了 Go 的保存与累加用例，并新增可执行的计划树归一化、Go 复合 duration、执行信息回退和 AST 去重测试。

Rust 当前不是 Go 环境接线的等价替换，主要差异如下：

- Go `Handle` 持有 `DestroyableSessionPool`，直接查询 `HIST_SNAPSHOTS` / `HIST_TIDB_STATEMENTS_STATS` 并在事务中插入 `mysql.tidb_workload_values`；Rust 持有抽象 `WorkloadStore`，没有 SQL、内部来源标记、会话回收/销毁、事务提交或 plan cache 设置。
- Go 版本号来自事务 `StartTS()`，并保存 category/type/table ID/value；Rust 版本来自 `latest_version + 1`，存储接口只接收 table ID 和 JSON。Rust 的 `feedbackCategory`、`tableReadCost` 因而尚未接入持久化协议。
- Go 解压真实 binary plan、反序列化 `tipb.ExplainData`，校验 `DiscardedDueToTooLong` / `WithRuntimeStats`，并遍历 main、CTE、subquery；Rust 把 `binary_plan` 直接当 JSON 单根 `ExplainOperator`。
- Go 遇到分析错误以日志记录并返回/跳过，`HandleTableReadCost` 不返回结果；Rust 使用 `Result<String>`，但对单条坏计划和未知表保持静默跳过。
- Rust 对频率与局部累计使用部分饱和运算，Go 直接相乘相加；Rust 对负频率的扫描与内存处理不一致，是适配层需要防御的输入边界。
- Go `DBNameExtractor` 操作真实 `ast.Node`，Rust 只操作本文件的 `Node` 枚举，因此目前不能直接访问解析器 AST。

所以本文件应被理解为保留主要计算语义、以可测试抽象替代 TiDB 基础设施的 Rust 移植层；将其接入真实服务器仍需实现存储、计划解码与调度桥接，不能仅调用现有构造函数即宣称完成 Go 等价迁移。

## 扩展指南

- 新增算子支持应修改 `extractMetricsFromOperatorTree`，必要时增加专用的表名/时间提取辅助函数，并在独立的 `handle_test.rs` 添加正常、缺字段、嵌套和错误传播用例。不要把 Rust 测试内嵌到生产源文件。
- 接入真实 TiDB binary plan 时，应在 `extractScanAndMemoryFromBinaryPlan` 的边界引入与 Go `plancodec.Decompress + tipb.ExplainData` 等价的解码，并显式覆盖 main、CTE、subquery、过长丢弃和无 runtime stats；若依赖外部 Rust crate，须按仓库规则在上游仓库移植并使用已发布 tag，不能用本地 patch 或 vendor 副本。
- 实现生产 `WorkloadStore` 时，需要定义版本唯一性、批写原子性、失败恢复、时间窗口快照选择和真实表 ID 查找语义。若继续使用当前多批接口，至少保证同版本重复写入的幂等性或提供清理策略。
- 若要严格对齐 Go 持久化格式，应决定 category/type 是否加入 `save_metrics` 参数、版本是否改用事务时间戳，以及起止时间如何记录；对应修改需同时检查 `cache.rs` 的加载协议和 `cache_test.rs`。
- 新增输入校验最适合放在 `analyzeBasedOnStatementStats` 或 `AccumulateMetricsGroupByTableID` 边界。尤其应明确拒绝负频率，而不是延续当前扫描归零但内存为负的混合结果。
- 性能扩展应先处理全量 `Vec` 和深递归问题：可将 `load_statements` 改为流式接口、边解析边聚合，并为计划深度/节点数设置限制。并行解析前必须确认存储调用、错误语义和确定性版本写入不被破坏。
- `DBNameExtractor` 若需进入真实 SQL 主链，应替换或适配为解析器真实 AST visitor，并在独立测试中覆盖未限定表名、跨库引用、大小写和重复 schema。

## 验证依据

- 生产源码：`pkg/workloadlearning/handle.rs`，核对全部 4 个常量、`StatementRecord`、`WorkloadStore`、`InfoSchema`、`Handle`、计划数据结构、分析/聚合/解析辅助函数、`Node` 与 `DBNameExtractor`。
- crate 边界：`pkg/workloadlearning/Cargo.toml` 与 `pkg/workloadlearning/lib.rs`，确认库入口、依赖、模块加载和公开重导出；`pkg/workloadlearning/metrics.rs`、`cache.rs` 用于核对指标 JSON 格式及保存后的缓存消费者。
- Rust 测试：`pkg/workloadlearning/handle_test.rs` 验证新版本保存、频率累加、单表代价 2.0、复合 Go duration、执行信息回退和 schema 去重；`pkg/workloadlearning/cache_test.rs` 提供额外保存到缓存读取链的调用证据。
- Go 对照：`pkg/workloadlearning/handle.go` 与 `handle_test.go`，核对系统会话/SQL/事务接线、真实 binary plan 解码、算子算法、批写和原始回归意图。
- RustCodeGraph：`status` 显示索引包含目标模块；`files --filter pkg/workloadlearning` 定位 12 个 Go/Rust 文件；`query` 区分 Go/Rust 同名符号；`callers/callees` 确认计划解析、算子辅助函数、聚合与 `InfoSchema::TableID` 的关键边。由于本次 `explore` 与 `node --file` 未返回正文，源码内容按技能规则由直接文件读取补足。
- 仓库引用搜索：公开入口在 Rust 侧仅被 `handle_test.rs` 和 `cache_test.rs` 使用，未发现生产服务器接线。该结论只覆盖当前检出中的静态引用，不证明动态加载或仓库外调用不存在。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定命令，要求本文件存在且恰有 11 个固定二级标题；同时人工复核结论均区分 Rust 当前事实与 Go 目标语义。
