# `pkg/dumpformat/csvfile/writer.rs`

## 文件定位

本文件实现 `astersql-dumpformat-csvfile` crate 的单流 CSV 行写入器。crate 入口 `pkg/dumpformat/csvfile/lib.rs` 将本模块公开为 `pub mod writer`，并把 `Writer` 重导出到 crate 根；crate 清单 `pkg/dumpformat/csvfile/Cargo.toml` 只依赖父级 `astersql-dumpformat`，其中 `FieldKind` 再经 `csvfile.rs` 暴露给本文件。

它位于 Dumpling 导出链的格式化末端：`dumpling/export/writer_util.rs::writeCSVFile` 把 Dumpling 的 `ObjectWriter` 包装成实现 `std::io::Write` 的 `CSVObjectWriter`，构造本文件的 `Writer`，逐行编码，并根据累计写入量决定文件轮转。文件自身只负责一个 sink 内的 CSV framing，不负责打开文件、缓冲策略、压缩、轮转、指标或行迭代器生命周期；这一边界也由 `pkg/dumpformat/csvfile/lib.rs` 的模块注释明确说明。

## 核心职责

- `Writer<W>` 持有输出 sink、完整 CSV `Config`、每列 `FieldKind`、可复用的行缓冲区及累计写入字节数。
- `write` 接收拥有所有权的 `Option<Vec<u8>>` 切片，并转交零复制视图给 `write_borrowed`。
- `write_borrowed` 校验数据行宽度，插入字段分隔符，委托 `csv.rs::append_field` 处理 NULL、数字、字符串/字节、转义与二进制编码，然后写出完整行。
- `write_header` 把每个列名一律按 `FieldKind::String` 编码；它不使用也不校验 `kinds` 的长度。
- `flush_row` 追加行终止符并对 sink 发起一次 `Write::write`；`estimate_file_size` 返回 sink 实际接受的累计字节，`close` 当前为空操作。

CSV 字段语义并不在本文件重复实现，而由 `pkg/dumpformat/csvfile/csv.rs::append_field` 集中决定：`None` 输出 `null_value`，数字原样输出，其他类型按配置包围并转义，字节还可选择 HEX 或 Base64。

## 主要符号

- `pub struct Writer<W>`：泛型 sink 容器；只有在 `W: std::io::Write` 时提供行为方法。字段均为私有，调用者不能绕过方法修改计数或暂存行。
- `pub fn new(sink, kinds, cfg) -> Self`：取得 sink、列类型和配置的所有权；缓冲区为空，`written` 从 0 开始。它不补充任何 CSV 默认值，空配置会产生空分隔符/终止符。
- `pub fn write(&mut self, row: &[Option<Vec<u8>>])`：兼容拥有型行数据的便利入口；通过 `as_deref` 借用每个值，再调用 `write_borrowed`。
- `pub fn write_borrowed<'a>(&mut self, row: impl ExactSizeIterator<Item = Option<&'a [u8]>>)`：核心数据行入口。`ExactSizeIterator` 使函数能在修改缓冲区或 sink 前检查列数。
- `pub fn write_header(&mut self, names: &[Vec<u8>])`：写表头；每个名字均按字符串字段编码，允许表头长度与 `kinds` 不同。
- `fn flush_row(&mut self)`：唯一实际访问 sink 的内部函数；追加 `lines_terminated_by`，调用一次 `sink.write`，并按返回值增加 `written`。
- `pub fn estimate_file_size(&self) -> u64`：读取已被 sink 接受的字节数，而不是缓冲区容量、逻辑行长度或底层对象最终持久化大小。
- `pub fn close(&mut self)`：当前始终返回成功，不调用 `Write::flush`，也不关闭 sink；sink 的终结由所有者负责。

文件没有模块级常量、trait、枚举、条件编译项或异步入口。

## 执行流程

数据行主流程如下。

1. 上游以 `Writer::new` 固定本文件实例的列类型和 CSV 配置。在真实导出链中，`dumpling/export/writer_util.rs::writeCSVFile` 根据数据库列类型生成 `FieldKind`，并从 Dumpling 配置映射分隔符、包围符、转义符、行终止符、NULL 文本和二进制格式。
2. `write` 将 `Option<Vec<u8>>` 转为借用切片；Dumpling 则直接调用 `write_borrowed(raw.iter().map(|r| r.as_opt()))`，避免每行复制扫描值。
3. `write_borrowed` 先比较迭代器精确长度与 `kinds.len()`；不相等时立即返回 `InvalidInput`，既不清空旧缓冲区也不访问 sink。
4. 校验通过后清空但保留 `buf` 容量。遍历字段时，在非首字段前追加 `fields_terminated_by`，再按同下标 `FieldKind` 调用 `append_field`。
5. `flush_row` 在已编码字段后追加 `lines_terminated_by`，对 sink 做一次 `write(&buf)`，并把成功返回的 `n` 加入 `written`。

表头路径从 `write_header` 开始，跳过行宽校验，将所有名字视为字符串，然后复用同一个 `flush_row`。Dumpling 仅在未禁用表头、存在选中字段且列名非空时调用该路径。写行后，Dumpling 用 `estimate_file_size() >= conf.FileSize` 停止当前文件，外层再用同一行迭代器开始下一个文件，因此轮转不属于本文件。

## 数据与状态

- `sink: W` 是输出资源本身，由 `Writer` 按值持有；当 `Writer` 被丢弃时 sink 随之丢弃，但本文件没有显式 close/flush 协议。
- `cfg: Config` 是构造时的快照。字段和行终止符允许多字节；具体字段转义规则读取这些字节向量。
- `kinds: Vec<FieldKind>` 是数据行的固定 schema，也定义合法行宽。表头不受它约束。
- `buf: Vec<u8>` 是跨调用复用的 scratch buffer。`clear` 仅把长度归零，容量保留，因此稳态可减少分配；遇到历史超大行时容量也会保留到 `Writer` 释放。
- `written: u64` 只在 sink 的 `write` 返回 `Ok(n)` 后增加 `n`。它不会预加逻辑行长度，也不会统计失败写入或仍在其他缓冲层中的字节。

关键不变量是：数据行进入字段循环前，行宽必须等于 `kinds.len()`；一次成功调用最多对 sink 发起一次写操作；字段之间才有分隔符，每次数据行或表头末尾都追加一次行终止符。零列数据行是合法的，只输出行终止符，`csv_test.rs::empty_row` 验证连续两次写出 `\n\n`。

## 依赖与调用关系

下游直接依赖：

- `crate::{Config, FieldKind}`：描述 framing 参数和列语义；定义位于 `pkg/dumpformat/csvfile/csvfile.rs`。
- `crate::csv::append_field`：字段级序列化；RustCodeGraph 明确显示 `write_borrowed` 和 `write_header` 均调用它。
- `std::io::Write`：抽象实际 sink；`flush_row` 依赖其允许部分写入的 `write` 合约。

已验证的内部调用边为 `write → write_borrowed → append_field/flush_row`，以及 `write_header → append_field/flush_row`。RustCodeGraph 的文件节点还确认本文件被 `csv.rs`、`csv_test.rs`、`lib.rs`、`dumpling/export/writer_test.rs` 和 `pkg/objstore/gcs_test.rs` 引用；其中应用主链的直接、可定位证据来自 `dumpling/export/writer_util.rs::writeCSVFile`。

上游主链为 `WriteInsertInCsv → writeCSVFile → Writer::{new, write_header, write_borrowed, estimate_file_size, close}`。`CSVObjectWriter::write` 再把标准 I/O 写调用适配到 `ObjectWriter::Write`，并把 Dumpling 错误包入 `io::Error`；`csv_io_error` 尝试在返回链上恢复原始错误。`dumpling/export/Cargo.toml` 通过路径依赖 `../../pkg/dumpformat/csvfile` 接入本 crate。

## 错误处理与边界

- 数据行宽度不匹配时返回 `io::ErrorKind::InvalidInput`，消息包含实际与期望字段数；`csv_test.rs::row_width_mismatch` 验证错误文本且 sink 保持为空。
- `append_field` 不返回错误，字段编码在内存中完成；本文件可见的运行时错误来自行宽校验或 sink 的 `write`。
- `flush_row` 使用 `write` 而不是 `write_all`。如果 sink 成功但只接受前缀，函数仍返回 `Ok(())`、只累计该前缀，并不会重试剩余字节；`csv_test.rs::short_write_counts_actual_bytes` 明确锁定“实际接受 1 字节就计 1 字节”的现状。扩展 sink 时必须接受这一合约，或在上游适配器中保证完整写入。
- sink 返回错误时 `?` 直接传播，`written` 不增加；已经编码的整行仍留在 `buf` 中，但下一次合法写入会先 `clear`，本文件没有重试 API。
- `write_header` 不检查表头宽度；这是与数据行不同的有意现状，调用者负责提供列名集合。
- `Config::default()` 的字节向量均为空。`Writer::new` 不施加逗号、引号或换行默认值，所以调用者必须提供所需 framing。
- `close` 是 no-op，不保证刷新自带缓冲的 sink；若 sink 需要刷新/提交，责任在其拥有者或适配层。

## 并发与资源生命周期

`Writer` 没有锁、原子量、线程或异步任务。所有改变状态的方法都要求 `&mut self`，Rust 借用规则防止同一实例被无同步地并发写入；能否跨线程移动或共享取决于泛型 `W` 的自动 trait，但本类型不额外承诺并发安全。

构造时 `Writer` 取得 sink、配置和类型向量的所有权；每行的借用值只在 `write_borrowed` 调用期间有效，不被保存。行缓冲在实例生命周期内复用。`close` 不消费实例，也不释放或归还 sink，文件中也没有 `into_inner`；资源最终随 `Writer` drop。Dumpling 的 `writeCSVFile` 在退出前调用 `cw.close()`，但真正的行迭代器由 `WriteInsertInCsv` 关闭，真正的对象 writer 则由更外层所有者管理。

文件大小计数用于轮转时是调用线程内的普通 `u64`。它表示 `std::io::Write` 层已接受的字节；若适配器内部缓冲、压缩或延迟上传，不能把它解释成远端已持久化大小。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dumpformat/csvfile/writer.go`。结构字段与流程基本逐项对应：Go `Writer` 的 `w/cfg/kinds/buf/written` 对应 Rust 的 `sink/cfg/kinds/buf/written`；`NewWriter`、`Write`、`WriteHeader`、`flush`、`EstimateFileSize`、`Close` 分别对应 `new`、`write`/`write_borrowed`、`write_header`、`flush_row`、`estimate_file_size`、`close`。

两版共同保持以下语义：写数据前校验列数；复用 scratch buffer；字段级逻辑委托给同目录 CSV 编码器；表头统一按字符串编码；每行只调用一次底层 `write`；累计底层返回的实际字节数；`Close/close` 当前不做事。Go 的 `flush` 即使同时得到 `n` 和错误也会先累计 `n` 再返回错误；Rust `Write` 的返回类型不能在 `Err` 中同时携带 `n`，因此 Rust 只在 `Ok(n)` 时计数，这是接口模型带来的细微差异。

Rust 额外提供 `write_borrowed`，让 Dumpling 从行接收器借用原始字节，避免为 `Vec<Option<Vec<u8>>>` 再分配；公开 `write` 保留拥有型便利接口。Go 使用 `[]sql.RawBytes` 与 nil 表示 NULL，Rust 使用 `Option<&[u8]>`/`Option<Vec<u8>>` 表达同一语义。

Rust `pkg/dumpformat/csvfile/csv_test.rs` 与 Go `csv_test.go` 对齐覆盖反斜杠转义、引号倍增、NULL 与类型、HEX/Base64、空行、无包围模式、表头、大小估算和行宽错误；Rust 还增加多字节包围符/自定义转义与短写计数测试。Dumpling 集成级 `dumpling/export/writer_test.rs::csv_file_size_rotation_preserves_all_rows` 验证小文件阈值会切分且拼接结果不丢行。

## 扩展指南

- 修改字段内容、转义或二进制格式时，应改 `csv.rs::append_field` 及其辅助函数，而不是在 `Writer` 中复制规则；同步扩展独立的 `csv_test.rs`，并与 `csv_test.go` 的既有语义核对。
- 修改数据行 framing、行宽策略、表头规则或计数时，入口分别是 `write_borrowed`、`write_header`、`flush_row` 和 `estimate_file_size`。必须同时检查 `dumpling/export/writer_util.rs::writeCSVFile` 的轮转与指标逻辑。
- 若要保证完整行写出，应明确评估把 `write` 改为 `write_all` 对 Go 对齐、短写测试、计数以及 Dumpling `CSVObjectWriter` 错误恢复的影响；不能只修改测试来掩盖当前部分写入合约。
- 若要让 `close` 真正 flush/终结 sink，需要决定调用 `Write::flush` 是否足够、错误如何传播、是否需要消费 `self` 或提供 `into_inner`；还要避免与上游 `ObjectWriter::Close` 的所有权职责重复。
- 若加入内部缓冲、异步写入或压缩，`estimate_file_size` 的定义必须保持可用于轮转，或同步修改调用者；同时记录逻辑字节、接受字节与持久化字节的区别。
- 性能上应保留 `buf` 复用与借用行入口；新逻辑需关注超大行导致的容量常驻、多字节分隔符扫描以及每字段追加次数。并发扩展不能破坏当前单实例顺序写入和行边界。

本仓库要求 Rust 测试与生产文件分离；新增回归应放在 `pkg/dumpformat/csvfile/csv_test.rs`，跨文件轮转/对象存储行为则放在现有 Dumpling 独立测试文件中，不应把 `#[cfg(test)]` 测试内嵌到 `writer.rs`。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件；目标文件节点列出 85 行、11 个符号及 5 个引用文件。
- RustCodeGraph `node --file pkg/dumpformat/csvfile/writer.rs`：核对 `Writer`、七个方法、字段与完整控制流。
- RustCodeGraph `explore`/`query`：确认 `append_field` 的两条目标调用边，以及 `dumpling/export/writer_util.rs::writeCSVFile`、`CSVObjectWriter` 等上游符号。对同名 `writer.rs::*` 执行 `callers/callees` 时工具会混入其他同名文件，本文未把这些歧义结果当作目标调用证据。
- crate 与模块边界：`pkg/dumpformat/csvfile/Cargo.toml`、`pkg/dumpformat/csvfile/lib.rs`、`pkg/dumpformat/csvfile/csvfile.rs`、根 `Cargo.toml` workspace 成员声明，以及 `dumpling/export/Cargo.toml` 的路径依赖。
- 下游实现与应用接线：`pkg/dumpformat/csvfile/csv.rs`、`dumpling/export/writer_util.rs`。
- Go 对照：`pkg/dumpformat/csvfile/writer.go` 与 `pkg/dumpformat/csvfile/csv_test.go`。
- Rust 测试证据：`pkg/dumpformat/csvfile/csv_test.rs`；集成级轮转证据：`dumpling/export/writer_test.rs::csv_file_size_rotation_preserves_all_rows`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 章结构检查和人工事实复核为验收。
