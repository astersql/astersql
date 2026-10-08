# `pkg/util/topsql/reporter/datamodel.rs`

源文件：[`datamodel.rs`](./datamodel.rs)；Go 对照：[`datamodel.go`](./datamodel.go)；Rust 测试：[`datamodel_test.rs`](./datamodel_test.rs)、[`datamodel_1_aster_unit_test.rs`](./datamodel_1_aster_unit_test.rs)。

## 文件定位

本文件属于 Cargo 包 `astersql-util-topsql-reporter`，由同目录 `lib.rs` 的 `datamodel` 模块通过 `include!` 编译，并经 `pub use datamodel::*` 从 crate 根公开。它把 TopSQL 一个上报窗口内的 CPU、语句统计、SQL/Plan digest 和规范化元数据组织成可聚合的数据结构，最终转换为 `tipb` protobuf。crate 边界、`tipb`、collector、state 和 stmtstats 的依赖见 `pkg/util/topsql/reporter/Cargo.toml` 与 `pkg/util/topsql/reporter/lib.rs`。

当前接线需要特别区分：这些类型是公开、可编译且被独立测试覆盖的数据模型，但仓库内生产 Rust 代码没有找到对 `Collecting`、`Records`、`CpuRecords`、`NormalizedSqlMap` 或 `NormalizedPlanMap` 的直接使用；当前 `pkg/util/topsql/reporter/reporter.rs` 另有 `TopSQLCollecting`、`TopSQLRecord` 和 `MetaState`，实际收集/上报主链使用的是那套结构。因此本文件目前更接近已移植的数据模型 API 与兼容基线，而不是 `RemoteTopSQLReporter` 的直接运行时存储实现。

## 核心职责

- 用 `TsItem` 和 `Record` 将同一 SQL digest + Plan digest 在不同秒的 CPU 与语句统计聚合成时间序列，并维护 `timestamp -> Vec 下标` 的快速索引。
- 用 `Records::top_n`、`CpuRecords::top_n` 按 CPU 贡献裁剪 Top-N；用 `Collecting` 记录被淘汰的 digest，并把低贡献项汇总到空 digest 的 `KEY_OTHERS` 桶。
- 在上报前由 `Collecting::remove_invalid_plan_record` 修正同一 SQL 恰有一个空 Plan 和一个有效 Plan 的情况，把空 Plan 数据并入有效 Plan。
- 用 `NormalizedSqlMap`、`NormalizedPlanMap` 限制一个快照中收集的规范化元数据数量，支持原子式取走快照及 protobuf 转换。
- 在 `TsItem::to_proto`、`Record::to_proto` 和两个元数据表的 `to_proto` 中形成 `tipb::TopSqlRecordItem`、`TopSqlRecord`、`SqlMeta`、`PlanMeta` 边界对象。

## 主要符号

- `KEY_OTHERS: &[u8] = b""`：特殊空键，保存被 Top-N 淘汰项的汇总；对应 Go 的 `keyOthers = ""`。
- `MAX_TS_ITEMS_CAPACITY = 1000`：`Record::new` 预分配上限，只限制初始容量，不限制后续 `ts_items` 增长。
- `IGNORE_EXCEED_SQL_COUNT` / `IGNORE_EXCEED_PLAN_COUNT`：容量拒绝计数器；仅 `register` 在先发现已达上限时递增。
- `KvStatementStatsItem::merge`、`StatementStatsItem::merge`：分别累加 KV 目标执行数和语句级执行次数、耗时、网络字节；所有整数累加采用 `wrapping_add`。
- `TsItem`：单个时间戳的私有采样点，`zero` 保证 KV map 为 `Some(empty)`，`to_proto` 导出全部指标。
- `Record`：保存 `sql_digest`、`plan_digest`、`ts_items`、`ts_index` 和累计 CPU。公开入口是 `new`、两个 `append_*`、`merge`、`to_proto` 及只读访问器；`sort_and_rebuild`、`rebuild_ts_index` 维护内部不变量。
- `Records(Vec<Record>)`：记录集合，提供 `top_n` 和批量 `to_proto`，并通过 `Deref<Target=[Record]>` 只读暴露切片。
- `Collecting`：以拼接后的 digest 字节为键持有 `Record`，以时间戳持有淘汰集合，并复用 `key_buf` 降低构键临时分配。
- `SQLCPUTimeRecord` / `CpuRecords`：reporter 侧 CPU 输入模型及 Top-N 容器；`From<crate::collector::SQLCPUTimeRecord>` 完成 collector 类型转换。
- `NormalizedSqlMap` / `NormalizedPlanMap`：`Mutex<HashMap<...>> + AtomicUsize + max_collect` 的有界并发元数据表；各自的私有 `SqlMeta` / `PlanMeta` 保存首次注册值。
- `encode_key`：清空并复用缓冲区，将 SQL digest 与 Plan digest 直接拼接后克隆为 map 键。

## 执行流程

1. 创建记录时，`Record::new` 读取 `topsql_state::GlobalState.PrecisionSeconds`，以“上报间隔 / 精度 + 1”估算容量，并夹在 `0..=1000`；精度至少按 1 秒处理。
2. CPU 样本进入 `Record::append_cpu_time`：已有时间戳则原位累加，否则通过 `TsItem::zero` 新增采样点，同时无条件累加 `total_cpu_time_ms`。语句样本由 `append_stmt_stats_item` 走同一索引，不改变总 CPU。
3. `Record::merge` 对两边先按时间戳排序并修复索引，再用双指针合并；相同时间戳合并 CPU 和语句统计，不同时间戳保留原项，最后重建 `ts_index` 并累加总 CPU。
4. CPU Top-N 可在原始输入上调用 `CpuRecords::top_n`，在已聚合记录上调用 `Records::top_n`。两者都用 `select_nth_unstable_by` 分区，保留区再按 CPU 降序排序；淘汰区不承诺顺序。
5. 收集期间，`Collecting::get_or_create_record` 按 digest 复合键取得记录；`mark_as_evicted` / `has_evicted` 按时间戳追踪淘汰状态；调用者可把淘汰 CPU 或语句统计追加到 Others。
6. `Collecting::report_records` 先暂时移除 Others，再执行空 Plan 修正，克隆普通记录，最后把 Others 放到结果末尾。该方法不会清空普通 `records`；完整轮换应使用 `Collecting::take` 先转移当前 `records` 和 `evicted`。
7. SQL/Plan 元数据通过 `register` 首次写入；`take` 在持锁时移出整张表并把原表长度归零。快照随后用 `to_proto` 附加 keyspace；Plan 的大对象调用压缩回调，普通对象调用解码回调。

## 数据与状态

`Record` 的关键不变量是：`ts_index[timestamp]` 必须指向 `ts_items` 中相同时间戳的位置，且一个时间戳至多一个条目。普通追加保持索引但不保证时间顺序；`merge` 会排序并重建索引。`total_cpu_time_ms` 只反映 CPU 追加/合并，不从 `ts_items` 临时重算。

`Collecting.records` 同时保存普通记录和 Others；Others 使用空键且记录自身的 SQL/Plan digest 也为空。`evicted` 的作用域包含时间戳，同一个 digest 在不同时间戳的淘汰状态互不影响。`report_records` 对 HashMap 迭代，除 Others 保证最后之外，普通记录顺序不稳定。

两个规范化元数据表把 `length` 与 map 放在同一把互斥锁操作范围内更新，但长度本身用原子值保存。重复 digest 保留首次值；`take` 产生一个持有旧数据和旧计数的新表，原表继续接收下一代数据，且两者沿用相同 `max_collect`。protobuf 列表同样来自 HashMap 迭代，不保证顺序。

## 依赖与调用关系

上游模块装配边为 `lib.rs -> datamodel.rs`；`lib.rs` 还在 `#[cfg(test)]` 下把 `datamodel_test.rs` 内嵌到模块，并把 `datamodel_1_aster_unit_test.rs` 作为独立测试模块挂载。RustCodeGraph 将目标文件识别为 60 个符号，并能查询到对应 Rust/Go 类型及测试符号。

下游直接依赖包括：

- `crate::topsql_state::GlobalState` 与 `DefTiDBTopSQLReportIntervalSeconds`：决定预分配容量和默认元数据上限。
- `crate::collector::SQLCPUTimeRecord`：经 `From` 转为本文件的 CPU 记录。
- `crate::tipb_protobuf`：所有对外上报 protobuf 类型。
- 标准库 `HashMap` / `HashSet`、`Mutex`、`AtomicU64` / `AtomicUsize`：承载索引、淘汰集合、线程安全元数据及计数。
- `log`：普通 Plan 解码失败时记录警告。

仓库级搜索没有发现测试之外对主要模型的直接 Rust 调用；`reporter.rs` 的 `processCPUTimeData`、`processStmtStatsData`、`takeDataAndSendToReportChan` 使用其本地模型完成相似职责。这是后续接线或去重时必须先处理的边界，而不能假定替换已经完成。

## 错误处理与边界

- 本文件没有统一的 `Result` 返回链。整数计数全部回绕，极端溢出不会报错；这是与 Go 无符号整数自然回绕语义对齐的选择。
- `NormalizedSqlMap::register` / `NormalizedPlanMap::register` 在容量已满时返回 `false` 并递增忽略计数；重复键也返回 `false`，但若表已经满，当前检查顺序会先计为超限，即使该 digest 已存在。
- 两个元数据表遇到 poisoned mutex 时用 `into_inner` 继续工作，不传播 panic；这会保留数据可访问性，但调用者无法获知此前持锁线程曾 panic。
- `NormalizedPlanMap::to_proto` 对普通 Plan 解码失败只写 warning 并跳过该条；大 Plan 压缩回调不返回错误。回调在持有 map 锁期间执行，慢回调会阻塞注册、take 和其他转换。
- `encode_key` 没有长度前缀或分隔符，因此 `(sql="a", plan="bc")` 与 `(sql="ab", plan="c")` 会得到相同键；Go `encodeKey` 也有同一兼容行为。新增或接入调用者必须保证 digest 组合不存在这种歧义，或先设计兼容迁移，不能单边改键格式。
- 空 digest 预留给 Others；真实空 SQL/Plan digest 会与特殊桶冲突。`append_others_cpu_time` 对零值直接跳过，语句统计版本不会跳过零值。
- `top_n` 在元素数不大于 `n` 时不排序，调用者不能把“无需淘汰”等同于“结果必然降序”。

## 并发与资源生命周期

`Record`、`Records`、`Collecting` 和 `CpuRecords` 本身不做同步，要求由拥有者串行修改或置于外部锁后。`Collecting.key_buf` 是内部可变 scratch buffer，所以查淘汰状态也需要 `&mut self`；它每次构键后克隆返回，不把缓冲区借用泄漏出去。

`NormalizedSqlMap` 与 `NormalizedPlanMap` 支持共享引用下并发调用：HashMap 由 `Mutex` 串行化，长度和全局忽略计数由原子变量维护。当前实现使用 `SeqCst` 同步长度，用 `Relaxed` 增加只作观测的忽略计数。`take` 的 map 移出、原表清空和长度归零都在同一持锁区间完成，因此不会把同一次受锁保护的注册拆到两个快照；返回快照拥有独立 Mutex 和计数。

本文件不创建线程、任务、通道或 I/O 资源，也没有显式关闭流程。内存生命周期由容器所有权控制；`take` 用 `mem::take` 转移数据，`Record::merge` 当前克隆另一记录的采样点而不消费另一记录。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/topsql/reporter/datamodel.go`，测试对照是 `datamodel_test.go`。Rust 的 `TsItem`、`Record`、`Records`、`Collecting`、`CpuRecords`、`NormalizedSqlMap`、`NormalizedPlanMap` 分别对应 Go 的 `tsItem`、`record`、`records`、`collecting`、`cpuRecords`、`normalizedSQLMap`、`normalizedPlanMap`；protobuf 字段、Others 约定、Top-N 方向、空 Plan 合并和 Plan 解码失败跳过均保持同一意图。

可见差异如下：

- Go 当前元数据表使用 `atomic.Pointer<normalizedMetadataMap>`、`sync.Map`、CAS 预留和 generation 重试，并包含并发 failpoint；Rust 使用单个 `Mutex<HashMap>`，语义更易串行化但并发性能与 Go 的无锁 generation 模型不同。
- Go `register` 不返回布尔值，Rust 返回是否首次插入；Rust 超限计数是本地 `AtomicU64`，Go 接入 reporter metrics counter。
- Go `record.merge` 注释要求接收者已排序，只在需要时排序 `other`；Rust 对两边都调用 `sort_and_rebuild`，对乱序接收者更稳健。
- Go 空接收者合并直接接管 `other` 的 slice/map；Rust 克隆 `other`，保留对方内容。
- Go `records.topN` 在 quickselect 出错时退回全部记录；Rust 标准库选择操作无错误返回。
- Rust `MAX_TS_ITEMS_CAPACITY` 与 Go 一样只参与预分配保护，不是硬容量限制。

Rust 的 `datamodel_test.rs` 基本逐项映射 Go 测试；额外的 `datamodel_1_aster_unit_test.rs` 验证了非法 UTF-8 digest 不被有损转换、首次值保留、快照切换和 Plan 解码失败跳过等移植边界。

## 扩展指南

- 新增采样指标时，应同步修改 `StatementStatsItem` 或 `TsItem`、合并逻辑、`TsItem::to_proto`，并同时更新 `datamodel_test.rs` 的追加/合并/protobuf 用例以及 Go 对照。若指标来自外部 `stmtstats` crate，还需先确认类型归属，避免与 `lib.rs` 再导出的同名类型混淆。
- 调整 Top-N 时，修改 `Records::top_n` 和 `CpuRecords::top_n`，明确 `n=0`、无需淘汰时是否排序、并列值顺序是否需要稳定；同步测试保留集与淘汰集，不依赖淘汰区顺序。
- 改复合键时，必须同时评估 `get_or_create_record`、淘汰集合、空 Plan 合并、Others 空键和 Go `encodeKey` 的兼容性。推荐新增长度编码前先设计旧快照/跨版本兼容方案，并为歧义 digest 增加独立回归测试。
- 扩展元数据时，修改私有 meta、`register` 与 `to_proto` 三处；保持首次写入策略、容量计数和 `take` 的 generation 边界。涉及并发语义时，应在独立测试文件中增加多线程注册/take 交错测试，而不是把测试嵌入生产文件。
- 若要让当前远程 reporter 复用本文件，接线点是 `reporter.rs` 的 `TopSQLCollecting`、`MetaState`、`processCPUTimeData`、`processStmtStatsData` 与 `takeDataAndSendToReportChan`。这属于行为重构，必须先证明 protobuf、backpressure、指标和并发快照语义一致，不能只替换类型名。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/util/topsql/reporter` 确认 `datamodel.rs`、Go 对照和测试均已索引；`node --file ... --offset 1/500` 读取目标文件 686 行与 60 个符号；`query` 找到 `Collecting`、`NormalizedSqlMap`、`NormalizedPlanMap`、`CpuRecords`、`encode_key` 及对应 Go/测试符号。组合 `explore` 与类型级 callers/callees 查询在 30 秒内无结果，因此调用边又用局部源码搜索核验。
- 已读生产与配置：`pkg/util/topsql/reporter/datamodel.rs`、`datamodel.go`、`Cargo.toml`、`lib.rs`，以及为核对实际接线读取的 `reporter.rs` 相关收集和上报路径。
- 已读测试：`pkg/util/topsql/reporter/datamodel_test.rs`、`datamodel_1_aster_unit_test.rs`、`datamodel_test.go`。覆盖时间序列排序/合并、Top-N、Others、空 Plan 修正、元数据容量/take/protobuf、解码错误和任意 digest 字节。
- 仓库搜索：排除目标文件和两份直接 Rust 测试后，未发现主要模型类型的生产 Rust 使用；这支持“公开但尚未接入当前 reporter 主链”的结论。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查，并人工复核所有事实均可回链到上述符号或文件。
