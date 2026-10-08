# `pkg/util/rowcodec/common.rs`

## 文件定位

`common.rs` 是 `astersql-util-rowcodec` crate 的公共基础实现，由 [`lib.rs`](lib.rs) 的 `rowcodec_impl` 通过 `include!("common.rs")` 与 `row.rs`、`encoder.rs`、`decoder.rs` 合并到同一模块，再由 crate 根和 `rowcodec` 命名空间公开。它不是独立模块文件，因此其中的私有函数也能被另外三个实现文件直接调用。

该文件位于行值编解码链的底层：定义新行格式版本和 Datum flag，提供紧凑整数编解码、行键/格式判断、行 checksum 输入编码、CRC32 更新和 next-gen keyspace 前缀处理。直接执行路径可见于 [`encoder.rs`](encoder.rs)（写入整数和 raw-handle checksum）、[`decoder.rs`](decoder.rs)（读取紧凑整数）、[`row.rs`](row.rs)（解析/序列化头部并校验版本）以及 `pkg/tablecodec/tablecodec.rs`（识别新格式、移除 keyspace 前缀）。

## 核心职责

1. 维持 rowcodec 的线上字节约定：`CodecVer = 0x80`，Datum 编码 flag，以及 1/2/4/8 字节小端整数表示。
2. 在字节行、列 ID/offset 数组和上层 Datum 之间提供公共转换。`row.rs` 使用四个 slice 转换函数，`encoder.rs`/`decoder.rs` 使用整数编解码函数。
3. 为列级 checksum 提供确定性输入：`ColData::Encode` 按 MySQL 字段类型规范化 Datum，`RowData::Checksum` 按列顺序增量计算 IEEE CRC32。
4. 提供轻量识别函数 `IsRowKey`、`IsNewFormat` 和 `FieldTypeFromModelColumn`。
5. 仅在 next-gen 且测试态或 standalone 场景中由 `RemoveKeyspacePrefix` 剥离四字节 API V2 txn-mode 前缀；正常非 standalone 进程依赖 client-go 已完成该步骤。

源码同时保留了 Go sorter 的结构映射，但当前 Rust 编码器的真实排序入口是 [`Encoder::reformatCols`](encoder.rs)，并不调用 `largeNotNullSorter`、`smallNotNullSorter`、`smallNullSorter`、`largeNullSorter`。

## 主要符号

- `CodecVer`：新行格式首字节 `128`。`row::fromBytes` 验证它，`row::toBytes` 写入它，`IsNewFormat` 和 crate 内旧行转码入口用它快速判定。
- `NilFlag` 至 `VectorFloat32Flag`：旧 Datum 编码类型标记；同 crate 的字段类型映射和兼容编码测试依赖这些值。
- `errInvalidCodecVer`、`errInvalidChecksumVer`、`errInvalidChecksumTyp`：构造共享错误。前两个由 `row.rs` 的头部解析使用，后一个由 checksum Datum 编码的未知类型分支使用。
- `bytesToU32Slice`、`bytes2U16Slice`、`u16SliceToBytes`、`u32SliceToBytes`：在行头的列 ID/offset 数组与小端字节之间转换。Rust 返回新 `Vec`，不保留 Go `unsafe.Slice` 的底层内存别名。
- `encodeInt`/`encodeUint`：把值追加到已有 `Vec<u8>`，选择能无损表示值的最短 1、2、4 或 8 字节宽度；`decodeInt`/`decodeUint` 按输入长度反解。
- 四个 `*Sorter`：分别描述 small/large 布局下非 NULL 与 NULL 列 ID 的比较/交换规则。非 NULL 交换必须同时交换 `values`；NULL 列没有 value，只交换 ID。它们目前不是 Rust 编码器的实际排序实现。
- `IsRowKey`：检查长度至少 19、首字节为 `t` 且下标 10 为 `r`；不校验中间 table ID、分隔符和 handle 内容。
- `IsNewFormat`：安全读取首字节并与 `CodecVer` 比较，空输入返回 `false`。
- `FieldTypeFromModelColumn`：克隆 `ColumnInfo.FieldType`，使结果不借用原字段对象。
- `ColData<'a>`：借用一份 `ColumnInfo` 和对应 `Datum`；`Encode` 使用列的 MySQL 类型生成 checksum 字节。
- `RowData<'a>`：拥有 `ColData` 列表和可复用 `Data` 缓冲；`Len`/`Less`/`Swap` 支持按列 ID 排序，`Encode` 串联列字节，`Checksum` 对每列编码结果增量更新 CRC32。
- `appendDatumForChecksum`：checksum 规范化核心；NULL 不写字节，定长数值写 8 字节小端，字符串类和文本化时间/decimal/JSON 写四字节长度前缀，vector 使用自身序列化格式。
- `RemoveKeyspacePrefix`：依据 kernel、`intest::InTest`、`kv::StandAloneTiDB`、长度和首字节条件返回原 slice 或 `&key[4..]`。
- `crc32_update`：以给定初值执行 `crc32fast` IEEE CRC32，供 `RowData`、行序列化和 `RawChecksum` 复用。

## 执行流程

常规新行写入时，`Encoder::Encode` 收集列，`Encoder::reformatCols` 将非 NULL/NULL 分区并分别按列 ID 排序；`encodeRowCols` 再按 Datum kind 调用 `encodeInt`、`encodeUint` 等函数生成数据区。若启用 `RawChecksum`，`encoder.rs` 先序列化行和 checksum header，再以 `crc32_update(0, value_bytes)` 开始、继续混入 `Handle::Encoded()`，最后追加四字节 checksum。

读取时，`row::fromBytes` 验证 `CodecVer`，用本文件的字节/整数 slice 转换函数恢复列 ID 与 offset；`decoder.rs` 根据列类型对字段片段调用 `decodeInt` 或 `decodeUint`。这四个 decoder 分支只把长度 1、2、4 视为对应宽度，其余长度进入八字节分支，因此上游必须保证字段片段至少八字节且宽度合法。

列 checksum 流程是：调用方构造 `ColData`，按 `RowData::Less`/`Swap` 将列 ID 排序，然后调用 `Checksum`。每一列先清空 `Data`，经 `ColData::Encode`/`appendDatumForChecksum` 规范化，再以先前 checksum 为初值更新 CRC32。`Encode` 则不逐列清空，而是连续追加所有列，测试据此验证 `crc32fast::hash(RowData::Encode(...)) == RowData::Checksum(...)`。

`appendDatumForChecksum` 的关键分支包括：timestamp 仅在类型确为 timestamp 且提供非 UTC 时区时先转换到 UTC；NaN/Inf 浮点归零以匹配 TiCDC；BIT 去除前导零后按大端数值解释，超过八个有效字节饱和为 `u64::MAX`；未知非 NULL 类型返回 `errInvalidChecksumTyp`。

keyspace 路径先检查 classic kernel；next-gen 下，若既非测试态又非 standalone，直接返回原 key。只有测试态或 standalone 才继续检查长度大于 4 且首字节为 `x`，满足时返回去掉前四字节的借用 slice。

## 数据与状态

绝大多数函数是无持久状态的纯字节转换。`ColData` 只持有借用；`RowData` 拥有列列表和 `Data` 缓冲，`Encode`/`Checksum` 会改变该缓冲。`RowData::Checksum` 要求调用方预先按列 ID 排序，否则 CRC32 会随输入列顺序变化；函数本身不自动排序。

行布局有 small/large 两种列 ID 和 offset 宽度。四个转换函数一律按本机明确写出的 little-endian 规则复制数据，尾部不足完整 2/4 字节的片段会被 `chunks_exact` 忽略；真实调用点由 `row.rs` 根据头部计数计算完整长度。

`RemoveKeyspacePrefix` 读取两个进程级原子开关：`intest::InTest` 使用 `SeqCst`，`kv::StandAloneTiDB` 使用 `Relaxed`。返回值借用输入，不分配、不改写 key。`FieldTypeFromModelColumn` 和所有字节/slice 转换则会克隆或分配。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：`crc32fast` 实现 IEEE CRC32，`chrono-tz` 提供 `time::Location`；本地依赖提供 Datum/FieldType、ColumnInfo、KV Handle、kernel 类型、测试态和基础 codec。`nextgen` feature 同时传递给 `kerneltype` 与 `kv`，默认 feature 为空，即 classic 行为。

已核对的主要上游边包括：

- [`encoder.rs`](encoder.rs) → `encodeInt`/`encodeUint`/`crc32_update`；
- [`decoder.rs`](decoder.rs) → `decodeInt`/`decodeUint`；
- [`row.rs`](row.rs) → `CodecVer`、版本错误、四个 slice 转换和 `crc32_update`；
- [`lib.rs`](lib.rs) 的 `encode_from_old_row` → `CodecVer`；
- `pkg/tablecodec/tablecodec.rs` → `CodecVer`、`RemoveKeyspacePrefix`、`IsNewFormat`。

RustCodeGraph 报告 `common.rs` 被 23 个索引文件关联；精确符号查询确认 Rust/Go 两套 `RemoveKeyspacePrefix`、`appendDatumForChecksum`、`IsNewFormat`、`FieldTypeFromModelColumn` 定义。由于图对 include 后的同模块私有调用没有输出 callers，以上直接边进一步由精确 `rg` 交叉核对；没有据此扩张到间接业务调用者。

## 错误处理与边界

`ColData::Encode`、`RowData::Encode`、`RowData::Checksum` 返回 `SharedError`，首个列错误立即中止。checksum 编码在执行类型 getter 前显式检查 Datum kind；不匹配返回带 kind/type 数值的错误，未知 MySQL 类型返回“invalid type for checksum”。时区转换错误原样转成共享错误。

需要调用方保证的边界包括：`decodeInt`/`decodeUint` 只适用于 1、2、4、8 字节，其他长度会尝试读取前八字节，短于八字节会 panic；`RowData` checksum 前必须排序；`IsRowKey` 只是结构前缀判断而非完整 key 解码；`RemoveKeyspacePrefix` 只凭 `x`、长度和运行模式判断，不解析 keyspace ID。

与 Go 相比，Rust `IsNewFormat(&[])` 返回 `false`，而 Go 直接索引空 slice 会 panic。Go 的 `appendDatumForChecksum` 用 `defer/recover` 包装类型错配 panic；Rust 改为预检 Datum kind，但源码注释仍将完整 panic 捕获/格式化列为未等价接线。`RowData` 为保持所有权简单会克隆 `Data`，正确性由测试覆盖，但不等价于 Go slice 原地复用的分配特征。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务。checksum hasher 是函数内局部值；`Vec` 和借用 slice 遵循普通 Rust 生命周期。`ColData<'a>` 和 `RemoveKeyspacePrefix` 的返回借用不会超过其输入。

唯一共享状态是 keyspace 判定读取的两个原子布尔值。生产函数只读它们；[`common_test.rs`](common_test.rs) 会修改并通过 `Drop` 恢复。测试若并行修改同一全局开关仍可能互相影响，因此新增相关测试应继续集中管理状态、恢复原值，并避免与其他修改这些开关的测试并发假设冲突。

## 与 Go 版本的对应关系

直接对照文件为 [`common.go`](common.go)。常量、整数宽度、排序不变量、row key 判定、checksum 类型分支和 keyspace 条件总体保持同一协议。Rust 特意保留了 Go 风格符号名，便于逐项对照。

已经确认的实现差异：

- Go 的四个 slice 转换通过 `unsafe.Slice` 与原内存共享；Rust 按小端复制成新 `Vec`，安全但增加分配并失去别名写回语义。
- Go encoder 通过四个 sorter 和 `sort.Sort` 重排；Rust 当前由 `Encoder::reformatCols` 构造临时 `(col_id, datum)`/NULL ID 集合后排序，四个 sorter 仅保留对照形状。
- Go `IsNewFormat` 假设非空；Rust 对空输入返回 `false`，这已由 `rowcodec_test.rs::TestCodecUtil` 覆盖。
- Go 以 panic/recover 处理 Datum/type 错配；Rust 先检查 kind 并返回错误。错误文字与 Go 的 Datum/类型名称格式并不完全相同。
- Go slice append 原地复用容量；Rust `RowData` 每列传递 `self.Data.clone()`，语义相同但有额外复制成本。
- BIT 的 Go `ToInt` 忽略截断错误并使用返回值；Rust 手动实现去前导零和超长饱和，目标是复现该数值结果。

对应测试包括 [`common_test.go`](common_test.go) / [`common_test.rs`](common_test.rs) 的 keyspace 分支，以及 `rowcodec_test.go` / `rowcodec_test.rs` 的 Datum checksum、NULL、时区、排序与整行 checksum 用例。`common_1_aster_unit_test.rs` 另覆盖整数宽度、small/large 排序布局、raw handle checksum、行键和新格式判断。

## 扩展指南

新增 MySQL/Datum 类型的 checksum 支持时，应同时更新 `appendDatumForChecksum` 的 kind 合法性表和编码分支，并在独立的 `rowcodec_test.rs` 增加正常值、NULL、类型错配和边界表示测试；还要与 `common.go` 及 TiCDC 的规范化行为核对，避免跨组件 checksum 漂移。

修改整数宽度、flag 或 `CodecVer` 会改变持久化行协议，必须同步检查 `row.rs`、`encoder.rs`、`decoder.rs`、tablecodec 以及 Go 实现，不能只改单个函数。优化 slice 转换或 `RowData` 缓冲时，应分别证明字节序、尾部长度、所有权/别名和分配行为，并把测试放在独立 `*_test.rs` 文件，不能嵌入生产源文件。

调整 keyspace 逻辑应扩充 `common_test.rs::TestRemoveKeyspacePrefix` 的 classic/nextgen、测试态、standalone、短 key、非 `x` key 矩阵，并保持全局开关恢复。若希望重新启用四个 sorter，应先确认它们与当前 `Encoder` 字段布局和 borrow 规则一致，并避免同时维护两条排序路径。

性能风险集中在 `Vec` 转换和 `RowData` 的逐列 clone；兼容风险集中在 checksum 字节、时间标准化、NaN/Inf/BIT 特例和线上 row header。任何改动都应先以 Go 测试夹具或跨语言固定字节作为证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/rowcodec` 找到目标及 Rust/Go 对照；`node --file pkg/util/rowcodec/common.rs` 读取 582 行完整实现；精确查询/节点读取核对 `RemoveKeyspacePrefix`、`appendDatumForChecksum`、`IsNewFormat`、`FieldTypeFromModelColumn` 和相邻实现。
- 源码与边界：[`common.rs`](common.rs)、[`lib.rs`](lib.rs)、[`row.rs`](row.rs)、[`encoder.rs`](encoder.rs)、[`decoder.rs`](decoder.rs)、[`Cargo.toml`](Cargo.toml) 及 `pkg/tablecodec/tablecodec.rs`。
- Go 对照：[`common.go`](common.go)、[`common_test.go`](common_test.go)、`rowcodec_test.go`。
- Rust 测试：[`common_test.rs`](common_test.rs)、[`common_1_aster_unit_test.rs`](common_1_aster_unit_test.rs)、`rowcodec_test.rs`；重点证据为整数最短宽度、空输入格式判断、checksum Datum 类型矩阵、列排序后 `hash(Encode) == Checksum`、keyspace 模式矩阵和 raw-handle checksum。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行固定 11 章节结构检查，并人工核对本文区分了当前真实路径、Go 对照结构和尚未等价的实现差异。
