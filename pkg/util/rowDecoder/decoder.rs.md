# `pkg/util/rowDecoder/decoder.rs`

## 文件定位

本文件是 `astersql-util-rowDecoder` crate 的实际实现文件。crate 入口 `pkg/util/rowDecoder/lib.rs` 以 `mod decoder` 引入它，再用 `pub use decoder::*` 重导出全部公共 API；根 workspace 的 `Cargo.toml` 将该 crate 列为成员，并通过 `facade_util_rowDecoder` 暴露到 `pkg/lib.rs` 的 `util::rowDecoder` 门面。`pkg/util/rowDecoder/Cargo.toml` 没有声明第三方依赖，说明这里当前使用的是自包含的简化数据模型和行编码，而不是直接组合 Rust 版 `tablecodec`、`types`、`table` 或 `expression` crate。

从接线状态看，`pkg/ddl/Cargo.toml` 和 `pkg/executor/Cargo.toml` 虽声明了该 crate，但对 Rust 源码检索未找到本文件公共 API 的生产调用；可确认的直接 Rust 调用者位于独立测试 `pkg/util/rowDecoder/decoder_test.rs`。因此它目前是已纳入 workspace、已被 facade 导出的移植实现，但不能据此声称已经替代 Go 主链。Go 对应实现 `pkg/util/rowDecoder/decoder.go` 则真实接入 DDL 回填、executor sample 和 admin 行检查等路径。

## 核心职责

- `RowDecoder` 把一段行 payload 解码成“列 ID → `Datum`”映射，并把行 key 中携带的整数 handle 或 common handle 补进该映射。
- 解码时只接收 `column_types` 中列出的目标列；未知列仍会被完整解析并跳过，以允许调用方只取所需列（`decode_row_with_map`）。
- 对 payload 缺失的普通列，把默认值放入内部 `mutable_row`，供后续生成列表达式读取；完整路径不会把普通默认值补入返回 map，这一点由 `TestRowDecoder` 明确验证。
- 对 DDL 列类型变更列，从 `ChangeStateInfo::source_column_id` 找源值并转换到目标类型；源值缺失时回退到目标列默认值（`DecodeAndEvalRowWithMap`）。
- 生成列按列 `offset` 升序求值并写回内部行及返回 map，保证后面的生成列可以看到前面已计算的槽位（`EvalRemainedExprColumnMap`）。
- `DecodeTheExistedColumnMap` 支持分阶段处理：只解码已有列并补普通默认值，生成列和变更列先保持 `Null`，留给后续统一转换/求值。
- `EncodeRow` 是公开的对称简化编码器，主要用于独立测试构造本文件能够读取的新/旧格式 payload；它不是 Go `tablecodec.EncodeRow` 的完整 Rust 等价实现。

## 主要符号

- `ExtraHandleID: i64 = -1`：没有显式 PK handle 时，整数 handle 写入的隐式列 ID，对应 Go 的 `model.ExtraHandleID` 用途。
- `NEW_ROW_FORMAT_MARKER: u8 = 0x80`：本地新格式头标记。新格式随后使用两字节大端列数；旧格式没有头和列数。
- `DecodeError`：统一错误枚举，分为 `InvalidRow`、`InvalidHandle`、`Cast`、`Eval`；实现 `Display` 和标准 `Error`。
- `Datum`、`FieldKind`、`FieldType`：本文件自有的简化值与类型系统，只覆盖 null、整数、无符号整数、浮点、字节、字符串和布尔值。
- `ColumnInfo`、`ChangeStateInfo`、`TableColumn`、`Column`：分别描述列 ID/offset/类型、列变更源、默认值和可选生成表达式。
- `Expression`：要求 `Send + Sync` 的求值 trait；闭包只要满足相同签名与线程安全约束即可自动实现该 trait。
- `TableMeta`、`Table`、`SchemaColumn`、`Schema`、`BuildContext`、`Handle`：对 Go 表元数据、表达式 schema、构建上下文和 KV handle 的最小替身。
- `RowDecoder`：保存表、按 offset 排列的可变行、目标列映射/类型、默认值缓存、完整列列表和 handle 目标列。字段私有，通过 `table()`、`columns()` 和 `CurrentRowWithDefaultVal()` 提供只读观察或快照。
- `NewRowDecoder`：构造入口。由最大列 offset 推导行宽，从 common handle、PK-is-handle、`ExtraHandleID` 三种情况推导 `pk_columns`。
- `DecodeAndEvalRowWithMap`：完整解码、默认值/变更列处理、生成列求值入口。
- `DecodeTheExistedColumnMap` 与 `EvalRemainedExprColumnMap`：分阶段入口，对应 Go DDL 列类型变更第一阶段的接口形态。
- `BuildFullDecodeColMap`：按列 offset 从 `Schema` 取得虚拟表达式并建立列 ID 映射；schema 太短会像 Go 直接索引一样 panic，独立测试对此有 `#[should_panic]` 约束。
- `decode_row_with_map`、`decode_handle_to_datum_map`、`cast_column_value`、`decode_datum`：内部解码和转换流水线。
- `EncodeRow`、`encode_datum`、`encode_bytes_datum`：与本地解码格式配套的编码流水线。

## 执行流程

1. 调用方用 `NewRowDecoder` 传入表元数据、完整列序列和需要解码的列映射。构造器复制目标列类型，按最大 offset 建立 `mutable_row`/`default_values`，并确定 handle 应落入哪些列。
2. `DecodeAndEvalRowWithMap` 调用 `decode_row_with_map`。若首字节为 `0x80`，先读取声明列数；随后循环读取 8 字节大端列 ID和一个带 tag 的 `Datum`。目标列的值会按字段类型转换后写入调用方提供或新建的 map，非目标列只被消费。
3. `decode_handle_to_datum_map` 把 key 中的 handle 合并到 map。整数 handle 写入第一个 PK 目标列，目标为 `UInt` 时执行 `i64 as u64`；common handle 必须与 PK 列数量完全一致，并逐列转换。
4. 完整路径遍历 `column_map`：已有值直接写入对应 offset；生成列先写 `Null`；变更列取源列或默认值后转换；其余缺失列读取默认值。后三类结果先进入 `mutable_row`，普通默认值不会自动加入返回 map。
5. `EvalRemainedExprColumnMap` 将列按 offset 排序，依次用当前 `mutable_row` 求值，转换为目标类型，再同时更新槽位与返回 map。
6. 分阶段路径 `DecodeTheExistedColumnMap` 的前两步相同，但生成列和变更列均只写 `Null`；缺失普通列既写内部行也写返回 map。调用方可在外部完成变更列转换后，再调用 `EvalRemainedExprColumnMap`。
7. `EncodeRow` 执行逆向的测试格式编码：可选写新格式头和 `u16` 列数，每列写大端 ID 与 tag/value；字节和字符串使用 `u32` 大端长度。

## 数据与状态

`RowDecoder` 是有状态且可复用的对象。`mutable_row` 保存最近一次解码过程中按 offset 排列的值，生成表达式从该数组读取依赖；`CurrentRowWithDefaultVal` 返回 clone，调用方修改快照不会反向影响 decoder。`default_values` 以 `Datum::Null` 同时表示“尚未缓存”和“缓存值就是 NULL”，因此 NULL 默认值会重复复制，但语义不变。`column_default` 与 `set_row_value` 都按 offset 操作，后者可在输入列 offset 超出构造时推导的宽度时同步扩展两个数组。

传入 `DecodeAndEvalRowWithMap`/`DecodeTheExistedColumnMap` 的 map 按值取得所有权并返回；预置项会保留，payload 和 handle 对同一 ID 的后写值会覆盖旧值。`HashMap` 的遍历顺序不稳定，但普通列填充互不依赖；生成列另行按 offset 排序，从而消除求值顺序的不确定性。`columns` 保存完整列定义，当前本文件仅通过访问器暴露；变更列简化逻辑直接使用 `change_state_info`，没有像 Go 的 `tables.GetChangingColVal` 那样遍历完整列集合。

## 依赖与调用关系

内部调用主链为 `DecodeAndEvalRowWithMap` → `decode_row_with_map` / `decode_handle_to_datum_map` → `column_default` / `cast_column_value` / `set_row_value` → `EvalRemainedExprColumnMap`。行解析继续下沉到 `decode_datum`；测试编码由 `EncodeRow` → `encode_datum` → `encode_bytes_datum` 完成。`BuildFullDecodeColMap` 独立负责把 schema 表达式装配到列定义中。

crate 只依赖标准库的 `HashMap`、`Arc`、错误和格式化 trait。`Arc<TableColumn>` 避免在 decoder、列映射和调用方之间复制列定义，`Arc<dyn Expression>` 允许共享动态表达式。`pkg/util/rowDecoder/lib.rs` 是模块出口，根 `pkg/lib.rs::util::rowDecoder` 是 facade 出口。

RustCodeGraph 将 `NewRowDecoder` 的可见直接调用定位到 `pkg/util/rowDecoder/decoder_test.rs` 的 `build_decoders` 和 `TestClusterIndexRowDecoder`；对目标方法的精确 callers/callees 查询没有返回生产调用边。普通源码检索也未发现目标 API 在其他 Rust 文件中的使用，因此当前 Rust 生产主链应记为“未接线”，而不是从 Go 同名调用者推断 Rust 已接线。Go 的直接上游证据包括 `pkg/ddl/index.go`、`pkg/ddl/column.go`、`pkg/ddl/partition.go`、`pkg/executor/sample.go` 和 `pkg/util/admin/admin.go`。

## 错误处理与边界

- 新格式少于三字节、列 ID不足八字节、datum tag/数值/长度/内容截断、未知 tag、非法布尔字节及声明列数不符均返回 `DecodeError::InvalidRow`。旧格式没有声明列数，只能靠逐项边界检查发现损坏。
- 字符串 datum 必须是 UTF-8；无效 UTF-8 在行解析时归为 `InvalidRow`，在 Bytes→String/Int 转换时归为 `Cast`。
- common handle 的值数必须等于 PK 列数；整数 handle 没有目标列时返回 `InvalidHandle`。常规构造器总会选择 common PK、显式 PK 或 `ExtraHandleID`，该错误主要保护异常内部状态。
- 转换规则是简化的 Rust `as`/字符串解析语义：负整数转 `UInt`、过大 `UInt` 转 `Int` 都可能按位回绕；浮点转整数按 Rust cast 规则处理。这里没有 Go `types` 的 SQL mode、截断告警、collation、时区或精度语义。
- `FieldKind::Bytes`/`String` 对部分其他值使用 `Debug` 文本，不等价于完整 SQL cast；`Bool` 只接受布尔或整数类。
- `BuildFullDecodeColMap` 对 schema offset 越界选择 panic 而非 `Result`，测试要求保留与 Go 直接索引相同的失败方式。
- `EncodeRow` 限制列数为 `u16`，单个变长值长度为 `u32`；超限返回 `InvalidRow`。浮点以原始 bits 编码，可保留 NaN payload。
- 表达式自身的错误按 `DecodeError` 原样传播；目标类型转换失败则返回 `Cast`，后续列不会继续计算，decoder 可能保留部分更新后的内部行。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部 I/O。`RowDecoder` 的两个解码入口和生成列求值都要求 `&mut self`，同一个实例不能在安全 Rust 中被并发修改；若上层需要并行解码，应为每个 worker 建立实例，或在外部加锁并接受串行化。

`Expression: Send + Sync` 以及 `Arc<dyn Expression>` 允许表达式对象在线程间安全共享，但这并不会让 `RowDecoder` 自身变为无锁并发解码器。所有行 map、payload 切片和临时排序向量均限定在一次调用内；`Arc` 在最后一个所有者释放时回收列/表达式，`Vec`/`HashMap` 由所有权自动释放。复用 decoder 可以复用逻辑状态，但当前方法仍会克隆列映射值和为生成列顺序分配临时 `Vec`。

## 与 Go 版本的对应关系

Rust 的 `RowDecoder`、`Column`、`NewRowDecoder`、`DecodeAndEvalRowWithMap`、`DecodeTheExistedColumnMap`、`EvalRemainedExprColumnMap`、`CurrentRowWithDefaultVal` 和 `BuildFullDecodeColMap` 均与 `pkg/util/rowDecoder/decoder.go` 的同名符号对应。共同语义包括：三种 handle 目标选择、先填内部可变行再求值生成列、生成列按 offset 顺序执行、普通默认值在完整路径中只供内部行使用，以及分阶段路径暂不处理生成列/变更列。

重要差异如下：

- Go 使用真实的 `table.Table`、`chunk.MutRow`、`types.Datum`、`expression.Expression`、`kv.Handle`、`tablecodec` 和 `rowcodec`；Rust 在单文件内定义简化替身及私有编码格式，类型覆盖和 SQL 兼容性明显更窄。
- Go 根据 `rowcodec.IsNewFormat` 分派 `tablecodec.DecodeRowWithMapNew`/`DecodeRowWithMap`，并接收 `decodeLoc`；Rust 只识别本地 `0x80 + u16 count` 格式，`BuildContext::time_zone` 也没有参与解析或 cast。
- Go 默认值和列变更分别委托 `tables.GetColDefaultValue`、`tables.GetChangingColVal`，带完整上下文和错误规则；Rust 仅 clone 默认值或从单个 source ID 取值再做简化 cast。
- Go 生成表达式通过 `EvalCtx` 和真实 `chunk.Row` 运行，并调用 `table.CastColumnValue`；Rust trait 接收简化 `BuildContext` 与 `&[Datum]`。
- Go 主链已有 DDL/executor/admin 调用者；Rust 当前只有 crate/facade 声明和独立测试调用证据，生产接线未验证到。
- `pkg/util/rowDecoder/decoder_test.rs` 移植了 Go `TestRowDecoder` 与 `TestClusterIndexRowDecoder` 的关键意图：默认值参与生成列、NULL 传播、有符号/无符号整数 handle、common handle，以及无生成列路径。它用秒数整数代替 timestamp/duration，用 `AddColumns` 代替 SQL 表达式，不能覆盖 Go 的真实类型系统、时区和 collation 行为。

## 扩展指南

- 扩充值类型或编码格式时，应成对修改 `Datum`、`FieldKind`、`cast_column_value`、`decode_datum`、`encode_datum`，并在独立 `pkg/util/rowDecoder/decoder_test.rs` 增加正常值、截断、非法 tag、溢出和新旧格式用例；不要把测试嵌回生产文件。
- 接入真实 AsterSQL 行格式时，优先复用现有 Rust `tablecodec`/`rowcodec`/类型与表达式抽象，明确迁移掉本地简化 codec 的兼容边界，而不是继续扩张一套平行协议。必须用真实编码 fixture 验证 Go/Rust 互操作。
- 调整 handle 规则时集中修改 `NewRowDecoder` 和 `decode_handle_to_datum_map`，同步覆盖负整数到 unsigned、无显式 PK、common handle 数量/类型错误，以及 payload 与 handle 同列覆盖顺序。
- 调整默认值或列变更语义时关注两条入口的刻意差异：完整路径的普通默认值不进入返回 map，分阶段路径会进入；变更列只能在完整路径转换。需要新增与 Go DDL reorg 相同的分阶段测试后才能安全接线。
- 新增生成列能力应保持 offset 有序不变量，并测试生成列依赖前序生成列、表达式错误后的部分状态、schema offset 越界策略以及目标类型 cast。
- 若开始在 DDL/executor Rust 生产代码中调用，应先确认实例生命周期为 worker/请求隔离，避免跨并发任务共享可变 `RowDecoder`；同时评估每行 clone/sort/allocation 的性能，必要时在构造期预计算有序生成列列表。
- Go 对照行为发生变化时，应同时检查 `pkg/util/rowDecoder/decoder.go`、`decoder_test.go` 和 Rust 独立测试，不能仅靠当前简化测试宣称完整一致。

## 验证依据

- Rust 源码：`pkg/util/rowDecoder/decoder.rs`（624 行），核对了全部常量、类型、trait、函数、`impl` 和错误分支；文件无条件编译项。
- crate/出口：`pkg/util/rowDecoder/Cargo.toml`、`pkg/util/rowDecoder/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`，确认 workspace 成员、porting 元数据、无 crate 级依赖以及 facade 重导出；另核对 `pkg/ddl/Cargo.toml`、`pkg/executor/Cargo.toml` 的依赖声明。
- Rust 独立测试：`pkg/util/rowDecoder/decoder_test.rs`，确认 `TestRowDecoder`、`TestClusterIndexRowDecoder` 和 short-schema panic；`pkg/util/rowDecoder/main_test.rs` 只覆盖一次性测试初始化，不覆盖 decoder 运行逻辑。
- Go 对照：`pkg/util/rowDecoder/decoder.go`、`pkg/util/rowDecoder/decoder_test.go`、`pkg/util/rowDecoder/main_test.go`；直接调用位置通过 `pkg/ddl/index.go`、`pkg/ddl/column.go`、`pkg/ddl/partition.go`、`pkg/executor/sample.go`、`pkg/util/admin/admin.go` 交叉核对。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `decoder.rs` 有 64 个符号；`files --filter pkg/util/rowDecoder` 确认同目录七个索引文件；`explore`/`query` 定位 `RowDecoder`、`NewRowDecoder`、`DecodeAndEvalRowWithMap`、`EvalRemainedExprColumnMap`，并将 Rust `NewRowDecoder` 的直接调用归到独立测试。精确 callers/callees 查询未返回目标 Rust 方法的生产边，因此又以 Rust 源码检索确认当前未见生产 API 使用。
- 本任务是纯文档分析，依任务说明不运行 Cargo。交付前使用任务指定命令验证本文恰有十一个固定二级章节，并人工复核所有“已接线/已支持”陈述都有上述源码、索引、manifest 或测试证据。
