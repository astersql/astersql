# `pkg/planner/core/operator/logicalop/logical_mem_table.rs`

## 文件定位

本文件定义 Rust 规划器中的逻辑内存表扫描节点 `LogicalMemTable`，对应 Go 同目录文件中的同名类型。这里的“内存表”是由 TiDB/AsterSQL 进程或远端诊断接口动态提供的系统、诊断和虚拟表，而不是普通持久化表；典型对象包括 `INFORMATION_SCHEMA`、慢查询和语句摘要表。节点通过 `LogicalPlan` trait 进入逻辑优化阶段，再由物理规划代码转换为 `PhysicalMemTable`。

crate 边界由同目录 `Cargo.toml` 声明：包名为 `astersql-planner-core-operator-logicalop`，库入口是 `lib.rs`；`lib.rs` 的 `mod logical_mem_table` 与 `pub use logical_mem_table::*` 将本文件的类型纳入逻辑算子公共 API。该 crate 直接依赖 `expression`、`base`、`model`、`parser_ast`、`property`、`statistics` 等规划与元数据 crate，没有为本文件设置条件 feature。

当前 Rust 接线状态需要谨慎理解：代码搜索没有发现生产 Rust 文件直接用结构体字面量或构造函数创建 `LogicalMemTable`，唯一直接构造点在 `logical_mem_table_test.rs`；不过 `pkg/planner/cascades/pattern/pattern.rs` 已将该类型识别为 `OperandMemTableScan`，`pkg/planner/cascades/old/implementation_rules.rs::ImplMemTableScan` 和 `pkg/planner/core/operator/physicalop/physical_mem_table.rs::ExhaustPhysicalPlans4LogicalMemTable` 已提供下游物理化路径。因此，本文只确认“逻辑节点及下游消费已存在”，不声称 Rust SQL plan builder 已完成上游构造接线。

## 核心职责

`LogicalMemTable` 集中承担五项职责：

1. 以 `Init` 把通用 `LogicalSchemaProducer` 初始化成类型名为 `MemTableScan` 的逻辑计划节点。
2. 以 `PredicatePushDown` 把可下推谓词交给可选的 `MemTablePredicateExtractor`，并将抽取器不能消费的谓词返回给上层继续精确求值。
3. 以 `PruneColumns` 仅对明确白名单中的系统表同步裁剪输出 schema、输出名和列元数据，同时保证非空 schema。
4. 以 `PushDownTopN` 向抽取器传递安全的行数上限以及慢日志时间排序方向，但保留原 `LogicalTopN`，因此提示不代替上层语义算子。
5. 以 `DeriveStats` 为虚拟表生成伪统计信息，并缓存到逻辑计划基类，供成本估算与物理规划使用。

这些职责都是规划期元数据变换；本文件不读取表数据，也不创建执行线程或网络请求。实际扫描由后续 `PhysicalMemTable` 及执行器负责。

## 主要符号

- `MemTablePredicateExtractor`：对象安全的抽取器 trait。`CloneBox(&self)` 支持在逻辑计划物理化或复制时克隆 trait object；`Extract(&mut self, schema, names, predicates)` 修改抽取器内部提示状态并返回剩余谓词；`SetRowLimitHint` 和 `SetDesc` 默认无操作，使不支持相应提示的抽取器仍能实现基础契约。
- `impl Clone for Box<dyn MemTablePredicateExtractor>`：把动态分派的克隆转发给 `CloneBox`。这使 `LogicalMemTable::Extractor` 能被物理计划构造代码复制，而无需知道具体抽取器类型。
- `LogicalMemTable`：逻辑节点主体。`LogicalSchemaProducer` 保存基类、schema、输出名和统计信息；`Extractor` 保存可选抽取器；`DBName`、`TableInfo`、`Columns` 描述目标表及当前投影；`QueryTimeRange` 保存诊断类表使用的可选时间范围。
- `Default`：建立空的 schema producer、无抽取器、默认库/表元数据、空列以及无时间范围的未初始化值。可执行的计划仍应再调用 `Init` 并设置 schema/列信息。
- `Init(ctx, offset)`：以 `NewBaseLogicalPlan(ctx, "MemTableScan", offset)` 绑定计划上下文和查询块偏移。
- `PredicatePushDown(predicates)`：无抽取器时原样返回；有抽取器时把当前 `Schema()`、`OutputNames()` 和谓词向量传给 `Extract`。
- `PruneColumns(parent)`：对白名单表按父节点列的 `UniqueID` 求保留索引，并按同一索引同时重建 schema、名称和 `Columns`。
- `PushDownTopN(top_n)`：识别普通 Limit 或慢日志按 `time` 单列排序，设置提示后仍返回原 TopN；`None`、错误类型和分区 TopN 均不下推提示。
- `pushDownRowLimit(offset, count)`：用 `saturating_add` 计算 `offset + count`，溢出时饱和为 `u64::MAX`。
- `isSlowLogTopNByTime(column)`：只有 `slow_query`/`cluster_slow_query` 且列 ID 对应表元数据中名为 `time` 的列时为真。
- `DeriveStats(reload)`：复用缓存或基于 `statistics::PseudoTable(TableInfo.ID)` 生成新的 `StatsInfo`，为输出 schema 的每列设置等于伪行数的 NDV。
- `impl LogicalPlan for LogicalMemTable`：提供运行时向下转型、基类访问，并把四个优化接口转发给上述固有方法。

## 执行流程

典型的规划期流程如下：

1. 调用方准备表元数据、输出列、schema、可选抽取器和时间范围，再用 `Init` 建立 `MemTableScan` 逻辑节点。当前仓库尚未找到生产 Rust 构造调用，此步骤是类型契约而非已验证的 plan-builder 主链。
2. 谓词下推阶段调用 `LogicalPlan::PredicatePushDown`。若 `Extractor` 存在，抽取器从谓词中记录表侧可用的过滤条件，只把无法消费或必须二次验证的表达式返回；若不存在，全部谓词留在上层。
3. 列裁剪阶段调用 `PruneColumns`。非白名单表不变；白名单表用父节点列的 `UniqueID` 选出索引，并对 schema、输出名和列元数据实施相同顺序的投影。父节点没有引用列但旧 schema 非空时强制保留第 0 列。
4. TopN/Limit 下推阶段调用 `PushDownTopN`。分区 TopN不处理；纯 Limit 只传递行数提示；慢查询表按唯一 `time` 列排序时还传递升降序提示。无论提示是否被抽取器实际实现，原 TopN 都会返回并保留在内存表之上。
5. 统计推导阶段调用 `DeriveStats`。`reload == false` 且已有缓存时返回缓存并标记 `false`；否则生成伪表统计、填充每个输出列的 NDV、缓存结果并标记 `true`。
6. 物理化阶段有两条已接线路径：旧 cascades 规则 `ImplMemTableScan::OnImplement` 复制数据库名、表、列和抽取器；核心物理枚举函数 `ExhaustPhysicalPlans4LogicalMemTable` 还复制 `QueryTimeRange`，并拒绝 IndexJoin 属性、非任意 MPP 分区或排序要求。两条路径最终都产生 `PhysicalMemTable`。

## 数据与状态

节点自身的持久状态都属于单个计划：

- `LogicalSchemaProducer` 是逻辑计划公共状态，包含 `BaseLogicalPlan`、输出 schema、名称及缓存统计。
- `Extractor: Option<Box<dyn MemTablePredicateExtractor>>` 可能在谓词和 TopN 下推时被可变调用，具体过滤状态由实现类型持有；动态克隆通过 `CloneBox` 完成。
- `DBName` 与 `TableInfo` 标识虚拟表。表名判断使用 `TableInfo.Name.L`，即 `CIStr` 的小写形式。
- `Columns` 与 `Schema().Columns`、`OutputNames()` 构成三个平行序列。`PruneColumns` 依靠相同索引同步更新它们；如果调用方初始化时长度不一致，`filter_map` 会静默跳过缺项，文件自身不校验该不变量。
- `QueryTimeRange: Option<(i64, i64)>` 只是携带状态，本文件不解释单位、不校验起止关系，也不在优化方法中使用；核心物理枚举路径会复制它。
- `StatsInfo` 缓存在基类中。新统计的 `RowCount` 取伪表 `HistColl.RealtimeCount`，`StatsVersion` 为 `PseudoVersion`，每个输出列的 `ColNDVs[UniqueID]` 都等于行数。

列裁剪白名单由 `PruneColumns` 内的 `matches!` 固定为 15 个小写表名。新增可裁剪表必须显式加入，否则该节点会保留全部列。

## 依赖与调用关系

上游接口关系：`LogicalPlan` trait 的优化流程通过本文件末尾的转发实现调用谓词下推、列裁剪、TopN 下推和统计推导；`pkg/planner/cascades/memo/group_expr.rs` 还能经包装后的逻辑计划转发这些操作。RustCodeGraph 将本文件列为被 `logical_mem_table_test.rs`、`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/memtable_extractors/memtable_extractors_test.rs` 和 `pkg/executor/infoschema_cluster_table_test.rs` 使用。

下游依赖关系：

- `LogicalSchemaProducer`/`BaseLogicalPlan` 提供上下文、schema、输出名、查询块偏移和统计缓存。
- `expression::{Column, Schema}` 与 `types::metadata::NameSlice` 构成抽取器和裁剪接口。
- `model::{TableInfo, ColumnInfo}` 与 `parser_ast::CIStr` 提供数据库对象元数据。
- `statistics::PseudoTable`、`statistics::PseudoVersion` 和 `property::StatsInfo` 提供成本估算所需的伪统计。
- `LogicalTopN` 和 `planner_util::ByItems` 提供 Limit/排序形态及 `Offset`、`Count`、`Desc` 信息。
- `physicalop::ExhaustPhysicalPlans4LogicalMemTable` 将本节点转换为 `PhysicalMemTable`；`cascades::old::ImplMemTableScan` 是另一条物理实现规则。`pattern.rs` 的类型判断让 cascades 能匹配该逻辑节点。

与具体谓词语义相关的实现位于 `pkg/planner/core/memtable_extractors/`，不在本文件内；例如其测试验证慢查询抽取器会保存更严格的最小行数提示、排序方向和时间范围。

## 错误处理与边界

本文件公开的 `PredicatePushDown`、`PruneColumns` 和 `DeriveStats` 使用 crate 的 `Result<T>`，但当前实现路径没有主动构造 `PlannerError`，均返回 `Ok`。抽取器 `Extract` 本身也不返回 `Result`，因此具体抽取失败不能经该 trait 显式传播，只能由实现选择保留谓词或记录跳过状态。

主要边界如下：

- 无 `Extractor`：谓词保持不变，TopN/Limit 提示被忽略，不报错。
- 非白名单表：列裁剪完全跳过；白名单表无人引用任何列时，只要旧 schema 非空就保留第 0 列。
- `parent` 中重复列或不存在的 `UniqueID`：`HashSet` 去重，输出仍按原 schema 顺序；不存在的列不会导致错误。
- `top_n == None`：返回 `None`；传入的 trait object 不是 `LogicalTopN` 或含 `PartitionBy`：原样返回，不设置提示。
- 慢日志 TopN：必须恰有一个排序项、表达式可向下转型为 `Column`、表名匹配且列 ID/名称同时匹配；否则不设置方向和行数提示。
- `offset + count`：使用饱和加法，避免 release/debug 模式下溢出差异。
- 统计缓存：只有 `reload == false` 才复用；强制 reload 总会覆盖缓存。空 schema 会得到没有 `ColNDVs` 条目的合法伪统计。
- `SCtx`、schema 与元数据是否初始化由调用方保证；本文件没有完整性检查。

## 并发与资源生命周期

`LogicalMemTable` 不创建线程、异步任务、通道、锁、文件句柄、网络连接或事务。所有方法都同步执行，资源生命周期跟随逻辑计划对象。

`Extractor` 的修改要求 `&mut self`，因此同一个节点不能在安全 Rust 中被多个线程同时无保护地调用这些方法；trait 也没有声明 `Send` 或 `Sync`。是否使用锁是具体抽取器自己的选择，生产契约不要求内部同步。独立测试中的 `RecordingExtractor` 使用 `Arc<Mutex<HintState>>` 只是为了让测试在抽取器 trait object 外观察提示状态，不代表节点本身的并发模型。

物理化时抽取器被 `CloneBox` 深浅程度由具体实现决定；实现者必须保证克隆后的状态和共享资源符合其生命周期预期。元数据向量和 schema 在列裁剪时会重新分配；旧副本在方法返回后按 Rust 所有权规则释放。

## 与 Go 版本的对应关系

Rust 文件直接对照 `logical_mem_table.go`，总体流程保持一致：同名逻辑节点、可选抽取器、白名单列裁剪、TopN/Limit 提示、慢日志时间列识别、溢出保护以及伪统计缓存均有对应实现。`logical_mem_table_test.rs` 明确验证了 15 个 Go 白名单表、溢出饱和值、慢日志降序提示和伪表统计契约。

仍存在以下可见差异或简化，扩展时不能忽略：

- Go 的 `TableInfo`/`ColumnInfo` 是指针，Rust 按值拥有并克隆；Go 的 `QueryTimeRange` 是 `util.QueryTimeRange`，Rust 是 `Option<(i64, i64)>`。
- Go 抽取器 `Extract` 接收 session/plan context，Rust trait 只接收 schema、名称和谓词；Go 还能通过 failpoint `skipExtractor` 跳过抽取，Rust 本文件无对应 failpoint。
- Go 用能力接口分别探测 `MemTableRowLimitHintSetter` 与 `MemTableDescHintSetter`；Rust 把两个默认无操作方法放在同一 trait 中。结果仍是“不支持的实现可忽略提示”，但能力发现方式不同。
- Go `PruneColumns` 通过 `expression.GetUsedList` 计算引用，并逆序删除；Rust 直接比较 `UniqueID` 后重建三个向量。对测试覆盖的直接列引用结果一致，但更复杂表达式的引用展开依赖调用方已传入正确的 `Column` 集合。
- Go `PushDownTopN` 返回 `topN.AttachChild(p)`；Rust 返回原 TopN trait object，本方法没有在这里附加 child。是否已在调用前建立子关系取决于外层优化流程，当前文件不验证这一点。
- Go `DeriveStats` 依据完整 `TableInfo` 生成 HistColl；Rust 以 `TableInfo.ID` 调用 `PseudoTable` 并克隆其 HistColl。独立 Rust 测试只验证物理 ID、伪行数和版本等当前契约。
- 旧 Rust cascades 规则复制了数据库名、表、列和抽取器，但没有复制 `QueryTimeRange`；核心 `ExhaustPhysicalPlans4LogicalMemTable` 会复制。若旧规则仍用于时间范围表，这是需要单独审计的兼容风险，本文不把它判定为运行时缺陷。

## 扩展指南

新增或修改功能时，优先在最窄的契约点接入：

- 新增抽取能力：实现或扩展 `MemTablePredicateExtractor`，确保 `CloneBox` 保留必要状态，并在 `pkg/planner/core/memtable_extractors/` 的独立测试中覆盖已消费谓词、剩余谓词、冲突条件与空条件。不要把测试内嵌到本源文件。
- 新增可裁剪系统表：把其规范化小写名加入 `PruneColumns` 白名单，并扩展 `logical_mem_table_test.rs::prune_columns_covers_every_go_whitelisted_memory_table`；同时核对 Go 常量列表，防止名称漂移。
- 修改 TopN 提示：保持“提示不替代 TopN 正确性”的原则，覆盖非 TopN、分区 TopN、多排序项、非列表达式、错误列 ID、升降序及 `u64` 溢出。涉及慢查询抽取器状态时同步扩展 `pkg/planner/core/memtable_extractors/memtable_extractors_test.rs`。
- 修改统计推导：同步检查 `statistics::PseudoTable`、`StatsInfo` 消费方和 `logical_mem_table_test.rs::derive_stats_uses_pseudo_table_contract`，评估行数/NDV变化对物理计划选择的性能风险。
- 新增字段：同时检查 `Default`、两条物理化路径、`PhysicalMemTable::Clone`、哈希/相等生成代码以及 Go 对照。特别注意当前两条物理化路径对 `QueryTimeRange` 的复制并不一致。
- 补上上游构造接线：应从 Rust plan builder 或信息模式计划入口建立有真实 `TableInfo`、schema、列、抽取器和时间范围的节点，并增加独立集成/单元测试；在此之前不要仅凭下游类型存在就宣称端到端可用。

兼容风险主要是剩余谓词被错误吞掉、schema/名称/列向量失配、提示被当成语义保证，以及 Go/Rust 白名单或特殊表名漂移；性能风险主要是少裁列、少下推过滤或伪统计变化导致的过量扫描和错误计划选择。

## 验证依据

本说明基于以下直接证据完成：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；随后用 `node --file` 完整读取本文件 226 行。
- RustCodeGraph 对 `LogicalMemTable` 的查询同时定位 Go/Rust 定义，并定位 `physical_mem_table.rs::ExhaustPhysicalPlans4LogicalMemTable`；目标文件上下文报告 4 个使用文件。
- 完整阅读 `pkg/planner/core/operator/logicalop/logical_mem_table.rs`、Go 对照 `logical_mem_table.go`、独立 Rust 测试 `logical_mem_table_test.rs`、crate 入口 `lib.rs` 与同目录 `Cargo.toml`。
- 阅读直接下游 `pkg/planner/core/operator/physicalop/physical_mem_table.rs`、`pkg/planner/cascades/old/implementation_rules.rs` 和类型匹配入口 `pkg/planner/cascades/pattern/pattern.rs`。
- 阅读谓词实现测试 `pkg/planner/core/memtable_extractors/memtable_extractors_test.rs` 以及 `logicalop_test/logical_mem_table_predicate_extractor_test.rs` 的相关内容，用于核对抽取器状态和边界；后者测试的是具体抽取器，不是本逻辑节点的直接调用。
- 用 `rg` 搜索全仓 Rust 的 `LogicalMemTable {`、`LogicalMemTable`、`ExhaustPhysicalPlans4LogicalMemTable` 和 `MemTableScan`，确认当前直接构造点仅见于独立测试，并确认两条物理化与 cascades 匹配接线。
- 本任务为纯文档分析，按计划不运行 Cargo。最终使用任务指定的命令验证目标文档存在且恰好包含 11 个固定二级标题，并人工复核所有“已支持”陈述均有上述源码或测试依据。
