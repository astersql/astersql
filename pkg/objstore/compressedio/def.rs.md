# `pkg/objstore/compressedio/def.rs`

## 文件定位

本文件是 `astersql-objstore-compressedio` crate 的公共定义层，源码见 [`def.rs`](def.rs)。crate 根 [`lib.rs`](lib.rs) 以 `pub mod def` 声明模块，并通过 `pub use def::*` 再导出这里的全部公开符号；[`Cargo.toml`](Cargo.toml) 则把该 crate 映射到 Go 包 `pkg/objstore/compressedio`，声明实际编解码所需的 `flate2`、`snap` 和 `zstd` 依赖。

它不执行压缩或解压，而是为 [`reader.rs`](reader.rs)、[`writer.rs`](writer.rs) 和 [`buffer.rs`](buffer.rs) 提供共同协议：压缩类型、文件后缀、压缩器刷新能力、zstd 解码配置，以及配置字符串到压缩类型的解析。上层 [`pkg/objstore/objectio/lib.rs`](../objectio/lib.rs) 又将该 crate 暴露为 `objectio::compressedio` 并单独再导出 `CompressType`，所以这些定义同时构成对象存储适配层的类型边界。

## 核心职责

- `CompressType` 以稳定的 `u8` 判别值表达无压缩、Gzip、Snappy 和 Zstd；`reader::new_reader`、`writer::new_writer` 与 `buffer::new_buffer` 都以它选择具体实现。
- `CompressType::file_suffix` 给出规范输出后缀，供调用者从类型生成文件名；后缀映射由独立迁移测试覆盖。
- `Flusher` 抽象“把压缩器自有缓冲推送到底层 writer”的能力，使 [`writer.rs`](writer.rs) 的三种编码器和 [`buffer.rs`](buffer.rs) 能共享刷新协议。
- `DecompressConfig` 携带 zstd 解码并发提示；其他格式忽略该字段。
- `parse_compress_type` 按 Go 版本的大小写敏感别名表解析配置文本，并用独立错误类型报告未知值。

本文件不是格式探测器：它不会查看文件内容或后缀来自动判断压缩格式。调用者必须先确定 `CompressType`，再交给 reader/writer 工厂。

## 主要符号

- `pub enum CompressType`：带 `#[repr(u8)]`，数值明确为 `NoCompression = 0`、`Gzip = 1`、`Snappy = 2`、`Zstd = 3`；派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`，但没有 `Default`。
- `pub fn CompressType::file_suffix(self) -> &'static str`：返回静态字符串；四种映射依次为 `""`、`.gz`、`.snappy`、`.zst`。由于 Rust 枚举不能安全地产生未列举变体，匹配是穷尽的。
- `pub trait Flusher { fn flush(&mut self) -> io::Result<()>; }`：要求独占可变借用，错误沿用标准 I/O 错误。它没有提供默认实现，也不承诺持久化或关闭资源。
- `pub struct DecompressConfig { pub zstd_decode_concurrency: i32 }`：派生 `Default`，因此默认值为 `0`；字段公开，调用者可直接构造。当前 [`reader.rs`](reader.rs) 仅区分值是否等于 `1`。
- `pub struct ParseCompressTypeError(String)`：内部字符串字段私有；实现 `Display` 和 `std::error::Error`，没有公开构造器或错误源。
- `pub fn parse_compress_type(&str) -> Result<CompressType, ParseCompressTypeError>`：接受空串、`no-compression`、`gzip`/`gz`、`snappy`、`zstd`/`zst`；其他输入产生 `unknown compress type <输入>`。

文件中没有条件编译项、全局可变状态、宏、异步函数或生命周期参数。

## 执行流程

解析配置时，`parse_compress_type` 对输入做精确字符串匹配，不会 trim、折叠大小写或按文件后缀解析。命中别名后立即返回相应 `CompressType`；未命中则格式化错误文本并包进 `ParseCompressTypeError`。例如 `"gz"` 成功映射为 `Gzip`，而 `"GZIP"`、`".gz"` 和带空格的输入都会失败。

生成文件名时，调用者持有 `CompressType` 并调用 `file_suffix`，函数以穷尽 `match` 返回规范后缀。它只给出后缀，不拼接路径、不验证原文件名，也不参与读取端的格式识别。对象存储写读回归测试 [`../objectio/writer_test.rs`](../objectio/writer_test.rs) 使用该方法生成文件名，再用相同类型完成三种格式的往返。

构建读写链时，类型和配置沿以下路径流动：

1. 上层选择 `CompressType`；压缩写入由 `new_writer` 或 `new_buffer` 消费，解压读取由 `new_reader` 消费。
2. `writer.rs` 的 Gzip、Snappy、Zstd 包装器实现 `Flusher`，将调用委派给各自编码器；`Buffer::flush` 也通过该 trait object 刷新压缩器。
3. `reader::new_reader` 对 Gzip/Snappy 只看类型；对 Zstd，`zstd_decode_concurrency == 1` 选择同步惰性读取，否则选择后台异步读取；无压缩返回 `Ok(None)`。

## 数据与状态

`CompressType` 是小型值类型，复制不转移所有权；显式 `repr(u8)` 固定了四个已知判别值，便于与 Go 的 `uint8`/`iota` 常量保持数值一致。但源码没有实现从任意 `u8` 的反序列化，因此外部数值输入仍需由边界层验证。

`DecompressConfig` 仅包含一个 `i32`。`Default::default()` 生成并发度 `0`；就当前 Rust 实现而言，只有值 `1` 表示同步 zstd 解码，`0`、负数和大于 `1` 的值都走异步实现。这个行为是当前 [`reader.rs`](reader.rs) 的二分逻辑，不应解释为 Rust 已支持任意指定数量的 zstd 解码 worker。

`ParseCompressTypeError` 只保存最终展示文本，不保存原输入的独立字段、错误码或底层 source。`file_suffix` 返回静态常量，不分配内存；解析成功也不分配，只有未知输入的错误路径会格式化并分配字符串。

## 依赖与调用关系

本文件直接只依赖标准库 `std::fmt` 和 `std::io`。第三方编解码依赖出现在相邻实现文件中，并由本 crate 的 [`Cargo.toml`](Cargo.toml) 声明：`flate2` 对应 Gzip，`snap` 对应 Snappy，`zstd` 对应 Zstd。

下游关系如下：

- [`reader.rs`](reader.rs) 导入 `CompressType`、`DecompressConfig`，在 `new_reader` 中选择四个读取分支。
- [`writer.rs`](writer.rs) 导入 `CompressType`、`Flusher`；公共 `Writer` trait 继承 `Flusher`，三种编码器包装分别实现该 trait。
- [`buffer.rs`](buffer.rs) 导入 `CompressType`、`Flusher`，用类型创建压缩 writer，并通过 `Flusher::flush` 显式刷新。
- [`../objectio/writer.rs`](../objectio/writer.rs) 使用再导出的 `CompressType` 控制对象写入是否压缩；[`../compress.rs`](../compress.rs) 持有 `CompressType` 和 `DecompressConfig` 来包装对象存储读路径。
- [`../../session/runtime/import_compression.rs`](../../session/runtime/import_compression.rs) 根据扩展名选择 `CompressType`，并以 `zstd_decode_concurrency: 1` 创建同步解压 reader；[`../../session/runtime/load_data.rs`](../../session/runtime/load_data.rs) 也直接使用这些定义。

RustCodeGraph 将 `def.rs` 标记为被 15 个文件使用，并确认 `file_suffix` 被 `parse_compress_type_and_suffix_match_go` 调用、`DecompressConfig` 被对象存储测试和 session 导入路径实例化。仓库级 `rg` 进一步显示：本 crate 的 `parse_compress_type` 当前没有非测试 Rust 调用者，`file_suffix` 也没有非测试 Rust 调用者；它们仍是经 crate 根公开再导出的 API，不能据此视为可删除代码。Dumpling 有另一个私有同名解析函数，属于不同模块，不能混作本函数的调用者。

## 错误处理与边界

`parse_compress_type` 的唯一失败条件是输入不在精确别名集合中。错误消息会原样包含输入且没有错误源链；调用者若需要结构化分类，只能依据 `Result` 的错误类型，不能读取私有字段。空串被明确视为 `NoCompression`，不是缺失配置错误。

`Flusher::flush` 保留底层 `io::Result<()>`，不吞掉或重写 I/O 错误。它只定义接口；关闭压缩流、写出 footer/帧结束标记由 `writer::Writer::close` 负责，单独调用 `flush` 不能替代 `close`。无压缩类型在读写工厂中返回 `None`，上层必须继续使用原始 reader/writer；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 明确锁定了这一 Go `nil` 对应语义。

需要特别注意 Go/Rust 的异常枚举边界差异：Go 的 `FileSuffix` 对未知数值返回空串，而 Rust 的安全 `CompressType` 值只能是四个已声明变体，因此 `file_suffix` 没有默认分支。若未来从不可信整数反序列化类型，应在转换边界拒绝未知值，不应使用不安全方式构造枚举。

## 并发与资源生命周期

本文件自身不创建线程、任务、锁、通道或文件句柄。`CompressType` 和 `DecompressConfig` 都是普通拥有型值；`Flusher::flush(&mut self)` 通过可变借用要求单次调用对实现对象具有独占访问，但 trait 本身没有声明 `Send`、`Sync` 或并发安全保证。

`DecompressConfig` 会间接改变资源生命周期。当前 Zstd reader 在并发度为 `1` 时由调用线程按需解码；其他值创建后台异步解码实现。独立测试 [`../objectio/writer_test.rs`](../objectio/writer_test.rs) 用零容量同步通道证明：默认配置会在调用方开始读取前由后台消费输入，而值 `1` 必须用另一个线程供给输入以避免阻塞。这个后台生命周期属于 [`reader.rs`](reader.rs) 的实现，而非 `DecompressConfig` 自身持有的资源。

刷新与关闭也必须区分：`Flusher` 只推动缓冲数据；Gzip/Zstd 的流尾由相邻 writer 的 `close` 写出。扩展实现若只实现 flush 而遗漏正确关闭，可能生成可见但不完整的压缩流。

## 与 Go 版本的对应关系

直接对照文件是 [`def.go`](def.go)。Rust 的 `CompressType` 显式写出 `0..=3`，对应 Go 的 `type CompressType uint8` 和 `iota` 顺序；文件后缀与解析别名完全相同，未知解析输入的文本也保持 `unknown compress type <值>`。Rust 用 `Result` 与 `ParseCompressTypeError` 替代 Go 的 `(CompressType, error)`；失败时 Rust 不额外返回 `NoCompression` 值。

Go 的 `Flusher.Flush() error` 对应 Rust 的 `flush(&mut self) -> io::Result<()>`。Rust 把错误限定为标准 I/O 错误并显式表达可变访问；Go 接口没有这种借用约束。Go `DecompressConfig.ZStdDecodeConcurrency int` 对应 Rust `i32` 字段。

Go [`reader.go`](reader.go) 对所有正并发值调用 `zstd.WithDecoderConcurrency`，其中 `1` 表示同步，默认值 `0` 使用库默认并发。Rust 当前只精确复刻测试所要求的同步/异步分界：等于 `1` 同步，其余值异步，并没有按大于 `1` 的具体数值配置 worker 数量。这是已知实现粒度差异，扩展前应先明确兼容目标。Go [`writer.go`](writer.go) 与 Rust writer 都让 Gzip、Snappy、Zstd 实现刷新和关闭，并让无压缩返回 `nil`/`None`。

[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖全部解析别名、规范后缀、未知类型错误、三种格式往返、Zstd 同步配置和无压缩工厂语义；[`../objectio/writer_test.rs`](../objectio/writer_test.rs) 与 Go [`../objectio/writer_test.go`](../objectio/writer_test.go) 进一步对照真实对象存储读写及同步/异步管道行为。

## 扩展指南

新增压缩格式时，应把 `CompressType` 变体、稳定判别值、`file_suffix` 和 `parse_compress_type` 别名视为同一项兼容变更，同时补齐 [`reader.rs`](reader.rs)、[`writer.rs`](writer.rs) 的工厂分支及 [`buffer.rs`](buffer.rs) 的使用验证。还应同步 Go [`def.go`](def.go)、[`reader.go`](reader.go)、[`writer.go`](writer.go)，并扩展独立 Rust 测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；不要把测试内嵌到 `def.rs`。

改变已有后缀、别名或判别值有持久化与兼容风险：文件名约定可能被对象存储、导入流程或外部 crate 消费，判别值也与 Go 顺序对齐。新增别名应明确是否继续保持大小写敏感，且需加入成功与未知输入回归用例。

若要完整支持 `zstd_decode_concurrency > 1` 的具体 worker 数，应主要修改 [`reader.rs`](reader.rs) 的构造策略，而不是只扩充本结构体；需要同步 [`../objectio/writer_test.rs`](../objectio/writer_test.rs) 中的管道/线程行为测试，并评估线程数、内存、取消和 reader drop 时的后台回收。若扩充 `Flusher` 的契约，应同步所有 Gzip/Snappy/Zstd 实现及 `Buffer::flush`，并保持 flush 与 close 的语义边界。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/objstore/compressedio` 确认本目录的 Rust/Go 源与独立测试均已收录。
- RustCodeGraph `node --file pkg/objstore/compressedio/def.rs`：核对了目标文件 82 行源码及 9 个符号；`node`/`query` 进一步确认 `file_suffix` 的迁移测试调用边、`DecompressConfig` 的实例化位置，以及同名 Dumpling 解析函数并非本模块调用者。
- RustCodeGraph `node --file`：读取并核对了 [`reader.rs`](reader.rs)、[`writer.rs`](writer.rs)、[`buffer.rs`](buffer.rs)、[`../objectio/writer_test.rs`](../objectio/writer_test.rs) 和 [`../../session/runtime/import_compression.rs`](../../session/runtime/import_compression.rs) 的直接实现/测试证据。限定目标文件的 `callers/callees` 命令在 30 秒内未返回，因此使用仓库 `rg` 补充直接引用清单，未把无结果解释为无调用。
- 配置与模块证据：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`../objectio/lib.rs`](../objectio/lib.rs)；本目录不存在 `doc.go`。
- Go 对照与测试证据：[`def.go`](def.go)、[`reader.go`](reader.go)、[`writer.go`](writer.go)、[`../objectio/writer_test.go`](../objectio/writer_test.go)；Rust 独立测试为 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`reader_test.rs`](reader_test.rs) 和 [`../objectio/writer_test.rs`](../objectio/writer_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo；最终用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核所有行为陈述均可回链到上述源码、图查询或测试。
