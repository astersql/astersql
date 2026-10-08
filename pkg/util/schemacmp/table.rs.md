# `pkg/util/schemacmp/table.rs`

## 文件定位

本文件（[Rust 源码](./table.rs)）是 `astersql-util-schemacmp` crate 的表级 schema 适配层：它把 `meta_model::TableInfo` 编码成 `lattice.rs` 定义的格元素，并向外提供比较、求上确界（join）、字段类型解码和调试 SQL 还原能力。模块由 `pkg/util/schemacmp/lib.rs` 的 `mod table; pub use table::*;` 导出；crate 又通过根 `Cargo.toml` 的 workspace 成员和 `facade_util_schemacmp` 依赖进入仓库门面（`pkg/lib.rs`）。

当前 Rust 仓库中可检索到的直接使用主要位于独立测试：`pkg/util/schemacmp/table_test.rs`、`pkg/util/schemacmp/charset_collation_1_aster_unit_test.rs` 和 `pkg/util/dbutil/table_test.rs`。因此它已具备与 Go 实现对应的表比较/合并能力，但代码搜索没有证明它已接入 Rust 运行时 SQL/DDL 主链；不能把测试覆盖等同于生产接线。

## 核心职责

- `Encode` / `EncodeWithOptions` 将表排序规则、列、索引、自增 ID、分片位数、自动随机位数、预分裂 Region 数和压缩选项编码为固定八维 `Tuple`。固定下标常量 `TABLE_*`、`COLUMN_*`、`INDEX_*` 是编码和解码共同依赖的不变量。
- `ColumnMap` 为“列名到列元组”定义缺项语义：没有默认值的列不能在比较时缺失；join 遇到只在一侧出现的列时保留该列，清除不再成立的键标志，并为需要的 `NOT NULL` 列补标准默认值。
- `IndexMap` 为“索引名到索引元组”定义缺项语义：单侧索引不阻止比较，但 join 会删除只在一侧存在或两侧不兼容的索引。
- `Table::Compare` 把整个表元组的偏序比较委托给格实现；`Table::Join` 在通用 join 后重新从存活索引推导列的主键、唯一键和普通键标志，并拒绝失去所有键约束的 `AUTO_INCREMENT` 列。
- `Table::Restore` / `String` 把解包后的格值还原成稳定顺序的 `CREATE TABLE` 文本，主要用于调试与测试，而不是无损 DDL 序列化。

源文件没有处理 view、partition、sequence、foreign key 等对象；这些维度也不在八维表元组中。

## 主要符号

- `TableOptions`：编码时可覆盖的表级选项。`Default` 使用 `utf8mb4_bin`、零数值和空压缩串；`Encode` 则从传入的 `TableInfo` 构造选项后调用 `EncodeWithOptions`。
- `Table { value: LatticeBox }`：公开的已编码表。内部值始终应为本文件构造的八维 `Tuple`；字段不公开，调用方不能直接破坏布局。
- `Encode(&TableInfo) -> Table`、`EncodeWithOptions(&TableInfo, &TableOptions) -> Table`：公开编码入口。后者调用 `encodeTableInfoToLattice`，前者保留 `TableInfo` 自身的表选项。
- `DecodeColumnFieldTypes(&Table) -> HashMap<String, FieldType>`：解包每个列元组的 `COLUMN_FIELD_TYPE`，克隆为独立映射。
- `Table::Compare(other: Table) -> Result<i32, IncompatibleError>`：返回负数、零、正数表示当前表分别小于、等于、大于另一表；不可比较时返回错误。参数按值传入，但内部只借用其格值完成比较。
- `Table::Join(other: Table) -> Result<Table, IncompatibleError>`：求两表上确界并执行索引/列键标志一致性修复。
- `Table::Restore`、`Table::String` 和 `Debug for Table`：分别写入给定恢复上下文、以固定表名 `tbl` 生成字符串，以及令调试格式等同于该字符串。
- `ColumnMap` / `IndexMap`：`LatticeMap` 的两种内部实现，决定 map 键缺失及不兼容值的比较/join 策略。
- `IndexColumn` / `IndexColumnSlice`：以小写列名和前缀长度描述索引列序列；`IndexColumnSlice` 实现 `Equality`，所以列顺序、名称或长度不同均不是相等单点。
- `encodeColumnInfoToLattice`、`encodeIndexInfoToLattice`、`encodeImplicitPrimaryKeyToLattice`：把元数据对象分别转换为列/索引元组；隐式主键来自没有显式 primary index 时带 `mysql::PriKeyFlag` 的列。

## 执行流程

1. 调用方把 `TableInfo` 交给 `Encode`。`Encode` 收集该表当前选项后进入 `EncodeWithOptions`；显式需要替换选项的调用方可直接使用后者。
2. `encodeTableInfoToLattice` 先扫描 `info.Indices`，以索引小写名为键编码索引，并判断是否已有显式主键。随后扫描 `info.Columns`，以列小写名编码列；若没有显式主键且列带 `PriKeyFlag`，则补名为 `primary` 的单列 BTREE 索引。
3. 编码结果按固定顺序装入 `Tuple`：`Collation`、`Map(ColumnMap)`、`Map(IndexMap)`、`Int64(AutoIncID)`、三个 `Singleton<u64>` 和 `MaybeSingletonString(Compression)`。
4. `Compare` 直接调用根格值的 `Compare`。元组逐维比较；列/索引 map 再通过本文件的 `CompareWithNil` 和底层 `Tuple`、`Type`、`Collation` 等规则决定顺序或不相容。
5. `Join` 先调用根格值的 `Join`。列 map 保留单侧列并可能补默认值；索引 map 删除单侧或不兼容索引。随后遍历存活索引：primary 的所有列获得 `PriKeyFlag`，单列 unique 获得 `UniqueKeyFlag`，其余索引只有第一列获得 `MultipleKeyFlag`。最后逐列调用 `typ::setAntiKeyFlags` 写回推导结果，并检查自增列仍是键。
6. `Restore` 解包元组，将列和索引分别按名称字典序输出，再追加 collation，以及非零的 `SHARD_ROW_ID_BITS`、注释形式的 `AUTO_RANDOM_BITS` 和非空 `COMPRESSION`。`String` 用默认恢复标志和固定表名 `tbl` 调用它。

## 数据与状态

表、列、索引的结构均以异构 `Tuple(Vec<LatticeBox>)` 保存，下标就是序列化协议。列四维依次为默认值、生成列表达式、是否 stored、字段类型；索引四维依次为有序索引列、是否非唯一、是否非主键、索引类型；表为八维。修改任一布局必须同步所有编码、解包、join 后处理和恢复位置，不能只新增常量。

`ColumnMap` 和 `IndexMap` 使用 `HashMap<String, Tuple>`，键来自 `model.CIStr.L`，即规范化小写名。哈希迭代顺序不稳定，但 `Restore` 在输出前排序，因此字符串结果确定。`Table`、map、tuple 和字段类型在 join/解码路径中会克隆；`DecodeColumnFieldTypes` 返回的类型不与 `Table` 共享可变状态。

表元组中的 `TABLE_AUTO_INC_ID` 会参与格比较，但当前 `Restore` 不输出它；`TABLE_PRE_SPLIT_REGIONS` 被编码，却同样没有输出。它们是比较状态而非完整 DDL round-trip 的保证。

## 依赖与调用关系

上游边界是 `meta_model::{TableInfo, ColumnInfo, IndexInfo, DefaultValue}`。`pkg/util/schemacmp/Cargo.toml` 将它声明为 `meta-model` 路径依赖，并通过 `lib.rs` 的 `model`、`ast`、`mysql` 再导出供本文件使用。`parser-types` 提供 `format`、字符集/字段类型，`tidb-types` 提供字段类型构造能力；后两者也由 crate 入口组合为 `types` 命名空间。

内部主要调用边为 `Encode -> EncodeWithOptions -> encodeTableInfoToLattice`；编码再调用三个细粒度编码函数。`Compare` / `Join` 下沉到 `LatticeBox` 的动态分派实现（`pkg/util/schemacmp/lattice.rs`），字段类型规则下沉到 `typ`（`pkg/util/schemacmp/type.rs`），排序规则规则下沉到 `Collation`（`pkg/util/schemacmp/charset_collation.rs`）。`Restore` 下沉到 `parser_types::format::RestoreCtx` 和 `FieldType::Restore`。

RustCodeGraph 对目标文件报告 47 个符号，并识别 `EncodeWithOptions` 被 `Encode` 调用、它调用 `encodeTableInfoToLattice`。精确仓库搜索显示目标 API 的 Rust 使用点为测试：表级主测试直接覆盖 `Encode`、`Compare`、`Join`、`String` 和 `DecodeColumnFieldTypes`；`pkg/util/dbutil/table_test.rs` 还验证从 dbutil 得到的真实 `TableInfo` 可被编码。未发现 `EncodeWithOptions` 的文件外调用。

## 错误处理与边界

可预期的 schema 不兼容使用 `IncompatibleError` 返回：无默认值列缺失由 `ColumnMap::CompareWithNil` 报错；同名字段类型、生成表达式、索引单点或其他 tuple 维不相容由下层格实现传播；join 后自增列没有任何存活键时，`Table::Join` 用 `ErrMsgAtMapKey` 包装列名和 `ErrMsgAutoTypeWithoutKey`。

若格值不是本文件约定的具体类型或 tuple 长度不正确，`value`、map 插入、解码、恢复和 join 后处理中的 `downcast_ref` / `downcast_mut` 会 `expect` 或 `unwrap`。这属于内部表示不变量被破坏，而非面向调用者的可恢复输入错误；由于 `Table.value` 私有，正常公开入口不会构造这种值。

`Restore` 有意忽略 `WriteName`、`WritePlain`、`WriteKeyWord`、字段类型恢复等写入结果，和 Go 对照的调试用途一致。`defaultValueString` 对字符串默认值执行 UTF-8 有损转换并直接写普通文本，不承诺完整保留任意二进制默认值或为所有字符串补 SQL 引号。因此 `String` 只应被视为规范化诊断文本。

当前表示不含 view、partition、sequence、foreign key、索引可见性等维度；把这些属性不同的表判作兼容是现有模型边界，不应在文档外推为完整 schema 等价。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。编码和比较是进程内纯计算；`Restore` 唯一借用外部资源是调用方提供的可变 `RestoreCtx`，借用只持续到调用返回。

`Table` 可克隆，但没有内部同步原语。公开操作不暴露共享可变引用：`Compare` 只读，`Join` 创建并修改新的 joined 格值，`DecodeColumnFieldTypes` 克隆结果。是否可跨线程传递最终由 `LatticeBox` trait object 的约束决定，本文件没有声明或保证额外的 `Send`/`Sync` 契约。

`Compare` 和 `Join` 按值消费 `other`，而 `self` 仅借用；如调用方仍需另一张表，应像测试一样预先克隆。大表的编码、join、解码和恢复会随列/索引数量分配和克隆，且恢复包含排序，约为列和索引各自的 `O(n log n)` 输出准备成本。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/schemacmp/table.go`，核心布局和算法逐项对应：Go 的 `columnMap` / `indexMap` 缺项规则、隐式主键合成、八维表 tuple、`Compare`/`Join`、键标志回填、无键自增拒绝和按名称排序恢复，都在 Rust 中保留。`pkg/util/schemacmp/Cargo.toml` 的 `[package.metadata.porting]` 也明确记录 `go-package = "pkg/util/schemacmp"`。

可见差异包括：Rust 新增公开 `TableOptions` 与 `EncodeWithOptions`，允许不修改 `TableInfo` 就覆盖表级编码选项；Go 只提供直接读取 `TableInfo` 的 `Encode`。Rust 的 `DecodeColumnFieldTypes` 返回拥有所有权的 `FieldType` 克隆，Go 返回 `*FieldType` 指针。Rust 恢复函数内联在 `Table::Restore`，Go 通过 `restoreTableInfoFromUnwrapped` 和 `sortedMap` 辅助函数完成；结果顺序语义相同。

Rust `table_test.rs` 基本复刻 Go `table_test.go` 的 join/compare/string 案例，包括 DM 编号案例、缺失 `NOT NULL` 列、字段类型不兼容、生成列、索引差异和自增键约束。当前 Rust 用例未包含 Go 用例中的 `latin1_to_utf8mb4` 以及若干 2020 日期/BLOB 变体；同时 Rust 测试采用子串错误断言而 Go 使用正则。扩展或修复时应检查两套用例，不能仅凭 Rust 当前集合认定语义已全部覆盖。

## 扩展指南

- 新增表级比较维度时，先确认 Go 对照语义，再同时更新 `TABLE_*` 下标、`encodeTableInfoToLattice`、必要的恢复逻辑及 `TableOptions`（若允许覆盖）；为旧 tuple 兼容或迁移策略作显式决定。最接近的独立测试是 `pkg/util/schemacmp/table_test.rs`，对应 Go 测试是 `table_test.go`。
- 新增列或索引属性时，应修改各自的 tuple 布局、编码与恢复函数，并检查 `ColumnMap` / `IndexMap` 的缺项语义。索引属性还必须检查 `Table::Join` 的键标志回填是否仍正确，尤其是组合 unique 只有首列标 `MultipleKeyFlag` 的 MySQL 兼容规则。
- 改动缺失列策略时，要同时覆盖 `CompareWithNil` 和 `JoinWithNil`；回归至少应包含可空列、有默认值列、无默认值 `NOT NULL` 列及各种标准默认值类型。
- 改动索引 join 时，要覆盖同名不同列、列顺序、前缀长度、单列/组合 primary、unique、普通索引，以及索引被删除后 `AUTO_INCREMENT` 报错。
- 改动 SQL 恢复时，要保留列/索引确定性排序，并明确它仍是调试表示还是升级为可执行、无损 DDL；后者需要系统处理默认值 quoting、当前未输出的 tuple 维及写入错误。
- Rust 单元测试必须继续放在独立的 `table_test.rs`（或同目录其他独立测试文件），不要嵌入 `table.rs`。若行为来自 Go 移植，应同步比较 `table_test.go`，避免为了通过 Rust 测试而缩减 Go 逻辑。

主要风险是固定 tuple 下标错位造成运行时 downcast panic；其次是 join 后索引与列键标志不一致，导致错误接受/拒绝自增列；兼容性风险集中在字符集、默认值、字段类型和未建模表属性；性能风险主要来自大表上的克隆和恢复排序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/schemacmp` 显示目标、Go 对照和独立测试均已索引；`node --file pkg/util/schemacmp/table.rs` 完整读取 599 行源码并报告 47 个符号；`node EncodeWithOptions` 确认 `Encode -> EncodeWithOptions -> encodeTableInfoToLattice` 调用链。限定名 callers/callees 查询无额外输出，因此文件外调用点用精确仓库搜索补证。
- 源码：`pkg/util/schemacmp/table.rs`，重点符号为三个编码函数、`ColumnMap`/`IndexMap` 的 `LatticeMap` 实现、`TableOptions`、`Encode*`、`DecodeColumnFieldTypes`、`Table::{Restore,Compare,Join,String}`。
- crate 边界：`pkg/util/schemacmp/Cargo.toml`、`pkg/util/schemacmp/lib.rs`、根 `Cargo.toml` 和 `pkg/lib.rs`；它们证明路径依赖、模块导出、workspace 归属及门面再导出。
- Go 对照：`pkg/util/schemacmp/table.go`；核对了 tuple 布局、缺项规则、索引 join、键标志修复、错误和恢复流程。
- 独立测试：`pkg/util/schemacmp/table_test.rs`、`pkg/util/schemacmp/charset_collation_1_aster_unit_test.rs`、`pkg/util/dbutil/table_test.rs`，并与 `pkg/util/schemacmp/table_test.go` 的案例清单和断言方式对照。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰有十一个固定二级标题，并人工复核唯一新增生产物、源文件链接、事实边界和扩展测试位置。
