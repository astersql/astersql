# `pkg/planner/cardinality/trace.rs`

## 文件定位

`trace.rs` 属于 `astersql-planner-cardinality` crate，是基数估算过程中记录“本语句实际触碰了哪些未完整加载统计”的辅助模块。crate 根 `pkg/planner/cardinality/lib.rs` 以私有 `mod trace` 装配该文件，再通过 `pub use trace::*` 导出其公开符号；因此调用者仍从 cardinality crate 的统一命名空间使用它。

它不计算基数，也不触发统计加载。它位于“估算代码取得列/索引统计”与“`StatementContext` 保存诊断状态”之间：`row_count_column.rs`、`row_count_index.rs` 和 `selectivity.rs` 在读取统计时调用 `recordUsedItemStatsStatus`，后者把非完整加载状态按 `(table_id, item_id, is_index)` 记录到语句上下文。文件头注释把用途限定为用户查询规划期间的诊断记账；函数本身没有另行判断语句类型，是否处于用户查询路径由调用点保证。

## 核心职责

本文件只承担三项职责：

1. 用 `UsedStatsItem<'a>` 显式表达“列或索引”以及各自“对象存在或缺失”四种状态，替代 Go `any` 加类型断言的动态分派。
2. 在 `recordUsedItemStatsStatus` 中过滤无需诊断的对象：非正 ID（包括 `_tidb_rowid` 的 `-1`）直接忽略，已经 `IsFullLoad()` 的统计也直接忽略。
3. 对剩余对象产生稳定的状态字符串并交给 `StatementContext::RecordUsedStatsLoadStatus`：存在对象使用自身 `StatsLoadedStatus::StatusToString()`；缺失但已 ANALYZE 的对象记为默认状态的 `"unInitialized"`；缺失且没有已分析证据的对象记为 `"missing"`。

此模块不会修改 `Column`、`Index` 或 `ColAndIdxExistenceMap`，不会创建加载任务，也不会返回估算结果。其产物是当前语句的诊断侧状态，而不是优化器决策输入。

## 主要符号

- `pub enum UsedStatsItem<'a>`：借用式联合类型。`Column(Option<&statistics::Column>)` 与 `Index(Option<&statistics::Index>)` 同时编码对象种类和可空性；生命周期 `'a` 保证函数只观察上游统计对象，不取得所有权。四个枚举分支也决定写入键中的 `is_index` 和是否进入缺失判定。
- `pub fn recordUsedItemStatsStatus(sctx: &dyn planctx::PlanContext, stats: UsedStatsItem<'_>, table_id: i64, id: i64)`：唯一执行入口。`sctx` 只用于取得 `SessionVars.StmtCtx`，`table_id` 与 `id` 标识物理表及列/索引，`stats` 提供类型、缺失性和加载状态。函数返回 `()`，所有可见效果都是更新语句上下文。
- `statistics::StatsLoadedStatus::{IsFullLoad, StatusToString}`：存在对象的过滤与字符串化依据。当前字符串包括 `unInitialized`、`allLoaded`、`allEvicted`、`unknown`；其中 `allLoaded` 会在本函数内被过滤，不应由正常路径写入。
- `stmtctx::cache_downcast_ref::<statistics::ColAndIdxExistenceMap>`：从类型擦除的 `ColAndIdxStatus` 中恢复存在性映射。代码兼容缓存值直接保存映射和保存 `Box<...>` 两种形态。
- `StatementContext::RecordUsedStatsLoadStatus`：最终落点。它把值插入由互斥锁保护的 `HashMap<(i64, i64, bool), String>`；同一键再次记录会覆盖旧值。

## 执行流程

1. 检查 `id`。当 `id <= 0` 时立即返回。这覆盖 Go 注释中特别指出的 `_tidb_rowid`（`id == -1`），也避免把缺省映射产生的 `0` 当作真实列 ID。
2. 匹配 `UsedStatsItem`，一次性得到 `(is_index, missing, load_status)`：列对应 `false`，索引对应 `true`；`None` 表示缺失，`Some` 则借用对象的 `StatsLoadedStatus`。
3. 如果对象存在且 `load_status.IsFullLoad()` 为真，立即返回。完整加载对象无需出现在“部分/缺失统计”诊断中。
4. 如果对象缺失，先尝试通过 `GetUsedStatsInfo(false)` 读取已经附着于本语句的表级记录。这里传 `false`，所以本函数不会为了追踪而初始化 `UsedStatsInfo`。链式查询要求表 ID 已有记录且 `ColAndIdxStatus` 已设置，然后分别尝试把缓存值向下转换成 `ColAndIdxExistenceMap` 或 `Box<ColAndIdxExistenceMap>`。
5. 若存在性映射的 `HasAnalyzed(id, is_index)` 为真，使用默认 `StatsLoadedStatus` 的 `StatusToString()`，当前得到 `"unInitialized"`；否则使用字面值 `"missing"`。随后调用 `RecordUsedStatsLoadStatus` 并返回。
6. 对存在但未完整加载的对象，将其实际状态字符串（例如 `"allEvicted"` 或 `"unknown"`）写入同一个语句级映射。

生产调用顺序也说明了它在估算链中的位置：`GetRowCountByColumnRanges` 在检查 `ColumnStatsIsInvalid` 之前记录列状态；`GetRowCountByIndexRanges` 在快速全范围返回和 `IndexStatsIsInvalid` 判断之前记录索引状态；`Selectivity` 在找不到非隐藏列的统计对象时显式传入 `UsedStatsItem::Column(None)`。因此即使后续退回伪统计，缺失或驱逐状态仍可被观察。

## 数据与状态

输入统计对象均为不可变借用。本函数读取 `Column::StatsLoadedStatus` 或 `Index::StatsLoadedStatus`，不复制直方图、TopN 等大对象。对于缺失对象，它只读取 `UsedStatsInfoForTable::ColAndIdxStatus` 中的分析存在性信息。

输出键为 `(table_id, id, is_index)`。第三个布尔值使相同数值 ID 的列和索引不会冲突；`table_id` 使用 `HistColl::PhysicalID` 的调用点值，使分区或物理表之间保持隔离。输出值是拥有所有权的 `String`，避免把统计对象中的借用带入语句上下文。

状态含义必须区分：

- 不写入：`id <= 0`，或对象存在且已经全量加载。
- `unInitialized`：对象指针缺失，但已附着的 `ColAndIdxExistenceMap` 证明该列/索引曾被 ANALYZE。
- `missing`：对象缺失，且没有“已分析”的证据；这也包括表级记录、存在性映射或类型转换缺失的情况。
- `allEvicted` / `unknown` 等：对象存在但未全量加载，直接保留 `StatsLoadedStatus` 的语义。

## 依赖与调用关系

直接上游调用边为：

- `pkg/planner/cardinality/row_count_column.rs::GetRowCountByColumnRanges` → `recordUsedItemStatsStatus`，传入 `HistColl::GetCol` 的结果、物理表 ID 和列信息 ID。
- `pkg/planner/cardinality/row_count_index.rs::GetRowCountByIndexRanges` → `recordUsedItemStatsStatus`，传入 `HistColl::GetIdx` 的结果、物理表 ID 和索引 ID。
- `pkg/planner/cardinality/selectivity.rs::Selectivity` 内的缺失列分支 → `recordUsedItemStatsStatus`，传入 `Column(None)`。

直接下游依赖为 `planctx::PlanContext::GetSessionVars`、`StatementContext::GetUsedStatsInfo`、`UsedStatsInfo::GetUsedInfo`、`ColAndIdxExistenceMap::HasAnalyzed`、`StatsLoadedStatus::{IsFullLoad, StatusToString}`、`stmtctx::cache_downcast_ref` 和 `StatementContext::RecordUsedStatsLoadStatus`。

`pkg/planner/cardinality/Cargo.toml` 表明这些路径分别来自本地 `astersql-planner-planctx`、`astersql-sessionctx-stmtctx`、`astersql-statistics` 和 `astersql-sessionctx-variable` 依赖；`lib.rs` 用 `planctx`、`stmtctx`、`statistics`、`variable` 再导出模块保持 Go 风格路径。Cargo manifest 没有为此文件声明专用 feature 或条件编译项，`trace` 模块始终随 library 编译；只有独立的 `trace_test.rs` 受 `#[cfg(test)]` 控制。

## 错误处理与边界

函数没有 `Result` 返回值，也不产生可恢复业务错误。无效或信息不足的输入采用保守诊断行为：非正 ID 静默忽略；缺少表记录、`ColAndIdxStatus` 或可识别的缓存类型时，不猜测已分析状态，而是记录 `"missing"`。

类型边界由 `UsedStatsItem` 在编译期封闭。与 Go 的 `any` 不同，Rust 调用者不能传入第三种统计对象；也不存在 Go `switch` 未命中后 `loadStatus == nil` 而继续执行的动态类型路径。缓存向下转换则仍可能失败，但失败被 `Option` 链吸收，不会 panic。

需要注意两个运行时边界：`GetSessionVars` 的可用性由 `PlanContext` 契约保证；`RecordUsedStatsLoadStatus` 获取互斥锁时使用 `expect("used-stats status lock poisoned")`，若先前持锁线程 panic 导致锁中毒，本次记录也会 panic。除此之外，本函数没有 I/O、网络、事务或显式分配失败处理。

## 并发与资源生命周期

`UsedStatsItem` 仅在调用期间借用统计对象，函数结束后不保留引用。对缺失项查询得到的 `Arc<UsedStatsInfo>` 和克隆的 `CacheValue` 也只作为局部读取句柄存在；本文件不启动任务、不建立通道、不持有事务，也不管理异步统计加载生命周期。

最终状态保存在 `StatementContext::plannerUsedStatsLoadStatus` 的 `Mutex<HashMap<...>>` 中。单次写入只在 `insert` 期间持锁，读取接口 `UsedStatsLoadStatus()` 会加锁并克隆整个映射。锁保证并发写入的数据竞争安全，但不提供事件历史：相同键遵循最后一次写入覆盖前值的映射语义。表级 `UsedStatsInfo` 的读取与最终独立映射写入不是一个原子事务，因此扩展代码不应假设两者构成一致性快照。

状态生命周期跟随当前 `StatementContext`。测试和 mock session 通过 `UsedStatsLoadStatus()` 读取它；本文件本身不负责跨语句复制、清空或输出到日志。

## 与 Go 版本的对应关系

Rust 的早退规则、列/索引区分、FullLoad 过滤以及 `unInitialized`/`missing`/实际加载状态三类结果，直接对应 `pkg/planner/cardinality/trace.go::recordUsedItemStatsStatus`。`UsedStatsItem` 的 `Option<&Column/Index>` 精确保留了 Go 中“动态类型是 `*Column` 或 `*Index`，但指针可为 nil”的四种有效输入。

当前实现也存在必须如实保留的接线差异。Go 函数会调用 `GetUsedStatsInfo(true)`，并在表记录不存在时通过 `statsutil.GetTblInfoForUsedStatsByPhysicalID` 创建带名称和表信息的 `UsedStatsInfoForTable`；随后状态直接写进该表记录的 `ColumnStatsLoadStatus` 或 `IndexStatsLoadStatus`。Rust 函数使用 `GetUsedStatsInfo(false)`，不创建表记录、不解析表元数据，而是将状态写入 `StatementContext::plannerUsedStatsLoadStatus` 独立映射。它只在表记录已经由其他路径附着时借用其中的 `ColAndIdxStatus` 判断是否已分析。

这个差异意味着当前 Rust 代码已覆盖状态判定和语句级可观察性，但不能由本函数单独保证 Go 版本的表名、`TblInfo` 及表内状态映射都被建立。`pkg/planner/core/stats/stats.rs` 中 Go 风格的 `LoadTableStats` 接线仍主要保留在注释块，进一步佐证不应把完整 Go 表级记录流程宣称为已迁移。

测试语义也有对应关系：`trace_test.rs` 验证已分析但缺失得到 `unInitialized`、未分析缺失得到 `missing`、非正 ID 被忽略，以及 FullLoad 不写而 AllEvicted 被记录；`pkg/planner/core/main_test.rs` 另从实际 `DeriveStats` 路径验证缺失列状态可被语句上下文观察。Go 同目录没有独立 `trace_test.go`，相关集成意图可在 `selectivity_test.go` 的真实 session 场景中找到。

## 扩展指南

新增统计对象种类时，应先扩展 `UsedStatsItem`，再在唯一 `match` 中明确其 `is_index` 键语义、缺失表示和加载状态来源；不要用无法区分对象种类的通用 `Option` 替代现有枚举。若新的对象不能用布尔 `is_index` 表达，则必须同步演进 `StatementContext` 的键类型和所有消费者，而不是复用一个会冲突的布尔值。

修改状态分类时，应保持三项不变量：非正 ID 不进入诊断映射；FullLoad 不产生噪声；“已分析但对象缺失”与“没有已分析证据”继续可区分。任何新增字符串还要检查读取和格式化端是否依赖固定集合。

若要补齐 Go 的表级记录语义，接入点应围绕 `GetUsedStatsInfo`、表信息解析和 `UsedStatsInfoForTable`，并先确认与当前独立 `plannerUsedStatsLoadStatus` 映射的单一事实来源，避免双写漂移。该改动跨越本文件与 planner core/stmtctx，不应在纯局部重构中顺手完成。

测试必须继续放在独立的 `pkg/planner/cardinality/trace_test.rs`，不要内嵌到生产文件。新增分支至少覆盖 Column/Index、Some/None、FullLoad/非 FullLoad、存在性缓存的直接值与 `Box` 形态，以及重复键覆盖。涉及实际规划链时同步扩展 `pkg/planner/core/main_test.rs` 或对应 cardinality 调用者的独立测试。主要兼容风险是状态字符串或键语义变化影响诊断输出；主要性能风险是在热估算路径增加额外锁、克隆或表元数据查询。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`node --file pkg/planner/cardinality/trace.rs` 返回完整 96 行源码及该文件被 cardinality 相关文件使用的索引信息。
- RustCodeGraph 符号查询：`query recordUsedItemStatsStatus --kind function` 定位 Rust 与 Go 同名实现，`query UsedStatsItem` 定位枚举及两个成员。对 Rust 函数执行 `callers`/`callees` 未返回静态边，因此按技能规则用定向源码搜索补齐调用关系。
- RustCodeGraph 源码证据：读取了 `row_count_column.rs::GetRowCountByColumnRanges`、`row_count_index.rs::GetRowCountByIndexRanges`、`selectivity.rs` 的缺失列分支、`stmtctx.rs::{RecordUsedStatsLoadStatus, GetUsedStatsInfo, UsedStatsInfoForTable}`、`table.rs::ColAndIdxExistenceMap::HasAnalyzed` 和 `histogram.rs::StatsLoadedStatus::{IsFullLoad, StatusToString}`。
- crate 与装配证据：读取 `pkg/planner/cardinality/Cargo.toml` 和 `pkg/planner/cardinality/lib.rs`，确认依赖别名、模块始终装配、公开再导出以及测试文件的独立 `#[cfg(test)]` 声明。
- Go 对照：读取 `pkg/planner/cardinality/trace.go`，并用定向搜索核对 `row_count_column.go`、`row_count_index.go`、`selectivity.go` 的对应调用点；读取 `pkg/planner/core/stats/stats.rs` 的迁移注释以限定表级 UsedStats 接线现状。
- 测试证据：读取 `pkg/planner/cardinality/trace_test.rs`，并核对 `pkg/planner/core/main_test.rs` 的实际规划链状态断言及 `pkg/testkit/mockstore.rs` 的状态读取接口。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文档存在且恰好包含 11 个固定二级章节，并人工检查没有修改 Rust、Go、Cargo 或只读 `plan.md`。
