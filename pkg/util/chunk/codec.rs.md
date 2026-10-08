# `pkg/util/chunk/codec.rs`

## 文件定位

本文件属于 `astersql-util-chunk` crate（`pkg/util/chunk/Cargo.toml`），由 `pkg/util/chunk/internal/group1/lib.rs:473` 通过 `include!("../../codec.rs")` 注入内部聚合模块，再由 `pkg/util/chunk/lib.rs:110-113` 公开重导出。它处在 Chunk 列式内存表示与字节传输格式之间：`Codec` 负责完整 Chunk/Column 的序列化与反序列化，`Decoder` 负责把一次解出的中间 Chunk 分批搬入调用方 Chunk，`GetFixedLen`、`EstimateTypeWidth` 则把同一套物理类型宽度知识提供给规划和执行层。

从当前 Rust 调用证据看，宽度接口已经进入生产链：`pkg/executor/utils.rs:103` 用 `EstimateTypeWidth` 估算行宽，`pkg/planner/cardinality/row_size.rs:125,135,191,213`、`pkg/planner/core/plan_cost_ver2.rs:62,119,131,223` 和 `pkg/planner/core/operator/physicalop/physical_sort.rs:166` 用它们估算基数、成本或排序内存。对 `NewCodec`、`NewDecoder` 的 Rust 文本引用目前位于本文件及 `codec_test.rs`、`codec_2_aster_unit_test.rs`；因此 Chunk 字节格式和增量解码逻辑已经实现并被独立测试覆盖，但未从搜索结果确认到这些构造器的生产调用者。

## 核心职责

1. `Codec::Encode` / `encodeColumn` 把 Chunk 按列依次编码为 `length`、`nullCount`、可选 `nullBitmap`、可选 `offsets`、`data`（`codec.rs:36-66`）。列数和类型不写入流中，解码端必须从外部持有一致的 `colTypes`。
2. `Codec::Decode` 创建新 Chunk 并解到输入耗尽；`DecodeToChunk` 按目标 Chunk 现有列数解码；二者最终都走 `decodeColumn`（`codec.rs:68-139`）。
3. `getFixedLen` / `GetFixedLen` 给出 Chunk 物理存储宽度；`EstimateTypeWidth` 在此基础上为未知或变长类型给出规划用平均宽度估计（`codec.rs:182-231`）。
4. `Decoder` 保存可复用的 `intermChk` 和剩余行数，支持 `Reset` 后分批 `Decode`，也支持 `ReuseIntermChk` 直接交换列，减少复制（`codec.rs:234-352`）。

## 主要符号

- `Codec { colTypes: Vec<types::FieldType> }`：编解码器；`colTypes` 只在解码和固定/变长判断时使用。`NewCodec` 把类型向量装入 `Box<Codec>`（`codec.rs:23-34`）。
- `Codec::Encode(&Chunk) -> Vec<u8>`：以 `Chunk::MemoryUsage` 预分配容量，按 `chk.columns` 顺序追加列编码（`codec.rs:36-45`）。
- `Codec::encodeColumn(Vec<u8>, &Column) -> Vec<u8>`：写入单列帧。`length` 与 `nullCount` 明确使用小端 `u32`；仅存在 NULL 时写 bitmap；仅变长列写 `(length + 1)` 个 `i64` offset；最后写 data（`codec.rs:47-66`）。
- `Codec::Decode(&[u8]) -> (Box<Chunk>, &[u8])`：循环到输入为空，每次按递增 ordinal 解一列（`codec.rs:68-87`）。
- `Codec::DecodeToChunk(&[u8], &mut Chunk) -> &[u8]`：严格按目标的列数覆盖各列，并返回未消费尾部（`codec.rs:89-96`）。
- `Codec::decodeColumn`：读取头部、重建 bitmap/offset/data，并设置 `avoidReusing = true`（`codec.rs:98-139`）。
- `setAllNotNull` 与 `allNotNullBitmap`：当编码流的 `nullCount == 0`、因而没有 bitmap 字节时，用 128 字节的 `0xff` 模板分段重建全非空 bitmap（`codec.rs:141-152,179-180`）。
- `i64SliceToBytes` / `bytesToI64Slice`：以本机端序在 offset 切片和字节之间复制转换（`codec.rs:155-177`）。
- `VarElemLen = usize::MAX`：Rust 版的变长哨兵；对应 Go 的 `-1`（`codec.rs:182-184`）。
- `getFixedLen` / `GetFixedLen`：Float 为 4 字节；整数族、Double、Year、Duration 为 8；日期时间为 `sizeTime`；Decimal 为 `MyDecimalStructSize`；其余视为变长（`codec.rs:186-208`）。
- `EstimateTypeWidth`：固定长直接返回物理宽度；变长且 `flen <= 32` 返回声明长度，`32 < flen < 1000` 返回 `32 + (flen-32)/2`，更大时封顶为 516，未知长度默认 32（`codec.rs:210-231`）。
- `Decoder { intermChk, codec, remainedRows }` 与 `NewDecoder`：持有中间 Chunk、类型驱动的 Codec 和尚未搬出的行数（`codec.rs:234-250`）。
- `Decoder::{Reset, Decode, ReuseIntermChk}`：分别载入一批编码数据、增量搬运和零追加交换；`IsFinished` / `RemainedRows` 暴露消费状态（`codec.rs:252-302`）。
- `Decoder::decodeColumn`：处理变长 offset 平移、非字节对齐 bitmap 拼接、末字节冗余位清零，以及源/目标 data 的裁剪和追加（`codec.rs:304-351`）。

## 执行流程

编码流程从 `Codec::Encode` 开始。每列先写两个小端 `u32`；如果有 NULL，再写恰好 `ceil(length/8)` 字节 bitmap；变长列随后写 `length + 1` 个 offset；最后写完整 data。固定列的数据长度不显式写入，而由解码端的字段类型和行数相乘得到；变长列的数据长度取最后一个 offset（`codec.rs:39-65,121-135`）。

完整解码时，`Decode` 从 ordinal 0 开始反复调用 `decodeColumn`，直到 buffer 为空。定列解码时，`DecodeToChunk` 只迭代目标 Chunk 的列，因此可留下未消费尾部。`decodeColumn` 依次读取长度、NULL 数和可选 bitmap；依据 `colTypes[ordinal]` 决定固定或变长路径，重建 offsets/data，并把 `avoidReusing` 置为真（`codec.rs:70-95,100-139`）。

增量流程为 `NewDecoder` → `Reset` → 一次或多次 `Decode`。`Reset` 将整个输入解入 `intermChk` 并记录行数。`Decode` 计算目标仍需的行数，将其向上取整到 8 的倍数后再以 `remainedRows` 封顶；每列移动相同行数，最后扣减剩余行。字节对齐能使常见路径直接追加 bitmap；未对齐时则跨字节移位合并（`codec.rs:244-269,305-350`）。如果上层判定适合整体复用，可调用 `ReuseIntermChk`：先修正变长列首 offset 为 0，再 `SwapColumns` 并清零 `remainedRows`（`codec.rs:284-302`）。

## 数据与状态

编码格式本身不含列数、类型或校验信息，其解释依赖调用者提供的 `colTypes` 及列顺序。单列的逻辑布局为：`u32 length`、`u32 nullCount`、可选 bitmap、可选 `i64[length+1]` offsets、data。长度头使用小端；offset 辅助函数使用本机端序，因此该实现保持与同机 Go `unsafe` 布局一致，但不能据此声称 offset 字段具备跨端序可移植性（`codec.rs:49-64,155-177`；`codec.go:49-81,155-160`）。

`Column.length`、`nullBitmap`、`offsets`、`data` 和 `elemBuf` 是解码写入的主要状态。固定列通过类型宽度计算 data 长度；变长列依靠最后一个 offset。`nullCount` 只决定流中是否携带 bitmap，实际 NULL 状态由 bitmap 表示。`Decoder.remainedRows` 是消费游标的行数表示；源列则通过裁掉 offsets、bitmap 和 data 前缀同步前移（`codec.rs:107-138,262-269,304-350`）。

`ReuseIntermChk` 转移的是列集合而非逐行数据。调用后目标获得中间列，原中间 Chunk 接收目标旧列，且 decoder 被标记为已消费完；再次处理新数据前应 `Reset`（`codec.rs:286-301`）。

## 依赖与调用关系

crate 边界由 `pkg/util/chunk/Cargo.toml` 定义，crate 名为 `astersql-util-chunk`，入口为 `lib.rs`。本文件直接使用聚合模块作用域中的 `types::FieldType`、`types::MyDecimalStructSize`、`mysql::Type*`，以及同组实现提供的 `Chunk`、`Column`、`sizeTime`；这些类型和常量由 `internal/group1/lib.rs` 的导入及 `include!` 组合到同一作用域。

下游调用关系为：`Codec::Encode → encodeColumn → Column::{MemoryUsage,nullCount,IsFixed}`；`Codec::{Decode,DecodeToChunk} → decodeColumn → {getFixedLen,setAllNotNull,bytesToI64Slice}`；`Decoder::Reset → Codec::DecodeToChunk`；`Decoder::Decode → Decoder::decodeColumn → Column::appendMultiSameNullBitmap`；`ReuseIntermChk → getFixedLen + Chunk::SwapColumns`。这些关系可直接由 `codec.rs:39-65,70-152,255-350` 复核。

上游生产调用目前由文本搜索确认到宽度接口：执行器行宽估算、planner cardinality、plan cost v2 与 physical sort。RustCodeGraph 将 `codec.rs` 标为被 15 个索引文件使用，但对 `NewCodec`、`NewDecoder` 的精确 callers 查询未返回边；仓库 Rust 文本搜索也只找到测试中的构造器引用。因此文档不把 Go 版 `Codec` 在 distsql/coprocessor 链中的使用直接推断为 Rust 已接线事实。

## 错误处理与边界

所有接口都不返回 `Result`，并假定输入可信且元数据匹配。短于 8 字节的列头、缺失 bitmap/offset/data、`ordinal >= colTypes.len()`、负数或越界的变长 offset、目标 Chunk 列数/类型不匹配，都会通过切片、索引、`try_into().unwrap()` 或算术路径 panic；格式没有 magic、版本号、列数和校验和（`codec.rs:100-138`）。调用者必须在进入该层前保证数据完整、类型顺序一致。

`Decode` 的停止条件是 buffer 为空，因此尾部垃圾会被当作下一列解析；`DecodeToChunk` 则允许返回尾部，调用者应检查是否符合协议预期。`Decoder::Decode` 假定 `RequiredRows() >= NumRows()`，并假定源列至少含 `requiredRows` 行；零行场景还会进入 bitmap 末元素计算，因此调用方不应在无需追加行时调用它（`codec.rs:255-263,324-345`）。

固定长 NULL 槽的 data 仍可能保留上一元素字节，NULL 可见性只由 bitmap 决定；`codec_2_aster_unit_test.rs:76-81` 明确锁定了这一 Go 兼容语义。新增输入校验时需要同时评估 Go 格式兼容和热路径开销，不能只把 panic 改成静默截断。

## 并发与资源生命周期

本文件不创建线程、锁、通道、异步任务或 I/O 资源。`Codec::Encode` 只借用输入并返回自有 `Vec<u8>`；Rust `decodeColumn` 会把 bitmap、offset 和 data 复制到新的 `Vec`，因此数据所有权与输入切片分离。它仍设置 `avoidReusing = true`，是对 Go 版“列引用 gRPC 响应底层内存，复用会延长响应生命周期”约束的保守兼容标记（`codec.rs:113-138`；`codec.go:114-140`）。

`Decoder` 的变更操作需要 `&mut self`，Rust 借用规则阻止同一个 decoder 被无同步地并发修改；类型本身未声明额外线程安全承诺。中间 Chunk 在 decoder 生命周期内持续复用，`Decode` 会破坏性裁剪源列，`ReuseIntermChk` 会交换所有权。调用方不能把这些操作当作只读，也不能假定 `Reset` 前一批数据仍可从 `intermChk` 访问。

## 与 Go 版本的对应关系

Rust 文件逐段移植自 `pkg/util/chunk/codec.go`：列帧字段顺序、固定宽类型表、宽度估算公式、Decoder 的 8 行对齐、变长 offset 平移、bitmap 移位拼接及列交换语义均保持一致。Rust 使用 `usize::MAX` 表示 Go 的 `VarElemLen = -1`；分支仅做相等判断，所以在本文件内语义等价（`codec.rs:182-208`；`codec.go:162-185`）。

关键实现差异是内存安全策略。Go 用 `unsafe.Slice` 零拷贝重解释 offsets，并让解码列切片直接引用输入；Rust 用 `to_vec` 和逐项本机端序转换建立自有存储，因此通常增加分配/复制，但避免输入切片悬垂。Rust 的 `allNotNullBitmap` 在静态定义处直接初始化为 `0xff`，对应 Go 的零值数组加 `init()` 填充（`codec.rs:155-180`；`codec.go:143-160,218-222`）。

测试对应关系如下：`codec_test.rs:35-100` 对齐 Go `TestCodec` 和 `TestEstimateTypeWidth` 的混合固定/变长/NULL 往返与宽度边界；`codec_2_aster_unit_test.rs:49-131` 增补完整 `Decode` 重编码一致性和 Decoder 跨 bitmap 边界的分批消费；Go 基准位于 `codec_test.go:98-191`，当前 Rust 独立测试中没有对应性能基准。因此功能语义有证据，Rust 相对 Go 的分配成本没有本任务内的基准证据。

## 扩展指南

新增固定宽类型时，应同步修改 `getFixedLen`，确认对应 `Column` 读写布局和 `sizeTime`/decimal 尺寸不变量，并扩展 `codec_test.rs` 或 `codec_2_aster_unit_test.rs` 的往返与 `GetFixedLen` 断言；若该类型影响 planner 估算，还要检查 `pkg/planner/cardinality/row_size.rs` 和 `pkg/planner/core/plan_cost_ver2.rs` 的预期。

修改线格式时，应同时更新 `encodeColumn` 与 `decodeColumn`，并核对 Go `pkg/util/chunk/codec.go`，因为当前格式没有版本协商。至少覆盖：全非空与含 NULL、固定与变长、空列、多个列、尾部处理、截断/畸形数据策略，以及编码后由 Go/Rust 双向读取的兼容性。测试逻辑应继续放在独立的 `codec_test.rs` 或 `codec_2_aster_unit_test.rs`，不要内嵌到生产文件。

优化 Decoder 时，主要接入点是 `Reset`、`Decode`、`decodeColumn` 和 `ReuseIntermChk`。必须维持三项不变量：各列搬运相同行数；变长 offset 在目标中连续且源首 offset 可归零；bitmap 末字节无超出有效行数的置位。若要减少 Rust 当前的 `to_vec` 前缀裁剪成本，可考虑保存游标而非反复复制，但需同步调整交换路径和所有相关测试，不能改变 Go 可观察语义。

若要为不可信输入增加错误返回，应设计新的 fallible API 或兼容包装层，明确旧 panic API 的迁移策略；不要悄悄吞掉格式错误。若要声明跨架构线格式，则还必须把 offsets 从本机端序改成明确端序，并提供 Go/Rust 兼容迁移与版本识别方案。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/chunk` 确认目标、Go 对照和独立测试均在索引内。
- RustCodeGraph `node --file pkg/util/chunk/codec.rs`：读取完整 352 行生产实现；`query NewCodec/NewDecoder/GetFixedLen/EstimateTypeWidth` 确认 Rust/Go 同名符号及路径；精确 `callers` 未产出边，因此未据此扩张生产调用结论。
- RustCodeGraph 读取：`pkg/util/chunk/codec.go`、`pkg/util/chunk/codec_test.rs`、`pkg/util/chunk/codec_2_aster_unit_test.rs`、`pkg/util/chunk/codec_test.go`、`pkg/util/chunk/lib.rs`。
- 配置与装配读取：`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/internal/group1/lib.rs:473`；文本调用核验覆盖 `pkg/executor/utils.rs`、`pkg/planner/cardinality/row_size.rs`、`pkg/planner/core/plan_cost_ver2.rs`、`pkg/planner/core/operator/physicalop/physical_sort.rs`。
- 行为测试证据：混合列 Encode/DecodeToChunk 往返和宽度分段见 `codec_test.rs:35-100`；完整 Decode、NULL 槽、重编码一致性、Decoder 分批追加及 bitmap 边界见 `codec_2_aster_unit_test.rs:49-131`；Go 原始语义及基准见 `codec_test.go:26-191`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前另执行固定 11 章节结构检查，并人工核对本文件只陈述上述源码、图查询、Cargo、Go 与测试能够支持的事实。
