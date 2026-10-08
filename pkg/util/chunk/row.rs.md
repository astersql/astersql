# `pkg/util/chunk/row.rs`

## 文件定位

本文件属于 `astersql-util-chunk` crate。`pkg/util/chunk/Cargo.toml` 以 `lib.rs` 为 crate 入口，并用 `package.metadata.porting.go-package = "pkg/util/chunk"` 记录 Go 来源；`pkg/util/chunk/lib.rs` 通过 `#[path = "row.rs"] pub mod row` 挂载模块，再把 `Row` 重导出到 crate 根。文件直接依赖 crate 内的 `Chunk`、`Column`、`mysql` 和 `types`，标准库依赖只有 `Arc`。

它位于列式 `Chunk` 与按行消费数据的上层逻辑之间：`Chunk` 负责按列保存数据，`Row` 只保存 Chunk 地址和物理行下标，把 `Column` 的读取接口包装成行视角。`Chunk::GetRow` 是主要构造入口；`List::GetRow`、`MutRow::ToRow`、迭代器以及磁盘行容器再通过这一入口或内部构造器向 executor、expression 等上层提供行访问。RustCodeGraph 的目标文件节点显示它被 45 个索引文件使用，说明它是 chunk crate 的基础数据视图，而不是独立存储格式。

## 核心职责

- 定义廉价、可克隆的 `Row` 句柄，以 `*mut Chunk + idx` 定位单行，不复制整块列数据。
- 把定长数值、字符串/字节、时间、Duration、Decimal、Enum、Set、JSON 和 VectorFloat32 的读取统一转交给相应 `Column`。
- 按 `FieldType` 将列值转换为通用 `types::Datum`，保留 NULL、无符号整数、YEAR、collation 和 Decimal 精度信息。
- 提供原始编码读取、NULL 判断、整行调试格式化，以及把借用式行深拷贝成自持有行的能力。
- 用 `Default` 空行和基于地址/下标的相等性支持 iterator 的结束哨兵协议；用 `RowSize` 提供与 Go `unsafe.Sizeof(Row{})` 同目的的记账常量。

本文件不拥有列编码逻辑，也不校验 schema 与 Chunk 是否匹配。数据布局、NULL bitmap、offset 和具体类型解码均由 `Column` 完成；行下标映射由 `Chunk::GetRow` 完成。

## 主要符号

- `pub const RowSize: i64`：当前 Rust `Row` 句柄本身的字节数，即 `size_of::<Row>()`，用于句柄级内存记账，不代表一行数据的列载荷大小。由于 Rust 结构多了 `owner: Option<Arc<Chunk>>`，数值不应被理解为与 Go 常量逐字节相等。
- `pub struct Row { c, idx, owner }`：`c: *mut Chunk` 指向来源 Chunk，空指针表示空行；`idx` 是物理行号；`owner` 只用于让自持有行延长 Chunk 生命周期。`c`、`idx` 为 crate 可见，`owner` 为本模块私有。
- `Default`、`PartialEq`、`Eq`、`Debug`：默认值是 `(null, 0, None)`；相等性只比较 Chunk 指针和下标，不比较 `owner`；调试输出也只显示指针和下标。
- `unsafe impl Send/Sync for Row`：绕过原始可变指针默认不具备线程安全 trait 的限制。它只是类型层声明，不会冻结底层 Chunk，也不会自动同步并发读写。
- `Row::view(&Chunk, usize)`：crate 内借用式构造器，保存 Chunk 的裸地址，不持有所有权。`Chunk::GetRow` 和 `MutRow::ToRow` 使用它。
- `Row::from_owned(Chunk, usize)`：crate 内自持有构造器，把 Chunk 放入 `Arc`，再令 `c` 指向同一分配；`CopyConstruct` 和磁盘单行读回使用它。
- `Chunk`、`IsEmpty`、`Idx`、`Len`：分别返回来源 Chunk、空句柄状态、行号和列数。对空行调用 `Chunk`/`Len` 会 panic。
- `GetInt64`、`GetUint64`、`GetFloat32`、`GetFloat64`、`GetString`、`GetBytes`、`GetTime`、`GetDuration`、`GetEnum`、`GetSet`、`GetMyDecimal`、`GetJSON`、`GetVectorFloat32`：类型化单元格访问器。私有 `column` 先按列下标取 `Column`，再以 `idx` 读取该行。
- `GetDatumRow`、`GetDatumRowWithBuffer`、`GetDatum`、`DatumWithBuffer`：从列式值构造 `Datum` 的主转换链。`GetDatumRow` 创建与 `Len()` 相同长度的新缓冲；带缓冲版本复用传入 `Vec`；单列版本复用同一个核心 switch。
- `GetRawLen`、`GetRaw`、`IsNull`：读取某单元格的原始编码长度、复制后的原始字节和 NULL 状态。
- `CopyConstruct`：创建容量为一的新 Chunk、追加当前行，再返回下标 0 的自持有 `Row`。
- `ToString`：依照字段求值类型格式化所有列，以 `", "` 分隔，NULL 固定显示为 `NULL`。

## 执行流程

1. 上游通常调用 `Chunk::GetRow(logical_idx)`。若 Chunk 安装了 selection vector，该方法先把逻辑下标映射为 `sel[idx]` 的物理下标，再调用 `Row::view`；否则直接保存传入下标。由此，`Row::idx` 始终用于直接索引各 Column。
2. 类型化 getter 先经私有 `column(colIdx)` 调用 `self.Chunk().columns[colIdx]`，再把 `self.idx` 传给 `Column::Get*`。`GetString` 和 `GetBytes` 在 Row 层返回拥有所有权的 `String`/`Vec<u8>`；其他复合类型按其 Rust 值类型返回。
3. `GetDatumRow(fields)` 创建 `Len()` 个默认 Datum，并委托 `GetDatumRowWithBuffer`。后者按 `datumRow` 的长度枚举，而不是按 `fields` 或 Row 的长度枚举；每项调用 `DatumWithBuffer(colIdx, fields[colIdx], datum)`。
4. `DatumWithBuffer` 先短路 NULL。非 NULL 时按 `FieldType::GetType` 分派：整数族依据 unsigned flag 选择 `SetUint64`/`SetInt64`，YEAR 固定走有符号整数；字符串/Blob 附带 collation；时间、Duration、Enum、Set、Bit、JSON、Vector 分别进入对应 Datum setter。
5. Decimal 分支先读取值，再写入字段长度；若字段 decimal 是 `UnspecifiedLength`，使用值的 `GetDigitsFrac()`，否则使用 schema 指定的小数位。这避免未指定精度被错误编码成异常的大 fraction。
6. `ToString` 遍历全部列：NULL 直接追加字面量；其他值按 `EvalType` 格式化，字符串求值类型再对 Enum/Set 特判，实数再按 MySQL Float/Double 分流。每列结果最后以逗号和空格连接。
7. `CopyConstruct` 调用 `renewWithCapacity(self.Chunk(), 1, 1)` 建立同布局的新 Chunk，再用 `AppendRow(self.clone())` 逐列复制，最后把新 Chunk 移入 `Arc`，使返回行不再依赖原 Chunk 的生命周期。

## 数据与状态

普通 `Row` 是视图而不是快照。`owner == None` 时，`c` 的有效性完全依赖外部对象：来源 Chunk 必须保持存活且地址稳定，使用 Row 期间也不能以会失效或改变其语义的方式重置、替换或并发修改来源。`Box<Chunk>`、`List` 内保存的装箱 Chunk 和拥有容器的数据结构为常见稳定地址来源，但 Rust 借用检查器无法从裸指针字段自动验证该不变量。

`owner == Some(Arc<Chunk>)` 时，Arc 与 `c` 指向同一个 Chunk 分配。克隆 Row 会克隆 Arc，从而延长数据生命周期；`owner` 虽不被业务方法显式读取，却承担保活作用。当前 `from_owned` 只在本 crate 内用于深拷贝结果和磁盘单行读取结果。相等性不考虑 owner，因此两个句柄只要裸指针和行号相同即相等；`Row::default()` 则作为唯一常规空句柄。

Row 不缓存 schema、列数或解码结果。`Len` 每次读取 Chunk 当前列数，各 getter 每次进入 Column。`GetDatumRowWithBuffer` 把缓冲区长度视为需要转换的列数：短缓冲只填前缀，长于字段或列数会越界 panic；重复使用缓冲时，受支持类型会覆盖对应 Datum，NULL 会设为 NULL，但未知 MySQL 类型的默认分支不会改写原 Datum。

## 依赖与调用关系

向下依赖方面，所有单元格访问都落到 `Column::{GetInt64, GetUint64, GetFloat32, GetFloat64, GetString, GetBytes, GetTime, GetDuration, GetEnum, GetSet, GetDecimal, GetJSON, GetVectorFloat32, GetRawLength, GetRaw, IsNull}`。Datum 转换依赖 `mysql::HasUnsignedFlag`、MySQL 类型常量、`FieldType` 元数据与 `Datum::Set*`；深拷贝依赖 `renewWithCapacity` 和 `Chunk::AppendRow`。

向上入口方面，`Chunk::GetRow` 调用 `Row::view`，并负责 selection-vector 的逻辑到物理下标映射；`List::GetRow` 先解析 `RowPtr` 再调用对应 Chunk 的 `GetRow`；`MutRow::ToRow` 直接对其单行 Chunk 调用 `Row::view`。`DataInDiskByRows::GetRow` 读回单行后调用 `Row::from_owned`，让返回值脱离局部 Chunk 变量仍然有效。`pkg/util/chunk/lib.rs` 将 `Row` 重导出，因此上层通常通过 chunk crate 根使用它。

可核实的上层消费示例包括：`pkg/expression/column.rs` 用 `GetDatum`、`IsNull` 以及类型 getter 实现表达式求值；`pkg/expression/builtin.rs` 读取 NULL、raw bytes 和 Datum；`pkg/dxf/framework/storage/*` 与 `pkg/dxf/importinto/*` 以类型 getter 解码系统表查询结果；`pkg/expression/util_runtime_parity_aster_unit_test.rs` 用 `CopyConstruct` 保存独立行。RustCodeGraph 对精确 `callers/callees` 的查询在本次 30 秒窗口内未返回，因此这些生产调用点由精确源码搜索补证，不能据此宣称已经穷举所有调用者。

## 错误处理与边界

本文件没有 `Result` 返回路径；调用契约违例主要表现为 panic 或底层索引失败。空行调用 `Chunk` 会因显式断言 `empty Row has no Chunk` panic；列下标越界、行下标越界、字段元数据不足，以及过长 Datum 缓冲都会在 Rust 索引或 Column 访问处 panic。`pkg/util/chunk/row_test.rs` 明确验证 `ToString` 的字段类型切片短于行列数时必须 panic，而不是静默格式化前缀。

`DatumWithBuffer` 对 NULL 总是先调用 `SetNull` 并返回。对已列出的 MySQL 类型会完整赋值；对未匹配类型的 `_ => {}` 不报错且保留传入 Datum 的旧状态，这在复用缓冲时尤其需要调用方注意。YEAR 无论 unsigned flag 如何都读取为 `Int64`。Decimal 未指定小数位时采用实际 fraction，避免 Go 注释指出的编码 `BadNumber` 风险。

`ToString` 对未覆盖的 `EvalType` 返回空字符串；`ETReal` 下不是 Float/Double 的类型同样返回空字符串。它是调试/展示辅助，不是可逆编码。`GetRaw` 和 `GetBytes` 在 Rust 层复制数据，调用方修改返回 Vec 不会修改 Column。`CopyConstruct` 会完整复制当前行，但前提仍是源 Row 在复制过程有效。

## 并发与资源生命周期

`Row` 自身没有锁、任务、通道或显式释放动作。借用式 Row 的生命周期由来源 Chunk/容器管理；自持有 Row 由 `Arc<Chunk>` 引用计数管理，最后一个克隆销毁时释放复制的 Chunk。`CopyConstruct` 的新 Chunk 与原 Chunk 独立，适合把行带出原容器生命周期；相对地，普通 `clone()` 只克隆句柄和可选 Arc，不深拷贝列数据。

`unsafe impl Send` 和 `unsafe impl Sync` 允许 Row 跨线程移动或共享，但本文件没有同步底层 Chunk，也没有建立不可变快照。安全使用要求共享期间底层 Chunk 仍然存活、地址稳定且不发生与读取竞争的修改；尤其不能把裸指针可达性误解为并发写安全。自持有 Arc 解决的是生命周期，不自动为 Chunk 内部可变访问提供锁。

Row 使用裸 `*mut Chunk`，但所有公开读取方法把它解引用成 `&Chunk`；本文件自身不通过该指针写入。唯一构造裸指针的位置是 `view` 的借用地址转换和 `from_owned` 的 `Arc::as_ptr`。扩展这些路径时必须保持 `c` 与 `owner` 指向同一分配，并避免移动或提前释放借用来源。

## 与 Go 版本的对应关系

Rust 实现逐项对应 `pkg/util/chunk/row.go`：`RowSize`、行句柄、类型化 getter、Datum 转换 switch、raw/NULL 访问、深拷贝和 `ToString` 的分支顺序均保留 Go 意图。关键兼容点包括整数 unsigned flag、YEAR 恒为有符号、字符串 collation、Decimal 未指定 scale 时使用真实 fraction、Enum/Set 的字符串显示和 `NULL` 字面量。

语言和所有权差异包括：Go Row 只有 `*Chunk + int`，Rust 额外增加 `Option<Arc<Chunk>>` 以支持安全保活已复制或磁盘读回的数据；因此 Rust `RowSize` 是 Rust 句柄大小，不保证数值等于 Go。Go `Chunk()` 可返回 nil，Rust `Chunk()` 在空行上 panic；Go 的 slice/string 可直接引用 Column backing storage，Rust `GetBytes` 和 `GetString` 返回拥有值。Go `GetMyDecimal` 返回指针，Rust 返回值类型。Go 的 `CopyConstruct` 依赖逃逸分析保持新 Chunk 存活，Rust 显式用 Arc 保活。

Go `GetDatumRowWithBuffer` 同样按传入 datumRow 长度循环，Rust保留了短缓冲只转换前缀的语义。Go `ToString` 也遍历全部 Chunk 列并索引对应 FieldType；Rust 独立测试据此把短字段切片定义为调用契约违例。Go `row_in_disk_test.go` 比较原 Chunk 行与磁盘读回行的 `GetDatumRow`，Rust 的 `row_container_4_aster_unit_test.rs` 则覆盖类型读取、NULL、`ToString`、`CopyConstruct` 和磁盘读回。

## 扩展指南

新增 MySQL 类型时，优先同步修改 `DatumWithBuffer` 和 `ToString`：前者决定通用 Datum 语义，后者决定展示语义；同时确认 `Column` 已有对应 getter、`types::Datum` 有 setter、Go `row.go` 的分支和字段元数据规则一致。未知类型当前静默保留旧 Datum，若要改变为清空或报错，会影响缓冲复用兼容性，不能只为方便测试而单方面简化。

修改生命周期或构造路径时重点检查 `Row::{view, from_owned, CopyConstruct, clone}`、`Chunk::GetRow`、`MutRow::ToRow` 和 `DataInDiskByRows::GetRow`。任何新增借用式构造都必须证明 Chunk 地址和存活期稳定；任何自持有构造都必须保证裸指针来自最终 Arc 分配。修改并发声明前必须审计 Chunk/Column 的可变访问，不能仅凭读取方法签名维持 `unsafe Send/Sync`。

测试必须继续放在独立文件，不得嵌入 `row.rs`。直接同步 `pkg/util/chunk/row_test.rs`；基础行访问、深拷贝和磁盘生命周期可扩展 `pkg/util/chunk/row_container_4_aster_unit_test.rs`，Go 对照应核查 `pkg/util/chunk/row.go`、`chunk_test.go`、`row_in_disk_test.go`。建议覆盖每种 MySQL 类型、NULL、unsigned/YEAR、Decimal 未指定 scale、未知类型的缓冲复用、selection vector、空 Row、非法列/行下标、短/长 buffer、短 FieldType、复制后销毁或重置原 Chunk，以及仅在具备同步保证时才添加跨线程读取测试。性能风险主要来自新增克隆/分配、把直接 Column 访问改成重复转换，以及无意让常用 getter 进入锁或 Arc 热路径。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，其中 Rust 文件 7,032 个；`files --filter pkg/util/chunk` 确认目标、模块入口和独立测试均已索引。
- 目标源码：`node --file pkg/util/chunk/row.rs --offset 1 --limit 400` 完整读取 342 行，核对 `RowSize`、`Row` 三个字段、trait impl、两个构造器、全部 getter、Datum switch、深拷贝和格式化逻辑；`query CopyConstruct`、`query DatumWithBuffer`、`query GetDatumRowWithBuffer` 和 `query from_owned` 用文件路径消歧 Rust/Go 同名符号。
- crate 与入口：`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/lib.rs`；后者在 106--108 行挂载并重导出 Row，在 195--197 行把 `row_test.rs` 作为独立测试模块挂载。
- 直接构造与调用链：`pkg/util/chunk/chunk.rs` 的 `GetRow`/`AppendRow`，`pkg/util/chunk/list.rs` 的 `GetRow`，`pkg/util/chunk/mutrow.rs` 的 `ToRow`，`pkg/util/chunk/row_in_disk.rs` 的 `GetRow`/`GetRowAndAppendToChunk`；生产消费示例经精确源码搜索核对 `pkg/expression/column.rs`、`pkg/expression/builtin.rs` 和 `pkg/dxf/framework/storage/*`。
- Go 对照与测试：`pkg/util/chunk/row.go`、`pkg/util/chunk/row_test.rs`、`pkg/util/chunk/row_container_4_aster_unit_test.rs`、`pkg/util/chunk/row_in_disk_test.go`、`pkg/util/chunk/chunk_test.go`。仓库中不存在独立的 `pkg/util/chunk/row_test.go`，因此 Go 证据来自上述相邻测试。
- 调用图限制：对三个精确 Rust 方法 ID 执行 `callers/callees` 时在 30 秒窗口内未产生输出；文档已用目标文件节点的 used-by 信息、精确符号查询和调用点源码搜索补齐，不把无输出解释为无调用者。
- 已人工核对：本文说明了文件存在原因、行读取与 Datum 转换流程、裸指针/Arc 生命周期、panic 与未知类型边界、Go 对齐关系和安全扩展位置。任务是纯文档分析，按计划未运行 Cargo。
