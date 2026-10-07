# `pkg/ingestor/simplesst/codec.rs`

## 文件定位

本文件是 `astersql-ingestor-simplesst` crate 的范围统计元数据编解码层。crate 由 [`lib.rs`](lib.rs) 声明 `codec` 模块，并把 `RangeProperty` 重导出为 crate 级类型；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `pkg/ingestor/simplesst`。它不编码 SST 数据文件中的 KV 正文，而是编码伴随数据文件写出的 range property（范围属性）记录。

写侧主链为 [`file.rs`](file.rs) 的 `KeyValueStore::add_encoded_data` 更新文件偏移并调用 `RangePropertiesCollector::on_next_encoded_data`，随后 [`writer.rs`](writer.rs) 的 `RangePropertiesCollector::encode` 调用本文件的 `encode_multi_props` 生成统计字节流。读侧由 [`stat_reader.rs`](stat_reader.rs) 的 `StatsReader::next_prop` 读取四字节长度头和对应正文，再调用 `decode_prop`。因此，本文件定义了写侧和读侧共享的统计文件线格式。

## 核心职责

- 用 `RangeProperty` 表达连续键范围的 `FirstKey`、`LastKey`、数据文件起始 `Offset`、有效载荷 `Size` 和 `Keys` 数量。
- 用固定的大端格式编码/解码单条属性正文：`u32 FirstKey长度`、`FirstKey`、`u32 LastKey长度`、`LastKey`、`u64 Size`、`u64 Keys`、`u64 Offset`。
- 用 `<u32 正文长度><属性正文>` 重复序列表示多条属性，使统计文件可以逐条读取。
- 在 Rust 边界上检查游标加法、缓冲区长度和 `usize -> u32` 转换，把畸形或过大的输入转换为 crate 的 `Error::InvalidData`。
- 保留 `decodeProp`、`encodeProp`、`decodeMultiProps`、`encodeMultiProps` 这组 Go 风格兼容入口，同时提供惯用 snake_case API。

## 主要符号

- `PROPERTY_LENGTH_EXCEPT_KEYS: usize`：两段 `u32` 键长度和三个 `u64` 数值字段的固定开销，共 32 字节；键正文长度另计。
- `RangeProperty`：可克隆、可比较且有全零/空键默认值的公开结构。字段名刻意沿用 Go 命名；`lib.rs` 允许 `non_snake_case` 并重导出该类型。
- `take(data, cursor, count)`：私有的边界检查切片器。成功时返回当前片段并推进游标；加法溢出或数据不足时返回 `InvalidData`。
- `decode_prop(data)`：解码一条正文。只消费格式规定的字段，不要求输入恰好耗尽。
- `encode_prop(buf, property)`：把一条正文追加到调用者提供的缓冲区，不写外层正文长度。
- `encode_multi_props(props)`：为每条属性计算正文长度、写四字节长度头，再调用 `encode_prop`。
- `decode_multi_props(data)`：消费完整的长度前缀序列，逐条切出正文并调用 `decode_prop`。
- 四个 camelCase 函数：Go 风格适配层；拥有型编码适配器会保留已有缓冲内容并在尾部追加结果。

## 执行流程

单条编码从 `encode_prop` 开始。函数先分别把两个键长度转换为 `u32`，然后按大端顺序追加长度、键正文、`Size`、`Keys`、`Offset`。字段的结构体排列是 `Offset/Size/Keys`，但线格式数值顺序固定为 `Size/Keys/Offset`，扩展或重构时不能按结构体字段顺序推断线格式。

多条编码由 `encode_multi_props` 遍历属性列表。每条正文长度等于 `PROPERTY_LENGTH_EXCEPT_KEYS + FirstKey.len() + LastKey.len()`；函数先用受检加法得到长度并转换为 `u32`，写入长度头后再追加单条正文。空列表编码为空字节串。

单条解码由 `decode_prop` 用局部游标依次读取字段。所有定宽读取均先经过 `take`，所以后续 `try_into().unwrap()` 的数组转换建立在“切片长度已经精确验证”的不变量上，不会因外部输入长度而 panic。键被复制到独立 `Vec<u8>`，返回值不借用输入缓冲。

多条解码由 `decode_multi_props` 循环到输入完全耗尽：先要求至少四字节长度头，再验证剩余缓冲能容纳声明正文，调用 `decode_prop` 后把输入推进到下一条。与此不同，生产读侧 `StatsReader::next_prop` 自己读取长度头，只把单条正文交给 `decode_prop`。

## 数据与状态

本文件没有全局可变状态。`RangeProperty` 是每个范围的值对象；两个键由对象拥有，三个计数均为 `u64`。其语义来源于 [`writer.rs`](writer.rs) 的 `RangePropertiesCollector`：首个键设置 `FirstKey`，每次写入更新 `LastKey`，`Size` 累计编码 KV 去掉两个八字节长度头后的字节数，`Keys` 计数，达到阈值后下一段的 `Offset` 设为当前数据文件尾。

单条正文的固定开销为 32 字节，实际长度还要加上两个键正文；完整多条记录每条另带四字节正文长度头。编码函数只追加、不清空传入缓冲；解码函数不修改输入。`decode_prop` 会忽略已定义字段之后的尾随字节，这是与 Go 行为一致且由独立测试固定的兼容性质；`decode_multi_props` 则把外层声明长度之后的字节解释为下一条记录，因而要求整个序列由合法记录组成。

## 依赖与调用关系

本文件仅直接依赖 crate 根的 `Error` 和 `Result`，以及标准库的整数转换、切片和 `Vec`；没有外部 crate 依赖。`Cargo.toml` 将库入口设为 `lib.rs`，并记录 Go 移植来源；其中列出的工程依赖受 `cfg(windows)` 限定，但本编解码文件本身不包含条件编译项。

直接下游调用边为 `encode_multi_props -> encode_prop`、`decode_multi_props -> decode_prop`，四个兼容别名分别转发到对应 snake_case 实现。直接上游生产调用边包括 `RangePropertiesCollector::encode -> encode_multi_props` 和 `StatsReader::next_prop -> decode_prop`。`RangeProperty` 还被 `writer.rs` 用于收集范围、被 [`iter.rs`](iter.rs) 用于迭代期间携带当前属性，并经 `lib.rs` 暴露给 crate 使用者。

RustCodeGraph 的文件查询将 `codec.rs` 识别为含 13 个符号的已索引文件；精确 `query` 能唯一定位四个 snake_case 编解码函数。图的 `callers/callees` 命令本次未返回边，因此上述直接调用边又用精确符号引用搜索和相邻源码核验，未把空图结果解释为“没有调用者”。

## 错误处理与边界

- `take` 对游标加法溢出返回 `InvalidData("property length overflow")`，对正文不足返回 `InvalidData("truncated range property")`。
- `encode_prop` 拒绝无法用 `u32` 表示的单个键长度，分别报告 `first key too large` 或 `last key too large`。
- `encode_multi_props` 对两个键长度相加溢出报告 `property length overflow`，对完整正文超过 `u32` 报告 `property too large`。
- `decode_multi_props` 区分不足四字节的长度头（`truncated property length`）与声明正文不足（`truncated property`）；正文内部字段不足继续由 `decode_prop`/`take` 报错。
- 空键和全零数值是合法数据，`RangeProperty::default()` 的单条正文正好为 32 字节。
- 编解码层不检查 `FirstKey <= LastKey`、范围排序、偏移单调性或统计值与数据文件一致性；这些是生成者和更高层管线的不变量，不应在此文档中误称为已验证。
- `decode_prop` 有意接受尾随字节；若调用方需要“单条输入必须完全消费”的严格协议，应在调用层用声明长度或独立长度检查实现，而不能直接改变该函数并破坏 Go 兼容性。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、文件句柄或网络资源。所有函数均同步执行；编码只可变借用调用者传入的 `Vec<u8>`，解码返回拥有键数据的新值，因此结果可脱离输入缓冲生命周期。

资源生命周期位于相邻模块：写侧收集器随 `KeyValueStore` 累计并在 `finish`/`into_parts` 时封存最后范围；读侧 `StatsReader` 持有 `ByteReader` 并由其 `close` 释放读取状态。本文件只定义可跨这些生命周期边界传递的字节格式，不负责关闭或提交资源。并发安全取决于调用者如何共享缓冲和对象；API 本身不保存共享可变引用。

## 与 Go 版本的对应关系

[`codec.go`](codec.go) 是直接对照实现。Rust 的 `RangeProperty` 字段、32 字节固定开销、大端序、正文顺序以及外层四字节长度前缀均与 Go 的 `RangeProperty`、`propertyLengthExceptKeys`、`encodeProp`/`decodeProp`、`encodeMultiProps`/`decodeMultiProps` 对齐。两端的单条解码都只读取已定义字段，因此允许正文后存在尾随字节。

差异主要在安全接口而非线格式：Go 返回指针切片且键直接引用输入切片，Rust 返回拥有键副本的值；Go 的解码对截断输入会因切片越界而 panic，Rust 返回 `Result`/`InvalidData`；Go 将长度直接转为 `uint32`，Rust 在编码前拒绝超出 `u32` 的键或正文；Rust 的 `decode_multi_props` 是可用于生产代码的公开安全函数，而 Go 源码注明 `decodeMultiProps` 仅供测试。camelCase Rust 入口保留了调用形态上的迁移便利，但错误返回仍遵循 Rust crate 约定。

[`codec_test.go`](codec_test.go) 验证单条和三条属性往返以及零值固定长度；[`codec_test.rs`](codec_test.rs) 覆盖相同意图，并额外验证截断多条输入返回错误和单条尾随字节兼容性。

## 扩展指南

若新增字段或改变线格式，至少应同步修改 `PROPERTY_LENGTH_EXCEPT_KEYS`、`encode_prop`、`decode_prop`、外层长度计算、Go `codec.go` 对照实现及独立的 `codec_test.rs`/`codec_test.go`。还要检查 `StatsReader::next_prop`、`RangePropertiesCollector::encode` 和所有独立线格式 fixture；字段顺序或宽度变化属于持久化兼容性变更，必须考虑旧统计文件能否读取，不能只做 Rust 内部结构调整。

若只新增校验，先判断它属于字节安全还是业务不变量。长度溢出、截断适合留在本文件；键排序、范围连续性、偏移和数据文件一致性更适合在收集器、writer 或 reader 上层验证。改变 `decode_prop` 的尾随字节策略会偏离 Go，需新增明确的兼容测试与迁移说明。

新增 Rust 回归测试应继续放在独立的 [`codec_test.rs`](codec_test.rs)，由 `lib.rs` 的 `#[cfg(test)] mod codec_test` 接入，不要把测试嵌入生产源文件。建议覆盖空属性、多属性边界、长度头截断、正文内部各字段截断、伪造的超大声明长度和已知 Go 字节 fixture。性能改动应保留顺序追加和线性扫描特性；若要预分配容量，需要用受检算术，避免重新引入溢出。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引可用（11,467 个文件、307,296 个节点）；`files --filter pkg/ingestor/simplesst` 确认目标、Go 对照及独立测试均在索引中；`node --file pkg/ingestor/simplesst/codec.rs` 读取完整 139 行源文件；`query` 唯一定位 `decode_multi_props`、`encode_multi_props`、`encode_prop`，并定位目标 `decode_prop` 与同名测试。
- 目标实现：[`codec.rs`](codec.rs) 的 `RangeProperty`、`take`、四个 snake_case 编解码函数和四个 Go 风格适配函数。
- crate 与错误边界：[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs) 的模块声明、重导出、`Error::InvalidData` 与 `Result`。
- 生产调用链：[`writer.rs`](writer.rs) 的 `RangePropertiesCollector::{on_next_encoded_data,encode}`、[`file.rs`](file.rs) 的 `KeyValueStore::{add_encoded_data,finish}`、[`stat_reader.rs`](stat_reader.rs) 的 `StatsReader::next_prop`。
- Go 对照：[`codec.go`](codec.go)、[`writer.go`](writer.go) 的 `RangePropertiesCollector`、[`stat_reader.go`](stat_reader.go) 的 `StatsReader::NextProp`。
- 测试证据：[`codec_test.rs`](codec_test.rs) 和 [`codec_test.go`](codec_test.go)；其他调用样例可见 `iter_test.rs`、`util_test.rs`，但它们不是本文件语义的主要断言来源。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节结构、文件范围和事实链接。
