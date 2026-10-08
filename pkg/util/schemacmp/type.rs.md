# `pkg/util/schemacmp/type.rs`

## 文件定位

本文件属于 `astersql-util-schemacmp` crate（见 `pkg/util/schemacmp/Cargo.toml`），由 `lib.rs` 以 `mod r#type; pub use r#type::*;` 纳入并公开。它位于表级 schema 格实现的列类型层：`table.rs::encodeColumnInfoToLattice` 调用 `Type`，把 `model::ColumnInfo.FieldType` 包装成实现 `Lattice` 的 `typ`，随后表的 `Compare`/`Join` 间接使用这里的列类型比较和合并规则。

crate 通过工作区依赖 `meta-model`、`parser-types` 和 `tidb-types` 获得表元数据、MySQL 类型/字符集定义以及 `FieldType` builder；工作区根 `Cargo.toml` 将该 crate 登记为 `facade_util_schemacmp`，`pkg/lib.rs` 再通过 facade 公开。当前检索到的直接生产调用集中在同 crate 的 `table.rs`，外部 Rust 使用点主要通过表级 `Encode`/`Table` API，而不是直接操作 `typ`。

## 核心职责

1. 用 `encodeFieldTypeToLattice` 将一个 `types::FieldType` 拆成九维 `Tuple`：类型编号、长度、小数位、必须精确相等的其余 flag、可空性、反向编码的键 flag、默认值 flag、排序规则和 ENUM/SET 元素。
2. 用 `decodeFieldTypeFromLattice` 将上述九维结果重建为规范化的 `FieldType`，包括从 collation 推导 charset。
3. 以 `typ` 实现 `Lattice::Compare` 和 `Lattice::Join`，让列类型参与表结构兼容性判断与上确界合并。
4. 为表级缺失列处理提供 flag 调整和标准默认值；为表级索引重建提供键 flag 回写。
5. 拒绝 join 后仍带 `AUTO_INCREMENT`、却不属于任何键的非法列，错误文本由 `ErrMsgAutoTypeWithoutKey` 固定。

这里的“更大”表示 schema 更宽松或能容纳更多输入，并非数值或 SQL 类型编号更大。例如 nullable 列位于 NOT NULL 列之上，更长的普通整数显示宽度位于较短宽度之上，而 DECIMAL 的长度和标度被当作必须相等的单点值。

## 主要符号

- `flagMaskKeys`：合并 `PriKeyFlag`、`UniqueKeyFlag`、`MultipleKeyFlag`，限定所有键相关变换的位范围。
- `flagMaskDefVal`：合并 `AutoIncrementFlag` 与 `NoDefaultValueFlag`，构成默认值语义维。
- `notPartOfKeys`：反键编码的全 1 哨兵，表示列不属于任何键。
- `fieldTypeTupleIndex*`：九个 tuple 下标；其顺序必须与 `encodeFieldTypeToLattice` 构造顺序、`decodeFieldTypeFromLattice` 读取顺序同步。
- `ErrMsgAutoTypeWithoutKey`：`typ::Join` 和 `table.rs::Table::Join` 共用的非法自增列错误消息。
- `encodeAntiKeys` / `decodeAntiKeys`：只处理键 flag，先掩码、反转位序再按位取反，使“无键约束”成为较大值，并保持普通索引、唯一索引、主键的 Go 顺序；解码执行逆变换。
- `encodeFieldTypeToLattice`：私有编码入口。`TypeNewDecimal` 的 `flen`/`decimal` 使用 `Singleton`，其他类型使用可排序的 `Int`；默认值维以 `Maybe<Singleton<_>>` 表示“存在默认值语义”或“无默认值”。
- `decodeFieldTypeFromLattice`：私有解码入口。合成各组 flag，通过 collation 第一个下划线前的部分推导 charset，再经 `Charset` 规范化（测试覆盖大小写和 `utf8mb3` 到 `utf8`）；`elems_present` 用来保持 Rust `Option<Vec<_>>` 的存在性。
- `downcast_value<T>`：按 tuple 下标从 `AnyValue` 取出确定类型；它把内部布局错误视为不可恢复的实现违约。
- `typ`：持有规范化 `types::FieldType` 的列类型格包装；`Clone` 后不共享可变状态。
- `Type`：公开构造函数，立即执行一次编码/解码 round-trip，使输入进入本模块的规范表示。
- `typ::{hasDefault,setFlagForMissingColumn,isNotNull,inAutoIncrement,setAntiKeyFlags}`：供 `table.rs` 判断缺失列是否合法、清除缺失侧的键/无默认值约束，以及按 join 后索引回写键属性。
- `typ::getStandardDefaultValue`：按 MySQL 类型生成 Go 兼容的缺失列默认值；数值为字符串 `"0"`，时间类型保留小数位零尾，JSON 为 `"null"`，向量为 `"[]"`，ENUM 取首元素，binary CHAR 生成 `flen` 个零字节，其余为空串。
- `typ::getStandardDefaultModelValue`：把上述值转换成 Rust 元数据模型的 `model::DefaultValue`；数值族使用 `Int(0)`，其他类型使用字节串。
- `impl Lattice for typ`：`Unwrap` 返回克隆的 `FieldType`；`Compare`/`Join` 只接受同为 `typ` 的对象；`clone_box` 支持 trait object 克隆。

## 执行流程

构造与比较流程如下：

1. `table.rs::encodeColumnInfoToLattice` 对每列调用 `Type(&column.FieldType)`。
2. `Type` 调用 `encodeFieldTypeToLattice`，把 `FieldType` 投影到九个彼此独立的格维度，再调用 `decodeFieldTypeFromLattice` 保存规范化结果。
3. `typ::Compare` 将左右 `field_type` 临时重新编码，并委托 `Tuple::Compare` 逐维比较。任一维不可比较时，底层错误携带 tuple 下标和具体原因向上传播。
4. `typ::Join` 同样重新编码，委托 `Tuple::Join` 取得逐维上确界；随后选择较长元素列表一侧的 `GetElemsOption()` 状态，解码为 `FieldType`。
5. 解码后若仍为 `AUTO_INCREMENT` 且反键编码等于 `notPartOfKeys`，返回 `ErrMsgAutoTypeWithoutKey`；否则返回新的 boxed `typ`。

表级 join 还有两条直接路径：

- 当列仅存在于一侧时，`ColumnMap::JoinWithNil` 调用 `setFlagForMissingColumn`，清除键 flag 和 `NoDefaultValueFlag`；若原列无默认值且 NOT NULL，则调用 `getStandardDefaultModelValue` 写入可落盘的标准默认值。
- 表及索引完成通用 join 后，`Table::Join` 从保留下来的索引重新计算每列键 flag；无键的自增列立即报错，其他列调用 `setAntiKeyFlags` 覆盖旧键 flag。这避免列 flag 与最终索引集合不一致。

## 数据与状态

`typ` 唯一持久状态是拥有所有权的 `types::FieldType`。编码产生的 `Tuple` 仅在构造、比较和 join 时临时存在；没有全局缓存或隐藏注册表。`Unwrap` 返回克隆值，因此调用方修改解包结果不会反向改变 `typ`。

九维 tuple 的主要不变量是：

- `TypeNewDecimal` 的精度与标度必须完全一致；其他类型的 `flen`/`decimal` 可由 `Int` 选择较大值。
- 非默认值、可空性、键属性以不同维度表示，避免无关 flag 干扰各自偏序。
- 非键、非默认值、非 NOT NULL 的剩余 flag 使用 `Singleton`，不同值不允许合并。
- collation 决定解码后的 charset；输入 `FieldType.Charset` 不作为独立 tuple 维保存。
- ENUM/SET 元素使用 `StringList` 的前缀顺序；join 选择能覆盖另一方的较长列表。
- `elems_present` 区分 `None` 和 `Some(empty)`。`Join` 按元素长度选择一侧的 Option 状态；等长时选择右侧，这与源码注释记录的 Go nil/non-nil slice 身份兼容意图一致。

## 依赖与调用关系

上游直接调用关系均可在 `pkg/util/schemacmp/table.rs` 核验：

- `encodeColumnInfoToLattice -> Type`：列元数据进入列类型格。
- `ColumnMap::CompareWithNil -> typ::hasDefault`：无默认值的列不能在另一张表中缺失。
- `ColumnMap::JoinWithNil -> typ::{setFlagForMissingColumn,isNotNull,getStandardDefaultModelValue}`：缺失列放宽约束并按需补默认值。
- `Table::Join -> typ::{inAutoIncrement,setAntiKeyFlags}`：根据最终索引集合校验自增列并重建键 flag。

下游依赖由 `use crate::{...}` 和 `Cargo.toml` 界定：

- `lattice.rs` 提供 `Lattice`、`Tuple`、`Singleton`、`Maybe`、`Bool`、`Byte`、`Int`、`StringList`、`FieldTp` 及 `IncompatibleError`。
- `charset_collation.rs` 提供 `Charset`、`Collation` 的比较、join 与规范化规则。
- `meta-model` 提供 MySQL flag、`model::DefaultValue` 和表元数据；`parser-types`/`tidb-types` 提供 `FieldType` 访问器和 builder。

RustCodeGraph 对 `encodeFieldTypeToLattice`、`Type`、`setFlagForMissingColumn`、`getStandardDefaultValue` 的精确查询找到了 Rust/Go 对应定义；本次 `callers`/`callees` 查询未返回这些方法的边，因此调用关系以同 crate 精确符号引用和源码路径补证，不推断跨 crate 动态调用。

## 错误处理与边界

- `Compare`/`Join` 遇到非 `typ` 的 `Lattice` 时返回 `typeMismatchError`，不会尝试跨类型转换。
- tuple 各维的不相容由底层格返回 `IncompatibleError`。现有测试覆盖不兼容 MySQL 类型、不同 collation 后缀、DECIMAL 精度/标度冲突、剩余 flag 冲突和默认值语义冲突。
- `Join` 显式拒绝无键的 `AUTO_INCREMENT`；`Table::Join` 在索引被删除或合并后再做一次表级校验，因为最终键集合只有表层知道。
- `downcast_value`、tuple/unwrap downcast 和 `Tuple::Join` 结果 downcast 使用 `expect`。这些 panic 表示内部九维布局或格实现违反静态约定，不是面向用户的兼容性错误。
- ENUM 标准默认值要求至少一个元素，空列表会在 `first().expect(...)` 处 panic；调用方若允许空 ENUM，必须在进入缺失列补值路径前验证。
- binary CHAR 默认值按 `flen as usize` 分配零字节；负数或异常巨大的 `flen` 不在本函数内验证，错误输入可能导致超大分配风险。
- 时间小数位只在 `decimal > 0` 时扩展；非正值不生成尾部。普通字符串、BLOB 等未单列类型统一返回空串。
- `getStandardDefaultModelValue` 假定非数值分支的 `getStandardDefaultValue` 返回 `String`；新增返回其他动态类型的分支时必须同步修改该转换，否则会 panic。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或 I/O。`Type`、`Compare`、`Join` 的临时 tuple 均为函数局部值；`Join` 返回新分配的 `Box<dyn Lattice>`，不修改输入。`setFlagForMissingColumn` 与 `setAntiKeyFlags` 需要 `&mut self`，其可变生命周期由 `table.rs` 在独占遍历单个列 tuple 时控制。

资源风险主要来自按 `decimal` 生成零尾、按 binary CHAR 的 `flen` 分配零字节，以及反复比较时重新编码 `FieldType`；均无跨调用持有资源。若将来引入缓存，需要保持 `typ` 克隆隔离和 trait object 使用下的线程安全边界，当前代码没有声明或依赖 `Send`/`Sync` 契约。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/schemacmp/type.go`，测试对照是 `type_test.go` 与 Rust 的 `type_test.rs`、`type_2_aster_unit_test.rs`。

核心语义保持一致：九维 tuple 顺序、反键 flag 顺序、DECIMAL 单点维、默认值 `Maybe`、charset/collation 与 ENUM/SET 规则、标准默认值，以及无键 `AUTO_INCREMENT` 错误文本均来自 Go 实现。Rust `type_test.rs::test_type_compare_join` 复刻 Go 的表驱动用例，并额外检查 join 交换性和上界性质；`type_2_aster_unit_test.rs` 补充编解码各维、`utf8mb3` 规范化、缺失列 flag、默认值与错误分支。

实现表示存在有意差异：Go 的 `typ` 内嵌并长期保存 `Tuple`，Rust 的 `typ` 保存规范化 `FieldType`，在每次 `Compare`/`Join` 时重新编码；因此 Rust 另有 `decodeFieldTypeFromLattice(..., elems_present)` 维护 `Option<Vec<String>>` 存在性。Go 的 `getStandardDefaultValue` 返回 `any`，Rust 保留 `Box<dyn Any>` 兼容接口，并新增 `getStandardDefaultModelValue` 将值适配为 `model::DefaultValue`，供 Rust `ColumnInfo` 使用。

Rust 的 `setFlagForMissingColumn` 直接修改 `FieldType.flag`；Go 版本直接替换 tuple 的反键和默认值维。两者的外部结果都清除键 flag、移除 `NoDefaultValueFlag` 并返回修改前是否无默认值，现有补充测试对此做了断言。

## 扩展指南

- 新增或改变 `FieldType` 比较维度时，必须同步修改 tuple 下标常量、`encodeFieldTypeToLattice`、`decodeFieldTypeFromLattice`、Go `type.go` 对应逻辑，以及两个独立 Rust 测试文件；不要把测试内嵌到 `type.rs`。
- 新增 MySQL 类型的标准默认值时，同时更新 `getStandardDefaultValue` 与 `getStandardDefaultModelValue`，明确其动态 Rust 类型、元数据编码和 Go 返回值。至少在 `type_2_aster_unit_test.rs` 增加具体值与边界测试。
- 调整键或默认值 flag 时，复核 `flagMaskKeys`、`flagMaskDefVal`、反键排序、`setFlagForMissingColumn` 和 `Table::Join` 的索引回写；自增列必须继续满足“属于某键”的不变量。
- 修改 collation 到 charset 的推导时，应同时检查 `charset_collation.rs`，并覆盖无下划线、前导下划线、大小写、`utf8mb3` 规范化和未知 charset。
- 修改 ENUM/SET join 时要保留元素前缀规则以及 `None`/`Some(empty)` 状态；新增空 ENUM 支持前，应先消除默认值路径的 panic。
- 性能优化若缓存 tuple，应测量表级大量列比较场景，并确保 `setFlagForMissingColumn`/`setAntiKeyFlags` 后缓存不会失效或与 `field_type` 分叉。
- 兼容风险集中在恢复后的 `FieldType` flag、charset/collation、默认值字节表示和错误文本；这些字段会影响 schema 比较结果及下游 SQL 恢复，不能仅用编译通过代替行为验证。

## 验证依据

- 目标源码：`pkg/util/schemacmp/type.rs`，核对全部常量、函数、`typ` 及 `Lattice` 实现；文件当前无条件编译分支。
- crate 边界与入口：`pkg/util/schemacmp/Cargo.toml`、`pkg/util/schemacmp/lib.rs`、工作区根 `Cargo.toml`、`pkg/lib.rs`。
- 直接生产调用：`pkg/util/schemacmp/table.rs` 的 `encodeColumnInfoToLattice`、`ColumnMap::{CompareWithNil,JoinWithNil}`、`Table::Join`。
- Go 对照：`pkg/util/schemacmp/type.go`；Go 测试：`pkg/util/schemacmp/type_test.go`。
- Rust 独立测试：`pkg/util/schemacmp/type_test.rs` 与 `pkg/util/schemacmp/type_2_aster_unit_test.rs`；前者覆盖 round-trip 和 Compare/Join 表格，后者覆盖迁移补充边界。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/util/schemacmp/type.rs --offset 1 --limit 500` 返回完整 318 行；精确 `query` 找到 Rust/Go 同名定义；`callers`/`callees` 对选定私有函数和方法无输出，故直接边由精确引用检索补证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证恰有十一个固定二级标题，并人工检查所有关键结论均可回溯到上述源码、调用点或测试。
