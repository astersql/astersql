# `pkg/executor/internal/exec/indexusage.rs`

## 文件定位

本文件属于 `astersql-executor-internal-exec` crate；crate 根 `pkg/executor/internal/exec/lib.rs` 通过 `pub mod indexusage` 暴露它，并把同目录的 `indexusage_test.rs` 作为 `#[cfg(test)]` 独立测试模块编译。`Cargo.toml` 将 Go 对照包标为 `pkg/executor/internal/exec`，当前依赖表只在 Windows 目标下声明执行器相关 workspace crate；本文件自身仅直接使用标准库的 `HashMap`、`Arc` 和 `Mutex`。

它实现一套执行器侧索引使用量采样模型：把 Coprocessor 扫描或 Point Get 得到的请求数、访问行数与表统计行数组合成 `IndexUsageSample`，再交给语句级收集器。需要特别区分“本文件提供的具体类型”和“执行器主链中的同名接口”：RustCodeGraph 与 `rg` 复核表明，生产 Rust 代码中的 `pkg/executor/point_get.rs::IndexUsageReporter` 和 `pkg/executor/builder.rs::IndexUsageReporter` 是各自定义的 trait，目前没有实现适配到本文件的 `IndexUsageReporter`；对本文件具体 API 的 Rust 调用目前只见于 `indexusage_test.rs`。因此它是已实现、可测试的 Go 语义移植单元，但尚不能据此声称已接入完整 Rust 执行主链。

## 核心职责

- 用 `TableInfo`、`IndexInfo`、`TableStats` 和若干 trait 表达上报所需的最小元数据边界，避免把完整执行器/统计系统耦合进本 crate。
- 用 `getClusterIndexID` 判定表句柄能否归入某个聚簇索引：整数主键句柄映射到索引 ID `0`，公共句柄映射到主索引 ID，隐藏 row ID 表不产生句柄索引记录。
- Coprocessor 路径通过 `RuntimeStatsCollection::GetCopCountAndRows(plan_id)` 读取运行时计数，只在存在非伪统计且请求数或访问行数至少一项非零时调用收集器。
- Point Get 路径直接接收请求数和行数；缺少真实统计时仍上报，并以 `i32::MAX` 作为表行数兜底，使非零访问落入最小非零比例桶，保持 Go 行为。
- 提供 `InMemoryIndexUsageCollector` 作为线程安全的观测实现，服务于独立单元测试；它不是持久化或全局统计后端。

## 主要符号

- `PSEUDO_VERSION: u64 = 0`：本文件的伪统计哨兵；`getTableRowCount` 遇到该版本即返回 `None`。
- `IndexInfo { id, primary }` 与 `TableInfo { id, pk_is_handle, is_common_handle, indices }`：聚簇索引判定所需的最小表/索引元数据。它们不是完整的 TiDB 元数据模型。
- `Table`：要求实现者可在线程间共享，并提供 `Meta()`；`GetPhysicalID()` 默认返回 `None`，非分区表因此回退到逻辑表 ID。
- `TableStats` 与 `UsedStatsInfo`：按物理表 ID 查询本语句实际使用的统计版本及实时行数。
- `RuntimeStatsCollection`：按 `i32` 计划 ID 返回 `(Coprocessor 请求数, 访问行数)`。
- `IndexUsageSample`：四个 `u64` 字段依次表示查询次数、KV 请求数、访问行数和表行数。本文件生成的样本总把 `query_total` 设为 `0`；语句级查询次数由更高层聚合语义负责。
- `StmtIndexUsageCollector::Update(table_id, index_id, sample)`：最终写入边界；本文件不规定合并、持久化或异步刷新策略。
- `InMemoryIndexUsageCollector`：按 `(table_id, index_id)` 保存样本序列；`samples` 返回克隆快照。
- `IndexUsageReporter`：持有收集器、运行时统计源和可选表统计映射，公开 Cop、Point Get 及句柄便利入口。
- `goUint64(i64) -> u64`：用 Rust `as` 转换复现 Go 的 `uint64(int64)` 模运算语义，包括负数变为大无符号数。
- `getClusterIndexID(&TableInfo) -> Option<i64>`：公开的聚簇索引 ID 解析函数。

## 执行流程

Cop 句柄入口从 `ReportCopIndexUsageForHandle` 开始：先调用 `getClusterIndexID(table.Meta())`；没有聚簇句柄时立即返回。命中后进入 `ReportCopIndexUsageForTable`，以 `TableInfo.id` 作为记录归属的逻辑表 ID，以 `Table::GetPhysicalID()` 或逻辑表 ID作为统计基数对应的物理表 ID。随后 `ReportCopIndexUsage` 调用 `getTableRowCount(physical_table_id)`；统计映射缺失、条目缺失或版本为 `PSEUDO_VERSION` 时跳过。统计有效时按 `plan_id` 取得 KV 请求数和访问行数，两者同时为零则跳过，否则经 `goUint64` 转换并调用 `StmtIndexUsageCollector::Update`。

Point Get 句柄入口 `ReportPointGetIndexUsageForHandle` 同样先解析聚簇索引 ID，然后把逻辑表 ID、调用方给出的物理表 ID、请求数和行数转交给 `ReportPointGetIndexUsage`。后者尝试读取真实表行数；失败时取 `i32::MAX`，且没有 Cop 路径的“双零跳过”分支，最后无条件调用 `Update`。这一差异是显式兼容行为，而不是遗漏。

内存收集器的 `Update` 获取互斥锁，把样本追加到 `(table_id, index_id)` 对应的向量；`samples` 再次加锁并克隆指定向量。锁获取失败（包括锁中毒）时，写入被静默丢弃、读取返回空向量。

## 数据与状态

`IndexUsageReporter` 本身不维护累计计数，只保存三个共享依赖：`Arc<dyn StmtIndexUsageCollector>`、`Arc<dyn RuntimeStatsCollection>` 和可选的 `Arc<dyn UsedStatsInfo>`。因此样本是否合并、何时刷新以及生命周期多长由注入对象决定；Reporter 的方法只读取依赖并发出一次 `Update`。

逻辑表 ID 决定索引使用记录归属，物理表 ID 只用于选择表行数基数。这使分区扫描仍累计到逻辑表/索引键，同时按分区大小计算访问比例；`indexusage_test.rs::cop_report_uses_physical_table_stats_and_skips_zero_or_pseudo_stats` 用逻辑 ID `10`、物理 ID `20` 验证了该不变量。

`getClusterIndexID` 的返回值也承载“是否应记录”：`pk_is_handle` 优先并返回 `Some(0)`；否则公共句柄搜索第一个 `primary` 索引。若公共句柄元数据中找不到主索引，仍返回 `Some(0)`，对应 Go 中默认 `idxID` 为零后返回 `(0, true)` 的行为；只有两种句柄标志均为假时返回 `None`。

## 依赖与调用关系

下游依赖均由 trait 注入：Reporter 调用 `Table::Meta/GetPhysicalID`、`UsedStatsInfo::GetUsedInfo`、`RuntimeStatsCollection::GetCopCountAndRows` 和 `StmtIndexUsageCollector::Update`；`getClusterIndexID` 只遍历 `TableInfo.indices`。源码没有 I/O、时钟、任务调度或外部 crate 调用。

RustCodeGraph 对 `ReportCopIndexUsage`、`ReportPointGetIndexUsage`、`getTableRowCount` 和 `getClusterIndexID` 的探索结果确认了上述文件内调用边，并识别到 `indexusage_test.rs` 的覆盖调用。进一步用精确文本检索检查 Rust 生产文件，只发现目标文件内部与独立测试对这些具体 PascalCase API 的引用；`point_get.rs` 的实际关闭流程则调用它自己的注入接口 `report_index/report_handle`，`builder.rs` 也只有自己的空标记 trait 和构建依赖边界。安全接线需要新增明确适配器，不能把同名符号当作已有调用关系。

Go 生产侧则已广泛接线：RustCodeGraph 显示 `indexusage.go` 被 `batch_point_get.go`、`builder.go`、`distsql.go`、`index_merge_reader.go`、`point_get.go` 等使用；这描述的是 Go 主链，不等价于当前 Rust 主链状态。

## 错误处理与边界

本文件 API 不返回 `Result`，所有缺失信息都通过提前返回或默认值处理。Cop 路径对无统计、伪统计和完全零访问采取“不上报”；Point Get 对无统计或伪统计采用 `i32::MAX` 后继续上报。调用方传入的负请求数、负访问行数或负表行数不会报错，而会按 Go 强制转换语义映射到相应 `u64` 大值；测试用 `(-1, -2)` 验证得到 `u64::MAX` 与 `u64::MAX - 1`。

`InMemoryIndexUsageCollector` 对互斥锁中毒不传播错误：`Update` 丢弃该样本，`samples` 返回空集合。这个策略适合测试辅助对象，但若未来复用于生产诊断，应重新评估静默丢数是否可接受。

公共句柄但缺失主索引元数据时返回 ID `0` 是刻意复制 Go 默认值的兼容边界；它可能掩盖不完整元数据，扩展时不可擅自改为 `None`。`Table::GetPhysicalID` 的默认 `None` 也意味着调用方若忘记为分区表覆写，将按整个逻辑表统计计算比例。

## 并发与资源生命周期

所有依赖 trait 都要求 `Send + Sync`，Reporter 通过 `Arc` 共享所有依赖，因此可安全地被多个所有者持有；Reporter 方法只借用 `&self`，无局部可变状态。具体线程安全仍依赖 trait 实现遵守其契约。

内存收集器用单个 `Mutex<HashMap<...>>` 串行化所有键的追加与读取。锁范围只覆盖一次映射查找/追加或克隆，不跨越外部回调；不存在本文件创建的线程、异步任务、通道、事务或显式清理协议。最后一个 `Arc` 释放时依赖对象自然析构，Reporter 没有 `Drop` 行为。

读取样本会克隆完整向量，样本数增加时具有线性时间和额外内存成本；单锁也可能成为高并发热点。这些成本目前限于测试收集器，生产收集器的并发与资源策略由 `StmtIndexUsageCollector` 实现决定。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/internal/exec/indexusage.go`。Rust 的 `IndexUsageReporter` 三个依赖分别对应 Go 的 `reporter`、`runtimeStatsColl` 和 `statsMap`；两个 Cop 便利入口、两个 Point Get 入口、`getTableRowCount` 及 `getClusterIndexID` 保持同样的分支顺序和逻辑/物理表 ID 分工。

Go 用 `statistics.PseudoVersion`，Rust 在本文件用值为 `0` 的 `PSEUDO_VERSION`；Go 用 `indexusage.NewSample`，Rust显式构造等价的四字段 `IndexUsageSample`。Go 的 `math.MaxInt32` 对应 Rust 的 `i32::MAX as i64`。Go 的 `uint64(...)` 转换由 `goUint64` 明确保留，而不是对负数做校验或饱和。

Rust 为可独立测试而引入了最小 trait/数据模型及 `InMemoryIndexUsageCollector`，这些并非 Go 文件中的生产类型本体。Rust 独立测试覆盖核心分支；Go 测试还覆盖真实 SQL、预处理语句、分区、全局索引、禁用执行信息收集和多种聚簇/非聚簇索引场景。由于当前 Rust 具体 Reporter 尚未适配到 Rust 执行器主链接口，Go 集成测试所证明的端到端接线不能直接算作 Rust 的端到端证据。

## 扩展指南

- 新增采样字段时，应同时修改 `IndexUsageSample`、所有构造点、`StmtIndexUsageCollector` 的实现和 `indexusage_test.rs` 断言，并核对 Go 的 `indexusage.Sample/NewSample` 聚合含义，尤其不要在本层擅自把 `query_total` 改成 `1`。
- 新增统计可用性规则时，集中修改 `getTableRowCount`，分别补充 Cop“跳过”和 Point Get“兜底继续”的测试；两条路径不能无意合并。
- 扩展表类型或句柄模式时，修改 `getClusterIndexID`，在独立测试中覆盖优先级、缺失主索引和 row ID 表；若改变公共句柄缺失主索引时的 ID `0` 行为，必须先确认 Go 兼容要求。
- 接入 Rust 生产主链时，应在 `point_get.rs::IndexUsageReporter` 的具体实现及 `PointGetDependencies::build_index_usage_reporter` 构建路径中增加显式适配；分布式扫描还需把 `builder.rs` 的报告器边界接到本文件的 Cop API。必须避免仅因名称相同就强制转换两个无继承关系的类型/trait。
- 若生产化 `InMemoryIndexUsageCollector`，需解决锁中毒可观测性、无界样本增长、全局锁竞争和读取全量克隆问题；否则保持它只作为测试辅助。
- Rust 测试逻辑继续放在同目录独立文件 `pkg/executor/internal/exec/indexusage_test.rs`，不要内嵌到生产源文件；端到端接线完成后还应补充执行器侧独立集成测试，对齐 `indexusage_test.go` 的真实 SQL 场景。

## 验证依据

- Rust 源码：`pkg/executor/internal/exec/indexusage.rs`，核对全部 246 行、公开数据类型/trait、Reporter 方法、`goUint64` 和 `getClusterIndexID`。
- crate 边界：`pkg/executor/internal/exec/lib.rs` 与 `pkg/executor/internal/exec/Cargo.toml`，核对模块导出、独立测试模块、Go 包元数据及条件依赖。
- Rust 独立测试：`pkg/executor/internal/exec/indexusage_test.rs`，核对聚簇索引三类分支、公共句柄缺主索引、物理表统计、伪统计、零访问、Point Get 兜底与负数转换。
- Go 对照：`pkg/executor/internal/exec/indexusage.go` 和 `pkg/executor/internal/exec/indexusage_test.go`，核对逐函数语义及真实 SQL、分区、全局索引、开关与聚簇索引场景。
- Rust 上游边界：`pkg/executor/point_get.rs`（trait、构建注入、`Close` 中的 `report_index/report_handle`）与 `pkg/executor/builder.rs`（同名 trait 和构建委托），用于确认当前适配缺口。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/internal/exec` 确认目标、Go 对照、crate 根和独立测试均已索引；`explore`、`query IndexUsageReporter/ReportCopIndexUsage/ReportPointGetIndexUsage/getClusterIndexID` 与目标文件 `node` 查询用于核对符号、文件内调用和测试调用。图查询对常见同名 `reporter` 产生跨模块噪声，因此又以限定 `*.rs` 的精确引用检索确认本文件具体 API 尚无测试外 Rust 调用。
- 结构验证按任务指定命令执行；本任务为纯文档分析，按计划不运行 Cargo。
