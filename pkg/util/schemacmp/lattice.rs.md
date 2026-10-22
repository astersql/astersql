# `pkg/util/schemacmp/lattice.rs`

## 文件定位

提供组合格：逐维组合的 `Tuple`、加入空值底元素的 `maybe`、按前缀包含比较的 `StringList`，以及把业务自定义字符串键 map 接入格运算的 `latticeMap`。

## 核心职责

- 用 `Lattice` trait 统一三项操作：`Unwrap` 取回业务值，`Compare` 返回 `-1/0/1` 表示偏序方向，`Join` 求两元素的上确界；动态类型不一致或不存在共同兼容关系时返回 `IncompatibleError`。
- 提供基础格：`Bool`、宏生成的 `Byte/Int/Int64/Uint`、`BitSet`、`fieldTp`，以及只允许相等值的 `singleton`/`equalitySingleton`。
- 提供组合格：逐维组合的 `Tuple`、加入空值底元素的 `maybe`、按前缀包含比较的 `StringList`，以及把业务自定义字符串键 map 接入格运算的 `latticeMap`。
- 保留 Go 动态值与错误契约：`AnyValue` 支持克隆、同类型相等、向下转型和 `nil`；错误常量及包装函数保留类型、元组位置和 map 键上下文。

## 主要符号

- `AnyValue(Option<Box<dyn DynValue>>)`：类型擦除容器。`new` 装箱满足 `Any + Clone + PartialEq + Debug` 的值，`nil` 表示 Go `nil`，`downcast_ref` 供解码层恢复具体类型。其相等语义要求两侧动态类型和值均相同。
- `IncompatibleError { Msg, Args }`：唯一业务错误类型。`Display` 根据 `ErrMsg*` 模板生成可读信息；`wrapTupleIndexError`、`wrapMapKeyError` 增加组合结构中的精确失败位置。
- `Lattice`、`LatticeRef`、`LatticeBox`：对象安全的格接口及借用/所有权别名。`as_any/as_any_mut` 支持运行时向下转型，`clone_box` 使 trait object 可克隆。
- `Bool` 与 `ordered_lattice!`：`Bool` 定义 `false < true` 且 join 为逻辑或；宏为 `Byte(u8)`、`Int(isize)`、`Int64(i64)`、`Uint(usize)` 生成全序比较和取最大值的 join。
- `Singleton`、`EqualitySingleton`：前者用 `PartialEq`，后者通过调用方实现的 `Equality::Equals`；只有相等值可比较或 join，否则返回 `ErrMsgDistinctSingletons`。
- `BitSet`：按位集合包含关系形成偏序；互不包含时比较失败，但 join 始终为按位或。
- `FieldTp`/`fieldTp`：仅在 MySQL 整数族或 BLOB 族内部按宽度比较并取较宽类型，完全相同的其他类型也相等；跨族或不同的其他类型返回 `ErrMsgIncompatibleType`。
- `Tuple` 与 `CombineCompareResult`：逐维运算。比较方向可以由 `0` 向 `-1` 或 `1` 收敛，但已有非零方向与新方向相反时返回 `ErrMsgContradictingOrders`。
- `Maybe`、`MaybeSingletonInterface`、`MaybeSingletonString`：在任意内层格下增加 `None` 通用底元素；字符串便捷构造把空串解释为缺失值。
- `StringList`：只有公共前缀逐项相同才可比较，较长列表更大，join 取较长者。
- `LatticeMap`、`Map`、`latticeMap`：把具体 map 的创建、插入、遍历、缺失键策略和不兼容 join 策略抽象出来，再对两侧键并集执行组合格运算。

## 执行流程

1. 上层先把 schema 属性编码成本文件提供的格元素。例如 `type.rs::encodeFieldTypeToLattice` 构造九维 `Tuple`；`table.rs::encodeTableInfoToLattice` 再组合排序规则、列 `Map`、索引 `Map`、计数和可选压缩选项。
2. `Compare` 首先检查另一元素的动态 Rust 类型。类型匹配后，基础格直接比较；`Tuple` 和 `latticeMap` 则递归比较子元素。
3. `Tuple::Compare` 逐维调用子元素 `Compare`，再用 `CombineCompareResult` 合并方向。任一维失败或不同维产生相反方向，错误会附加 tuple 下标。
4. `latticeMap::Compare` 由 `iter` 遍历两侧键并集：共有键比较值，单侧键交给具体 `LatticeMap::CompareWithNil`，右侧独有键的结果取反；各键方向同样通过 `CombineCompareResult` 合并并附加键名。
5. `Join` 对基础全序格取较大值，对 `BitSet` 取并集，对 `Tuple` 逐维 join。`Maybe` 在一侧为空时返回另一侧，两侧非空时递归 join；`StringList` 验证前缀后取较长列表。
6. `latticeMap::Join` 新建同类 map，遍历键并集：共有键调用值的 `Join`，缺失键调用 `JoinWithNil`。结果为 `Some` 才插入；若 join 失败且 `ShouldDeleteIncompatibleJoin` 为真则丢弃该键，否则包装键名并向上传播。
7. 上层解码 join 结果。`type.rs::decodeFieldTypeFromLattice` 恢复字段类型，`table.rs::Table::Join` 再按保留下来的索引回填列键标志并校验 `AUTO_INCREMENT` 约束。

## 数据与状态

所有基础格值以及 `Tuple`、`maybe`、`latticeMap` 都是按值拥有的数据；`Join` 返回新的 `LatticeBox`，不会就地修改输入。动态对象通过 `clone_box` 深度复制其外层和已装箱子值。`AnyValue` 的 `None` 与 `maybe(None)` 分工不同：前者是解包后的 Go `nil` 表示，后者是仍可参与格运算的底元素。

关键不变量包括：`Compare` 的合法结果仅为 `-1/0/1`；同类全序格的 join 是最大值；`Tuple` 两侧长度必须相同；`StringList` 可比较时短列表必须是长列表前缀；`latticeMap::iter` 借助 `HashSet` 保证两侧共有键只处理一次。具体 map 决定缺失键含义：`table.rs::ColumnMap` 保留可补默认值的列并传播不兼容错误，而 `IndexMap` 在 join 中删除单侧或不兼容索引。

## 依赖与调用关系

下游依赖仅有 crate 再导出的 `mysql` 和 `types`：`fieldTp` 使用 `mysql::IsIntegerType`、类型常量及 `types::IsTypeBlob` 判断两个有序类型族；其余实现使用标准库的 `Any`、`HashMap`、`HashSet` 和格式化接口。文件没有 I/O、网络或数据库依赖。

主要上游位于同 crate：

- `type.rs::encodeFieldTypeToLattice` 调用 `FieldTp`、`Singleton`、`Maybe` 并构造 `Bool/Byte/Int/StringList/Tuple`；`typ::Compare/Join` 直接委托给编码后的 `Tuple`。
- `table.rs::encodeColumnInfoToLattice` 和 `encodeIndexInfoToLattice` 使用 `MaybeSingletonInterface`、`Singleton`、`EqualitySingleton`、`Tuple`；`encodeTableInfoToLattice` 使用 `Map`、`Int64`、`MaybeSingletonString` 组成表级 `Tuple`。
- `table.rs::ColumnMap` 与 `IndexMap` 实现 `LatticeMap`，分别表达缺失列与缺失索引的不同规则；`Table::Compare/Join` 是用户可见的表 schema 运算入口。
- `lib.rs` 公开再导出 `lattice::*`，同时用独立的 `lattice_test.rs` 注册本模块测试。

RustCodeGraph 对 `lattice.rs` 的文件节点记录了同 crate 使用关系；精确 `callers/callees` 查询对动态 trait 调用没有返回静态边，因此上述调用关系以索引读取到的 `type.rs`、`table.rs` 直接调用点为证据，不把动态分派推断成静态调用图结论。

## 错误处理与边界

错误分三层。第一层是动态类型或值域错误：不同格实现相互比较得到 `ErrMsgTypeMismatch`，单点值不同得到 `ErrMsgDistinctSingletons`，字段类型族不兼容得到 `ErrMsgIncompatibleType`。第二层是偏序本身不成立：互不包含的 `BitSet`、前缀元素不同的 `StringList`、方向矛盾的多维/多键比较分别返回专用错误。第三层是组合上下文：`Tuple` 和 `latticeMap` 将内层错误包装为具体下标或键。

需要特别注意的边界：`BitSet::Compare` 可能失败而 `Join` 仍可通过并集成功；`latticeMap::Join` 可按具体 map 策略吞掉某个键的不兼容错误；`latticeMap::Unwrap` 与 Go 版本一致，忽略 `ForEach` 返回的错误，因此实现者不应把正常解包可能失败的逻辑放入 `ForEach`。多个解码点依赖 `downcast_ref(...).expect/unwrap`，错误的自定义实现或不一致的 `Unwrap` 类型会 panic，这属于内部类型契约违例而非可恢复 schema 不兼容。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或外部资源。所有运算均同步发生在调用线程中；临时 `Vec`、`HashMap`、`HashSet` 和 trait object 由 Rust 所有权自动释放。trait 未声明 `Send`/`Sync`，因此不能据此假设格对象可跨线程共享；如上层需要并发，应在其边界自行建立线程安全约束。

递归 join 的结果拥有独立装箱值。`Tuple::Join` 先收集全部子结果，任一维失败即丢弃已构造的临时结果；`latticeMap::Join` 类似地在局部新 map 中累积，错误返回时由析构释放，输入保持不变。

## 与 Go 版本的对应关系

语言适配差异主要是：Go 用 interface/nil 和类型断言，Rust 用 `Box<dyn Lattice>`、`Option` 与 `Any` 向下转型；Go 的 `any` 由 `AnyValue` 模拟；Go map 的动态实现由对象安全的 `LatticeMap` trait 封装。错误显示文本手工覆盖已知模板，并非通用 Go `fmt` 模板解释器。当前 `fieldTp` 的整数/BLOB 顺序与 Go 一致，且两边都明确只支持这些特殊可比族。

## 扩展指南

- 新增基础格时，应实现全部 `Lattice` 方法：先验证动态类型，保证 `Compare` 只返回三态，验证 `Join` 确为双方上界，并让 `Unwrap` 类型与上层解码完全一致；同时在独立的 `lattice_test.rs` 增加正向、反向、相等、不兼容、join 交换性和上界断言。
- 扩展 `FieldTp` 的可比类型族时，必须同步修改 `fieldTp::Compare/Join` 的排序依据，核对 `mysql`/`types` 分类函数，并与 `lattice.go` 及 Go/Rust 两份兼容性测试保持一致。不能只让 join 成功而遗漏比较顺序。
- 新增组合维度时，优先复用 `Tuple`；必须保持两侧维数和固定下标一致，并在上层编码、解码和错误定位中同步更新。若维度可能缺失，应明确选择 `Maybe`，不要用裸 `AnyValue::nil` 参与格运算。
- 新增 map 实现时，重点定义 `CompareWithNil`、`JoinWithNil`、`ShouldDeleteIncompatibleJoin` 三者的一致语义，并保证 `New` 返回相同具体类型、`Insert/Get/ForEach` 的值类型与 `Unwrap` 契约相符。列/索引语义不同，不能机械复用删除策略。
- 测试逻辑应继续放在同目录独立测试文件 `pkg/util/schemacmp/lattice_test.rs`，不要嵌入生产源文件；若修改 Go 对照语义，还应同步 `lattice_test.go`。兼容风险集中在错误类别/上下文、动态解包类型及空值表示；性能风险集中在 trait-object 克隆、每次 tuple/map join 的新分配和 map 键字符串复制。
