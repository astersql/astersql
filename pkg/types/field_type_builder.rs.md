# `pkg/types/field_type_builder.rs`

## 文件定位

本文件是 `astersql-types` 类型 crate 中 `FieldType` 的链式建造器实现。它本身不声明模块依赖；`pkg/types/internal/metadata/lib.rs` 的 `field_type_builder_defs` 模块先把 `crate::FieldType` 引入作用域，再用 `include!("../../field_type_builder.rs")` 编译本文件，并以 `pub use field_type_builder_defs::*` 对外导出。`pkg/types/Cargo.toml` 表明顶层包名为 `astersql-types`，同时通过 `types-group-4 = astersql-types-metadata` 组织这组元数据实现；本文件实际操作的 `FieldType` 最终是 `pkg/parser/types/field_type.rs` 中的结构。

它位于“收集字段类型参数”与“消费完整 `FieldType`”之间：调用方逐项设置 MySQL 类型码、标志、长度、小数位、字符集、排序规则、ENUM/SET 元素或数组标记，最后取得值快照或内部值的可变借用。当前 Rust 生产调用的直接证据是 `pkg/util/schemacmp/type.rs::decodeFieldTypeFromLattice`，该函数用建造器把格编码还原为 `FieldType`；`pkg/util/schemacmp/lib.rs` 则从 `tidb_types` 再导出 `NewFieldTypeBuilder`。

## 核心职责

- `FieldTypeBuilder` 只持有一个私有 `ft: FieldType`，集中转发 `FieldType` 的常用读写 API，避免调用方先构造临时对象再逐句修改。
- `NewFieldTypeBuilder` 以 `FieldType::default()` 初始化并返回 `Box<FieldTypeBuilder>`，对应 Go 构造器返回指针、支持连续修改的使用形状。这里的默认 `flen` 和 `decimal` 是派生 `Default` 的零值；它不同于 `pkg/parser/types/field_type.rs::new_field_type` 所使用的 `UnspecifiedLength` 初始化，调用方若需要 `-1` 必须显式设置。
- 六个读取方法反映当前内部状态；十二个设置方法修改内部 `FieldType` 后返回 `&mut Self`，形成链式调用。
- `Build` 克隆并返回独立值快照；`BuildP` 返回内部值的可变借用，供需要继续原地修改或按引用传递的调用方使用。二者的所有权语义是本文件最重要的 Rust 边界。

本文件不负责校验类型组合是否合法，也不填充某个 MySQL 类型应有的默认字符集、长度或精度；这些策略属于 `FieldType` 的构造/推断逻辑或上层调用方。

## 主要符号

- `pub struct FieldTypeBuilder { ft: FieldType }`：唯一状态容器；`ft` 私有，外部只能通过建造器方法或 `BuildP` 访问。
- `pub fn NewFieldTypeBuilder() -> Box<FieldTypeBuilder>`：公开入口。创建堆分配建造器，内部字段来自 `FieldType::default()`。
- 只读转发：`GetType() -> u8`、`GetFlag() -> usize`、`GetFlen() -> isize`、`GetDecimal() -> isize`、`GetCharset() -> &str`、`GetCollate() -> &str`。字符串结果借用自内部 `FieldType`，不能比建造器活得更久。
- 标量与标志设置：`SetType(u8)`、`SetFlag(usize)`、`AddFlag(usize)`、`ToggleFlag(usize)`、`DelFlag(usize)`、`SetFlen(isize)`、`SetDecimal(isize)`。标志操作分别是覆盖、按位或、按位异或和清位，实际位运算由 `pkg/parser/types/field_type.rs::FieldType` 完成。
- 拥有型字段设置：`SetCharset(String)`、`SetCollate(String)`、`SetElems(Vec<String>)`。参数所有权被移入内部对象；建造器不额外复制这些参数。
- `SetArray(bool)`：转发数组包装标记。需要注意 `FieldType::GetType` 在数组标记为真时对外报告 `mysql::TypeJSON`，而 `FieldType::SetType` 会清除已有数组标记，因此链式调用顺序会影响结果。
- `Build(&self) -> FieldType`：调用 `self.ft.clone()`，包括字符串和可选元素向量在内均形成独立快照。
- `BuildP(&mut self) -> &mut FieldType`：直接返回 `&mut self.ft`；它不分配新 `FieldType`，也不把值移出建造器。

本文件没有模块级常量、trait、自由辅助函数（构造器除外）或条件编译项。

## 执行流程

典型流程如下：

1. `NewFieldTypeBuilder` 创建默认 `FieldType` 并包入建造器。
2. 调用方按需要调用一组 `Set*`/标志方法。每次调用先修改 `ft`，随后返回同一个建造器的 `&mut Self`，所以后续调用观察到累积状态。
3. 可在链中或链后用 `Get*` 检查当前值；这些方法没有缓存，直接读取 `ft`。
4. 需要稳定结果时调用 `Build`。之后继续修改建造器不会反向改变先前快照；`pkg/types/enum_4_aster_unit_test.rs::field_type_builder_forwards_all_fields_and_buildp_aliases_builder_field` 明确验证了这一点。
5. 需要操作建造器内部对象时调用 `BuildP`。返回借用上的修改会立刻反映到建造器；同一测试通过借用调用 `SetType` 并再次读取建造器验证了别名关系。

真实生产流程见 `pkg/util/schemacmp/type.rs::decodeFieldTypeFromLattice`：它先从编码元组计算标志、字符集和排序规则，再链式设置类型、长度、小数位、标志、字符集与排序规则；仅当原始元素切片存在时调用 `SetElems`，最后以 `Build` 取得拥有型结果。这一条件设置保留了 `FieldType.elems` 的 `None` 与 `Some(empty)` 区别。

## 数据与状态

建造器唯一的可变状态是 `ft`，没有派生字段或额外不变量。底层 `FieldType`（`pkg/parser/types/field_type.rs::FieldType`）保存 `tp`、`flag`、`flen`、`decimal`、`charset`、`collate`、可选 `elems`、可选二进制元素标记以及 `array`。本建造器覆盖其中常用字段，但没有暴露二进制元素标记等全部 API。

状态更新的关键规则来自被转发的 `FieldType` 方法：`SetFlag` 覆盖全部位，`AddFlag`/`ToggleFlag`/`DelFlag` 分别执行 `|`、`^`、`& !`；`SetFlen` 与 `SetDecimal` 不做范围截断；`SetElems` 把 `elems` 设为 `Some`；`SetType` 同时把 `array` 清为 `false`。因此扩展或重排链式调用时，不能把这些方法当作完全彼此独立的简单字段赋值。

`Build` 的克隆成本与字符串及元素向量长度相关；`BuildP` 无克隆成本，但其返回值受建造器生命周期约束。`BuildP` 的可变借用存活期间，Rust 不允许再次借用或使用建造器，从类型系统层面阻止同时修改造成的数据竞争。

## 依赖与调用关系

- 模块装配：`pkg/types/internal/metadata/lib.rs::field_type_builder_defs` 提供 `FieldType` 名称、包含本文件并公开再导出。`pkg/types/Cargo.toml` 将 metadata 内部 crate 纳入 `astersql-types` 依赖图。
- 下游依赖：所有方法仅调用 `FieldType::default`、`clone` 以及 `pkg/parser/types/field_type.rs::FieldType` 的同名 getter/setter；无 I/O、网络、时钟或全局状态依赖。
- Rust 生产调用：`pkg/util/schemacmp/type.rs::decodeFieldTypeFromLattice` 使用 `types::NewFieldTypeBuilder` 恢复 schema 比较所需的规范化字段类型；`pkg/util/schemacmp/lib.rs::types` 负责再导出入口。
- Rust 测试调用：`pkg/types/enum_4_aster_unit_test.rs::field_type_builder_forwards_all_fields_and_buildp_aliases_builder_field` 是 RustCodeGraph 为 `NewFieldTypeBuilder` 识别出的直接调用者，并覆盖全部建造器设置方法、读取方法及两种构建结果。
- Go 应用范围更广：`pkg/types/field_type.go` 的构造函数、`pkg/expression/builtin.go`、`pkg/expression/expr_to_pb.go`、`pkg/server/handler/tikvhandler/tikv_handler.go` 和 `pkg/util/schemacmp/type.go` 等均消费同名 Go API。它们是迁移意图证据，不应误写成当前 Rust 建造器已经在所有对应 Rust 主链中接线。

RustCodeGraph 已索引目标文件并报告 22 个符号；精确 `node` 查询确认了结构体、构造器源码及测试调用边。当前索引对 impl 内大写方法名未能以 `FieldTypeBuilder::Build` 等限定名解析，所以上述方法下游和额外调用点以已索引源码、模块入口与 `rg` 结果交叉核对。

## 错误处理与边界

本文件的所有 API 都是非 `Result`、非 `Option` 的直接操作，没有主动错误返回、panic 分支或恢复逻辑。它也不检查类型码、长度、小数位、字符集/排序规则组合以及 ENUM/SET 元素是否有效；无效组合可以被构造，合法性必须由上层或后续 `FieldType` 校验承担。

主要边界包括：默认构造得到零值而非 `UnspecifiedLength`；`SetFlen`/`SetDecimal` 接受任意 `isize`；`SetCharset`/`SetCollate` 接受任意字符串；标志方法接受任意位图；`SetType` 会清除 array 标记；array 为真时 `GetType` 返回 JSON 而非底层 `tp`。`BuildP` 不能脱离建造器生命周期，也不能与同一建造器的其他活跃可变借用并存；这是编译期边界，不是运行时错误。

`Build` 依赖 `FieldType: Clone`，当前派生实现会深复制拥有型字段。若未来 `FieldType` 引入共享或外部资源，必须重新审查“快照独立”这一契约和测试，不能只依赖方法名推断。

## 并发与资源生命周期

建造器没有锁、原子量、通道、任务、事务或后台资源；所有操作都是调用线程上的同步内存修改。单个实例要求 `&mut self` 才能变更，Rust 借用规则保证安全代码中不会发生同一实例的并发可变访问。是否可跨线程移动或共享完全由 `FieldType` 字段的自动 trait 决定，本文件没有显式承诺并发接口。

生命周期从 `Box<FieldTypeBuilder>` 创建开始。`SetCharset`、`SetCollate` 和 `SetElems` 把参数所有权交给内部对象；`Build` 产生可独立于建造器存活的克隆值；`BuildP` 只产生临时可变借用，建造器销毁后不能继续使用。无显式清理过程，离开作用域后由 Rust 自动释放 `Box`、字符串和向量。

## 与 Go 版本的对应关系

`pkg/types/field_type_builder.go` 是逐方法对照文件：结构体同样只含 `ft FieldType`，构造器返回指针，getter/setter 名称和顺序一致，设置方法返回接收者以支持链式调用，`Build` 返回值，`BuildP` 返回内部字段地址。Rust 将 Go 的 `byte`/`uint`/`int` 映射为 `u8`/`usize`/`isize`，将字符串和切片入参映射为拥有型 `String`/`Vec<String>`。

两端的关键语义差异来自语言所有权：Go 的 `Build` 按结构体值复制，但切片底层存储可能仍共享；Rust 的 `Build` 调用派生 `Clone`，会深复制 `String` 和 `Vec`。Go 的 `BuildP` 指针可在建造器引用不可见后继续存活（由逃逸分析/GC 管理），Rust 的 `&mut FieldType` 必须受建造器生命周期约束。Rust 的 `Box` 对齐“构造器在堆上”的使用形状，但链式方法返回的是借用而非新的所有权指针。

Go 回归证据包括 `pkg/types/datum_test.go` 中用 `BuildP` 构造 DECIMAL 类型并验证精度/截断行为，以及 `pkg/types/etc_test.go` 中用 `Build` 构造字符串类型并验证 `NeedRestoredData`。Rust 的独立测试更直接覆盖建造器自身；这些 Go 测试验证消费语义，但其业务断言不属于本文件内部逻辑。

## 扩展指南

新增字段设置入口时，应先确认 `pkg/parser/types/field_type.rs::FieldType` 已有对应公开方法，再在 `FieldTypeBuilder` 中做最薄的转发并返回 `&mut Self`。若对齐 Go 新 API，应同步检查 `pkg/types/field_type_builder.go` 的签名、零值和指针/值语义；不能在建造器一侧擅自加入校验、默认值或范围截断，否则会改变 Go 对齐行为。

修改 `Build` 或 `BuildP` 时必须维持两项独立契约：前者是后续修改不影响的拥有型快照，后者是直接别名内部字段的可变借用。相应回归应扩展独立测试文件 `pkg/types/enum_4_aster_unit_test.rs`，不要把测试嵌入本生产文件。新增 `FieldType` 字段时还要检查建造器是否需要 getter/setter、`Clone` 是否仍满足快照语义，以及 `pkg/util/schemacmp/type.rs::decodeFieldTypeFromLattice` 是否需要参与编码/解码。

兼容风险主要是默认值、链式顺序和 Go/Rust 数值宽度差异；正确性风险集中在标志位覆盖/增删与 array/type 联动；性能风险集中在 `Build` 对长元素列表的深克隆。只有在生命周期允许且调用方确实需要原地借用时才选择 `BuildP`，不要为了避免克隆而扩大可变借用范围。

## 验证依据

- 目标源码：`pkg/types/field_type_builder.rs`，核对 1 个结构体、1 个公开构造函数、6 个 getter、12 个链式 setter/flag 操作以及 `Build`/`BuildP`；未发现常量、trait 或条件编译项。
- 模块与 crate：`pkg/types/internal/metadata/lib.rs`、`pkg/types/Cargo.toml`、`pkg/util/schemacmp/lib.rs`。
- 底层类型与转发语义：`pkg/parser/types/field_type.rs::FieldType` 及其 `Default`/`Clone` 派生、getter/setter 实现。
- Rust 生产调用：`pkg/util/schemacmp/type.rs::decodeFieldTypeFromLattice`。
- Rust 独立测试：`pkg/types/enum_4_aster_unit_test.rs::field_type_builder_forwards_all_fields_and_buildp_aliases_builder_field`。
- Go 对照与测试：`pkg/types/field_type_builder.go`、`pkg/types/field_type.go`、`pkg/types/datum_test.go`、`pkg/types/etc_test.go`；另以生产调用搜索确认 Go API 的使用范围。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/types/field_type_builder.rs` 确认目标在图中且有 22 个符号；`query FieldTypeBuilder`/`query NewFieldTypeBuilder` 同时定位 Go/Rust 定义；`node pkg/types/field_type_builder.rs::FieldTypeBuilder` 与 `node pkg/types/field_type_builder.rs::NewFieldTypeBuilder` 核对源码，并由构造器 trail 得到 Rust 测试调用边。`explore` 和部分 `callers/callees` 组合查询达到 30 秒预算，impl 方法限定名也未被解析，因此未把图中缺失误判为无调用。
- 结构检查按任务命令验证本文恰好包含 11 个固定二级章节；本任务只新增文档，按计划不运行 Cargo。
