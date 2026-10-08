# `tools/gen-parquet/main.rs`

## 文件定位

`tools/gen-parquet/main.rs` 是一个生成 Parquet 测试夹具的独立命令实现。Cargo 清单 `tools/gen-parquet/Cargo.toml` 将它注册为二进制 `astersql-tools-gen-parquet`，同时 `tools/gen-parquet/lib.rs` 又以 `#[path = "main.rs"] pub mod main` 挂载同一份实现，供 `parity_test.rs` 和库调用方复用。根 `Cargo.toml` 把该 crate 列为 workspace member。

这个工具不在 SQL 请求、事务或存储运行时主链中。它的 Go 对照程序由 `Makefile` 的 `build_for_lightning_integration_test` 目标构建成 `bin/parquet_gen`，因此其业务位置是 Lightning 集成测试的夹具生成侧。Rust Cargo 清单只声明 `parquet` 依赖，并通过固定 tag `astersql-parquet-v60.0.0-streaming-pages.1` 使用 AsterSQL 的 Arrow Rust 分支。

## 核心职责

- `main`、`parse_flags` 与 `run` 负责命令层：解析与 Go `flag` 风格相容的五个参数，按 chunk 数顺序生成文件，并把参数错误和运行错误映射为不同退出码。
- `chunk_file_name` 与 `chunk_file_path` 固定输出命名和路径拼接规则：`<schema>.<table>.<四位序号>.parquet`。
- `write_simple_parquet_file` 负责单个分片：创建文件、建立固定双列 schema、创建一个行组、依 schema 顺序写两列并结束 writer。
- `get_parquet_writer` 负责 Parquet schema 和列属性：所有字段为 optional，根 schema 为 required，每列启用字典编码和 Snappy 压缩。
- `write_column` 负责按物理类型生成 `0..rows` 的值。目前支持 `INT64`、`DOUBLE` 和 `BYTE_ARRAY`；字符串列写入整数的十进制文本。
- `WriteWrapper` 将 `std::fs::File` 暴露为 writer 所需接口；它的 `Read`/`Seek` 实现只是兼容占位，不提供真实读取或定位能力。

## 主要符号

- `pub struct WriteWrapper { pub writer: File }`：文件写入适配器。`Write::write`/`flush` 直接转发给内部文件；`Read::read` 总是返回 `0`，`Seek::seek` 总是返回位置 `0`。固有方法 `close(self)` 仅 flush，实际生成主流程依靠 Parquet writer 的关闭和对象析构释放文件句柄。
- `get_parquet_writer<W: Write + Send>(...) -> Result<SerializedFileWriter<W>, String>`：先验证列名与物理类型等长，再逐列创建 primitive optional 字段，字段 id 固定为 `8`，最后创建 required 根节点和带列级字典/Snappy 设置的 writer。等长检查是 Rust 版新增的显式防护，避免 `zip` 静默丢列。
- `write_column<W: Write + Send>(...) -> Result<(), String>`：取得行组的下一列，定义级别全部写 `1`；根据 `ColumnWriter` 变体构造整数、浮点或字节数组。没有下一列或类型不受支持时返回错误。
- `write_simple_parquet_file(file_path, rows)`：单文件入口。负行数在创建文件前由 `usize::try_from` 拒绝；固定列为 `iVal: INT64` 和 `s: BYTE_ARRAY`，只建立一个 row group。
- `pub struct Flags` 与 `Default`：保存 `schema_name`、`table_name`、`chunks`、`row_numbers`、`source_dir`；默认值依次为 `test`、`parquet`、`10`、`1000`、空目录。
- `parse_flags` 与私有 `split_flag`：接受 `-key value`、`-key=value`、`--key value`、`--key=value`；遇到 `--`、单独的 `-` 或首个位置参数停止解析。
- `chunk_file_name`/`chunk_file_path`：分别负责名称格式化和平台原生路径连接。
- `run`：以 `0..flags.chunks` 串行遍历，遇到第一个分片错误立即停止，并在错误中补充目标文件名。
- `main`：收集进程参数；解析失败打印 stderr 并退出 `2`，生成失败打印 stderr 并退出 `1`，全部成功则自然返回 `0`。
- `pub mod stubs`：挂载同目录兼容桩；本文件的 Parquet 生成链未调用其中符号。

## 执行流程

1. `main` 读取 `std::env::args()`，交给 `parse_flags`。解析从索引 `1` 开始，未知 flag、缺少值或整数格式错误都会立即返回字符串错误。
2. `main` 调用 `run(&flags)`。`run` 对每个 chunk 先由 `chunk_file_name` 形成可用于诊断的名称，再由 `chunk_file_path` 与输出目录拼接。
3. `write_simple_parquet_file` 先把 `rows: i32` 转成 `usize`，再通过 `File::create` 创建或截断目标文件。目录不会自动创建。
4. `get_parquet_writer` 构造两列 optional schema：`iVal` 使用 `INT64`，`s` 使用 `BYTE_ARRAY`；两列均开启字典编码与 Snappy 压缩。
5. `next_row_group` 只创建一个行组。随后按 `row_names` 的两个元素循环两次调用 `write_column`，因此列推进顺序必须与 schema 顺序一致。
6. 第一列生成 `0..rows` 的 `i64`；第二列把相同序列转成十进制字符串字节。两列定义级别均为 `1`，表示 optional 字段的每个值实际存在。
7. 不论列写入是否失败，函数都会尝试关闭 row group 和 file writer；这两个关闭结果按 Go 中忽略 `defer Close` 错误的语义丢弃。若批量写入曾失败，则返回最初的写入错误。
8. `run` 在一个分片成功后继续下一个；失败则附加 `generate test source failed, name: ...` 上下文并停止，不回滚之前已经生成的文件。

## 数据与状态

该文件没有全局可变状态。命令配置集中在值类型 `Flags` 中，并以不可变借用传给 `run`。每个分片的临时状态包括列名/类型向量、Parquet writer、单个 row group、定义级别向量以及当前列的值向量。

内存开销与单个分片的 `rows` 成正比：`write_column` 会为定义级别分配一个 `Vec<i16>`，再为当前列分配一个同规模的值向量；列是逐个写入的，但定义级别和值会在该列写完前同时驻留。文件之间串行处理，因此不会同时保留多个分片的 writer。`chunks <= 0` 时 Rust 的 `0..chunks` 为空，`run` 成功返回且不创建文件；`rows < 0` 则由单文件入口明确报错。

输出的稳定契约由 `parity_test.rs::contract_normal_paths` 验证：文件具有 `PAR1` 首尾魔数、一个 row group、两列、指定压缩/字典编码，且第 `n` 行为整数 `n` 和字符串 `"n"`。`contract_boundary` 验证零行仍会产生合法文件。

## 依赖与调用关系

RustCodeGraph 对本文件给出的内部主链是：

`main → parse_flags → Flags::default/split_flag`

`main → run → chunk_file_name/chunk_file_path → write_simple_parquet_file → get_parquet_writer/write_column`

`tools/gen-parquet/lib.rs::entry` 是进程入口的库级转发者，直接调用本文件的 `main`。`tools/gen-parquet/parity_test.rs` 直接调用 `Flags`、命名函数、参数解析、`run` 和三个 Parquet 写入函数，构成主要上游测试面。

下游标准库依赖是文件 I/O、路径、进程参数/退出和 `Arc`。外部 `parquet` crate 提供 schema 类型、writer、列 writer、物理类型、压缩配置与 `ByteArray`。Go 构建侧的 `tools/gen-parquet/BUILD.bazel` 只描述 `main.go` 的 Bazel binary，并不是 Rust binary 的构建声明。

## 错误处理与边界

- 所有可恢复内部错误统一擦除为 `String`。schema 构造、writer 创建、取下一列、批量写和文件创建错误都通过 `to_string()` 传播；`run` 只在外层补充分片名。
- `get_parquet_writer` 拒绝列名/类型数量不一致；Go 版直接按名称长度索引类型，数量不足可能越界，Rust 在此选择更安全的显式失败。
- `write_column` 在 schema 已无下一列时返回 `no more columns`；对 `INT32` 等未列出的 writer 返回 `unsupported column type`。受支持的 `DOUBLE` 分支虽然不用于固定双列输出，但与 Go type switch 保持一致，并由独立测试覆盖。
- `parse_flags` 拒绝未知参数、缺参和非法整数。它与 Go `flag.Parse` 一样在 `--` 或首个位置参数处停止，但当前实现不打印 usage；未解析的剩余参数也不会被使用。
- `File::create` 会截断同名文件，不创建父目录；中途失败不会删除部分文件，也不会回滚更早 chunk。
- row group 和 writer 的关闭错误被有意忽略，以避免覆盖列批量写入错误。这也意味着只有关闭阶段失败时，函数仍可能报告成功；这是为对齐 Go 现有 `defer` 忽略策略而保留的边界。
- `WriteWrapper::Read`/`Seek` 是伪实现，调用者不得把它当成可随机访问或可读取的文件对象。

## 并发与资源生命周期

实现没有线程、异步任务、锁、通道或共享可变状态。`run` 串行创建 chunk，所以确定性强，但生成大量文件时不会利用并行 I/O。

单个文件的所有权顺序为：`File` 被移入 `WriteWrapper`，wrapper 再被移入 `SerializedFileWriter`；row group 暂借 file writer；列 writer 暂借 row group。`write_column` 在返回前尝试关闭列 writer，`write_simple_parquet_file` 随后依次尝试关闭 row group 和 file writer，最终由 Rust 析构释放底层文件。该嵌套所有权阻止 row group/column writer 活得比其父 writer 更久。

`parity_test.rs::contract_resource_cleanup` 在 `run` 返回后立即读取文件 metadata，确认句柄已释放且结果已落盘，并验证零 chunk 不产生额外副作用。未来若引入并行 chunk，必须为每个任务保留独立 writer，并明确首错取消、已完成文件保留策略及同名路径竞争规则。

## 与 Go 版本的对应关系

Rust 文件逐项对照同目录 `main.go`：`WriteWrapper` 对应 `writeWrapper`，`get_parquet_writer` 对应 `getParquetWriter`，`write_column` 对应 `writeColumn`，`write_simple_parquet_file` 对应 `writeSimpleParquetFile`，`Flags`/`parse_flags` 对应五个全局 `flag` 定义与 `flag.Parse`，`run` 加 `main` 对应 Go `main` 的 chunk 循环。

保持一致的行为包括：默认参数、双列名与物理类型、optional 字段、字段 type length/id 参数 `8`、required 根 schema、逐列字典编码和 Snappy、单行组、三种 writer 类型分支、四位 chunk 序号、顺序生成及出错即停。Go 的 `log.Fatalf` 对运行错误退出 `1`；Rust 显式使用 `process::exit(1)`，并另行把参数错误设为退出 `2`。

已确认的实现差异包括：Rust 显式拒绝 schema 数组长度不一致和负行数；Rust 将库 API 错误转成 `Result<_, String>`，而 Go 的 `getParquetWriter` 忽略 schema 构造错误并直接返回 writer；Go `writeWrapper.Close` 关闭文件，Rust 固有 `close` 只 flush，但主流程在 Parquet writer 关闭/析构后释放文件；Go 的不支持类型错误包含动态类型，Rust 返回固定字符串。以上差异均应视为现有可观察契约，修改前需同步评估 parity 测试。

## 扩展指南

- 新增固定输出列时，应同时修改 `write_simple_parquet_file` 的 `row_names`/`row_types`，并确保 `write_column` 支持对应 `ColumnWriter` 变体；随后在独立的 `parity_test.rs` 扩展 schema、值序列、编码和错误断言。不要把测试内嵌回生产 `main.rs`。
- 新增物理类型时，入口是 `write_column` 的 match。需要明确值生成规则、定义/重复级别、空值策略和大数据量内存成本，并与 Go `writeColumn` 同步；仅让编译通过而不对齐 Go 语义不够。
- 新增命令参数时，应同步 `Flags`、`Default`、`parse_flags`、`main.go` 的 flag、错误行为和 `contract_boundary`/`contract_error_paths`。若参数改变文件格式，还应扩展正常路径的 reader 断言。
- 修改命名或目录规则时，应集中在 `chunk_file_name`/`chunk_file_path`，并检查 Lightning 脚本对 `bin/parquet_gen` 输出名的消费。路径仍应通过 `Path::join` 生成，避免写死分隔符。
- 改变关闭策略时，应区分“批量写错误”和“结束 footer/flush 错误”的优先级；若开始传播关闭错误，需要同步 Go 版本和资源清理测试，防止产生表面成功但不可读的文件。
- 若引入并发，应先定义 chunk 输出冲突、错误取消、峰值内存和确定性；当前串行行为及“保留先前成功文件”是兼容基线。
- crate 依赖升级必须继续通过 `tools/gen-parquet/Cargo.toml` 的已发布 tag 完成，不应复制或用本地 `[patch]` 覆盖外部 Arrow/Parquet 实现。

## 验证依据

- 源实现：`tools/gen-parquet/main.rs`，重点符号为 `WriteWrapper`、`get_parquet_writer`、`write_column`、`write_simple_parquet_file`、`Flags`、`parse_flags`、`chunk_file_name`、`chunk_file_path`、`run`、`main`。
- Crate/入口证据：`tools/gen-parquet/Cargo.toml`、`tools/gen-parquet/lib.rs`、根 `Cargo.toml`；构建用途证据为 `Makefile::build_for_lightning_integration_test` 和 `tools/gen-parquet/BUILD.bazel`。
- Go 对照：`tools/gen-parquet/main.go`，核对 schema、值生成、flag 默认值、文件命名、关闭顺序和错误停止策略。
- 独立 Rust 测试：`tools/gen-parquet/parity_test.rs` 的 `contract_normal_paths`、`contract_boundary`、`contract_error_paths`、`contract_write_column_type_switch`、`contract_resource_cleanup`。同目录没有 Go `*_test.go`；本次未把生产源码内嵌为测试文件。
- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,305 个节点和 1,849,011 条边；`files --filter tools/gen-parquet` 确认 `main.rs`、`lib.rs`、`main.go`、`parity_test.rs`、`stubs.rs` 均被索引。`node --file tools/gen-parquet/main.rs` 读取到 306 行及 18 个符号；`query` 定位了主要函数；`callees` 验证了上述内部主链，且图中测试函数 `contract_write_column_type_switch` 直接调用 `get_parquet_writer` 和 `write_column`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收以固定十一个二级标题检查为准；内容人工复核覆盖“为何存在、如何运行、如何安全扩展”。
