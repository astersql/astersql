# `pkg/util/chunk/mutrow.rs`

## 文件定位

本文件实现 `astersql-util-chunk` crate 中的可变单行容器 `MutRow`。crate 入口 [`lib.rs`](lib.rs) 以 `#[path = "mutrow.rs"] pub mod mutrow` 挂载该模块；[`Cargo.toml`](Cargo.toml) 将 crate 命名为 `astersql-util-chunk`，并通过工作区内的 `astersql-parser-mysql` 与 `astersql-types-*` 依赖取得 MySQL 类型常量、`Datum`、`FieldType` 及标量类型。

`MutRow` 位于值计算和列式批处理之间：调用者可以逐列改写一行，再用 `ToRow` 把它作为只读 `Row` 交给表达式、表索引或执行器代码。RustCodeGraph 的调用结果显示，`MutRowFromDatums` 的真实调用者包括 `pkg/executor/physical_plan_runtime.rs` 的表达式求值/聚合更新路径、`pkg/table/tables/index.rs` 的部分索引条件匹配，以及多个聚合测试辅助函数；因此该文件是已接线的运行时基础设施，不是桩或仅用于测试的兼容门面。

## 核心职责

- 用 `MutRow { c: Box<Chunk>, idx: 0 }` 持有一个容量和 required rows 均为 1 的列式行；每个逻辑单元格对应一个 `Column`（`MutRowFromValues`）。
- 提供三种构造入口：异构值 `MutRowFromValues`、统一值容器 `MutRowFromDatums`、按字段类型生成零值的 `MutRowFromTypes`。
- 通过 `SetRow`、`SetValues`/`SetValue`、`SetDatums`/`SetDatum` 和 `ShallowCopyPartialRow` 在既有列布局上覆盖值，同时维护 `data`、`elemBuf`、`offsets` 与 `nullBitmap` 的一致性。
- 将整数、浮点数、字符串/字节、DECIMAL、时间、时长、JSON、向量、ENUM/SET 等 TiDB 值编码为 `Column` 所使用的定长或变长字节布局（`makeMutRowColumn` 及其辅助函数）。
- 通过 `ToRow` 暴露廉价只读视图，通过 `Clone` 提供底层 `Chunk` 的深拷贝，避免可变副本之间共享列缓冲。

## 主要符号

- `pub enum GoAny`：模拟 Go `any` 类型分派的封闭枚举。`Nil` 表示 SQL NULL；数值、文本、二进制字面量及各 MySQL 复合类型分别携带具体值；`Other` 表示无法识别的 interface 载荷。
- `pub struct MutRow { pub c: Box<Chunk>, pub idx: usize }`：拥有底层 `Chunk` 的单行容器。所有本文件构造器都令 `idx == 0`，但字段公开，方法按字段值读取。
- `MutRow::ToRow(&self) -> Row`：调用 `Row::view` 生成不拥有 `Chunk` 的指针视图。`MutRow::Len` 返回列数；`MutRow::Clone` 调用 `Chunk::CopyConstruct` 深拷贝。
- `MutRow::SetRow` 与 `MutRow::ShallowCopyPartialRow`：从 `Row` 的目标行下标提取每列的定长片段或变长 offset 区间，再写入目标列。后者从指定目标列开始写入源行的全部列。
- `MutRow::SetValue` / `SetDatum`：单列更新入口。批量方法 `SetValues` / `SetDatums` 只是按下标依次调用单列入口。
- `MutRowFromValues` / `MutRowFromDatums` / `MutRowFromTypes`：三个公开构造函数；后两者分别先经 `datum_to_value` 和 `zeroValForType` 转成 `GoAny`。
- `datum_to_value` / `interface_to_value`：把 `Datum::Kind` 或 `KindInterface` 内的 `Any` 载荷映射为 `GoAny`。无法识别的 interface 值映射为 `GoAny::Other`，其他未覆盖的 datum kind 映射为 `Nil`。
- `makeMutRowColumn`、`newMutRowFixedLenColumn`、`newMutRowVarLenColumn`：创建长度为 1 的列及初始 bitmap/offset；新列会通过 `new_column_reference_id` 获得独立引用标识。
- `cleanColOfMutRow` / `mark_not_null`：更新前把 offsets 归零并标 NULL，成功编码后再标非 NULL。
- `set_fixed_bytes`、`write_sized`、`setMutRowBytes`、`setMutRowNameValue`、`setMutRowJSON`：分别处理定长原始字节、固定尺寸结构、普通变长字节、ENUM/SET 的 `value + name` 布局，以及 `TypeCode + Value` 的 JSON 布局。

## 执行流程

1. 构造时，`MutRowFromDatums` 或 `MutRowFromTypes` 先把每个输入转换为 `GoAny`，然后统一进入 `MutRowFromValues`。
2. `MutRowFromValues` 对每个值调用 `makeMutRowColumn`。NULL 建成 bitmap 为 0 的空变长列；整数和 `f64` 使用 8 字节定长列；`f32` 使用 4 字节列；字符串、字节、BIT 和向量走变长列；DECIMAL/Time 使用固定尺寸写入；JSON、ENUM/SET 使用带元数据前缀的变长布局。
3. 构造器把列集合装入单行 `Chunk`，设置 `capacity = 1`、`requiredRows = 1`、`idx = 0`，此后 `ToRow` 可生成读取视图。
4. 单列更新先调用 `cleanColOfMutRow`，使旧值在中间状态下表现为 NULL。`Nil`/NULL 立即返回；其余已知类型写入对应布局并由 `mark_not_null` 提交可见状态。
5. `SetDatum` 对常见 datum kind 复用目标列并覆盖其缓冲；`KindNull`、`KindInterface` 等不在快速分支内的 kind 会通过 `datum_to_value` 和 `makeMutRowColumn` 整列替换，然后返回。
6. `SetRow` 和 `ShallowCopyPartialRow` 先从源 `Row` 的原始 Chunk 指针取得列。固定列按 `row.idx * elemBuf.len()` 定位，变长列按 `offsets[row.idx..=row.idx+1]` 定位；Rust 实现把选中的字节复制进目标列。
7. `Clone` 深拷贝 Chunk 和列缓冲。后续对克隆的 `SetValue` 不会影响原行，`pkg/util/chunk/mutrow_test.rs::mutable_row_updates_values_without_aliasing_its_clone` 直接验证了该性质。

## 数据与状态

`Column` 的相关不变量定义在 [`column.rs`](column.rs)：`length` 是元素数，`nullBitmap` 的 bit 0 表示单行是否非 NULL，`offsets` 仅用于变长列，`data` 保存实际编码，`elemBuf` 的长度标识定长元素宽度。本文所有新建列的 `length` 均为 1。

定长列由 `newMutRowFixedLenColumn` 初始化等长的 `data` 和 `elemBuf`；变长列由 `newMutRowVarLenColumn` 初始化 `offsets = [0, value_size]`、空 `elemBuf`。更新变长值时，`setMutRowBytes` 重建 `data`，保证至少两个 offsets，并令第二个 offset 等于新字节长度。更新定长值时，`set_fixed_bytes` 同步调整 `data` 与 `elemBuf` 的长度；`write_sized` 先按目标尺寸清零，再截断或零填输入。

NULL 更新不会清空 `data` 或 `elemBuf`，只把 offsets 归零并清除 bitmap。这保留了可复用缓冲；Go 测试 `TestIssue29947` 明确要求写入 NULL 后旧缓冲内容不变。使用者必须以 bitmap 判断 NULL，不能把残留字节当作当前值。

数值编码主要使用小端字节；DECIMAL 的标志字段逐字节写入、word buffer 使用本机字节序，`Time` 的核心值也使用本机字节序。这些布局必须与 `Row`/`Column` 的读取实现同步修改，不能单独改变写侧。

## 依赖与调用关系

上游方面，RustCodeGraph 对 `MutRowFromDatums` 的查询给出了 `pkg/executor/physical_plan_runtime.rs`、`pkg/table/tblctx/buffers.rs`、`pkg/table/tables/index.rs` 以及多个表达式/聚合测试调用者；`MutRowFromValues` 还被 planner、extension 与 chunk 测试使用。这些调用把普通 `Datum` 或类型化值装成可由 `Row` API 消费的列式单行。

下游方面，RustCodeGraph 的 callees 结果确认：

- 构造链为 `MutRowFromDatums -> datum_to_value -> MutRowFromValues -> makeMutRowColumn`，或 `MutRowFromTypes -> zeroValForType -> MutRowFromValues`。
- 写入链为 `SetDatums -> SetDatum`、`SetValues -> SetValue`；单列方法继续调用清理、定长/变长编码、JSON/名称值编码与 bitmap 提交函数。
- 行复制链 `SetRow` / `ShallowCopyPartialRow` 依赖 `Row`、`Column::IsNull` 和相同的底层编码辅助函数。
- `Clone` 依赖 `Chunk::CopyConstruct`；`ToRow` 依赖 `Row::view`。

crate 内模块关系由 `lib.rs` 提供：`Row` 来自 `row.rs`，`Chunk`/`Column` 经 `group_1` 再导出，`mysql` 与 `types` 由该 crate 的依赖适配层提供。`mutrow.rs` 本身不执行 I/O、不使用 channel/failpoint，也不产生 `ChunkError`。

## 错误处理与边界

这些 API 不返回 `Result`，调用契约错误主要表现为 panic：列下标越界会触发 Vec 索引 panic；`SetRow` 明确断言源、目标列数相等；`ShallowCopyPartialRow` 要求从 `columnIndex` 起有足够目标列；损坏的固定宽度或 offsets 可能在切片时 panic。`ToRow` 产生的非拥有视图若在 `MutRow` 被移动释放后继续使用，会违反 `Row::view` 的存活期契约。

`MutRowFromTypes` 对未知 MySQL 类型使用 `GoAny::Nil`，而 TiDB vector 零值通过 `ParseVectorFloat32("[]").expect("empty vector is valid")` 构造；该 expect 依赖空向量字面量始终有效。`interface_to_value` 无法识别的动态类型成为 `Other`：构造时得到 `Column::default()`，写入既有列时只清 offsets/bitmap 后保留原 data 载荷并标为非 NULL。扩展动态类型时必须同时定义明确的构造和更新语义，不能只补一侧。

`SetDatum` 的快速分支显式覆盖 `KindMysqlBit` 并按字节处理；独立 Rust 测试 `mysql_bit_datums_preserve_their_binary_payload` 同时验证构造和赋值，防止 BIT 被错误当作普通整数。它还验证 `KindInterface` 中的 `i64` 会触发整列替换并可由 `GetInt64` 读取。

## 并发与资源生命周期

`MutRow` 独占 `Box<Chunk>`，变更方法都要求 `&mut self`，文件内没有锁、原子状态、后台任务、通道或外部资源清理。正常 Rust 借用规则阻止同一 `MutRow` 被并发改写；如调用者自行放入同步容器，并发协议由容器和调用者负责。

`ToRow` 返回的 `Row` 内含指向 Chunk 的原始指针且 `owner = None`（见 `row.rs::Row::view`），类型签名没有把视图生命周期绑定到 `&self`。因此调用者必须保证 `MutRow` 在所有派生 `Row` 使用期间仍存活，并避免在读取视图时发生使底层数据失效的并发修改。相反，`Clone` 的目标拥有独立 Chunk/列缓冲，可独立修改。

名称为 `ShallowCopyPartialRow` 的方法在 Go 中直接令目标 `data` 切片别名源行缓冲；Rust 为避免悬垂原始切片而复制选中字节，目标随后不依赖源 Chunk 的生命周期。这是资源安全上的有意差异，而不是共享所有权。

## 与 Go 版本的对应关系

主要对照文件是 [`mutrow.go`](mutrow.go)，公开概念和分支结构基本逐项对应：`MutRow`、三种构造器、读视图/长度/克隆、整行与单列更新、零值选择，以及定长/变长/JSON/ENUM/SET 编码辅助函数均保留。

Rust 用 `GoAny` 取代 Go type switch 的开放 `any`；`Vec<T>` 取代可变参数或切片，`Box<Chunk>` 取代 `*Chunk`。Go 借助 `unsafe` 把 DECIMAL、Time 等结构直接写入字节缓冲，Rust 则由 `decimal_bytes`、`time_bytes` 和 `write_sized` 显式序列化。Go 的 fixed-column 构造让 `data` 与 `elemBuf` 共享同一底层切片，Rust 使用两个独立 `Vec<u8>`，但仍以相同宽度标识定长布局。

两版最重要的可见差异是 `ShallowCopyPartialRow`：Go 测试 `TestMutRowShallowCopyPartialRow` 证明源行后续改变时目标可观察到共享缓冲的更新；Rust 方法注释和实现明确改为拥有字节副本，避免源行释放后的悬垂别名。因此不能把 Go 的“浅拷贝后联动”直接写成 Rust 保证。Rust 独立测试当前只验证克隆隔离和 BIT/interface datum；Go `TestMutRow`、`TestIssue29947` 与 partial-row 测试为更广的移植语义提供参考证据，但不等价于 Rust 已覆盖这些回归。

## 扩展指南

- 新增支持的值类型时，应同步修改 `GoAny`、`datum_to_value`、`interface_to_value`、`zeroValForType`（若有字段零值）、`makeMutRowColumn`、`SetValue` 与 `SetDatum`，并核对对应 `Row`/`Column` 解码路径。遗漏构造或更新任一侧会导致同一类型行为不一致。
- 改动列布局时，应保持 `length == 1`、bitmap bit 0、定长 `elemBuf` 宽度、变长 `[0, len]` offsets，以及 `new_column_reference_id` 的独立性；同时检查 `pkg/util/chunk/column.rs` 和 `pkg/util/chunk/row.rs` 的读取契约。
- 涉及 NULL 时必须保留“标 NULL、offset 归零、底层缓冲可复用”的行为，除非 Go 对照和调用者已同步改变。不要仅凭残留 `data` 判断有效值。
- 改动 `ToRow` 或 partial-row 复制时，要明确所有权和存活期。若希望继续靠原始指针实现零拷贝，需要给出不会悬垂、不会并发读写冲突的可验证方案。
- Rust 回归测试应继续放在独立的 `pkg/util/chunk/mutrow_test.rs`，不要内嵌到生产文件。类型矩阵与兼容语义还应对照 `pkg/util/chunk/mutrow_test.go`；至少覆盖新类型的构造、`SetValue`、`SetDatum`、NULL、克隆隔离和定长/变长读取。
- 性能敏感改动要关注每次写入的 Vec 重新分配与复制。当前 Rust 的 `setMutRowBytes` 会重建 `data`，而 Go 会尽量复用容量；优化时不能牺牲 bitmap/offset 正确性或重新引入悬垂别名。

## 验证依据

- RustCodeGraph 索引状态：项目索引含 `pkg/util/chunk/mutrow.rs`（560 行、50 个符号）；使用 `files --filter pkg/util/chunk`、`explore`、`query`、`node --file`、`callers` 与 `callees` 检查文件、符号及调用边。精确查询确认 Rust `MutRow`、`MutRowFromDatums`、`MutRowFromTypes`、`SetDatum`、`ShallowCopyPartialRow` 的定义位置；callers 查询在大范围图上超时，调用者证据由同次 `explore` 的 blast radius 与索引的文件使用关系补齐。
- 完整阅读的实现与边界文件：`pkg/util/chunk/mutrow.rs`、`pkg/util/chunk/lib.rs`、`pkg/util/chunk/row.rs` 的 `Row`/`Row::view`、`pkg/util/chunk/column.rs` 的 `Column`/引用 ID，以及 `pkg/util/chunk/Cargo.toml`。
- Go 对照：完整阅读 `pkg/util/chunk/mutrow.go`，逐项核对构造、写入、NULL、序列化与 partial-row 所有权差异。
- 测试证据：完整阅读 `pkg/util/chunk/mutrow_test.rs` 与 `pkg/util/chunk/mutrow_test.go`。Rust 测试证明深克隆隔离、BIT 字节载荷及 interface datum；Go 测试补充全类型零值、NULL 缓冲保留、行覆盖和浅拷贝别名预期。
- 本任务是纯文档分析，未修改 Rust/Go/Cargo 行为，也未运行 Cargo。交付结构按任务要求检查本文恰好包含十一个固定二级标题，并人工复核重要结论均可回溯到上述符号、调用图或对照测试。
