# [`pkg/dumpformat/testutils/parquet_writer.rs`](./parquet_writer.rs)

## 文件定位

本文件是 `astersql-dumpformat-testutils` crate 的 Parquet 测试数据写入夹具，而不是数据库运行时或逻辑导出的生产写入器。`pkg/dumpformat/testutils/lib.rs` 将本模块的公开项全部再导出；`pkg/dumpformat/testutils/Cargo.toml` 表明它直接依赖 `astersql-objstore`、`anyhow`，以及固定 tag `astersql-parquet-v60.0.0-streaming-pages.1` 的 `parquet` crate。真正的导出实现位于 `pkg/dumpformat/parquetfile`，本文件只为解析器、导入器和对象存储等测试生成可控的真实 Parquet 文件。

RustCodeGraph 的文件关系显示该文件包含 73 个索引符号。Rust 侧直接行为测试位于 `pkg/dumpformat/testutils/migration_aster_unit_test.rs`；Go 原文件 `pkg/dumpformat/testutils/parquet_writer.go` 则被 `pkg/dumpformat/parquetfile/parser_test.go`、`pkg/executor/importer/import_test.go`、`pkg/lightning/mydump/loader_test.go` 等测试夹具调用。源码没有条件编译项；测试模块由 `lib.rs` 通过 `#[cfg(test)]` 独立挂载，符合源码与测试分文件的仓库约定。

## 核心职责

1. 用 `ParquetColumn` 描述列名、物理类型、转换/逻辑类型、长度、精度、小数位和数据生成器，并由 `make_schema` 建出 required 根节点与 optional 叶子列。
2. 用 `ParquetValueBuffer` 封闭枚举表达 Go `any` type switch 支持的八种物理值缓冲，避免运行时向错误切片类型做静默断言。
3. 用 `calc_value_range` 和 `slice_column_data` 将 row-group 行区间映射到“仅保存非 NULL 值”的紧凑值区间，同时截取对应 definition levels。
4. 用 `WriterProperty`、`WriteMetadata`、`WriteOption` 和 `ParquetWriterOption` 表达 Go 可变 `...any` 参数，默认逐列启用字典和 Snappy，再允许调用方覆盖 row group、page、batch、压缩、字典及文件元数据选项。
5. 用 `WriteWrapper` 将 TiDB/AsterSQL 的 `ObjectWriter` 适配为 parquet-rs 所需的 `std::io::Write`，最终由 `write_parquet_file` 生成并关闭真实 Parquet 对象。

## 主要符号

- `ParquetValueBuffer`：公开值缓冲枚举，支持 `INT96`、`INT64`、`FLOAT`、`DOUBLE`、`BYTE_ARRAY`、`FIXED_LEN_BYTE_ARRAY`、`INT32`、`BOOLEAN`。`len`、`is_empty` 和 `kind` 提供统一观察接口；私有 `slice` 做有界复制切片。
- `ParquetColumnData { vals, def_levels }`：单列生成后的内部数据形状。`def_levels == None` 表示每行都有一个值；存在 levels 时，仅 `level > 0` 的行消耗一个值。
- `calc_value_range(def_levels, row_start, row_end)`：公开纯函数，计算某行区间对应的紧凑值下标区间；拒绝反向区间和超过 levels 长度的区间。
- `slice_column_data(col, row_start, row_end)`：调用 `calc_value_range`，同时复制值子区间与行级 levels 子区间。
- `WriterProperty` / `WriteMetadata` / `WriteOption` / `ParquetWriterOption`：公开、封闭的选项模型；`Unsupported(String)` 专门保留 Go 未知动态类型的报错路径。
- `ParquetGenerator = dyn Fn(i32) -> (ParquetValueBuffer, Option<Vec<i16>>)`：列生成器；接收原始 `rows`，包括负数。
- `ParquetColumn`：公开列描述。`Logical` 存在时优先于 `Converted`；`TypeLen <= 0` 会被规格化为 `-1`。
- `WriteWrapper`：持有 `Box<dyn ObjectWriter>`、background `Context` 和 `closed` 标记；`close` 幂等，`write` 转发到底层，`flush`、`seek`、`read` 是测试适配所需的空操作。
- `get_store`：用 `ParseBackend` 解析 URI，再用 `NewWithDefaultOpt` 创建 `StorageRef`。
- 私有实现函数 `make_schema`、`apply_writer_property`、`build_writer_properties`、`validate_generated_column`、`write_parquet_column_batch`：依次承担 schema 构造、选项应用、默认属性建立、生成结果校验和物理类型写入分派。
- `write_parquet_file`：公开主入口；`WriteParquetFile`、`calcValueRange`、`sliceColumnData`、`getStore` 是供迁移期调用方使用的 Go 风格兼容别名。

## 执行流程

`write_parquet_file(path, file_name, columns, rows, options)` 的流程如下：

1. 将用于循环和校验的 `row_count` 设为 `max(rows, 0)`，但之后仍把原始 `rows` 交给每个 `Gen`，保留 Go 对负行数的行为。
2. `make_schema` 遍历列描述，构造 optional primitive field；若有 `Logical` 就不再应用 `Converted`，最后组装名为 `schema` 的 required group。
3. `build_writer_properties` 先对每列设置字典与 Snappy，再按传入顺序应用覆盖项。未知选项、零 row-group/page/batch 大小在创建对象前失败。
4. `get_store` 解析后端；`StorageRef::Create` 创建目标对象；`WriteWrapper::new` 为 parquet-rs 建立写适配。
5. `SerializedFileWriter::new` 建立文件写入器。每个列生成器恰好调用一次，`validate_generated_column` 校验缓冲物理类型、levels 行数，以及值数量是否等于有值行数。
6. 从最终 `WriterProperties` 取得 row-group 长度；未设置时使用 `row_count.max(1)`。每组取 `[row_start, row_end)`，逐列调用 `next_column`、`slice_column_data`、`write_parquet_column_batch` 和列 writer 的 `close`。
7. 每组写完后额外调用一次 `next_column`，若仍有列 writer，说明 schema 列数多于生成数据并报错；随后关闭 row group。
8. 所有 row group 完成后关闭 `SerializedFileWriter`。无论内部闭包成功还是失败，都会再尝试 `wrapper.close()`；其错误被忽略，函数返回主要写入结果。

负 `rows` 时生成器仍收到负值，但 `row_count` 为 0、row-group 循环不执行，最终形成零行文件。该行为由 `negative_rows_write_an_empty_file_like_go` 明确验证。

## 数据与状态

文件没有全局可变状态。每次调用都独立构造 schema、不可变的 `Arc<WriterProperties>`、整份 `column_data` 和一个对象 writer。

definition level 是最关键的不变量：存在 levels 时，其长度必须等于非负化后的行数，且值缓冲长度必须等于 levels 中 `> 0` 的元素数量；不存在 levels 时，值缓冲长度必须等于行数。row group 切片不能直接按行下标切值数组，而要先统计区间之前和区间内部的非 NULL 行数。实现把每组值与 levels 复制成新 `Vec`，逻辑清晰但会带来与 row group 数量相关的分配和复制成本，因此注释也限定它只用于小型测试文件。

`WriteWrapper.closed` 是唯一的局部生命周期状态，用于保证显式 `close` 幂等。`SerializedFileWriter` 借用 wrapper，不拥有对象存储 writer；内部 writer 结束后才可再次关闭 wrapper。

## 依赖与调用关系

上游入口是 `lib.rs` 的 `pub use parquet_writer::*`。Rust 独立测试直接调用 `calc_value_range`、`slice_column_data` 和 `write_parquet_file`；当前仓库搜索未发现 Rust 生产代码调用本 crate。`pkg/dumpformat/parquetfile/Cargo.toml` 仅在 Windows 目标的 `dev-dependencies` 中引用本 crate，进一步确认它是测试支持边界，而非 parquetfile 生产写入链。

主调用链为：

`write_parquet_file` → `make_schema` → `build_writer_properties`/`apply_writer_property` → `get_store` → `StorageRef::Create` → `WriteWrapper` → `SerializedFileWriter` → 列生成器 → `validate_generated_column` → `slice_column_data`/`calc_value_range` → `write_parquet_column_batch` → 各级 `close`。

下游依赖分为三层：`anyhow` 提供统一 `Result`、上下文和早退；`astersql-objstore` 负责 URI 解析、存储创建及对象流式写入；固定 tag 的 parquet-rs 负责 schema、属性、列 writer、row group 和文件尾元数据。`tempfile` 仅是独立测试的 dev-dependency。

Go 对照文件的直接使用方体现了该夹具的用途：parser 测试覆盖逻辑类型、时间戳、decimal、NULL、多 row group/page 和元数据；importer 与 lightning 测试生成输入文件；这些是 Go 侧调用证据，不代表 Rust 侧已全部迁移接线。

## 错误处理与边界

- schema field/root 构造、后端解析和存储创建通过 `anyhow::Context` 补充列名、阶段或对象名；parquet-rs 与 objstore 错误用 `?` 传播。
- `ParquetValueBuffer::slice` 拒绝反向或越界值区间；`calc_value_range` 拒绝反向行区间和超过 definition-level 长度的终点。
- `apply_writer_property` 拒绝为 0 的 row-group、data-page 和 batch 大小；`Unsupported` 在打开目标对象之前失败，因此不会创建文件。
- `validate_generated_column` 在真正写列之前拒绝物理类型不匹配、levels 行数不匹配和紧凑值数量不匹配。与 Go 的 `any` 类型断言相比，Rust 报错更早、更明确。
- `write_parquet_column_batch` 再次匹配实际 `ColumnWriter` 与值枚举，防止 schema/writer 与缓冲不一致。
- 目标对象一旦创建，后续生成、写入或关闭错误可能留下不完整对象；本文件没有回滚或删除逻辑。调用方不应把失败后的同名对象视为原子地产生。
- `wrapper.close()` 的错误被有意忽略，以贴近 Go `defer pw.Close()` 未检查返回值的行为；因此若主体成功而仅底层对象关闭失败，当前函数也可能返回成功。这是扩展时必须审视的可观察边界。
- `flush`、`seek` 和 `read` 不提供真实能力。它们是兼容测试 writer 的空操作，不可据此认为该适配器支持随机访问或显式刷盘。

## 并发与资源生命周期

本文件不创建线程、任务、锁或通道，也不在多个调用间共享状态；生成器是 `Fn` 而非 `FnMut`，但类型本身没有 `Send`/`Sync` 约束，因此 API 未承诺跨线程使用。所有 I/O 使用 `Context::background()`，调用方不能通过本入口传递取消或截止时间。

资源顺序是“创建 storage → 创建 object writer → 借用 wrapper 创建 file writer → 逐组关闭 column writer 和 row-group writer → 关闭 file writer → 尝试关闭 wrapper”。`WriteWrapper::close` 的布尔标记避免重复关闭底层对象；但没有 `Drop` 实现，所以保证来自 `write_parquet_file` 末尾的显式调用，而不是离开任意作用域时自动关闭。列数据在写入前全部驻留内存，row group 只限制文件布局，不限制生成数据的总内存占用。

## 与 Go 版本的对应关系

Rust 文件逐段对应 `pkg/dumpformat/testutils/parquet_writer.go`：`ParquetColumn`、`calcValueRange`、`sliceColumnData`、物理 writer type switch、`writeWrapper`、`getStore` 与 `WriteParquetFile` 都有等价实现。共同语义包括：叶子列均为 optional、逻辑类型优先于 converted type、每列默认字典加 Snappy、选项按类型分类、按最大 row-group 行数切分、NULL 值不占紧凑值缓冲、Seek/Read 是空操作，以及负行数仍传给生成器但不产生 row group。

Rust 的主要差异是用枚举代替 `any`，因此 API 只能接收已列举类型；同时增加了行区间、属性非零、生成缓冲类型/长度和 schema 列数校验。Go 的切片越界通常 panic，错误类型断言可能得到 nil 切片后由下游失败；Rust 将这些情况转换为 `Result`。Rust 还显式关闭 `SerializedFileWriter` 并传播其错误，但与 Go 一样忽略最终 deferred/包装器关闭错误。

Go 的函数接收 `...any`，Rust 接收 `Vec<ParquetWriterOption>`；新增 Arrow Go 选项不会自动在 Rust 生效，必须扩充封闭枚举及 `apply_writer_property`/`build_writer_properties`。Go 当前有多个成熟调用场景，而 Rust 当前可确认的覆盖重点来自本 crate 的迁移测试，不能把 Go 测试覆盖直接等同于 Rust 覆盖。

## 扩展指南

- 新增 Parquet 物理值类型时，应同步修改 `ParquetValueBuffer` 的 `len`、`kind`、`slice`，`validate_generated_column` 的物理类型映射，以及 `write_parquet_column_batch` 的 writer 分派；并在 `migration_aster_unit_test.rs` 的全类型切片和真实 schema 用例中增加覆盖。
- 新增 writer/file 选项时，应扩充对应枚举和 `apply_writer_property` 或 `build_writer_properties`，确认覆盖顺序仍是“默认值在前、调用方选项在后”，并增加成功元数据断言和未知/非法选项失败断言。
- 改动 NULL 或嵌套语义前，应先确认当前规则仅支持单层 optional primitive 列；`level > 0` 即“有值”的简化不能直接推广到多级重复/定义级别。
- 若要用于大文件，应另建生产级流式实现，而不是让本夹具一次生成全部列；需要评估内存峰值、切片复制、对象写入失败清理、取消传播和 close 错误可见性。
- 若改变资源关闭策略，必须分别覆盖 file writer 关闭失败与 object writer 关闭失败，并明确是否保留 Go 兼容行为；不可只依赖幂等标记。
- 测试应继续放在独立的 `pkg/dumpformat/testutils/migration_aster_unit_test.rs`，不要内嵌到本源文件。跨模块消费行为可在 `pkg/dumpformat/parquetfile` 的独立测试中补充。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/dumpformat/testutils` 确认目标 Rust/Go/测试模块均已索引；`node --file pkg/dumpformat/testutils/parquet_writer.rs` 分段读取了 622 行完整源码；精确查询定位 `build_writer_properties`（第 395 行）和 `write_parquet_file`（第 520 行）。自然语言 `explore` 的同名 `write_file`/`write_row` 结果存在跨包符号碰撞，因此未把那些无关边当作本文件调用关系证据。
- 源与边界：`pkg/dumpformat/testutils/parquet_writer.rs`、`pkg/dumpformat/testutils/lib.rs`、`pkg/dumpformat/testutils/Cargo.toml`、`pkg/dumpformat/parquetfile/Cargo.toml`。
- Go 对照：`pkg/dumpformat/testutils/parquet_writer.go` 全文；并检索其在 `pkg/dumpformat/parquetfile/parser_test.go`、`pkg/executor/importer/import_test.go`、`pkg/lightning/mydump/loader_test.go` 等位置的调用。
- Rust 独立测试：`pkg/dumpformat/testutils/migration_aster_unit_test.rs` 覆盖 definition-level 切片、八种缓冲切片、真实 Snappy 文件、row-group 覆盖、全部 Go 物理 writer 类型、负行数、非法区间/选项/生成长度和 FLOAT NULL 跨组写入。
- 人工复核结论：该文件存在的原因是为测试产生精确可控的真实 Parquet 输入；运行核心是 schema/属性构造、全列生成、按 definition level 映射 row group、经 objstore 流式落盘；安全扩展必须同步封闭类型分派、独立迁移测试和资源关闭/内存边界。
