# `pkg/ttl/cache/table.rs`

## 文件定位

`table.rs` 属于 `astersql-ttl-cache` crate；crate 入口 `pkg/ttl/cache/lib.rs` 通过 `pub mod table` 导出本模块。它位于 TTL 元数据进入扫描调度之前的适配层：把简化的表、分区、列和索引元数据整理成 `PhysicalTable`，计算过期水位，并把 TiKV Region 原始键边界转换为 TTL 扫描使用的半开区间 `[start, end)`。

当前可确认的 Rust 运行时接线在 `pkg/session/runtime/ttl_metadata.rs`：该文件构造本模块的 `PhysicalTable`，在版本门禁允许时调用 `FindTTLIndex` 与 `SplitIndexScanRanges`，否则调用 `SplitScanRanges` 回退到表键路径；同一调用方也直接使用自由函数 `EvalExpireTime`。RustCodeGraph 的文件节点还把 `pkg/ttl/session/session.rs` 和 `pkg/ttl/sqlbuilder/sql.rs` 列为文件级使用方，但精确文本核验没有发现前者引用本模块，后者只有已注释的旧 `cache::PhysicalTable` 草稿并另有自己的 `PhysicalTable`，因此不能把二者视为已确认的活动调用链。

## 核心职责

- 用 `getTableKeyColumns` 决定 TTL 表扫描键：整型主键句柄、复合聚簇主键，或隐式 `_tidb_rowid`。
- 用 `NewPhysicalTable` / `NewBasePhysicalTable` 校验 TTL 时间列、表公开状态和分区名，并生成物理表 ID 对应的 `PhysicalTable`。
- 用 `BuildTTLIndexScanPlan` 验证一个索引能否提供严格、可继续分页的物理顺序；用 `FindTTLIndex` 按固定优先级选择候选。
- 用 `EvalExpireTime` 从当前秒级时间戳减去 TTL interval，覆盖固定长度单位以及按公历回退的月、季、年。
- 通过 `RegionProvider` 获取 Region 键范围，由 `splitRawKeyRanges` 合并成目标数量，再由表扫描或索引扫描方法解码为 `ScanRange`。
- 提供整数句柄、字节句柄、ASCII 前缀等边界推进辅助函数。

本文件只计算元数据、计划和范围，不读取行、不生成 SQL、不删除数据，也不维护跨调用缓存。

## 主要符号

- `KeyKind`：本模块关心的键值类别。`SignedInt`、`UnsignedInt`、`Bytes` 可参与表键范围拆分；`Float`、`Set` 会被索引分页计划拒绝。
- `Column`、`IndexColumn`、`IndexInfo`、`PartitionDefinition`、`TTLInfo`、`TableInfo`：从完整 TiDB 元数据投影出的最小 Rust 模型。索引模型显式保存 public、invisible、global、multi-valued、columnar、conditional 等安全判定字段。
- `ScanRange { Start, End }`：Datum 层的半开区间；空向量表示该侧开放。`newFullRange` 返回双侧开放区间，`newDatumRange` 把 `Datum::Null` 转成开放边界。
- `PhysicalTable`：一次 TTL 扫描所需的物理表视图，包含物理 ID、schema、原表元数据、可选分区、表键列、时间列和索引列表。
- `NewPhysicalTable(schema, table, partition)`：要求 `table.ttl` 存在且时间列为 public，然后委托 `NewBasePhysicalTable`。
- `BuildTTLIndexScanPlan`：产生 `TTLIndexScanPlan { Index, ScanColumns, OrderColumns, KeyColumnOffsets }`；其中 `OrderKey` 截取严格分页键，`TableKey` 按偏移重建表键。
- `FindTTLIndex`：忽略不能建计划的索引，在可用计划中优先选择单列索引，其次完整包含表键的索引，再按 order/scan 列数择小。
- `EvalExpireTime(now_seconds, interval, unit)`：自由函数负责纯秒级计算；`PhysicalTable::EvalExpireTime` 从表的 `TTLInfo` 取参数。
- `RegionProvider::locate_key_range`：存储边界抽象。生产适配器是 `pkg/session/runtime/ttl_metadata.rs` 的 `StorageRegions`，最终调用 `StorageHandle::TTLRegionRanges`。
- `splitRawKeyRanges`：把存储返回的 Region 连续分组，组数为 `min(split_count, region_count)`，前面的组接收余数，并把首尾裁剪到请求区间。
- `SplitScanRanges`、`SplitIndexScanRanges`：分别沿记录键前缀与索引键前缀切分扫描任务。
- `GetNextIntHandle`、`GetNextIntDatumFromCommonHandle`、`GetNextBytesHandleDatum`、`GetASCIIPrefixDatumFromBytes`：将原始边界转成下一段可用的 Datum。

## 执行流程

1. 调度侧把 InfoSchema/TTL worker 元数据投影成 `Column`、`IndexInfo`、`TableInfo` 或直接组装 `PhysicalTable`。构造函数路径先确认 TTL 配置与 public 时间列，再选择表键并解析物理分区 ID。
2. `pkg/session/runtime/ttl_metadata.rs` 计算 `split_count`，并用 `StorageRegions` 包装实际存储。若 `TTLEnableIndexScan` 打开，先调用 `FindTTLIndex`。
3. `BuildTTLIndexScanPlan` 逐层排除不安全索引：不可见或非 public、全局/MV/列存/条件索引、空索引、聚簇主键、前缀列、无效或隐藏列、TTL 列不在首位、unique 索引的后续 nullable 列、非 unique 索引只含部分表键、无法依赖的 unsigned 隐式后缀，以及 Float/Set 分页列。
4. 若找到索引且集群版本门禁允许，`SplitIndexScanRanges` 校验计划，构造索引前缀，调用 `splitRawKeyRanges`，把中间 Region 结束键的首个 8 字节负载解释为时间边界，并以 `expire_time` 为上限生成连续范围。边界无法解码时使用开放边界；若结果为空则回退全范围。
5. 索引不可用、被版本门禁回退或功能关闭时，`SplitScanRanges` 构造记录前缀并拆 Region。无 provider、`split_count <= 1`、没有键列或 Region 不能有效拆开时返回全范围。
6. 表键拆分按首键类型转换中间结束键：signed integer 用 `GetNextIntHandle`，bytes 用 `GetNextBytesHandleDatum`；unsigned integer 走 `split_unsigned_int_ranges`，把 TiKV 的有符号编码顺序重新排列为 SQL 的 `0..=u64::MAX` 顺序。Float/Set 不生成具体中间边界。
7. TTL 水位由 `PhysicalTable::EvalExpireTime` 读取 `TTLInfo` 后调用自由函数。微秒至周按秒数相减；月、季、年把 epoch 天数转公历日期、回退月份并将日期钳制到目标月合法末日。

## 数据与状态

`TableInfo` 和 `PhysicalTable` 在构造时克隆列、索引与分区元数据，因此本文件没有借用 InfoSchema 对象的生命周期，也不会在调用期间回写原表。`PhysicalTable.ID` 对非分区表是表 ID，对分区表是匹配到的 `PartitionDefinition.id`；这决定记录键和索引键前缀。

`ScanRange` 的核心不变量是 `[Start, End)`。每侧最多由当前实现写入一个 Datum；空 `Start`/`End` 表示负无穷/正无穷，而不是 SQL NULL。`TTLIndexScanPlan` 要求结果行先包含索引列，再补齐索引中缺失的表键列；`KeyColumnOffsets` 始终指向 `ScanColumns`，`OrderColumns` 是物理索引顺序可满足的严格游标。

`splitRawKeyRanges` 假设 provider 返回按键序排列、能覆盖所请求区间的 Region。它不持久化结果，也不验证 Region 是否重叠或乱序。日期换算函数均为纯函数；月末钳制保证诸如 3 月 31 日减一个月不会制造非法日期。

## 依赖与调用关系

上游已确认调用者是 `pkg/session/runtime/ttl_metadata.rs`：

- `build_scan_ranges` 一类调度逻辑组装 `CachePhysicalTable`，调用 `FindTTLIndex`、`SplitIndexScanRanges` 或 `SplitScanRanges`。
- `StorageRegions` 实现 `RegionProvider`，把 `StorageHandle::TTLRegionRanges` 的字节对转换为 `KeyRange`。
- TTL 元数据转换逻辑把 parser 的 `TimeUnitType` 映射为本文件的 `TimeUnit`，再调用 `EvalExpireTime`。

本文件直接依赖同 crate 的 `crate::task::Datum`。`pkg/ttl/cache/Cargo.toml` 声明 crate 名为 `astersql-ttl-cache`、入口为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/ttl/cache"` 标明 Go 对照包。清单中的外部 crate 依赖目前全部置于 Windows target 条件下；本文件本身只用标准库和 crate 内 Datum。

RustCodeGraph 对 `SplitScanRanges` 给出的下游边包括 `newDatumRange`、`splitRawKeyRanges`、`record_prefix`、`prefix_next`、`datum_less`、`split_unsigned_int_ranges`、`GetNextIntHandle` 和 `GetNextBytesHandleDatum`，与源码控制流一致。

## 错误处理与边界

公开构造与计划函数使用 `Result<_, String>`，错误直接携带表、分区、列或索引名称。主要硬错误包括：非 public 表、缺失 TTL 或时间列、分区参数与表形态不符、找不到分区、主键元数据非法、索引不能保证严格分页、key prefix 长于表键，以及 interval 解析失败。`RegionProvider` 的存储错误由 `splitRawKeyRanges` 原样向上传播。

部分情况刻意降级而非报错：没有 Region provider、请求一段、Region 数不足、边界无法解码或最终没有有效范围时返回全范围；`FindTTLIndex` 会吞掉单个索引的建计划错误并继续比较其他候选。这种降级保持正确性，但可能失去并行度或索引扫描收益。

调用者必须保证 `TTLIndexScanPlan::OrderKey` 的行长度至少为 `OrderColumns.len()`，并保证 `TableKey` 中每个偏移都有效；当前方法使用切片和直接索引，违反约定会 panic。`splitRawKeyRanges` 在 provider 返回非空时可避免除零，但依赖 Region 顺序和连续性。`EvalExpireTime` 的乘法/减法未显式使用 checked 运算，极端 `i64` interval 可能溢出；生产调用方应提供解析自合法 TTL 定义的合理值。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、全局可变状态或事务。所有计划和范围都在调用栈内同步构造并按值返回；传入的 `RegionProvider` 只在 `splitRawKeyRanges` 调用期间借用。

真正的存储资源生命周期位于调用方：`StorageRegions` 持有 `Arc<StorageHandle>`，在一次 `locate_key_range` 中借助 `with_storage` 查询 Region。并发安全、连接复用和 Region 缓存刷新由 `StorageHandle`/底层存储负责，不由本文件保证。克隆 `TableInfo`、列与索引避免共享可变元数据，但宽表上会产生与元数据规模线性相关的分配。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ttl/cache/table.go`，测试是 `pkg/ttl/cache/table_test.go`。Rust 保留了 Go 的主要形状和命名：`PhysicalTable`、构造函数、`ScanRange`、`ValidateKeyPrefix`、`FullName`、过期时间计算、主键/索引 Region 拆分、`TTLIndexScanPlan` 以及句柄边界辅助函数。Rust 独立测试 `pkg/ttl/cache/table_test.rs` 覆盖无 TTL、三类表键、分区、时间列、key prefix、日/月/周/分钟/HOUR_MINUTE、Region 回退与基础拆分，以及索引选择。

两者并非完整等价，扩展时必须以差异为约束：

- Go `PhysicalTable::EvalExpireTime` 从 session 读取全局时区，保留输入 location，并用表达式框架处理 DST、精度截断和 SQL interval；Rust 接口只有无时区的秒级 `i64`，月历计算按 UTC 式 epoch 天数进行。Go 测试覆盖上海、柏林、洛杉矶 DST 等语义，Rust 测试没有等价覆盖。
- Go 使用真实 `model.TableInfo`、`FieldType`、`tablecodec`、`codec` 与 TiKV Storage；Rust 使用简化元数据和自定义 `t{table_id}_r...` 前缀/8 字节时间负载。Rust 的字节编码不能据此宣称与真实 TiDB KV 编码完全兼容。
- Go 的表键拆分覆盖整数、BIT、字符串/二进制、复合 common handle、字符集和 collation，并有更严格的部分编码边界处理；Rust 主要按首个 `KeyKind` 处理 signed/unsigned/bytes。
- Go 索引计划会检查 common handle prefix、collation 版本、真实字段类型、nil 时间列，并为时间索引范围编码 MinNotNull 和 expire key；Rust 模型用布尔字段和 `KeyKind` 近似这些约束，索引边界解码也更简单。
- Go 测试包含真实 mock TiKV Region、任意截断时间负载、非法日期/时刻、SQL 生成链和更多索引元数据变体；Rust 当前独立测试只证明简化模型内的行为。

因此，本文件可视为已接入 Rust TTL 调度的局部移植实现，但不能把 Go 全部边界能力视为已经移植。

## 扩展指南

- 增加键类型或 common handle 支持时，应同时修改 `KeyKind`、`SplitScanRanges` 的类型分派、边界解码辅助函数和 `pkg/ttl/cache/table_test.rs`；不要把测试嵌入生产文件。还要逐项对照 Go 的 `splitIntRanges`、`splitCommonHandleRanges` 与字符序限制。
- 改索引资格或优先级时，从 `BuildTTLIndexScanPlan` 和 `FindTTLIndex` 接入，并同步检查 `TTLIndexScanPlan` 的列布局、`pkg/session/runtime/ttl_metadata.rs` 的索引列投影及 Go `BuildTTLIndexScanPlan`/`FindTTLIndex`。任何放宽都必须证明游标严格递增且 ORDER BY 可由物理索引顺序满足。
- 改 Region 拆分时保持 `[start, end)`、连续、单调和首尾开放不变量；为 provider 错误、零/一 Region、Region 少于目标数、乱序/截断边界及 unsigned 跨零顺序增加独立测试。
- 若要达到 Go 时间语义，不能只扩展 `TimeUnit`；需要设计时区、DST、SQL interval、亚秒精度和溢出策略，并在 session 调用链中传递 location。应移植 `table_test.go` 的时区回归用例，而不是仅比较 Unix 秒。
- 若要宣称真实 KV 编码兼容，应替换或验证 `record_prefix`、`index_prefix`、`decode_index_time_boundary` 与 Go `tablecodec`/`codec` 的一致性，并用真实编码向量或跨语言夹具验证。
- 性能上关注 `PhysicalTable` 的整表克隆、计划构造中的多次线性查找，以及 Region/范围 Vec 分配；优化时先保持错误降级和顺序不变量。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/ttl/cache/table.rs` 确认目标已索引；`node --file ... --offset 1/500` 读取 809 行源码并显示文件级使用关系；`query PhysicalTable`、`query splitRawKeyRanges`、`query EvalExpireTime` 消除同名符号歧义；`callees SplitScanRanges` 验证范围拆分下游边。部分 `callers` 查询无结果，因此上游关系又用精确文本搜索核验。
- Rust 源与装配：`pkg/ttl/cache/table.rs`、`pkg/ttl/cache/lib.rs`、`pkg/ttl/cache/Cargo.toml`。
- Rust 直接调用证据：`pkg/session/runtime/ttl_metadata.rs`；同时核验 `pkg/ttl/session/session.rs` 和 `pkg/ttl/sqlbuilder/sql.rs`，后者的 cache 类型引用仅存在于注释草稿，不能作为活动调用证据。
- Go 对照：`pkg/ttl/cache/table.go`、`pkg/ttl/cache/table_test.go`。
- Rust 独立测试：`pkg/ttl/cache/table_test.rs`，测试与生产源文件分离。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、文件存在性、链接与事实一致性。
