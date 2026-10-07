# `pkg/parser/types/field_type.rs`

## 文件定位

本文件实现 `astersql-parser-types` crate 的核心字段类型描述。crate 根 `pkg/parser/types/lib.rs` 在 `types::field_type` 模块中通过 `include!("field_type.rs")` 装入本文件，再经 `pub use field_type::*`、`pub use types::*` 公开，因此下游通常以 `parser_types::FieldType`、`types::NewFieldType` 等路径使用它。`pkg/parser/types/Cargo.toml` 表明该 crate 直接依赖 parser 的 `charset`、`format`、`mysql`、`terror`、`util` 子 crate，以及 `serde`/`serde_json`。

它位于 SQL 类型元数据链的基础层：解析和元数据代码构造 `FieldType`，表达式与执行器通过 `EvalType`、标志位和长度信息选择求值路径，AST/DDL 展示通过 `Restore`、`String`、`InfoSchemaStr` 重建类型文本。RustCodeGraph 将该文件识别为 73 个符号、被 162 个已索引文件引用；仓库搜索还找到 271 个含 `NewFieldType(` 的 Rust 文件，代表性生产调用包括 `pkg/expression/pb_to_expr_runtime.rs`、`pkg/expression/builtin.rs`、`pkg/meta/model/column.rs` 和 `pkg/ddl/create_table.rs`。

## 核心职责

- 以 `FieldType` 保存 MySQL/TiDB 列或表达式类型的完整元数据：底层类型码 `tp`、标志 `flag`、长度 `flen`、标度 `decimal`、字符集/排序规则、ENUM/SET 元素及数组包装标记。
- 提供构造、读取和原地修改 API，并维持数组类型的特殊视图：`GetType` 在 `array == true` 时对外返回 JSON，而 `RestoreAsCastType` 仍依据底层 `tp` 输出元素类型并追加 `ARRAY`。
- 将存储类型映射为表达式求值类别（`EvalType`），并提供严格等价、表达式等价及可选放宽长度的比较规则。
- 输出 information_schema、一般显示、AST 恢复和 CAST 目标类型文本，同时处理默认长度、整数显示宽度兼容开关、字符集和标志后缀。
- 计算固定/可变存储长度与近似内存占用，并用与 Go 中间结构同名的 JSON 键进行序列化。

本文件不负责解析 SQL 语法、验证所有类型组合或执行值转换；类型名映射、字符/二进制类型分类和默认长度来自同 crate 的 `etc.rs` 以及 `mysql` crate。

## 主要符号

- `UnspecifiedLength: isize = -1`：长度或标度未指定的哨兵。构造器和 `Init` 用它初始化 `flen`、`decimal`。
- `TiDBStrictIntegerDisplayWidth: static mut bool`：控制 `CompactStr` 是否隐藏已废弃的整数显示宽度。当前是未接入配置系统的全局可变静态量。
- `FieldType`：主数据结构。字段均为模块私有，调用者经 getter/setter 操作；派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`。
- `NewFieldType(tp)` / `FieldType::new_field_type`：创建指定底层类型，且将长度、标度置为未指定。Rust 返回值而非 Go 指针。
- `DeepCopy`、`Clone`、`Hash64`、`Equals`：复制及完整语义身份操作。`Hash64` 与 `Equals` 覆盖标量字段和两个元素数组的内容，但与 Go 一样不区分 nil 与非 nil 空数组；`Equals` 先用 `Any` 做动态类型检查。
- `Equal`、`PartialEqual`：面向表达式兼容性的比较。`Equal` 将 VARCHAR/VARSTRING 视为同类，只比较 unsigned 标志，按求值类型忽略部分 `decimal`/`flen`；`PartialEqual` 总是先比较 NotNull，在 `unsafe_compare` 且双方为字符串时进一步忽略 `flen` 和具体字符串类型码。
- `IsDecimalValid`、`SetFlenUnderLimit`、`SetDecimalUnderLimit`、`UpdateFlenAndDecimalUnderLimit`：DECIMAL 精度/标度检查与上限处理。setter 只做上限截断，不保证负值合法，最终有效性需由调用者或 `IsDecimalValid` 判断。
- `GetType`、`SetType`、`SetArray`、`IsArray`、`ArrayType`：维护底层类型与数组包装。`SetType` 会清除数组标记；Rust 的 `ArrayType` 总是返回克隆副本并清除标记。
- `SetElems`、`SetElemWithIsBinaryLit`、`GetElemIsBinaryLit`、`CleanElemIsBinaryLit`：维护 ENUM/SET 元素及惰性分配的二进制字面量位图。`Option<Vec<_>>` 用来区分 Go 的 nil slice 与已分配空 slice。
- `EvalType`、`Hybrid`、`IsVarLengthType`、`HasCharset`、`StorageLength`：类型分类与属性查询。ENUM/SET 在 `EnumSetAsIntFlag` 存在时映射为整数求值；ENUM/BIT/SET 被视为 hybrid。
- `CompactStr`、`InfoSchemaStr`、`String`、`Restore`、`RestoreAsCastType`、`FormatAsCastType`、`Display`：多种文本输出入口。`Display` 委托 `String`；`FormatAsCastType` 先写入内存缓冲再整体写入任意 `Write`。
- `jsonFieldType`、`MarshalJSON`、`UnmarshalJSON` 及 serde trait 实现：保证 JSON 键名为 Go 导出字段形式（如 `Tp`、`ElemsIsBinaryLit`），并在显式反序列化成功后才覆盖原对象。
- `MemoryUsage`、`emptyFieldTypeSize`、`VarStorageLen`：内存估算、结构本体大小以及可变存储长度哨兵。

## 执行流程

典型构造与消费流程如下：

1. 解析器、表达式或元数据层调用 `NewFieldType(tp)`，得到 `flen == decimal == UnspecifiedLength` 的对象；随后用 `SetFlen`、`SetDecimal`、`SetCharset`、`SetCollate`、标志位方法和元素方法补齐语义。`pkg/meta/model/column.rs`、`pkg/expression/builtin.rs`、`pkg/ddl/create_table.rs` 均有直接构造证据。
2. 求值路径调用 `EvalType`。整型码映射为 `ETInt`，浮点、DECIMAL、日期时间、JSON、向量分别映射到对应类别，其余默认为 `ETString`；例如 `pkg/expression/chunk_executor.rs` 和 `pkg/executor/select_into.rs` 根据该结果选择数据处理分支。
3. 比较或缓存路径按用途选择身份规则：需要完整哈希身份时使用 `Hash64`/`Equals`；判断表达式类型兼容时使用 `Equal`；仅在明确允许字符串长度不一致时使用 `PartialEqual(..., true)`。
4. 展示路径选择输出层级：`CompactStr` 产生紧凑类型名与必要长度；`InfoSchemaStr` 额外添加小写 `unsigned`；`String` 再添加大写标志和字符集/排序规则；`Restore` 按 AST 语法写入 `RestoreCtx`；CAST 使用 `RestoreAsCastType` 或其 writer 包装 `FormatAsCastType`。
5. 持久化或跨边界传输时，`MarshalJSON`/serde 将私有字段投影为 `jsonFieldType`；`UnmarshalJSON` 先完整解析临时值，成功后才替换 `self`，避免显式方法发生部分更新。

`CompactStr` 的关键分支是：先由 `TypeToStr` 获取类型名，并用 MySQL 默认值替换未指定长度/标度；ENUM/SET 转义元素；时间类型仅在非默认标度时显示精度；DECIMAL 总是显示精度和标度；整数是否显示宽度受全局严格模式、ZEROFILL 和 TINYINT(1) 特例共同控制。

## 数据与状态

`FieldType` 是可克隆的值对象，但多数 setter 需要独占 `&mut self`。`tp` 保存真实元素/标量类型；`array` 是独立包装状态，因此 `GetType()` 不总等于 `tp`。这一区分对 CAST 输出尤其重要：数组从外部分类为 JSON，却以底层 `tp` 生成元素类型文本。

`flag` 是 MySQL 标志位集合，文件只通过 `mysql::HasUnsignedFlag`、`HasNotNullFlag`、`HasZerofillFlag`、`HasBinaryFlag` 或位运算解释它。`flen` 和 `decimal` 允许 `-1` 哨兵；未调用 `IsDecimalValid` 时，结构本身可以暂时处于不合法 DECIMAL 状态。

`elems` 与 `elemsIsBinaryLit` 使用 `Option<Vec<_>>` 保留 nil/非 nil 状态。普通 getter 把 `None` 视为空切片，只有 `GetElemsOption` 暴露 nil 区别。二进制标记只在首次写入 `true` 时按元素数惰性分配；调用者必须保持两个数组的索引关系。

全局状态只有 `TiDBStrictIntegerDisplayWidth`。它影响类型字符串，不属于单个 `FieldType`，所以同一对象在开关变化前后可能产生不同的 `CompactStr`/`String` 结果。

## 依赖与调用关系

- crate 装配：`pkg/parser/types/lib.rs` 为本文件导入 `charset`、`format`、`mysql`、`util`，并将全部公开符号再导出；独立测试也由该 crate 根以 `#[path = "field_type_test.rs"]` 挂载。
- `mysql`：提供类型码、标志检测、DECIMAL 上限、默认长度/标度和默认字符集。`FieldType` 的分类与格式化规则都依赖这些常量保持稳定。
- `types::etc`（同 crate）：`TypeToStr`、`IsTypeChar`、`IsTypeBlob` 参与显示与字符集输出；`types::eval_type` 提供 `EvalType` 及 `ET*` 常量。
- `format`：`OutputFormat` 转义 ENUM/SET 的紧凑显示，`RestoreCtx` 提供带关键字/字符串语义的 SQL 恢复写入。
- `util::IHasher`：`Hash64` 的下游哈希接口。源码按字段固定顺序写入，并给两个数组先写长度，保证与 `Equals` 的完整身份范围相匹配。
- `serde`/`serde_json`：JSON 中间结构和显式方法的实现依赖；`std::io::Write` 是 CAST 格式化的最终输出抽象。
- 代表性上游：`pkg/expression/pb_to_expr_runtime.rs` 从协议类型码构造类型；`pkg/expression/builtin.rs` 为内建函数返回值构造/调整类型；`pkg/meta/model/column.rs` 把它放入列元数据；`pkg/ddl/create_table.rs` 为缺省列类型建立占位；`pkg/expression/chunk_executor.rs`、`pkg/executor/select_into.rs` 消费 `EvalType`。

RustCodeGraph 的文件级结果给出 162 个引用文件，但本次对 `NewFieldType`、`EvalType`、`Restore` 等方法运行 `callers/callees` 没有产生方法级结果；上面的具体边因此由仓库 `rg` 搜索补证，不把空图结果解释为“没有调用者”。

## 错误处理与边界

- `Restore`、`RestoreAsCastType`、`FormatAsCastType` 返回 `std::io::Result<()>`，每一步写入用 `?` 传播错误；与 Go 版本部分写入 API 不返回错误相比，Rust 调用者必须处理失败。
- `MarshalJSON`/`UnmarshalJSON` 返回 `serde_json::Error`。显式 `UnmarshalJSON` 只在完整解析成功后更新对象；派生 `Deserialize` 经同一中间结构构造新值。
- `SetElem`、`SetElemWithIsBinaryLit` 在 `elems == None` 时 `unwrap` panic，在索引越界时也会 panic；`GetElem` 同样要求合法索引。`GetElemIsBinaryLit` 对未分配或短标记数组返回 `false`，其容错行为与前三者不同，也比 Go 在非空短切片上直接索引更宽松。
- `StorageLength` 的 DECIMAL 分支假设 `flen`、`decimal` 已合法；负值或不一致值可能在数组索引转换处 panic。应先建立合法精度/标度或调用 `IsDecimalValid`，不能把 setter 的上限截断当作完整验证。
- `RestoreAsCastType` 对未列入 match 的底层类型不写类型关键字，但若 `array` 为真仍会写 ` ARRAY`；调用者应只传受支持的 CAST 目标类型。
- `DeepCopy(None)` 返回 `None`；`Equals` 对非 `FieldType` 的 `Any` 返回 `false`。Rust 的借用签名消除了 Go 非 nil 方法接收者上的一部分 nil 场景。

## 并发与资源生命周期

`FieldType` 本身没有锁、任务、通道、事务或外部句柄；字符串和向量由对象拥有，克隆后按 Rust 值语义独立释放。`FormatAsCastType` 的临时 `Vec<u8>` 和 `RestoreCtx` 只存活于调用期间，恢复完成后一次性写向调用者提供的 writer。

并发风险集中在 `TiDBStrictIntegerDisplayWidth: static mut bool`：`strict_integer_width` 以 `unsafe` 读取，写入方也必须使用 `unsafe`，当前没有原子或锁保护。并发读写会违反 Rust 的数据竞争要求；独立测试使用 `serial_test::serial` 串行切换并恢复旧值，但生产接线若允许动态修改，应改为线程安全配置源或在更高层保证初始化后只读。

`MemoryUsage` 是瞬时近似值：统计结构本体、字符串当前长度、Vec 容量对应的槽位及元素字符串长度，不保留资源、也不包含 allocator 元数据或共享分配的额外成本。

## 与 Go 版本的对应关系

主要控制流逐段对应 `pkg/parser/types/field_type.go`：字段、类型分类、比较、字符串化、AST/CAST 恢复、存储长度、JSON 键和内存估算的顺序均保持一致。`pkg/parser/types/field_type_test.rs` 对照 `field_type_test.go` 覆盖常见 `String`/`InfoSchemaStr`、`HasCharset`、ENUM/SET 输出、`Equal` 和严格整数显示宽度；Rust 另有 `test_json_nil_slice_parity` 明确验证 nil slice 编码为 `null`、非 nil 空 slice 编码为 `[]`。

需要注意的语言与迁移差异：

- Go `NewFieldType` 返回 `*FieldType`，Rust 返回值；Go `ArrayType` 在非数组时返回原指针，Rust 总是克隆并返回值，因此 Rust 调用者不能依赖对象身份或就地别名。
- Go 的 `Clone` 是结构浅拷贝，切片底层可能共享；Rust `Clone`/派生克隆会深复制 `String` 和 `Vec`。Rust `DeepCopy` 还会保留 `Some(Vec::new())`，而 Go `DeepCopy` 对长度为零的非 nil slice 不分配目标 slice，nil/空状态可能不同。
- Go 方法可表达 nil 接收者的部分行为；Rust 用 `DeepCopy(Option<&FieldType>)` 单独保留 nil 深复制语义，其余方法只接受有效引用。
- Rust 的 `Restore*` 与 `FormatAsCastType` 暴露 I/O 错误；Go 对应 `RestoreAsCastType`/`FormatAsCastType` 不返回错误。
- Rust `jsonFieldType` 的派生反序列化没有 `#[serde(default)]`；缺少字段的 JSON 会报错，而 Go `encoding/json` 会为缺失字段保留零值。已验证的 `null`/空数组语义一致，但“缺字段”兼容性不能假定一致。
- Rust 的整数宽度全局量需要 `unsafe`，且源码注释明确尚未接入真实配置系统；Go 包级 bool 没有同样的类型系统约束。

## 扩展指南

新增字段时必须同步修改 `FieldType`、`Hash64`、`Equals`、复制语义、`jsonFieldType` 两个 `From` 实现、JSON 兼容测试和 `MemoryUsage`；同时对照更新 Go 结构，否则哈希身份、JSON 或内存统计会悄然遗漏。若字段影响表达式兼容性，还要明确它属于完整身份比较还是 `Equal`/`PartialEqual` 的兼容比较。

新增 MySQL/TiDB 类型码时，至少审查 `IsVarLengthType`、`EvalType`、`Hybrid`、`CompactStr`、`String`/`Restore`、`RestoreAsCastType`、`StorageLength` 和 `HasCharset`，并同步 `etc.rs` 的类型名/分类。数组类型扩展还要同时检查 `GetType` 的 JSON 外观与基于 `tp` 的 CAST 输出。

修改格式化规则时，应扩展独立的 `pkg/parser/types/field_type_test.rs`，并与 `pkg/parser/types/field_type_test.go` 的对应断言保持一致；不要把测试嵌回生产文件。至少覆盖默认与显式长度、标度、字符集、BINARY/UNSIGNED/ZEROFILL、ENUM/SET 转义、严格整数宽度两种模式和 writer 错误传播。

修改元素 API 时，应消除或明确 `elems` 与 `elemsIsBinaryLit` 的长度不变量，并决定非法索引是继续 panic 还是改为 `Result`；该选择会影响 Go 兼容性。修改全局严格宽度接线时，应优先采用原子值或不可变配置快照，并保留 TINYINT(1) 与 ZEROFILL 的兼容特例。

性能上，`ArrayType`、`Clone`、JSON 投影和 `FormatAsCastType` 都会复制拥有的数据；对含大量 ENUM/SET 元素的热点路径做优化时，应先测量，不能通过引入共享别名破坏现有值语义。兼容性上，JSON 键名、nil/空数组区别、字段哈希顺序及类型文本都是外部可观察行为。

## 验证依据

- 源码全貌：`pkg/parser/types/field_type.rs`（648 行），核对了常量、两个结构体、全部 `FieldType` 方法、两个转换实现、serde trait、`Display` 和辅助函数；无条件编译分支。
- crate 边界：`pkg/parser/types/Cargo.toml`、`pkg/parser/types/lib.rs`；确认 crate 名、直接依赖、`include!`/再导出关系和独立测试挂载。
- Go 对照：`pkg/parser/types/field_type.go`；逐段核对字段、分支顺序、JSON 与内存估算，并记录 Rust 所有权、错误返回和反序列化差异。
- 测试证据：`pkg/parser/types/field_type_test.rs`、`pkg/parser/types/field_type_test.go`；Rust 测试覆盖 Go 的主要输出和比较场景，另覆盖 JSON nil/空 slice。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/parser/types` 覆盖目标 Rust/Go/测试；`node --file pkg/parser/types/field_type.rs` 返回完整 648 行及“used by 162 files”；`query FieldType --kind struct` 定位 `field_type.rs::FieldType` 与 `jsonFieldType`。方法级 `callers/callees` 本次无输出，调用关系改由仓库搜索验证。
- 调用搜索：`rg -l 'NewFieldType\\(' --glob '*.rs'` 得到 271 个文件；并核对 `pkg/expression/pb_to_expr_runtime.rs`、`pkg/expression/builtin.rs`、`pkg/expression/chunk_executor.rs`、`pkg/executor/select_into.rs`、`pkg/meta/model/column.rs`、`pkg/ddl/create_table.rs` 的直接使用点。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令及人工事实复核作为验收。
