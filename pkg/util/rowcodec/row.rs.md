# `pkg/util/rowcodec/row.rs`

## 文件定位

本文件位于 `astersql-util-rowcodec` crate 内，是新格式行（new-format row）的底层内存表示与字节布局实现。crate 入口 `pkg/util/rowcodec/lib.rs` 在同一个 `rowcodec_impl` 模块中依次 `include!` `common.rs`、本文件、`encoder.rs` 和 `decoder.rs`，因此本文件能够直接使用 `CodecVer`、切片转换函数、错误构造器和 `encodeValueDatum`，编码器与解码器也能访问 `row` 的私有字段和方法。

它不是 SQL 请求入口，也不直接操作存储。上层 `Encoder` 用它组织列 ID、offset、数据和 checksum；`DatumMapDecoder`、`ChunkDecoder`、`BytesDecoder` 则通过它解析 TiKV row value，再完成 Datum、Chunk 或旧格式 datum bytes 的转换。crate 的 `nextgen` feature 只向 `kerneltype`、`kv` 依赖传递，本文件没有条件编译分支。

## 核心职责

- 用 `row` 保存新格式行的 header、非空/空列计数、列 ID、列结束 offset、列数据和至多两个 checksum。
- 根据 `rowFlagLarge` 在 small 布局（`u8` 列 ID、`u16` offset）和 large 布局（`u32` 列 ID、`u32` offset）之间选择表示。
- 由 `fromBytes` 解析已有 row bytes，由 `toBytes` 重建 checksum 之前的主体字节。
- 由 `findColID` 在已排序的非空列区和空列区分别二分查找，并由 `getOffsets`/`getData` 定位非空列的编码值。
- 暴露 checksum 是否存在及版本信息；保留 extra checksum、NULL 判断和 raw checksum 计算的内部实现。
- 为 `Encoder` 提供四个缓冲区初始化方法，使同一个编码器可以重复使用已分配容量。

这里的 `row` 是包内机制而非完整的健壮 wire-format 验证器：它依赖编码数据满足长度、offset 和排序不变量。

## 主要符号

- `rowFlagLarge`：flags 的 bit 0。置位时使用 `colIDs32`/`offsets32`；编码器在列 ID 超过 255 或数据无法由 `u16` offset 表示时设置它。
- `rowFlagChecksum`：flags 的 bit 1，表示主体之后存在 checksum header 和主 checksum。
- `checksumMaskVersion`、`checksumFlagExtra`：分别读取 checksum header 的低 3 位版本和 extra-checksum 标志。
- `pub struct row`：格式状态主体。公开字段只有 `colIDs`、`colIDs32`；其余字段仍可被同一 `rowcodec_impl` 模块中的 encoder/decoder 使用。`Default` 提供空状态，`Clone` 执行拥有型复制。
- `large`、`hasChecksum`、`hasExtraChecksum`：三个位标志查询。
- `getOffsets(i)`、`getData(i)`：按第 `i` 个非空列读取 `[start, end)` 和对应 `data` 切片；第 0 列的起点固定为 0，后续列起点取前一个累计 offset。
- `fromBytes(rowData)`：读取版本、flags、计数、列 ID、offset、data 和可选 checksum；codec 版本或 checksum 版本不支持时返回 `SharedError`。
- `toBytes(buf)`：在调用者已有 `Vec<u8>` 后追加版本、header、列 ID、offset 和 data；不追加 checksum header/checksum 值。
- `findColID(colID)`：返回 `(idx, is_nil, not_found)`。只有命中非空列时 `idx` 有意义；命中空列返回 `(0, true, false)`。
- `ChecksumVersion`、`GetChecksum`、`GetExtraChecksum`：读取 checksum 元数据。`ChecksumVersion` 只有在 `GetChecksum().1` 为真时才有有效语义。
- `ColumnIsNull`：重新解析一行后判断列是否为 NULL；缺列时以 `defaultVal.is_none()` 决定结果。
- `initColIDs`、`initColIDs32`、`initOffsets`、`initOffsets32`：按当前计数 resize 或重新分配编码缓冲。
- `CalculateRawChecksum`：以给定 Datum 临时覆盖已存在的非空列字节，重建主体并计算 CRC32；checksum 版本决定最后拼 raw key 还是编码后的 handle。

## 执行流程

解码路径如下：

1. `DatumMapDecoder::DecodeToDatumMap`、`ChunkDecoder::DecodeToChunk` 或 `BytesDecoder::decodeToBytesInternal` 调用 `fromBytes`。
2. `fromBytes` 先要求 `rowData[0] == CodecVer`，再从字节 1 和小端字节 2..6 读取 flags 与两种列计数。
3. small/large 分支按计数解析两段列 ID（先非空、后 NULL）和仅属于非空列的累计 offsets；最后一个 offset 决定 data 长度。
4. 若有 checksum flag，则读取 checksum header，接受版本 0、1、2，随后读取主 checksum，并在 extra bit 置位时读取第二个 checksum；无 checksum 时清零三个相关字段。
5. 解码器针对所需列调用 `findColID`。命中非空列时以 `getData` 取值并继续类型解码；命中 NULL、缺列、handle fallback 和默认值由具体 decoder 分支处理。

编码路径由 `Encoder` 驱动：`reset` 清理当前状态，`appendColVal` 累计列和 large 标志，`reformatCols` 将非空与空列分别排序，`encodeRowCols` 调用 `init*` 并写入列 ID、offset 和 data。`NoChecksum::encode` 直接调用 `toBytes`；`RawChecksum::encode` 先调用 `toBytes`，再追加 checksum header、计算并追加主 checksum。

raw checksum 重算路径中，`CalculateRawChecksum` 逐个 `encodeValueDatum`，仅覆盖 row 中真实存在且非 NULL 的列；然后对 `toBytes(buf) + checksumHeader` 求 CRC32。版本等于 `checksumVersionRawKey` 时继续混入 `key`，否则混入 `handle.Encoded()`，以保留 v8.3.0 前后的兼容分支。

## 数据与状态

字节主体顺序为 `CodecVer | flags | non-null count | null count | column IDs | non-null offsets | data`，之后才可能出现 `checksum header | checksum1 | checksum2?`。两个 count 均为小端 `u16`。列 ID 区由两个各自有序的连续区组成：前 `numNotNullCols` 个对应 data/offset，后 `numNullCols` 个只表示 SQL NULL，不分配数据 offset。

small 布局使用 `Vec<u8>` 和 `Vec<u16>`，large 布局使用 `Vec<u32>` 和 `Vec<u32>`。offset 是 data 起点为 0 的累计结束位置，因此第 `i` 列范围为前一个 offset（或 0）到当前 offset。`data` 只含非空列编码值。

Rust `fromBytes` 会把列 ID、offset 和 data 复制进拥有型 `Vec`；Go `row.go` 则让这些 slice 别名原始 `rowData`。因此 Rust 行状态不依赖输入切片生命周期，但每次解析会产生复制和分配/复用成本。`init*` 只服务编码侧容量复用；解析侧会替换当前分支的 Vec，而未选中的 small/large Vec 可能保留旧容量和内容，但所有读取都由 `large()` 选择当前有效分支。

## 依赖与调用关系

同 crate 直接依赖如下：

- `common.rs` 提供 `CodecVer`、`errInvalidCodecVer`、`errInvalidChecksumVer`、字节/整数切片转换和 checksum 版本常量。
- `encoder.rs` 持有 `Encoder.row`，写入全部私有格式字段，调用 `large`、四个 `init*`、`toBytes`、`GetChecksum`；本文件反向调用其 `encodeValueDatum`。
- `decoder.rs` 的共享 `decoder.row` 调用 `fromBytes`、`findColID`、`getData`、`ColumnIsNull`、`GetChecksum`、`ChecksumVersion`。RustCodeGraph 显示 `DecodeToDatumMap`、`DecodeToChunk`、`decodeToBytesInternal` 都沿此路径消费本文件。
- `lib.rs::encode_from_old_row` 在测试配置中通过 encoder 构造 `row`，最后调用 `toBytes`。
- `kv::Key`、`kv::Handle`、`types::Datum`、`time::Location` 只在 `CalculateRawChecksum` 签名和值重编码中使用；CRC32 由同模块封装的 `crc32_update` 完成。

`Cargo.toml` 将该目录声明为 `astersql-util-rowcodec`，直接依赖 `astersql-kv`、Datum/type crates、chunk、codec、kerneltype 和 `crc32fast` 等。当前图中 `CalculateRawChecksum`、`GetExtraChecksum` 没有 Rust 调用者；它们是同模块私有实现，不能据此声称 Rust 公共 API 已接通 TiCDC extra/raw-checksum 重算流程。

## 错误处理与边界

- `fromBytes` 对不等于 `CodecVer` 的首字节返回 `errInvalidCodecVer()`；checksum 版本只接受 0、1、2，其他值返回 `errInvalidChecksumVer()`。
- 除上述两个协议检查外，它不验证最小 header 长度、区段总长度、offset 单调性、最终 offset 是否落在输入内、列 ID 是否排序或尾部是否完整。短输入、伪造计数/offset 或不完整 checksum 会在直接索引、切片或 `try_into().unwrap()` 处 panic；这是与 Go 直接索引/切片相近的受信编码输入前提，不应描述为安全拒绝畸形输入。
- `getOffsets`、`getData` 假定索引来自成功的 `findColID` 且 offsets/data 一致；错误索引或损坏 offset 同样会 panic。
- `findColID` 依赖非空列区与 NULL 列区分别升序。若编码器或外部数据破坏排序，二分查找可能报告缺列。
- `ColumnIsNull` 区分三种状态：实际非空、实际 NULL、缺列。缺列且无默认值视为 NULL；缺列但存在默认字节视为非 NULL。
- `CalculateRawChecksum` 假定 `values.len() >= colIDs.len()`；否则 `values[idx]` 会 panic。重编码值与原槽长度不同时，它刻意模拟 Go `copy`，只复制源/目标较短长度，不调整 offsets；调用者必须保证这一形状符合 checksum 重算场景。
- `toBytes` 不包含 checksum payload。调用它的 checksum 策略必须显式追加 header 和 checksum；单独把它当完整 checksummed row 会截断尾部。

## 并发与资源生命周期

`row` 不包含锁、原子、通道、异步任务、文件描述符或网络资源。所有 Vec 和 checksum 状态都由实例独占并随实例释放；`getData` 返回的切片借用 `self.data`，借用期间不能再可变修改该实例。

`fromBytes`、`ColumnIsNull`、四个 `init*` 和 `CalculateRawChecksum` 都修改实例，因此需要 `&mut self`；同一个 encoder/decoder 实例不能在多个线程中无同步地并发使用。上层若要并发解码，应为每个工作单元持有独立 decoder，或在外部加同步。容量复用意味着实例适合顺序重复使用，但可能保留一次大行分配的容量直到实例销毁。

Rust 解析结果拥有数据，不会悬垂到调用者的 `rowData`；这是相对 Go 零拷贝别名模型更清晰的生命周期边界，同时带来额外复制和峰值内存风险。

## 与 Go 版本的对应关系

`pkg/util/rowcodec/row.go` 是逐函数对照来源：常量、字段顺序、small/large 格式、二分查找三元返回、checksum 版本白名单、缺列默认值语义和 v8.3.0 raw-key 兼容分支均保持一致。Rust 用 `Result<_, errors::SharedError>` 代替 Go `error`，用 `Option<&[u8]>` 区分 Go 的 nil default slice，用 `&dyn kv::Handle` 表示接口值。

主要实现差异是内存所有权：Go 的 `bytesToU32Slice`/`bytes2U16Slice` 和 `data` 是原输入的视图，Rust helper 返回拥有型 Vec，small 列 ID 与 data 也显式 `to_vec`。因此 Go 的解析接近零拷贝，而当前 Rust 实现选择复制。`CalculateRawChecksum` 中 Rust 用 `min(dst.len(), data.len())` 后局部复制，精确对应 Go `copy` 的短者长度规则。

可见性也不同：Rust 的 `row`、`fromBytes`、`findColID`、`getData` 部分被声明为 `pub`，但位于私有 `rowcodec_impl` 的内部组合模型中；`ColumnIsNull`、`GetExtraChecksum` 和 `CalculateRawChecksum` 仍为私有，并通过相邻 include 文件或尚未接线的路径使用。Go 同名方法虽首字母大写，但接收者类型 `row` 本身未导出，通常通过嵌入它的 Decoder/Encoder 暴露。

测试对应关系包括：Rust `common_1_aster_unit_test.rs` 验证列排序、NULL 查找、`getData`、large 列 ID 和 raw-handle checksum；Rust `rowcodec_test.rs::TestCodecUtil` 验证 `ColumnIsNull` 的实际 NULL、非空、缺列无默认值和缺列有默认值四种分支，`TestEncodeDecodeRowWithChecksum` 验证无/有 checksum 及版本 2。Go `rowcodec_test.go` 还覆盖 small/large 编码、large→small 编码器复用和相同的 checksum/NULL 行为。

## 扩展指南

- 修改 wire format、flag 或 checksum header 时，应同步 `row` 字段、`fromBytes`、`toBytes`、encoder checksum 策略和三个 decoder 消费路径，并确认旧版本 0/1/2 的读取兼容性。
- 新增列查找或数据访问能力应建立在 `findColID` 与 `getOffsets` 的排序/累计 offset 不变量上；若需要支持不可信输入，应新增显式长度、溢出、单调性和尾部校验，而不是只在现有索引前补零散判断。
- 调整 small/large 阈值时，要同时检查 `encoder.rs::appendColVal`、`encodeRowCols`、四个 `init*` 以及 large→small 顺序复用，避免读取上一次编码遗留的另一组缓冲。
- 对外接通 `CalculateRawChecksum` 或 extra checksum 前，应先确定预期公共入口和 TiCDC 调用链，并补齐 checksum 版本、raw key/handle、缺列/NULL、值长度变化与 extra bit 的独立测试；当前无 Rust 调用边，不应仅提高可见性就宣称完成。
- 若优化为零拷贝解析，需要为 `row` 引入与输入绑定的生命周期或经过审计的安全表示，并评估 encoder 所需可变拥有状态；不能直接照搬 Go 的 unsafe slice alias。
- 测试应继续放在独立文件，而非嵌入 `row.rs`。优先扩展 `common_1_aster_unit_test.rs` 做内部布局/查找测试，扩展 `rowcodec_test.rs` 做公共 decoder 行为与 checksum 往返，并与 `rowcodec_test.go` 的边界用例保持一致。

## 验证依据

- 源码：`pkg/util/rowcodec/row.rs`（完整 403 行）、`common.rs`、`encoder.rs`、`decoder.rs`、`lib.rs`。
- crate 边界：`pkg/util/rowcodec/Cargo.toml`；workspace 由根 `Cargo.toml` 纳入 `pkg/util/rowcodec` 并提供 `facade_util_rowcodec` 依赖别名。
- Go 对照：`pkg/util/rowcodec/row.go`；相关行为测试为 `pkg/util/rowcodec/rowcodec_test.go`。
- Rust 独立测试：`pkg/util/rowcodec/common_1_aster_unit_test.rs`、`pkg/util/rowcodec/rowcodec_test.rs`；本目录没有 `row_test.rs` 或同名 `row.rs` 测试文件。
- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；索引包含本目录 19 个 Go/Rust 文件。
- RustCodeGraph 查询：`node --file pkg/util/rowcodec/row.rs` 核对全文件；`node fromBytes` 确认 Rust 调用者为 `ColumnIsNull`，并确认 decoder wrapper 的调用者为 `DecodeToDatumMap`、`DecodeToChunk`、`decodeToBytesInternal`；`node findColID` 确认调用者为 `ColumnIsNull`、`CalculateRawChecksum`；`node CalculateRawChecksum` 确认其下游为 `getOffsets`、`toBytes`、`findColID`、`ChecksumVersion`；`node DecodeToDatumMap`、`node DecodeToChunk`、`node decodeToBytesInternal` 核对上层解码主链。
- 人工复核结论：本文件存在的原因是集中维护新格式 row 的布局与共享访问不变量；运行时由 encoder 写、decoder 读；安全扩展必须同时维护格式两端、Go 兼容语义和独立测试。按任务约束未运行 Cargo，也未把未接线的内部 checksum 方法描述为现有公共能力。
