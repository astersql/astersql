# `pkg/util/rowcodec/encoder.rs`

## 文件定位

本文件实现 `astersql-util-rowcodec` crate 的新格式行编码器。crate 入口 `pkg/util/rowcodec/lib.rs` 通过 `include!("encoder.rs")` 把它纳入私有模块 `rowcodec_impl`，随后在 crate 根和 `rowcodec` 命名空间重新导出。其输出遵循 `row.rs` 定义的 Row Format：版本、标志、非空/空列计数、列 ID、非空列 offset、数据区，以及可选 checksum。

它位于“SQL Datum → TiKV 行 value”路径的底层。一个已接线的生产入口是 `pkg/table/tblctx/buffers.rs::EncodeRowBuffer::WriteMemBufferEncoded`：该函数准备列 ID、Datum、复用缓冲和可选 `RawChecksum`，调用 `pkg/tablecodec/tablecodec.rs::EncodeRow`；后者在 `rowcodec::Encoder.Enable` 为 `true` 时调用本文件的 `Encoder::Encode`，否则走旧格式 `EncodeOldRow`。

## 核心职责

- `Encoder::Encode` 把列 ID 与 `types::Datum` 配对，区分 NULL/非 NULL，按列 ID 排序，再编码为可随机查列的新格式行字节。
- `encodeValueDatum` 把支持的 Datum kind 压平成 row value 数据，不再保留通用 Datum 的外层类型标签；解码时由列元数据解释数据。
- 编码器根据列 ID 或数据区长度选择 small/large 布局：small 使用 1 字节列 ID 与 2 字节 offset，large 使用 4 字节列 ID 与 offset。
- `Checksum` 策略把“是否追加校验和”与主体编码分离；当前实现有 `NoChecksum` 和基于 raw handle 的 `RawChecksum`。
- `reset` 允许同一个编码器连续编码多行，并清除前一行的 flags、计数、offset、数据和 checksum 状态。

`Enable` 只供上层选择新旧编码格式，本文件的 `Encode` 不自行检查该字段；对应分支位于 `tablecodec.rs::EncodeRow`。

## 主要符号

- `pub struct Encoder`：持有内部 `row`、临时列 ID、非空 Datum 缓冲和公开开关 `Enable`。`new(enable)` 是 Rust 的显式构造入口；Go 版本还允许零值直接使用。
- `Encoder::Encode(loc, col_ids, values, checksum, buf)`：公开主入口。`buf` 不会在本函数内主动清空；上层 `tablecodec::EncodeRow` 在走新格式前会清空 `valBuf`。
- `Encoder::GetChecksum()`：把内部 `row::GetChecksum` 暴露给调用者，返回 checksum 值及存在标志。
- `reset`、`appendColVals`、`appendColVal`：内部状态复位和输入装载。`appendColVal` 在列 ID 大于 255 时预置 large 标志，并累计 NULL/非 NULL 数量。
- `reformatCols()`：把非空列放在前、空列放在后，两组分别按列 ID 升序排列；返回总列数和非空列数。
- `encodeRowCols()`：逐个编码非空 Datum、记录累计 offset，并在数据区超过 `u16::MAX` 时把已经形成的 small 元数据迁移为 large 元数据。
- `encodeValueDatum()`：支持有符号/无符号整数、字符串/字节、MySQL 时间与 duration、enum、set、binary literal/bit、浮点、decimal、JSON 和 `VectorFloat32`。
- `pub trait Checksum`：策略接口，方法接收可变编码器和已有输出缓冲。
- `pub struct NoChecksum`：清除 `rowFlagChecksum`，直接调用 `row::toBytes`。
- `pub struct RawChecksum { Handle }`：写入 checksum 版本 2，对已序列化 value（含 checksum header）和 `Handle::Encoded()` 依次执行 IEEE CRC32，最后以小端追加 4 字节结果。
- `checksumVersionRawKey = 1` 与公开的 `checksumVersionRawHandle = 2`：分别代表旧 raw-key checksum 和当前 raw-handle checksum 版本；版本 1 也被同 crate 的 `row.rs` 解码/校验逻辑引用。

## 执行流程

1. `Encoder::Encode` 调用 `reset`，保证复用对象不残留上一行的布局和 checksum 状态。
2. `appendColVals` 按 `col_ids` 驱动 zip 装载输入；若 Datum 少于列 ID，会由断言失败；多出的 Datum 不参与编码，这一行为由 `encoder_test.rs::encode_ignores_values_without_column_ids_like_go` 固定。
3. `reformatCols` 分离 NULL 与非 NULL：非空项保留 `(col_id, datum)` 配对并排序，NULL 只需保存列 ID；最终列 ID 顺序是“有值列升序 + NULL 列升序”。
4. 若已有列 ID 大于 255，初始化 `colIDs32/offsets32`；否则初始化 `colIDs/offsets`。只有非空列需要 offset 和数据。
5. `encodeRowCols` 调用 `encodeValueDatum` 追加各非空值，并把每列结束位置写入 offset。若累计数据首次超过 65,535 字节，它复制全部列 ID和已写 offset 到 32 位数组，再设置 `rowFlagLarge`。
6. Datum 编码规则包括：整数用同模块 `encodeInt/encodeUint`；字符串/字节直接追加；timestamp 在给定非 UTC 时区时先转 UTC；decimal 和 float 委托 `codec`；JSON 写 `TypeCode` 后接 payload；vector 调用自身序列化。
7. Datum 全部成功后，缺省 checksum 策略替换为 `NoChecksum`。策略调用 `row::toBytes(buf)` 形成主体，`RawChecksum` 再追加 header 与 CRC32。

列 ID 排序不是展示层行为，而是 row decoder 二分/定位列的格式不变量；NULL 不占数据区，靠 NULL 列 ID 集合表达。

## 数据与状态

`Encoder` 是有状态、可复用对象。`row` 保存本次编码的 flags、列计数、small/large 列 ID 和 offset、连续数据区、checksum header 与两个 checksum 槽；`temp_col_ids` 和 `values` 是整理布局前的暂存区。每次编码都把这些逻辑长度归零，但通常保留底层 `Vec` 容量。

Rust 实现与 Go 的内存模型并非完全相同：输入 `Vec` 和 Datum 由 `Encode` 取得所有权；`reformatCols` 会重新 collect `self.values`，而 Go 原版在已有 slice 内原地压缩和排序。因此格式意图一致，但分配/复用特征不能仅凭 Go 实现推断。

small/large 的两个触发条件是 `col_id > 255` 或非空数据区长度超过 `u16::MAX`。负列 ID 沿用 Go 的强制转换行为，写入 small/large 无符号列 ID 槽；调用者必须只传 rowcodec 协议允许的列 ID。`numNotNullCols` 与 `numNullCols` 是 `u16`，本文件没有额外的超量输入检查。

传入 `buf` 的已有前缀由 `row::toBytes` 的具体追加语义保留；直接调用 `Encoder::Encode` 的调用者不能假设缓冲会被截断。常规 `tablecodec::EncodeRow` 路径会先执行 `valBuf.clear()`。

## 依赖与调用关系

上游关键边：

- `pkg/table/tblctx/buffers.rs::WriteMemBufferEncoded` 构造可选 `RawChecksum`，随后调用 `tablecodec::EncodeRow` 并把结果写入 `kv::MemBuffer`。
- `pkg/tablecodec/tablecodec.rs::EncodeRow` 校验 row/colID 数量，检查 `Encoder.Enable`，然后调用 `Encoder::Encode`；这是新旧行格式的切换点。
- `pkg/util/rowcodec/lib.rs::encode_from_old_row`（仅测试配置）复用私有 `reset`、`appendColVal`、`reformatCols` 和 `encodeRowCols`，把旧格式列对转换成新格式。

下游关键边：

- `row.rs` 提供格式 flags、`row` 存储、数组初始化、`toBytes` 和 `GetChecksum`。
- `common.rs::encodeInt/encodeUint` 编码整数；`codec::EncodeFloat/EncodeDecimal` 处理浮点与 decimal。
- `types::Datum` 及 MySQL/scalar 类型提供 kind、取值、时间转换和 vector 序列化；`kv::Handle::Encoded` 提供 checksum 的行身份字节。
- `crc32fast`（Cargo 依赖名 `crc32fast = "1"`）经同模块公共辅助 `crc32_update` 实现 IEEE CRC32。

`pkg/util/rowcodec/Cargo.toml` 把本文件归入 `astersql-util-rowcodec`，直接依赖同工作区的 chunk、codec、kv、model、types 相关 crates，并提供 `nextgen` feature 向 kerneltype 与 kv 依赖透传。本文件没有条件编译分支，其编码逻辑在默认和 `nextgen` feature 下共用。

## 错误处理与边界

- `Encode` 只在 `encodeRowCols` 成功后生成最终行字节；任一 Datum 编码失败会返回 `errors::SharedError`，不会返回部分结果。
- 时间转换/打包、binary literal 转整数、decimal 编码以及未知 Datum kind 都可能报错；未知类型错误文本为 `unsupport encode type <kind>`。
- `encodeRowCols` 继续遍历以收集多个单列错误。当前 Rust 在多个错误时把错误字符串用 `"; "` 合并为一个 `errors::New`，不像 Go 的 `multierr.ErrorGroup` 保留结构化子错误；依赖 SQL mode/错误分类的扩展应先处理这一兼容差异。
- Rust 通过 `std::mem::take(&mut r.data)` 把数据区交给 `encodeValueDatum`。若该调用返回错误，原缓冲不会放回 `r.data`；由于最终仍返回错误，这不会产出成功行，但会改变错误后的内部暂存状态。修改错误恢复或部分结果语义时必须增加回归测试。
- `appendColVals` 明确断言 `col_ids.len() <= values.len()`；多余 values 被 zip 忽略。常规上游 `tablecodec::EncodeRow` 要求二者长度严格相等，因此不对称情况主要是直接调用 API 时可见。
- 本文件信任 `row` 的计数和转换容量，没有防御列数超过 `u16`、数据长度超过 `u32` 或非法列 ID；这些是协议/调用者边界，不应在没有 Go 兼容依据时单独改变。
- `RawChecksum` 要求有效 handle。CRC 输入是 `row.toBytes(buf)` 的全部结果、随后追加的 checksum header，再追加 handle 编码；改变顺序、header 位或字节序会破坏存储兼容。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`Encode` 需要 `&mut self`，因此同一 `Encoder` 不能在安全 Rust 中被并发编码；典型用法是把它作为会话/语句级缓冲的一部分顺序复用。

所有编码数据由 `Vec` 拥有，函数返回后不存在借用到输入 Datum 的引用。`RawChecksum` 在策略对象中拥有 `Box<dyn kv::Handle>`，策略执行结束后随 Box 释放。`WriteMemBufferEncoded` 的上游路径通过 `Rc<RefCell<WriteStmtBufs>>` 借用语句缓冲，编码完成后归还 `RowValBuf`；这属于调用者的单线程生命周期约束，而非本文件内部同步机制。

复用不等于跨调用保留语义状态：`reset` 必须清零 large/checksum flags 和所有计数。`rowcodec_test.rs::TestEncodeLargeSmallReuseBug` 专门验证同一实例从 large 行切回 small 行不会污染结果。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/rowcodec/encoder.go`。Rust 保留了 `Encoder`、`Encode`、状态整理函数、`Checksum`/`NoChecksum`/`RawChecksum`、两个 checksum 版本常量以及各 Datum 分支，核心 wire-format 流程一致：非空/空列分组排序、small/large 切换、timestamp 转 UTC、raw-handle CRC32 和小端 checksum。

已确认的实现差异：

- Go `Encoder` 可用零值；Rust 必须使用 `Encoder::new(enable)`，因为字段私有且未实现 `Default`。
- Go 输入 values 是 slice 且内部暂存 Datum 指针；Rust 接收并拥有 `Vec<Datum>`，排序时克隆非空 Datum。
- Go `appendColVals` 在 values 较短时因索引越界 panic、较长时忽略尾部；Rust 用断言表达前者，并由 zip 表达后者。
- Go 用 `multierr.Append` 返回可枚举的错误组；Rust 当前把多个错误文本拼成单个共享错误。
- Go 在 `encodeValueDatum` 出错时命名返回值仍保留当时 buffer；Rust 的所有权转移使错误路径不会把已取出的数据区恢复到 encoder。不过两者的公开 `Encode` 都在错误时不返回成功编码字节。
- Go 原地复用 slice；Rust `reformatCols` 有临时 Vec 和 clone/collect，可能有额外分配，属于性能审查点。

说明中的对应关系以当前两份源码为准；“接口形状相似”不代表错误类型和分配行为已经完全等价。

## 扩展指南

- 新增 Datum kind：修改 `encodeValueDatum`，同时核对对应 decoder、Go `encodeValueDatum`、codec 表示和列元数据解释；在独立的 `encoder_test.rs` 或 `rowcodec_test.rs` 增加成功往返、边界值和失败用例，不要把测试嵌入生产文件。
- 修改 row 布局或阈值：同步审查 `row.rs::{fromBytes,toBytes}`、`decoder.rs`、`common.rs`、Go 对照和 tablecodec；尤其验证 255/256 列 ID、65,535/65,536 数据长度、NULL 排序以及编码器复用。
- 新增 checksum 策略/版本：实现 `Checksum`，明确 header version/extra 位、CRC 输入范围与字节序，并同步 decoder 的版本识别。必须保留版本 1 的读取兼容和版本 2 的 raw-handle 行为。
- 改变错误模型：优先恢复 Go `ErrorGroup` 可分类语义，并覆盖多列同时失败、错误后复用 encoder、SQL mode 上层处理；不能只比较错误字符串。
- 优化分配：可围绕 `reformatCols` 的 clone/collect 和所有权接口做基准，但必须保持列和值的配对、稳定 wire bytes 以及 `buf` 前缀约定。基准入口可参考 `bench_test.rs::BenchmarkEncode`。
- 接入新调用方：通常应通过 `tablecodec::EncodeRow`，由它执行长度校验和新旧格式选择；直接调用 `Encoder::Encode` 时必须自行决定是否清空 `buf`、保证列 ID/Datum 契约并提供正确 handle。

相关独立测试位于 `pkg/util/rowcodec/encoder_test.rs`、`common_1_aster_unit_test.rs`、`rowcodec_test.rs`，上层接线测试位于 `pkg/table/tblctx/buffers_test.rs` 和 `migration_aster_unit_test.rs`。Rust 单元测试应继续保存在这些独立文件中。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/util/rowcodec` 确认本模块的 Rust/Go 源与测试均被索引。
- RustCodeGraph 源码/符号：`node --file pkg/util/rowcodec/encoder.rs`；`node Encoder`；`node encodeRowCols`；`node encodeValueDatum`；`node RawChecksum`。图边确认 Rust `Encode -> encodeRowCols -> encodeValueDatum`，并显示 `encode_from_old_row -> encodeRowCols`。
- RustCodeGraph 上游证据：`node --file pkg/tablecodec/tablecodec.rs --offset 380` 验证 `EncodeRow` 的长度检查、`Enable` 分支和调用；`node --file pkg/table/tblctx/buffers.rs --offset 1` 验证行级 checksum 与 MemBuffer 写入链。
- 格式证据：`node --file pkg/util/rowcodec/row.rs --offset 1` 验证 flags、small/large 字段宽度、checksum header 和 row 内部状态。
- crate/装配证据：`pkg/util/rowcodec/Cargo.toml` 与 `pkg/util/rowcodec/lib.rs`，核对 crate 名、feature、依赖、`include!` 和再导出边界。
- Go 对照：`pkg/util/rowcodec/encoder.go`，逐项核对输入整理、Datum 分支、large 迁移、multierr 和 checksum 算法。
- Rust 测试：`encoder_test.rs::encode_ignores_values_without_column_ids_like_go`；`common_1_aster_unit_test.rs::{encoder_sorts_columns_and_preserves_nulls,large_row_and_checksum_follow_go_layout}`；`rowcodec_test.rs::{TestEncodeLargeSmallReuseBug,TestEncodeKindNullDatum,TestEncodeDecodeRowWithChecksum}`，以及该文件中的类型往返和 65,535 字节边界用例。
- Go 测试：`pkg/util/rowcodec/rowcodec_test.go` 中对应的 large/small 复用、Datum 编码、65,535 字节和 checksum 测试；它用于确认迁移意图，而 Rust 独立测试用于确认当前实现。
- 本任务是只读分析加文档输出，按计划不运行 Cargo。最终以任务指定的 11 个固定二级标题结构命令校验文档形状，并人工复核上述符号与路径均可追溯。
