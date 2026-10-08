# `pkg/table/raw_row.rs`

## 文件定位

本文件属于 `astersql-table` crate（`pkg/table/Cargo.toml`），负责把一条表记录的原始 value 解码成调用者请求的列值，并补回未直接存放在 value 中的句柄列、在线 DDL 变更列和默认值。`pkg/table/lib.rs` 以 `pub mod raw_row` 挂载模块并用 `pub use raw_row::*` 再导出 API。

当前 Rust 文件提供的是解码能力，不负责从 KV 读取记录：调用者必须已经持有 `Handle`、原始 `value`、表元数据和目标列集合。仓库搜索仅发现 `pkg/table/column_test.rs` 对元数据级入口的直接 Rust 调用；尚未发现 Rust 运行时路径调用这两个入口。相对地，Go 对照实现已由 `pkg/table/tables/tables.go::RowWithCols` 接到“编码记录键、从事务取 value、再解码”的读行路径。

## 核心职责

- `DecodeRawRowData` 是面向 `Table` trait 的便利入口，从表对象取得 `Meta()` 和 `UseNewCollate()` 后委托给元数据级实现。
- `DecodeRawRowDataWithMeta` 先确定哪些列可从句柄恢复、哪些列必须从行 value 解码，再按请求列顺序组装结果。
- 对整数主键句柄列，依据 `HasUnsignedFlag` 保留有符号或无符号语义。
- 对无需恢复排序键原文的完整 common-handle 列，从 `Handle::EncodedCol` 解码并 `Unflatten`；common-handle 前缀列不能仅靠句柄恢复，仍进入 row value 解码集合。
- 对旧记录中缺失的非虚拟生成列，补在线 DDL 变更值或列默认值。默认值缓存按完整表模式的 `ColumnInfo.Offset` 编址，避免请求列子集导致偏移错位。
- 同时返回按请求顺序排列的 `Vec<Datum>` 和从原始 value 解出的 `HashMap<column_id, Datum>`，后者也供调用者观察实际存储列。

## 主要符号

- `pub fn DecodeRawRowData(ctx, table, handle, columns, value) -> Result<(Vec<Datum>, HashMap<i64, Datum>), Error>`（`raw_row.rs:27`）：公共表对象入口；除提取表元数据和新排序规则开关外不改变参数语义。
- `pub fn DecodeRawRowDataWithMeta(ctx, meta, use_new_collation, handle, columns, value) -> Result<(Vec<Datum>, HashMap<i64, Datum>), Error>`（`raw_row.rs:45`）：核心实现，也是聚焦测试可绕过具体 `Table` 实现使用的入口。
- `result: Vec<Datum>`（`raw_row.rs:53`）：长度固定为 `columns.len()`，位置与请求列切片一致；未被填充的位置保持 `Datum::default()`。
- `column_types: HashMap<i64, Box<FieldType>>`（`raw_row.rs:54`）：传给 `DecodeRowToDatumMap` 的列 ID 到类型映射，只包含确实需要从 value 解码的列。
- `prefix_columns: HashSet<i64>`（`raw_row.rs:55`）：记录 common handle 中仅保存前缀、不能从句柄完整恢复的列，确保第二阶段仍从 value 取值。
- `default_values: Vec<Option<Datum>>`（`raw_row.rs:109`）：长度取 `meta.Columns.len()`，以模式偏移缓存默认值；`Option` 区分“尚未计算”和“计算结果本身为 NULL”。

文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项；两个函数均为公开 API。

## 执行流程

1. `DecodeRawRowData` 从 `Table` 读取 `TableInfo` 和新排序规则状态，然后调用 `DecodeRawRowDataWithMeta`。
2. 核心函数以默认 Datum 初始化与请求列等长的结果，并准备类型映射和 common-handle 前缀集合。
3. 第一遍遍历请求列：
   - `Column::IsPKHandleColumn` 为真时，从 `Handle::IntValue` 构造 `Uint` 或 `Int` Datum，直接写入结果。
   - 对 common-handle 列，若 `NeedRestoredDataWithCollate` 为假，则在主索引中按列 ID 定位该列。只有索引长度为 `-1`（完整列）时才从 `EncodedCol(offset)` 解码、按字段类型和会话时区 `Unflatten`，随后跳过 value 解码。
   - 找不到完整主索引列或索引仅含前缀时，把列 ID 记入 `prefix_columns`；其他普通列也把字段类型加入 `column_types`。
4. 调用 `tablecodec_dependency::DecodeRowToDatumMap`，只按 `column_types` 解码原始 value，时区来自 `ctx.GetEvalCtx().Location()`。
5. 第二遍按请求顺序填充未完成的列：已经从整数句柄或完整 common handle 恢复的列被跳过，前缀列例外；row map 中存在的值优先直接复制。
6. row map 缺列时，虚拟生成列保持默认 Datum，留给表达式计算层处理；有 `ChangeStateInfo` 的列调用 `GetChangingColVal`，否则按完整模式偏移调用并缓存 `GetColDefaultValue`。
7. 返回结果向量和 row map。任一解码、反扁平化、偏移校验、类型转换或默认值计算失败都会提前返回错误。

## 数据与状态

函数不修改 `TableInfo`、`Column`、`Handle` 或输入字节；所有可变状态都局限于单次调用中的 `result`、`column_types`、`prefix_columns` 和 `default_values`。

重要不变量如下：

- `result.len() == columns.len()`，并保持调用者的列顺序，而不是表模式顺序。
- row map 的键是列 ID；默认补值和句柄恢复只写 `result`，不会伪装成 value 中真实存在的列写回 row map。
- 默认值缓存用 `meta.Columns.len()` 分配并用 `ColumnInfo.Offset` 索引。`pkg/table/column_test.rs::raw_row_defaults_use_full_schema_for_null_after_hidden_column` 验证请求列切片省略隐藏列时仍使用完整模式偏移。
- 完整 common-handle 列只有在新排序规则不要求 restored data 且主索引长度为 `-1` 时才能直接由句柄恢复；前缀索引不能提供完整原值。
- 虚拟生成列缺失时不会在这里求值，结果槽保持默认 Datum。

## 依赖与调用关系

上游边界：

- `pkg/table/lib.rs` 声明并再导出本模块，使两个函数成为 `astersql-table` crate 的公共 API。
- `DecodeRawRowData -> DecodeRawRowDataWithMeta` 是文件内唯一静态调用边。
- `pkg/table/column_test.rs` 中 3 个测试直接调用 `DecodeRawRowDataWithMeta`，覆盖变更列、整数句柄和完整模式偏移。RustCodeGraph 将本文件标为被 `pkg/table/column_test.rs`、`pkg/table/tables/tables.rs`、`pkg/table/tables/tables_test.rs`、`pkg/table/tables/partition_expr_test.rs` 使用；源码级符号检索只确认前述测试的直接调用，其余是模块/文件依赖，不能据此宣称存在运行时调用。

下游依赖：

- `expression_dependency::BuildContext` 提供求值上下文及会话时区。
- `kv_dependency::Handle` 提供整数值和 common-handle 编码列。
- `model_dependency::TableInfo`、`crate::Column` 和 `crate::Table` 描述模式、列角色和表级排序规则开关。
- `types_dependency::metadata::NeedRestoredDataWithCollate` 决定 common-handle 值能否直接复用；`Datum` 与字段类型承载解码结果。
- `tablecodec_dependency::{DecodeRowToDatumMap, Unflatten}` 及 `tablecodec_dependency::codec::DecodeOne` 负责行和句柄列的底层编解码。
- `crate::{GetChangingColVal, GetColDefaultValue}` 负责在线 DDL 值恢复和默认值计算；其实现位于 `pkg/table/column.rs:1145` 与 `pkg/table/column.rs:1168`。

`pkg/table/Cargo.toml` 明确声明以上 expression、kv、model、types 和 tablecodec 均为该 crate 的直接路径依赖；本文件没有 feature 门控。

## 错误处理与边界

- 底层 `DecodeOne`、`Unflatten` 和 `DecodeRowToDatumMap` 的错误被转换为 `types::errors::Error`，转换使用错误文本，因此本层不保留底层具体错误类型。
- `ColumnInfo.Offset` 为负时返回 `negative column offset`；偏移超出完整表列数时返回 `column offset out of range`，不会索引越界。
- `GetChangingColVal` 还会检查依赖列和目标列偏移，可能返回负偏移、越界、转换或默认值错误（见 `pkg/table/column.rs:1168`）。
- 若主索引中找不到 common-handle 列，或该列仅为前缀索引，本函数不会错误地从句柄构造完整值，而是要求从 row value 解码。
- 缺失的虚拟生成列被有意跳过；调用方不能把此处的默认 Datum 当作已计算的生成列结果。
- API 使用 `&[Arc<Column>]`，Rust 侧不存在 Go `[]*Column` 中的 nil 列元素分支。传入的 `Handle` 必须与表元数据的句柄形态匹配；例如整数主键路径会调用 `IntValue()`。
- `meta.Columns`、主索引列偏移与请求 `ColumnInfo` 必须来自一致模式；函数能防护默认值数组越界，但不会全面校验元数据内部一致性。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或长期缓存。输入均为共享借用；列通过 `Arc<Column>` 共享所有权，但函数只读列数据，没有锁操作。

解码出的 `Datum`、row map、结果向量和默认值缓存均属于当前调用。`Handle::EncodedCol` 返回的字节、`value.to_vec()` 以及克隆的字段类型和 Datum 会产生调用内分配；函数返回后仅结果向量和 row map 继续存活。默认值缓存不会跨行复用，因此表达式默认值最多在本次调用、同一列偏移内复用。事务和 KV value 的获取、快照一致性及取消传播均由上游负责。

## 与 Go 版本的对应关系

直接对照是 `pkg/table/tables/tables.go::DecodeRawRowData`（约 `1074` 行）。两版保持相同主序：恢复整数句柄列；识别可由 common handle 完整恢复的列和前缀列；按目标类型解码 row map；优先采用真实存储值；跳过虚拟生成列；最后补变更列或默认值。Go 的 `containFullColInHandle` 对应 Rust 在主索引上的内联搜索，Go 的 `GetChangingColVal`/`GetColDefaultValue` 对应 Rust `pkg/table/column.rs` 中的同名函数。

已确认的 Rust 适配差异：

- Rust 把核心逻辑拆成接受 `TableInfo` 和 `use_new_collation` 的 `DecodeRawRowDataWithMeta`，便于不构造具体表实现的独立测试；Go 只有表对象入口。
- Rust 请求列类型为 `Arc<Column>`，不需要 Go 对 nil 列指针的跳过逻辑。
- Rust 默认值缓存为 `Vec<Option<Datum>>`，可准确区分未计算与已计算出的 NULL；Go 使用 `[]Datum` 的 NULL 状态作为缓存哨兵。
- Rust 对负数和越界模式偏移显式返回错误；Go 对应代码直接按偏移索引。
- Go 的 `RowWithCols` 已把解码函数接入 KV 读行流程，并由 `pkg/table/tables/tables_test.go` 验证普通列、列子集和无符号主键。当前 Rust 搜索未找到等价运行时接线，因此这些 Go 测试只能作为目标语义证据，不能作为 Rust 集成覆盖声明。

## 扩展指南

- 增加新的句柄表示或恢复规则时，优先修改 `DecodeRawRowDataWithMeta` 的第一遍分类，同时保持“仅完整值可跳过 row value 解码”的不变量；在独立测试文件中覆盖完整列、前缀列和 restored-data 排序规则分支。
- 改变缺列策略时，应在第二遍填充逻辑中区分真实缺失、虚拟生成列、在线 DDL 变更列和普通默认值，且保持 row map 只表示实际解码内容。
- 修改默认值或变更列语义时，应同步检查 `pkg/table/column.rs::{GetColDefaultValue, GetChangingColVal}`，并在 `pkg/table/column_test.rs` 增加回归测试；测试逻辑不要内嵌到生产文件。
- 若要把能力接入 Rust 运行时，应在持有 KV value 与 `Table` 的读行边界调用 `DecodeRawRowData`，并增加独立集成测试，至少覆盖列子集、有符号/无符号整数句柄、common handle、旧模式缺列和错误传播。不要仅凭 Go `RowWithCols` 的存在假定 Rust 已接线。
- 性能敏感修改需留意 `value.to_vec()`、字段类型装箱、Datum 克隆、主索引逐列搜索及每行 HashMap/HashSet 分配；优化不能改变请求顺序、列 ID 映射或时区/排序规则语义。
- 兼容性审查应同时核对 Go `pkg/table/tables/tables.go::DecodeRawRowData`，尤其是 common-handle 前缀、在线 DDL 和默认值分支。

## 验证依据

已核对的事实来源：

- `pkg/table/raw_row.rs`：两个公开函数、全部分支、返回类型和错误转换。
- `pkg/table/lib.rs`：模块声明、crate 级再导出及独立测试挂载。
- `pkg/table/Cargo.toml`：`astersql-table` crate 边界、直接依赖和无 feature 门控事实。
- `pkg/table/column.rs:1145`、`pkg/table/column.rs:1168`：默认值与在线 DDL 变更列的下游实现。
- `pkg/table/column_test.rs::{go_merge_49_raw_row_decode_restores_changing_column, go_merge_49_raw_row_decode_restores_integer_handle, raw_row_defaults_use_full_schema_for_null_after_hidden_column}`：Rust 回归边界。
- `pkg/table/tables/tables.go::{RowWithCols, DecodeRawRowData, GetChangingColVal, GetColDefaultValue}`：Go 主链和移植语义。
- `pkg/table/tables/tables_test.go` 的 `RowWithCols`、`TestUnsignedPK` 与记录迭代相关断言：Go 普通行、列子集和无符号句柄行为。
- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/table/raw_row.rs` 显示 10 个符号；`node --file pkg/table/raw_row.rs --offset 1 --limit 260` 读取完整 144 行并报告 4 个文件级使用者；`query` 精确定位两个入口及 `GetChangingColVal`/`GetColDefaultValue`。精确 `callers` 查询未产出可用结果，故调用结论由 `rg` 的符号级结果交叉核对。

本任务为纯文档分析，按计划未运行 Cargo。结构验证使用任务指定命令，要求本文恰有 11 个固定二级标题；交付前另以 `git diff --check` 检查文档补丁格式。
