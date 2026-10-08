# `pkg/util/chunk/column.rs`

## 文件定位

[对应 Rust 源文件](./column.rs)实现 `astersql-util-chunk` crate 的单列内存容器 `Column`。它不是独立模块入口：`pkg/util/chunk/internal/group1/lib.rs` 的 `column_impl` 通过 `include!("../../column.rs")` 纳入实现，再由 `pkg/util/chunk/lib.rs` 重导出。上层 `Chunk` 把多个 `Column` 组成批式行集，`Row` 则以行号加列号调用这里的类型读取器，因此它位于 SQL 向量化执行、表达式计算和 Chunk 编解码所共用的内存表示底层。

crate 边界由 `pkg/util/chunk/Cargo.toml` 定义，包名为 `astersql-util-chunk`。本文件直接依赖 crate 内暴露的 `types` 类型族及 `getFixedLen`、`VarElemLen` 等同组符号；随机破坏测试数据的 `DestroyDataForTest` 使用 Cargo 中重命名为 `rand_crate` 的 `rand` 依赖（由 crate 根再次导出）。

## 核心职责

- 用 `Column::{length,nullBitmap,offsets,data,elemBuf}` 表示一列 Arrow 风格数据。定长列以 `elemBuf` 的非空长度标识元素宽度；变长列令 `elemBuf` 为空，并以长度为 `length + 1` 的 `offsets` 划分 `data`。
- 提供按 `FieldType` 建列、按容量预分配和按 `EvalType` 重置布局的入口：`NewEmptyColumn`、`NewColumn`、`newFixedLenColumn`、`newVarLenColumn`、`Reset`。
- 维护 null bitmap。每行一位，0 表示 NULL、1 表示非 NULL；追加、批量置位、计数、列间合并都以该约定为唯一判定依据。
- 为整数、浮点、时间、Duration、Decimal、字符串、字节、JSON、向量、Enum 和 Set 提供追加及读取 API，并维持物理字节布局与 Go 实现兼容。
- 支持列复制、selection 物化、重复追加和容量复用，以便表达式执行和 Chunk 过滤路径避免逐 Datum 重建。

## 主要符号

- `Column`：核心容器。`length` 是逻辑行数；`nullBitmap` 保存有效位；`data` 保存连续负载；`offsets` 只服务变长列；`elemBuf` 同时保存定长追加的暂存值并充当定长/变长布局判别；`avoidReusing` 供分配器策略使用。
- `reference_id`、`new_column_reference_id`、`same_ref`、`reference_clone`：Rust 为模拟 Go `*Column` 指针身份增加的机制。普通 `Clone` 分配新 ID，`reference_clone` 保留 ID，供 `Chunk::SetCol` 等逻辑识别“同一列引用”。原子计数仅保证 ID 分配不发生数据竞争，不代表列内容可并发修改。
- `ColumnAllocator` 与 `DefaultColumnAllocator`：按字段类型和容量创建列的抽象及默认实现，最终调用 `newColumn(getFixedLen(ft), capacity)`。
- `estimatedElemLen` 与 `getInit*Cap`：变长元素按 8 字节估算初始 `data` 容量；bitmap 以 `(capacity + 7) >> 3` 字节预留，offset 以 `capacity + 1` 预留。
- `Append*`、`finishAppendFixed`、`finishAppendVar`：类型追加入口及两种布局的统一收尾。定长值先写 `elemBuf` 再复制到 `data`；变长值直接扩展 `data` 后压入新尾 offset；两者都追加非 NULL 位并递增 `length`。
- `AppendNull`、`AppendNNulls`、`AppendCellNTimes`：分别追加一个 NULL、多个 NULL、或重复源列某单元格。固定长 NULL 仍占一个元素宽度，变长 NULL 则重复当前尾 offset。
- `Resize*`/`resize` 与 `Reserve*`/`reserve`：前者建立含 `n` 个定长元素的物理区并统一设置 null 状态；后者清空变长列并按数量和估计宽度复用或重建容量。
- `Get*`、`Int64s`、`Float64s`、`GetRaw`：从本机字节序固定槽或 offset 区间还原类型。批量访问器在 Rust 中返回新 `Vec`，而非 Go 的原地别名切片。
- `CopyConstruct`、`reconstruct`、`CopyReconstruct`：深拷贝或按 selection 重排。`reconstruct` 原地压缩，`CopyReconstruct` 可复用目标列，并在无 selection 或升序全集时走完整复制快路。
- `MergeNulls`：对 bitmap 按位与，使任一输入为 NULL 时结果为 NULL；要求结果为定长列且所有列行数一致。
- `ContainsVeryLargeElement`：只检查变长列；先以总尾 offset 快速排除，再查是否存在单个长度大于 `u32::MAX` 的元素。
- `decimal_to_bytes`、`decimal_from_bytes`、`time_from_bytes` 及 `read_*`：本机字节序的内部序列化辅助函数；只为本进程内列布局服务，不是跨平台持久化协议。

## 执行流程

1. 建列时，`NewColumn` 先通过 `getFixedLen` 判断类型。定长路径分配指定宽度的 `elemBuf`，变长路径建立首个 offset 0；两者只预留容量，不产生逻辑行。
2. 追加非 NULL 定长值时，类型方法把值编码进 `elemBuf`，`finishAppendFixed` 将完整槽复制进 `data`、写入 bitmap 的 1 位，再增加 `length`。追加变长值时先扩展 `data`，`finishAppendVar` 写 1 位并把新的 `data.len()` 压入 `offsets`。
3. 追加 NULL 时 bitmap 保持 0。为保持随机定位，定长列仍复制一个 `elemBuf` 槽；变长列追加与前一项相同的 offset，形成零长度区间。读取者必须先看 `IsNull`，不能仅凭负载字节判断 NULL。
4. `Chunk::Reconstruct` 取得自身 selection 后逐列调用 `Column::reconstruct`。定长列用 `copy_within` 移动被选槽；变长列把被选区间紧凑复制到 `data` 头部并重建 offsets；最后截断 data、offsets、bitmap，更新 `length`。
5. 表达式列求值路径（`pkg/expression/column.rs`）以 `CopyReconstruct(input.Sel(), None)` 物化选中行。`MergeNulls` 在本文件的独立 Rust 测试中验证按位与语义；仓库里还能搜到表达式运行时的同名方法，但那是另一套 `pkg/expression/legacy_vectorized_runtime.rs::Column`，不能当作本方法的调用者。
6. `Row::GetInt64` 等方法先从所属 `Chunk` 找到列，再把当前行号传给本文件的 `Column::Get*`，因此这里的边界检查、字节序和 offsets 不变量直接决定所有行级访问行为。

## 数据与状态

有效状态必须同时满足以下不变量：`length` 等于逻辑行数；bitmap 至少覆盖 `length` 位且尾部冗余位为 0；定长列 `data.len() == length * elemBuf.len()`；变长列 `offsets.len() == length + 1`、`offsets[0] == 0`，并且 offsets 单调不减且最后一项等于 `data.len()`。`reset` 保留现有布局和容量，而公开 `Reset(EvalType)` 会按目标求值类型重建定长或变长布局。

`SetNull`/`SetNulls` 只改变 bitmap，不清理槽内负载；因此 NULL 槽可能保留旧值。`nullCount` 仅统计 `length` 范围内的 0 位。`SetRaw` 要求替换字节与原变长区间等长，因为它调用 `copy_from_slice` 且不会调整后续 offsets。

`CopyConstruct` 对 bitmap、offsets、data 和 elemBuf 全部深拷贝。`avoidReusing` 在从现有目标复制时没有被重写，且 `CopyConstruct(None)` 通过自定义 `Clone` 获得新 `reference_id`；调用者不应把内容相等误认为引用相同。

## 依赖与调用关系

上游装配链是 `pkg/util/chunk/lib.rs` → `pkg/util/chunk/internal/group1/lib.rs::column_impl` → 本文件。直接核心调用者包括：`pkg/util/chunk/chunk.rs::Reconstruct` 调用 `reconstruct`；`pkg/util/chunk/row.rs::Get*` 调用各类型 getter；`pkg/util/chunk/chunk.rs` 的列级追加方法调用 `Append*`；`pkg/util/chunk/codec.rs` 读取/重建 bitmap、offset 和 data；`pkg/expression/column.rs` 调用 `CopyReconstruct`。本 `MergeNulls` 的已核验证据来自 `pkg/util/chunk/column_test.rs`，不把表达式运行时另一 `Column` 的同名方法混入调用链。

下游依赖主要是同一 crate 中的 `types::{FieldType,EvalType,Duration,MyDecimal,BinaryJSON,VectorFloat32,Enum,Set,Time}`，以及 `getFixedLen`/`VarElemLen` 类型宽度约定。`AppendVectorFloat32`/`GetVectorFloat32` 委托 `types` 序列化；Decimal 和 Time 则由本文件显式转换固定布局。

RustCodeGraph 的文件查询显示 `column.rs` 被 76 个文件使用；由于 `Column`、`Reset` 等名字在仓库中高度重载，宽泛 `explore` 会混入无关符号，调用关系结论采用精确文件节点，并由上述直接源码入口交叉核验。

## 错误处理与边界

本文件没有 `Result` 型 API，多数前置条件依赖 Rust 索引和断言失败：越界行号、过短 data、错误 offsets、错误 Decimal/Time 字节宽度会 panic；`GetVectorFloat32` 反序列化失败也显式 panic。`Reset` 遇到未支持的 `EvalType`、`MergeNulls` 用于变长结果或行数不一致时同样 panic。

`GetString` 使用 `String::from_utf8_lossy`，非法 UTF-8 会被替换字符容错；Enum/Set 名称同样按 lossy UTF-8 还原。`AppendCellNTimes` 假设源和目标布局类型兼容，未运行时核验元素宽度。`SetRaw` 不能改变单元格长度。容量计算是 `usize` 乘法后转 `i64`，极端容量可能在 debug 构建触发溢出或在转换后失真，正常 Chunk 容量应由上层约束。

`ContainsVeryLargeElement` 用 64 位 offset 支持总数据超过 4 GiB，并只在某一个元素自身超过 `u32::MAX` 时返回 true；Go 的 `TestLargeStringColumnOffset` 验证了 offset 不应缩窄为 32 位。

## 并发与资源生命周期

`Column` 拥有自己的 `Vec` 缓冲，没有锁、通道、异步任务或外部句柄；修改方法均需 `&mut self`，常规共享由 Rust 借用规则约束。全局 `NEXT_COLUMN_REFERENCE_ID` 使用 `AtomicUsize::fetch_add(Ordering::Relaxed)`，目的仅是生成进程内身份标识，不承载列内容同步或 happens-before 语义；计数回绕未被特殊处理。

`reset`、`resize`、`reserve` 和带目标的 `CopyConstruct` 会尽量复用已经分配的 Vec 容量；`CopyReconstruct` 也可接收旧目标列。借用型读取 `GetBytes`/`GetRaw` 的生命周期绑定到列，后续可变操作会使这些借用失效；其他类型 getter 多返回拥有所有权的值。`DestroyDataForTest` 仅破坏 data 内容以验证深拷贝，不能用于生产生命周期管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/chunk/column.go`，Rust 保留了主要名称、分支和布局：8 字节变长容量估值、0/1 bitmap 语义、固定/变长构造、NULL 占位、selection 重建、NULL 合并、64 位 offsets 以及类型追加/读取均逐项对应。`pkg/util/chunk/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/util/chunk"` 也明确记录了移植来源。

Rust 用安全切片和 `to_ne_bytes`/`from_ne_bytes` 替代 Go 的 `unsafe.Pointer` 视图；因此 `Int64s`、`Float64s` 等返回复制出的 `Vec`，修改返回值不会像 Go 切片那样回写列。Rust 还增加 `reference_id` 来表达 Go 指针相等，并以 `Box<Column>`/`Option<Box<Column>>` 表达可选目标。Decimal 在 Rust 中显式编码字段，Time 当前只编码 `coreTime.0`；这些布局必须继续与 `types` 的尺寸常量一致。

独立 Rust 测试 `pkg/util/chunk/column_test.rs` 聚焦 bitmap、resize/reserve、深拷贝、固定/变长重建、类型往返、NULL 合并和 Reset；`pkg/util/chunk/codec_2_aster_unit_test.rs` 额外覆盖 `AppendCellNTimes` 与编码往返。Go 的 `pkg/util/chunk/column_test.go` 覆盖面更宽，包括大 offset、各类型访问、原地 reconstruct、批量 NULL、预分配和 raw 访问，应作为继续移植边界用例的基准。

## 扩展指南

新增类型时，先确定它是固定长还是变长，并同步 `getFixedLen`/`VarElemLen` 约定、`Reset(EvalType)` 分支、对应 `Append*`/`Get*`、Chunk/Row 转发和 codec；固定布局还应增加尺寸常量及明确的字节转换，避免依赖结构体填充。新增变长格式必须保证首 offset 为 0、每次追加恰好新增一个尾 offset，NULL 追加不得增加 data。

修改 bitmap 算法时必须覆盖非 8 倍数长度、跨字节追加、尾部冗余位清零、NULL 与非 NULL 批量区间。修改重建逻辑时应同时覆盖定长/变长、NULL 行、非升序和重复 selection、复用目标后继续追加。相应 Rust 测试应放在独立的 `pkg/util/chunk/column_test.rs`（codec 交互放 `codec_2_aster_unit_test.rs`），不要内嵌进生产文件；同时对照 `column_test.go` 保持 Go 的行为边界。

兼容性风险主要在本机字节序、Decimal/Time 物理布局、NULL 槽残留负载、`SetRaw` 等长约束和 Rust 批量 getter 的复制语义。性能风险主要在频繁重分配、无谓复制以及 selection 压缩时的重叠移动；扩展前应优先复用现有收尾、reserve 和 reconstruct 路径，而不是建立第二套状态更新逻辑。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/util/chunk` 确认 Rust/Go 源和独立测试；`node --file pkg/util/chunk/column.rs --offset ...` 分段读取全部 1,070 行并报告该文件被 76 个文件使用；精确 callers 命令未返回可辨识边，因此以直接源码入口交叉验证，未采用宽泛同名结果。
- 已读生产与装配文件：`pkg/util/chunk/column.rs`、`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/lib.rs`、`pkg/util/chunk/internal/group1/lib.rs`、`pkg/util/chunk/chunk.rs`、`pkg/util/chunk/row.rs`；目标包不存在 `doc.go`。
- 已读对照与测试：`pkg/util/chunk/column.go`、`pkg/util/chunk/column_test.rs`、`pkg/util/chunk/column_test.go`、`pkg/util/chunk/codec_2_aster_unit_test.rs`。Rust 测试中的 8 个测试函数分别覆盖 bitmap、resize/null 区间、reserve/raw、两类重建、深拷贝、类型往返、NULL 合并和 Reset。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试。交付前使用任务文件指定命令确认目标存在且恰有 11 个固定二级章节，并人工复核每个重要行为都能回指上述符号或文件。
