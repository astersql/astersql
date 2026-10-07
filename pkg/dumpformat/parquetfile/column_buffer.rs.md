# `pkg/dumpformat/parquetfile/column_buffer.rs`

## 文件定位

本文件属于 `astersql-dumpformat-parquetfile` crate，crate 根由 [`lib.rs`](lib.rs) 公开为 `column_buffer` 模块。它位于 Parquet 写出链的中间层：上游 [`writer.rs`](writer.rs) 根据 SQL 列元数据建立 `Column` 列描述并创建每列缓冲，下游 [`column_value.rs`](column_value.rs) 将解析后的 `ColumnValue` 追加到相应物理类型向量，最终由 `ParquetWriter::flush_rows` 把这些向量交给 `parquet` crate 的列写入器。

该文件不负责 SQL 文本解析、Parquet schema 构造或 I/O；它只定义“一个 row group 尚未写出时，单列数据如何驻留内存”、缓冲如何初始化，以及写出后如何复用。源码中的有效实现位于 `ColumnBuffer`、`new_column_buffer`、`new_column_buffers` 及两个 Go 风格别名；文件前半段的注释代码是迁移说明，不参与编译。

## 核心职责

1. `ColumnBuffer` 为七种已支持的 Parquet 物理类型各保留一个强类型 `Vec`，并为可空编码独立保存 `definition_levels`。
2. `new_column_buffer` 根据 `Column.column_type.physical` 只为实际使用的值向量预留 `capacity`，避免所有类型同时分配；仅当 `Column.allows_null_encoding` 为真时预留定义级别容量。
3. 构造阶段拒绝 `PhysicalType::Int96` 和非正宽度的 `FixedLenByteArray`，把必然无法安全写出的列挡在 writer 初始化之前。
4. `new_column_buffers` 按输入列顺序构造等长缓冲数组，并在失败信息中加入列名，便于定位 schema 中的坏列。
5. `ColumnBuffer::reset` 清空长度但不释放 `Vec` 容量，使同一 `ParquetWriter` 能跨 row group 复用已分配内存。

## 主要符号

- `pub struct ColumnBuffer`：单列 row-group 缓冲。它派生 `Clone`、`Debug`、`Default`、`PartialEq`；八个字段均公开，供同 crate 的追加、写批次和测试逻辑直接访问。
  - `definition_levels: Vec<i16>`：可空列每行的存在性级别；当前 writer 使用 `0` 表示 NULL、`1` 表示存在。
  - `bool_values`、`int32_values`、`int64_values`、`float32_values`、`float64_values`：定长物理值。
  - `byte_array_values`、`fixed_len_byte_array_values`：以拥有所有权的 `Vec<u8>` 保存变长和定长字节值。
- `ColumnBuffer::reset(&mut self)`：对全部八个向量调用 `clear()`。它恢复空缓冲状态，但刻意保留容量。
- `new_column_buffer(column: &Column, capacity: usize) -> Result<ColumnBuffer>`：单列构造入口。先验证定长字节宽度，再按可空性及 `PhysicalType` 分配相应向量。
- `new_column_buffers(columns: &[Column], capacity: usize) -> Result<Vec<ColumnBuffer>>`：批量构造入口。输出顺序与 `columns` 完全一致；任一项失败时短路，并包装 `init parquet buffer for column <name>` 上下文。
- `newColumnBuffer`、`newColumnBuffers`：仅为 Go 风格命名提供的公开转发函数，没有独立状态或分支。

本文件没有模块级常量、trait、条件编译项或异步入口。

## 执行流程

正常写出路径如下：

1. `ParquetWriter::new` 在 [`writer.rs`](writer.rs) 中先调用 `build_parquet_schema_from_columns` 得到 `Vec<Column>`，再调用 `new_column_buffers(&columns, 0)`。当前生产入口传入容量 `0`，构造函数仍保留非零容量参数供直接调用者和测试预分配。
2. `new_column_buffers` 顺序遍历列；每列进入 `new_column_buffer`。后者先检查 `FixedLenByteArray` 的 `type_length > 0`，再创建默认空缓冲，为 nullable 列的 `definition_levels` 预留容量，最后仅为匹配的物理值向量预留容量。
3. 每写入一行，`ParquetWriter::append_parsed` 依据 `allows_null_encoding` 追加定义级别；非 NULL 值由 [`column_value.rs`](column_value.rs) 的 `append_column_value` 按 `(PhysicalType, ColumnValue)` 组合推入本结构对应向量。
4. 达到 row-group 内存阈值或关闭 writer 时，`ParquetWriter::flush_rows` 按列取出相应值向量和可选定义级别，调用 `parquet` crate 的类型化 `write_batch`。
5. 仅在所有列写入、列关闭及 row group 关闭均成功后，`flush_rows` 才将行数和内存计数归零，并逐列调用 `ColumnBuffer::reset`。因此失败发生在此之前时，缓冲不会被误标为空。

异常构造路径在分配实际值向量前返回：非法定长宽度返回 `invalid fixed-size byte width <n>`；`Int96` 返回 `unsupported parquet physical type Int96`；批量入口再为错误加上具体列名。

## 数据与状态

核心不变量是“每个 `ColumnBuffer` 对应同索引的一个 `Column`，且通常只有该列物理类型对应的值向量会增长”。类型选择不存储在缓冲自身，而由并行的 `Column` 元数据决定；调用方必须始终以同一列描述追加和写出该缓冲。

可空列的行数由 `definition_levels.len()` 表示，其中 level 大于零的条目数应等于物理值向量长度；NULL 行只追加 level，不追加物理值。required 列不使用定义级别，其物理值向量长度应等于缓冲行数。这些计数关系由 `writer.rs` 的追加与写出逻辑维护，并在其编码辅助路径中显式校验。

`capacity` 只影响预分配，不改变长度和可观察数据。`Vec::clear()` 会丢弃元素（包括内部字节数组的所有权）但保留外层向量容量；因此 reset 后缓冲逻辑上为空，可用于下一 row group。字节数组的每个值由 `parse_raw_column_value` 通过 `to_vec()` 拷贝后持有，不借用调用者输入。

## 依赖与调用关系

直接内部依赖只有 [`column_type.rs`](column_type.rs) 的 `Column`、`PhysicalType`，以及 [`lib.rs`](lib.rs) 定义的字符串包装 `Error` 和 `Result`。crate 的 [`Cargo.toml`](Cargo.toml) 将库入口设为 `lib.rs`，并通过带 tag 的 Git 依赖引入 `parquet`；本文件本身不直接引用该外部 crate 类型，而是在 `writer.rs` flush 时转换成 `ByteArray`/`FixedLenByteArray`。

RustCodeGraph 与源码交叉核对出的生产调用边为：

- `ParquetWriter::new` → `new_column_buffers` → `new_column_buffer`；
- `ParquetWriter::append_parsed` → `append_column_value` → `ColumnBuffer` 的类型化值向量；
- `ParquetWriter::flush_rows` → `ColumnBuffer` 字段 → `parquet::column::writer::ColumnWriter::write_batch`；
- `ParquetWriter::flush_rows` 成功尾部 → `ColumnBuffer::reset`。

测试调用者包括 [`writer_test_helpers_test.rs`](writer_test_helpers_test.rs) 的类型分派、定义级别与 reset 测试，以及 [`writer_behavior_test.rs`](writer_behavior_test.rs) 的非法宽度、`Int96`、批量错误上下文和浮点分派测试。Go 对应生产边为 [`writer.go`](writer.go) 的 `NewWriter` → `newColumnBuffers`，以及 row-group 写出后对各缓冲调用 `reset`。

## 错误处理与边界

- `FixedLenByteArray` 的 `type_length <= 0` 会在构造时失败，避免后续按负数/零宽度转换或让列 writer panic。实际值的长度是否恰好等于该宽度由 `column_value.rs` 的解析路径和 `writer.rs` 的写出检查继续保证。
- `PhysicalType::Int96` 明确不受支持。虽然枚举和 schema 映射仍包含该变体，本文件不会为它创建值缓冲，因而不能把“schema 可描述”误认为“writer 可写”。
- 批量构造使用迭代器 `collect::<Result<Vec<_>>>()` 的短路语义：首次错误即返回，不暴露部分构造结果；错误文本保留底层原因并加入列名。
- `capacity` 是 `usize`，不存在负数输入；非常大的值可能因 `Vec::with_capacity` 的分配失败而触发 Rust 分配器层面的失败，本文件没有把 OOM 转换为 `Result`。
- 公开字段允许外部代码直接破坏“定义级别数/值数/物理类型”不变量；最终 writer 会在部分路径发现计数或类型问题，但安全扩展应优先复用 `append_column_value`，不要直接写错向量。
- `reset` 本身不返回错误，也不重置 writer 的 `buffered_rows` 或 `buffered_memory_bytes`；这两个计数由 `ParquetWriter::flush_rows` 在调用 reset 前同步归零。

## 并发与资源生命周期

`ColumnBuffer` 不包含锁、原子量、通道、任务、文件句柄或事务。其可变操作要求 `&mut self`，并发协调由持有它的 `ParquetWriter` 负责；本文件没有提供跨线程共享或内部同步保证。虽然字段类型通常可随所有权移动到其他线程，但不能据此推断同一缓冲支持并发写入。

生命周期分为构造、逐行追加、row-group 写出、复用四段。构造时分配外层向量容量；追加阶段拥有实际值；成功 flush 后 `reset` 释放元素并保留外层容量；`ParquetWriter` 被关闭或丢弃时，缓冲及剩余容量随其一起释放。失败的 flush 不会执行 reset，便于错误向上传播时保留尚未完整提交的状态，但当前 writer 不承诺从这种失败中恢复。

## 与 Go 版本的对应关系

直接对照文件是 [`column_buffer.go`](column_buffer.go)：`columnBuffer` 对应 `ColumnBuffer`，`reset` 对应 `ColumnBuffer::reset`，`newColumnBuffer(s)` 对应 snake_case 主实现及同名 Go 风格转发函数。两版都按物理类型只初始化一个值切片/向量，都只为允许 NULL 编码的列初始化定义级别容量，也都拒绝非正的 fixed-len 宽度与未支持的物理类型；批量错误文本都包含 `init parquet buffer for column <name>`。

需要注意的表示差异：Go 的 byte-array 字段使用 Arrow Go 的 `parquet.ByteArray`/`FixedLenByteArray`，Rust 缓冲直接拥有 `Vec<u8>`，到 flush 时才转换为 `parquet-rs` 类型；Rust `Default` 产生非 null 的空 `Vec`，所以 Go 测试中用 `NotNil` 观察初始化的细节不能原样作为 Rust 的语义断言，Rust 测试验证的是容量/追加/读回行为。Rust 对不支持项把枚举分支写成显式 `Int96`，而 Go 使用 `switch default`，当前可观察错误意图一致。

[`writer_behavior_test.go`](writer_behavior_test.go) 与 Rust 的 [`writer_behavior_test.rs`](writer_behavior_test.rs) 均覆盖非法 fixed width、`Int96`、批量列名上下文以及 Float/Double 初始化；Rust 的 [`writer_test_helpers_test.rs`](writer_test_helpers_test.rs) 还直接验证 reset 后读出的批次为空和 definition levels 与值相互独立。

## 扩展指南

新增物理类型时，不能只在本文件增加一个向量。至少应同步更新 `PhysicalType`/列映射、`ColumnBuffer` 字段、`new_column_buffer` 分配分支、`reset`、`column_value.rs` 的解析/追加/内存估算/批次读取，以及 `writer.rs` 的 schema 映射和类型化 `write_batch` 分支；同时对照 Go 的 `column_buffer.go`、`column_value.go`、`writer.go` 保持行为和错误文本意图。

如果改变 NULL 编码，应优先修改 `ParquetWriter::append_parsed` 与 flush 规则，并保持“定义级别条目数等于 buffered rows、非零 level 数等于物理值数”的不变量。若引入 repetition levels 或更深嵌套，当前单一 `Vec<i16>` 模型不足，需要连同 schema 和 writer 一起设计，不能仅扩充本结构。

若要优化容量，生产构造当前传 `0`；可以从预计 row-group 行数推导容量后传入，但须评估多列、变长值和峰值内存，避免按最大行数为所有列过度预留。reset 复用策略改变时还需关注大 row group 后长期保留峰值容量的风险。

测试应继续放在独立文件而非 `column_buffer.rs` 内。优先扩展 `writer_test_helpers_test.rs` 覆盖类型分派、容量复用和定义级别，扩展 `writer_behavior_test.rs` 覆盖构造错误上下文及 writer 集成行为；Go 语义变化还应同步相应 `*_test.go`。兼容性重点是物理类型选择、错误边界、NULL/value 计数和 row-group flush 后状态，性能重点是分配次数与保留容量。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/dumpformat/parquetfile` 确认目标、writer、Go 对照和独立测试均在索引中。
- RustCodeGraph 源码/调用查询：`node --file pkg/dumpformat/parquetfile/column_buffer.rs` 确认全部 206 行及 7 个符号；限定查询确认 `new_column_buffers` 的生产调用者是 `ParquetWriter::new`，测试调用者包括 `parquet_writer_conversion_errors_leave_writer_unusable_like_go`；源码节点进一步核对了 `writer.rs` 的构造、追加、flush、reset 链。
- 读过的生产文件：`pkg/dumpformat/parquetfile/column_buffer.rs`、`column_type.rs`（由目标符号定义和引用查询核对）、`column_value.rs`、`writer.rs`、`lib.rs`、`Cargo.toml`、Go 对照 `column_buffer.go` 与 `writer.go` 的直接调用位置。
- 读过的独立测试：`writer_test_helpers_test.rs`、`writer_behavior_test.rs`、`writer_behavior_test.go`。它们验证可空定义级别、各值类型分派、reset、非法 fixed width、`Int96`、批量列名错误上下文和 Float/Double 缓冲。
- 结构验证使用任务指定命令，要求本文件存在且恰有十一个固定二级标题。本任务是纯文档分析，按计划不运行 Cargo 或代码测试；关于行为的结论来自已索引源码、Go 对照与现有独立测试，而不是本次执行测试的结果。
