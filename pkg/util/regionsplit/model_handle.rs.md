# `pkg/util/regionsplit/model_handle.rs`

## 文件定位

本文件属于 `astersql-util-regionsplit` crate。crate 入口 `pkg/util/regionsplit/lib.rs` 将其作为私有模块 `model_handle` 装配，并通过 `pub use model_handle::*` 暴露公开 API。它位于 DDL 持久化 Region split policy 与底层 TiDB 键编码之间：上游 `pkg/ddl/split_region.rs::split_table_regions` 先求值 policy 的字符串边界并转换成真实 `astersql_types::datum::Datum`，本文件再依据 `astersql_meta_model::{TableInfo, IndexInfo, ColumnInfo}` 生成表记录或索引切分键。

与同 crate 的 `split_handle.rs` 相比，本文件不是另一套简化模型，而是“真实元信息路径”：它复用其中的 `StatementContext`、`SplitError`、`MinRegionStepValue`、`gen_table_record_prefix`、`encode_i64` 和 `get_values_list`，同时把真实 model、Datum、tablecodec 和 kv handle 接到该算法上。Cargo 元数据把此 crate 对应到 Go 包 `pkg/util/regionsplit`；直接依赖包括 `astersql-kv`、`astersql-meta-model`、`astersql-parser-mysql`、`astersql-tablecodec`、`astersql-types` 和 `astersql-util-codec`。

## 核心职责

1. `GetHandleColumnInfos` 从真实表元信息确定 split policy 的边界列：整型主键、common handle 主索引列，或隐式 `_tidb_rowid`。
2. `ConvertValueToColumnType` 按列 `FieldType` 转换 policy 求值得到的 Datum，并把截断/坏数字错误规范化成包含值和列名的 Go 兼容错误。
3. `ModelHandleCols` 与 `BuildModelHandleColsForSplit` 封装整数 handle/common handle 的选择，并在 common handle 编码前处理前缀主键截断。
4. `GetSplitTableKeysForModel` 为表记录键空间生成切分点，分别处理有符号整数、无符号整数和 common handle。
5. `GetSplitIndexKeysForModel` 使用真实索引定义生成上下界索引键，补充必要的索引起止前缀，再调用共享插值算法生成切分点。

文件只计算键并返回结果；实际 `SplitRegions`、scatter、错误日志和“失败后继续”的 DDL 策略都由 `pkg/ddl/split_region.rs::split_table_regions` 负责。

## 主要符号

- `GetHandleColumnInfos(&TableInfo) -> Vec<ColumnInfo>`：`PKIsHandle` 时返回主键列（缺失则空）；`IsCommonHandle` 时按主索引 `Columns[*].Offset` 的顺序克隆表列（主索引缺失则空）；普通表返回 `NewExtraHandleColInfo()`。偏移必须能索引 `table.Columns`，这是已构造完成的 `TableInfo` 不变量。
- `ConvertValueToColumnType(&Datum, &ColumnInfo, Context) -> Result<Datum, Error>`：调用 `Datum::ConvertTo`。成功直接返回；失败时，仅对渲染为 `truncated incorrect ... value:` 或等价于 `ErrTruncated`、`ErrTruncatedWrongVal`、`ErrBadNumber` 的错误做规范化，其余错误原样返回。值转字符串若再次失败，也保留原错误。
- `ModelHandleCols { table, primary }`：私有字段持有 `TableInfo` 和可选主索引的克隆，因此实例不借用调用者元信息。结构体本身实现 `Clone`。
- `BuildModelHandleColsForSplit`：构造上述对象，并缓存第一个 `Primary` 索引；是否为整数 handle 仍由 `table.IsCommonHandle` 决定。
- `ModelHandleCols::BuildHandleByDatums`：以 UTC 调用带时区版本，是独立使用时的便捷入口。
- `ModelHandleCols::BuildHandleByDatumsAt`：非 common handle 要求至少一个 Datum，并把首值的 `i64` 翻转符号位后大端编码；common handle 复制输入，列数与主索引列数相等时调用 `tablecodec::TruncateIndexValues`，随后用指定时区 `codec::EncodeKey` 并经 `astersql_kv::NewCommonHandle` 校验/规范化。
- `ModelHandleCols::IsInt`：返回 `!table.IsCommonHandle`。
- `GetSplitTableKeysForModel`：真实表记录切分入口，接收可累积的 `keys`。
- `GetSplitIndexKeysForModel`：真实索引切分入口，自行创建返回列表。

## 执行流程

表 policy 的完整调用链为：`split_table_regions` → `policy_bounds` → `GetHandleColumnInfos`/`ConvertValueToColumnType` → `GetSplitTableKeysForModel` → `get_values_list`（common handle）或整数步长循环 → `SplittableStore::SplitRegions`。索引 policy 的对应链为：`split_table_regions` → `policy_bounds` → `ConvertValueToColumnType` → `GetSplitIndexKeysForModel` → `tablecodec::GenIndexKey` → `get_values_list` → `SplitRegions`。

`GetSplitTableKeysForModel` 的步骤如下：

1. 拒绝零 Region 数或空上下界，生成 `physical_id` 的记录前缀。
2. 表含索引且不是“common handle + 唯一主索引”时，先加入记录前缀，用于分开索引区与记录区。
3. 整数 handle 路径按主键 unsigned 标志选择 `GetUint64` 或 `GetInt64`；要求上界严格大于下界，以 `(high-low)/number` 求步长，并要求步长不少于原子全局值 `MinRegionStepValue`。之后从 `1..number` 生成 `number-1` 个内部边界，handle 采用符号位翻转的大端可比较编码。
4. common handle 路径解析 `StatementContext.time_zone`（空串为 UTC），在该时区分别编码上下界；要求编码后的下界字节严格小于上界，再把记录前缀拼到两端，交给 `get_values_list` 按字节键空间插值。

`GetSplitIndexKeysForModel` 先拒绝 `number == 0`，然后以同一语句时区、真实表/索引元信息和 `IntHandle(i64::MIN)` 调用 `tablecodec::GenIndexKey` 编码两端。最小整数 handle 与 Go 实现一致，避免 handle 后缀改变边界插值。编码后要求 `low < high`；当前索引不是表的第一个索引时加入本索引起始前缀，始终加入 `index.ID + 1` 的结束前缀，最后调用 `get_values_list`。

## 数据与状态

- 输入元信息均按值克隆进临时对象或传给编码器；`ModelHandleCols` 不持有外部引用。
- `BuildHandleByDatumsAt` 对 common handle 先执行 `row.to_vec()`，前缀索引截断只修改副本，不改变调用者的 Datum 切片。`go_merge_32_common_handle_truncates_prefix_without_mutating_input` 明确验证了这个不变量。
- 切分键以 `Vec<Vec<u8>>` 表示并保持调用过程的追加顺序。表整数路径可以在传入 `keys` 后追加记录前缀和内部边界；索引入口从空列表开始，附加索引边界后再插值。
- 唯一共享可变状态是 `split_handle.rs::MinRegionStepValue: AtomicI64`；本文件以 `Ordering::Acquire` 读取，不修改它。
- 时间状态通过字符串形式的 `StatementContext.time_zone` 显式传入；空值固定解释为 UTC，不读取进程全局时区。

## 依赖与调用关系

上游生产调用者是 `pkg/ddl/split_region.rs::split_table_regions`。它从表达式上下文构造 `StatementContext`，按逻辑表/分区和本地/全局索引筛选 physical ID，调用本文件生成键；生成成功后才调用存储的 `SplitRegions`。`policy_bounds` 仅在边界项数与列数相等时逐列调用 `ConvertValueToColumnType`，并对表 handle policy 额外拒绝 NULL。

主要下游依赖为：

- `astersql_meta_model`：表、列、索引元信息及隐式 handle 列。
- `astersql_types`：Datum 转换、错误分类和类型上下文。
- `astersql_tablecodec`：前缀索引截断、真实索引键和索引前缀编码。
- `astersql_util_codec`：带时区 Datum 键编码以及排序规则开关。
- `astersql_kv`：common/int handle 构造与编码。
- `split_handle.rs`：共享错误类型、最小整数步长、记录前缀编码和键空间插值。

RustCodeGraph 将 `model_handle.rs` 标记为由 `pkg/ddl/split_region.rs` 使用；精确源码查询也显示 `GetSplitTableKeysForModel` 调用 `BuildModelHandleColsForSplit`、`BuildHandleByDatumsAt`、`IsInt`、`encode_i64`、`SplitError::{InvalidRanges, InvalidDatum}` 和共享插值函数。

## 错误处理与边界

- Region 数为零、表上下界为空、上下界逆序/相等、整数步长过小都返回 `SplitError::InvalidRanges`。
- 非 common handle 的空 Datum 行由 `BuildHandleByDatumsAt` 返回字符串错误 `integer handle requires one datum`；在表入口中会映射成 `SplitError::InvalidDatum`。
- 非空但无法解析的语句时区返回 `InvalidDatum("invalid statement time zone")`。Datum/common handle/索引键编码错误均转成字符串后包入 `InvalidDatum`。
- `ConvertValueToColumnType` 的规范化错误把值限制到最多 128 字节、列原名限制到最多 192 字节，并使用有损 UTF-8 展示；这里按字节切片，当前实现隐含边界必须落在可切片位置的风险，扩展非 ASCII 超长错误文本时应专门回归。
- common handle 只有在输入 Datum 数恰好等于主索引列数时才做前缀截断；缺少主索引时仍尝试直接编码 Datum。调用者需要保证边界列与表定义一致。
- `GetHandleColumnInfos` 和 index policy 列选择都依赖合法的 `IndexColumn.Offset`；本文件不做越界恢复。
- 本文件只把错误返回给上游；`split_table_regions` 当前将 policy 错误记录到标准错误并跳过对应存储调用，因此单个 policy 失败不会由本文件触发 panic 或部分存储写入。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络资源。所有编码缓冲区均由调用栈拥有，函数返回后由 Rust 所有权规则回收。`ModelHandleCols` 克隆表和主索引，使其生命周期独立于构造参数；代价是每次构造及向 tablecodec 传参时存在元信息克隆。

并发访问点只有 `MinRegionStepValue.load(Ordering::Acquire)`，因此多个 split 请求可以无锁读取一致的已发布最小步长。键生成本身没有跨调用缓存，`StatementContext` 和时区按调用传入；同一 `ModelHandleCols` 只含不可变字段，可通过共享引用并发调用，但本文件未显式承诺或测试跨线程使用。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/regionsplit/split_handle.go`：

- `GetHandleColumnInfos` 对应同名 Go 函数，三个分支及缺失主键/主索引时返回空集合的语义一致；Rust 返回克隆的 `ColumnInfo`，Go 返回指针。
- `ConvertValueToColumnType` 对应同名 Go 函数。Rust 额外识别当前标量转换层以普通文本呈现的截断错误，作为 typed cause 尚未完整保留时的兼容路径；其余错误仍不吞并。
- `ModelHandleCols` 合并了 Go 的 `intHandleCols`、`commonHandleCols` 和 `BuildHandleColsForSplit`。common handle 的“复制后截断前缀索引值”与 Go 相同，Rust 返回编码字节而非 `kv.Handle` trait object。
- `GetSplitTableKeysForModel` 对应 Go `GetSplitTableKeys` 与 `calculateIntBoundValue`：索引/记录分界、unsigned 判定、最小步长、`number-1` 个整数切分点和 common handle 字节插值一致。
- `GetSplitIndexKeysForModel` 对应 Go `GetSplitIndexKeys` 与 `GetSplitIdxPhysicalStartAndOtherIdxKeys`：使用最小整数 handle 编码边界、非首索引加入起始前缀、总是加入结束前缀，再调用相同形状的插值算法。Rust 错误消息比 Go 的表/索引名格式更简化，错误类型也统一为本 crate 的 `SplitError`。

相关 Go 回归证据位于 `pkg/ddl/split_region_test.go`，其中 `oneShotCommonHandleSplitKeys` 验证前缀 common handle 的一次性切分路径；时间戳索引辅助函数验证语句时区进入索引键编码。Rust 的专门模型测试位于独立文件 `pkg/util/regionsplit/tests/go_merge_32_test.rs`，符合测试与生产源码分离约束。

## 扩展指南

- 新增 handle 类型或改变 handle 列选择时，首先修改 `GetHandleColumnInfos`、`BuildModelHandleColsForSplit`、`BuildHandleByDatumsAt` 和 `IsInt` 的共同分派，不能只改某一入口；同步扩展 `pkg/util/regionsplit/tests/go_merge_32_test.rs`，并逐项对照 Go `SplitHandleCols`。
- 新增 Datum/列类型转换规则时，应在 `ConvertValueToColumnType` 保留非目标错误的原始身份，补充成功、截断、坏数字、`ToString` 失败及长/非 ASCII 名称测试；同时检查 DDL `policy_bounds` 的 NULL 与列数规则。
- 修改表/索引边界算法时，应保持键严格排序、只产生内部切分点、索引起止前缀规则以及 `i64::MIN` handle 后缀。表路径覆盖放在 `go_merge_32_test.rs`，完整 DDL 接线覆盖放在独立的 `pkg/ddl/split_region_test.rs`；索引模型入口目前没有同文件级直接单元测试，扩展时应优先补到 `pkg/util/regionsplit/tests/`，不要把测试内嵌到生产 `.rs`。
- 性能敏感点是 `TableInfo`/`IndexInfo`/Datum 的克隆、每个切分点的前缀克隆及 `get_values_list` 的分配。优化必须用等价字节结果验证，不能通过跳过前缀截断、时区或真实 tablecodec 简化。
- 兼容风险集中在 TiDB 可比较编码、unsigned 跨 `i64` 边界的 wrapping 语义、排序规则开关和 Go 错误文本。任何变更都应与 `pkg/util/regionsplit/split_handle.go` 同步核对。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/util/regionsplit` 显示 `model_handle.rs` 有 12 个符号；`node --file ...` 读取完整 296 行；使用关系显示其生产调用者为 `pkg/ddl/split_region.rs`；`node GetSplitTableKeysForModel` 给出内部调用 trail。
- 源码与配置：`pkg/util/regionsplit/model_handle.rs`、`pkg/util/regionsplit/split_handle.rs`、`pkg/util/regionsplit/lib.rs`、`pkg/util/regionsplit/Cargo.toml`。
- 上游接线：`pkg/ddl/split_region.rs::split_table_regions` 与 `policy_bounds`，涵盖表/分区 physical ID、索引筛选、边界转换、存储 split 和 scatter 的职责边界。
- Go 对照：`pkg/util/regionsplit/split_handle.go`；辅助集成证据为 `pkg/ddl/split_region.go` 和 `pkg/ddl/split_region_test.go`。
- Rust 测试：`pkg/util/regionsplit/tests/go_merge_32_test.rs` 验证 handle 列选择、common handle 前缀截断且不修改输入、表切分键和坏数字错误规范化；`pkg/ddl/split_region_test.rs::persisted_policy_bounds_are_converted_to_handle_column_types` 验证 DDL policy 的整数表键、字符串索引边界等价和非法整数不触发存储调用。`pkg/util/regionsplit/tests/split_handle_test.rs` 为共享简化算法提供整数/common/index 编码证据，但不直接调用本文件的索引模型入口。
- 本任务按计划为纯文档分析，未运行 Cargo 或代码测试；最终以固定 11 章节的结构命令和人工事实复核作为交付验证。
