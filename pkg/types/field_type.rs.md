# `pkg/types/field_type.rs`

## 文件定位

本文件是 AsterSQL Rust 类型系统中“字段类型策略”层的实现：它不定义 `FieldType` 的存储结构，而是把 `pkg/parser/types/field_type.rs` 中的 `ast::FieldType` 取别名为 `FieldType`，围绕该结构提供默认构造、类型聚合、运行时值类型推断、DDL 列类型兼容性判断和 VARCHAR 长度校验。对应证据是 `pub type FieldType = ast::FieldType`，以及文件中的 `NewFieldType`、`AggFieldType`、`DefaultTypeForValue`、`CheckModifyTypeCompatible` 等入口。

编译归属不是由根 `pkg/types/lib.rs` 直接声明 `mod field_type`，而是由 `pkg/types/internal/field/lib.rs` 的 `field_type` 模块通过 `include!("../../field_type.rs")` 纳入 `astersql-types-field` crate，再由 `pkg/types/lib.rs` 的 `pub use types_field_group as field` 暴露。`pkg/types/Cargo.toml` 将这个子 crate 声明为路径依赖 `types-field-group`；目标文件所需的 parser 类型、字符集、排序规则、错误和整数长度工具则由 `pkg/types/internal/field/Cargo.toml` 的 `parser-types`、`collate`、`dbterror`、`mathutil` 提供。

## 核心职责

本文件承担五组相互关联的职责：

1. `NewFieldType`、`NewFieldTypeWithCollation` 和 `DefaultCharsetForType` 构造带 MySQL 类型码、长度、小数位、字符集和排序规则的字段类型。
2. `AggFieldType` 与 `AggregateEvalType` 分别聚合 SQL 表达式的存储类型和求值类型，服务 IF、IFNULL、COALESCE、比较和同类多参数表达式；前者还处理有符号/无符号整数的范围提升。
3. `InferParamTypeFromDatum`、`InferParamTypeFromUnderlyingValue` 和 `DefaultTypeForValue` 从预处理参数或动态 Rust 值推断 `FieldType`，包括 NULL、数值、字符串、二进制字面量、时间、Decimal、Enum/Set、JSON 和向量。
4. `CheckModifyTypeCompatible`、`needReorgToChange`、`checkTypeChangeSupported` 和 `ConvertBetweenCharAndVarchar` 判定 ALTER COLUMN 能否原地修改、需要数据重组，或当前完全不支持。
5. `IsVarcharTooBigFieldLength` 按字符集最大字节宽度折算 VARCHAR 字符数上限，生成与 Go 版本相同类别的标准错误。

文件中的 29×29 `fieldTypeMergeRules` 是类型聚合的核心数据表；`getFieldTypeIndex` 必须与表的行列顺序保持一一对应。

## 主要符号

- `UnspecifiedLength = -1`、`ErrorLength = 0`：分别表示未指定长度和错误长度。`VarStorageLen` 直接转发 `ast::VarStorageLen`。
- `FieldType = ast::FieldType`：只建立别名；字段、访问器、`EvalType`、`CompactStr` 及长度上限处理均来自 parser-types crate。
- `NewFieldType(tp) -> Box<FieldType>`：用 `DefaultCharsetForType` 和 `minFlenAndDecimalForType` 设置完整默认值。整数和 YEAR 使用 MySQL 默认宽度/小数位，其他类型暂用未指定哨兵。
- `NewFieldTypeWithCollation(tp, collation, length)`：解析给定排序规则并设置对应字符集、长度及未指定 decimal；无效 collation 会因 `expect` 触发 panic。
- `AggFieldType(tps)`：依次调用 `mergeFieldType` 和 `mergeTypeFlag`；混合符号整数在无符号输入占满当前类型范围时，按 Tiny→Short→Int24→Long→Longlong→NewDecimal 提升。
- `AggregateEvalType(fts, flag)`：忽略 NULL 输入，调用 `mergeEvalType`，并回写 `UnsignedFlag` 与 `BinaryFlag`。字符串求值类型优先于数值，Real 优先于 Decimal，Decimal 又优先于 Int；整数符号混合也提升为 Decimal。
- `SetTypeFlag`：按布尔开关置位或清除单个标志位。
- `InferParamTypeFromDatum`：先按底层动态值推断，再对字符串/字节 Datum 尝试采用 Datum 自身 collation；查找失败时保留默认值。
- `DefaultTypeForValue`：以 `Any::downcast_ref` 模拟 Go type switch，设置类型、flen、decimal、charset/collation 和 NotNull/Unsigned/Binary/Boolean 标志。
- `DefaultCharsetForType`、`SetBinChsClnFlag`：字符串族默认 utf8mb4；其他类型默认 binary。后者同时增加 `BinaryFlag`。
- `fieldTypeMergeRules`、`mergeFieldType`、`getFieldTypeIndex`：实现 MySQL 类型两两合并查表。未知类型码被映射到索引 0（TypeUnspecified 行/列），而不是返回错误。
- `CheckModifyTypeCompatible`：返回 `(can_reorg, error)`。`error == None` 表示可直接修改；`can_reorg == true` 且有错误表示可通过重组实现；`can_reorg == false` 且有错误表示当前不支持。
- `IsVarcharTooBigFieldLength`：从 `charset::GetCharsetInfo` 取得 `Maxlen`，以 `mysql::MaxFieldVarCharLength / Maxlen` 计算最大字符长度。

本文件没有自定义 struct、enum、trait、impl 或条件编译项；状态全部通过参数、返回值、常量和静态合并表表达。

## 执行流程

表达式类型聚合的主流程如下：调用方先收集非 NULL 参数的 `&FieldType`；`AggFieldType` 从首项克隆当前类型，对后续项查 `fieldTypeMergeRules`、合并 NotNull/Unsigned 标志，再在混合符号整数场景检查是否必须拓宽范围。需要求值类别时，`AggregateEvalType` 另行扫描输入，通过 `mergeEvalType` 得出 String/Real/Decimal/Int 等 `EvalType`，最后同步输出 flag。Rust 生产接线可见 `pkg/expression/planner_bridge.rs`：它对非 NULL 字段调用 `types_dependency::field::AggFieldType` 和 `AggregateEvalType`，随后用 `TryToFixFlenOfDatetime` 修正 DATETIME 显示宽度；`pkg/planner/core/logical_plan_builder_runtime.rs` 也调用聚合与 DATETIME 修正入口。

参数推断从 `InferParamTypeFromDatum` 开始。它调用 `InferParamTypeFromUnderlyingValue`：NULL 被明确设为 `TypeNull`、flen/decimal 为 0，并采用默认 charset/collation；非 NULL 值进入 `DefaultTypeForValue`，随后可变长度类型统一把 flen 改回 `UnspecifiedLength`，无法识别的动态类型从 `TypeUnspecified` 回退到 `TypeVarString`。最后，字符串类 Datum 尝试用 Datum 自身 collation 覆盖默认字符集信息。生产调用证据包括 `pkg/expression/constant.rs`、`pkg/expression/util.rs`、`pkg/expression/pb_to_expr_runtime.rs`、`pkg/planner/core/expression_rewriter.rs` 和 `pkg/session/runtime/planning.rs`。

列修改兼容性流程从 `CheckModifyTypeCompatible` 开始。同类型时先保护 ENUM/SET 原有元素前缀，并拒绝 DECIMAL 的 flen、decimal 或 unsigned 改动；之后 `needReorgToChange` 检查 CHAR/VARCHAR 互转、长度缩短、binary CHAR 长度变化、精度降低和符号翻转。不同类型先由 `checkTypeChangeSupported` 排除 BIT、ENUM/SET、时间及向量等不支持组合；字符串族或整数族再走 reorg 检查；其他允许的跨族变化返回“类型不匹配、可 reorg”。当前 Rust 仓库搜索没有找到这些 DDL 入口的生产调用，只有 `pkg/types/field_type_5_aster_unit_test.rs` 的直接验证；Go 主链则由 `pkg/ddl/modify_column.go`、`pkg/ddl/add_column.go`、`pkg/ddl/executor.go` 和 `pkg/planner/core/preprocess.go` 调用。

## 数据与状态

`FieldType` 的关键状态是 MySQL 类型码、`flen`、`decimal`、charset、collation、flags 和 ENUM/SET 元素。函数通过 parser-types 提供的 Get/Set/Add/Del 方法读写这些字段；`AggFieldType` 返回克隆后的新值，不修改输入，其他推断与辅助函数通常接收 `&mut FieldType` 原地更新。

`fieldTypeMergeRules` 是只读的 29×29 静态数组。其索引约定由 `getFieldTypeIndex` 固化，覆盖 TypeUnspecified、数值、时间、字符串/BLOB、JSON、Geometry 和 TiDB VectorFloat32。修改类型集合时必须同时更新映射、数组维度、每一行及 Go 对照表，否则会发生错误合并或数组越界风险。

flag 的不变量包括：`mergeTypeFlag` 只对 NotNull 与 Unsigned 采用“两侧都存在才保留”的规则；`AggregateEvalType` 仅在所有有效数值输入均 unsigned 时保留 Unsigned；非字符串求值类型或出现 binary string 时设置 Binary；`DefaultTypeForValue` 对非 NULL 输入先设置 NotNull，数值和多数非字符串值再设置 binary charset/collation 与 BinaryFlag。

## 依赖与调用关系

向下依赖方面，`ast::FieldType`、`EvalType` 和类型判断基础来自 `astersql-parser-types`；MySQL 类型码、flags 与长度常量来自 `mysql`；charset/collation 查询来自 parser charset 与 `astersql-util-collate`；标准错误由 `astersql-util-dbterror` 生成；整数显示长度由 `astersql-util-mathutil` 计算。`Datum`、`Time`、`Duration`、`MyDecimal`、字面量、JSON 和向量类型由包含该文件的 `pkg/types/internal/field/lib.rs` 在同一模块作用域提供。

RustCodeGraph 对目标文件报告 29 个符号，并显示该文件被表达式、元数据、planner 及测试等 15 个文件使用。精确 callee 查询确认：`AggFieldType` 调用本文件的 `mergeFieldType`/`mergeTypeFlag` 和外部类型访问器；`AggregateEvalType` 调用 `mergeEvalType`/`SetTypeFlag` 及 `IsTypeBlob`/`IsTypeChar`/`IsTypeVarchar`；`DefaultTypeForValue` 调用 FieldType setter、`SetBinChsClnFlag`、decimal/time 方法；`CheckModifyTypeCompatible` 调用 `needReorgToChange`、`checkTypeChangeSupported` 和 `IsString`。

向上调用方面，聚合入口已接入 `pkg/expression/planner_bridge.rs` 和 `pkg/planner/core/logical_plan_builder_runtime.rs`；参数推断已接入 expression、planner 和 session runtime。RustCodeGraph 的同名函数查询同时命中 Go/Rust 定义，且部分常见方法被解析到其他文件的同名符号，因此调用图只作为候选证据，最终路径用限定 crate 调用和仓库文本搜索交叉核对。DDL 兼容性与 VARCHAR 上限入口尚未发现 Rust 生产调用，不应据此宣称 Rust DDL 主链已经完成接线。

## 错误处理与边界

- `AggregateEvalType` 在进入循环前直接读取 `fts[0]`，调用者必须保证切片非空；空切片会 panic。全 NULL 切片不会 panic，但结果保持初始 `ETString`，并清除 unsigned/binary（除非后续逻辑改变）。
- `NewFieldTypeWithCollation` 对未知 collation 使用 `expect`，不是可恢复错误。调用者必须先保证名称有效。
- `InferParamTypeFromDatum` 对 Datum collation 查找失败选择静默保留默认 charset/collation；`DefaultTypeForValue` 对未知 `Any` 类型设置 TypeUnspecified，而上层推断入口再回退为 VarString。
- `getFieldTypeIndex` 对未知类型码返回 0。这样可以沿用 TypeUnspecified 合并规则，但也会掩盖遗漏的新类型；新增 MySQL 类型时必须显式补表。
- `CheckModifyTypeCompatible` 的布尔值不是“兼容”本身，必须与 error 联合解释。ENUM/SET 只允许保留原有元素的顺序前缀并在尾部扩展；DECIMAL 任一关键属性变化都会返回需 reorg 错误；VectorFloat32 与任何不同类型互转均被当前支持检查拒绝。
- `needReorgToChange` 对整数忽略用户显示宽度，改用类型默认宽度比较；长度缩短、decimal 降低、unsigned 翻转和 binary CHAR 长度变化需要重组。
- `ConvertBetweenCharAndVarchar` 的 VARCHAR→CHAR 总是要求 reorg，CHAR→VARCHAR 仅在 `collate::NewCollationEnabled()` 时要求 reorg。
- `IsVarcharTooBigFieldLength` 会传播未知字符集错误；未指定长度跳过上限错误。它假设已注册字符集的 `Maxlen` 非零。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、channel、事务或 I/O 资源。`fieldTypeMergeRules` 和常量只读，可被并发安全共享；绝大多数函数仅操作栈上局部值、克隆的 `FieldType` 或调用方独占的 `&mut FieldType`/`&mut usize`，Rust 借用规则阻止同一可变对象被并发无同步修改。

返回的 `Box<FieldType>` 由调用方拥有并按 Rust 所有权自动释放；错误使用 `errors::SharedError` 共享所有权，由返回值生命周期管理。charset/collation 描述由依赖模块查询，本文件不持有缓存或注册表句柄。唯一进程级初始化相关状态在 `pkg/types/internal/field/lib.rs`：标准错误使用 `LazyLock` 并通过平台启动段预初始化，而非由本文件自行管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/field_type.go`，Rust 基本保留其函数分组、29×29 合并表、整数范围提升、参数类型推断和列修改判定。`pkg/types/field_type_test.rs` 对应 `pkg/types/field_type_test.go` 的默认类型、全量聚合、flag 与 EvalType 表驱动测试；`pkg/types/field_type_5_aster_unit_test.rs` 额外覆盖 DATETIME flen、混合符号、动态值推断、列修改与 VARCHAR 上限。

已确认的表达差异包括：Go 构造器经 `NewFieldTypeBuilder`，Rust 直接调用 setter；Go `fieldTypeIndexes` 是 map，Rust 用 `match`；Go `any` type switch 的 `int`/`int64` 在 Rust 分别以 `i32`/`i64` 分支近似，Rust 还要求调用者以支持的具体类型放入 `Any`；Go 的 `*MyDecimal` 对应 Rust 的 `MyDecimal` 值引用；Go collation 构造忽略查询错误并随后解引用，Rust 明确 `expect`；Go 返回 `error`，Rust 返回 `Option<SharedError>` 或 `Result`。这些是语言适配，不应改变既有业务判定。

Go 当前生产主链对这些函数的使用更广：`pkg/expression/builtin_control.go` 使用聚合和 DATETIME 修正，`pkg/ddl/modify_column.go` 使用兼容性判断，DDL/planner 使用 VARCHAR 上限检查。Rust 已接线表达式聚合与参数推断，但仓库搜索未发现 Rust DDL 对后三类入口的生产调用，因此迁移状态应描述为“逻辑已移植并有独立测试，DDL 接线尚未由本次证据验证”。

## 扩展指南

新增 MySQL 字段类型时，至少同步修改 `getFieldTypeIndex`、`fieldTypeMergeRules` 的维度与所有行、`DefaultCharsetForType`、必要的 `hasVariantFieldLength`/`DefaultTypeForValue` 分支，以及 `pkg/types/field_type_test.rs` 的 `all_field_type_cases` 和聚合期望；同时对照更新 `pkg/types/field_type.go`，避免 Rust/Go 合并矩阵漂移。此类修改的主要风险是矩阵错位导致静默返回错误类型，不能只验证编译。

新增可推断运行时值时，应在 `DefaultTypeForValue` 增加精确 `Any` downcast 分支，明确 type/flen/decimal/charset/collation/flags，并在 `pkg/types/field_type_test.rs` 的表驱动用例中增加对应值；如果该类型用于预处理参数，还要决定 `hasVariantFieldLength` 是否清除具体 flen，并验证 `InferParamTypeFromDatum` 的 collation 行为。

修改表达式聚合规则时，应同步检查 `mergeEvalType`、`mergeTypeFlag`、`AggFieldType` 的混合符号提升与 `AggregateEvalType` 的输出 flag，并扩展 `TestAggFieldType`、`TestAggFieldTypeForIntegralPromotion` 和 `TestAggregateEvalType`。调用方必须继续过滤或处理空参数，或者在本入口增加显式空切片契约与回归测试。

扩展 ALTER COLUMN 支持时，应分别修改 `checkTypeChangeSupported`（是否允许）、`needReorgToChange`（是否需重组）和 `CheckModifyTypeCompatible`（错误类别/消息），并在独立的 `pkg/types/field_type_5_aster_unit_test.rs` 增加同类型、跨类型、ENUM/SET、DECIMAL、binary charset 和 VectorFloat32 边界。若要真正用于 Rust DDL，还需在 DDL 层增加接线与 DDL 独立测试；不要把测试写回本生产文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标文件有 29 个符号；`files --filter pkg/types/field_type.rs` 确认目标已索引；`query FieldType`、`query FieldTypeBuilder`、`query EvalType` 用于区分 parser 定义、types 别名和构建器；`node --file pkg/types/field_type.rs` 阅读目标源码；对 `AggFieldType`、`AggregateEvalType`、`DefaultTypeForValue`、`CheckModifyTypeCompatible`、`IsVarcharTooBigFieldLength` 执行 callers/callees 查询。
- Rust 源与装配：`pkg/types/field_type.rs`、`pkg/types/internal/field/lib.rs`、`pkg/types/internal/field/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`。
- Rust 直接调用证据：`pkg/expression/planner_bridge.rs`、`pkg/expression/constant.rs`、`pkg/expression/util.rs`、`pkg/expression/pb_to_expr_runtime.rs`、`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/logical_plan_builder_runtime.rs`、`pkg/session/runtime/planning.rs`。
- Go 对照与生产调用：`pkg/types/field_type.go`、`pkg/expression/builtin_control.go`、`pkg/expression/builtin_compare.go`、`pkg/planner/core/plan_cache_param.go`、`pkg/ddl/modify_column.go`、`pkg/ddl/add_column.go`、`pkg/ddl/executor.go`、`pkg/planner/core/preprocess.go`。
- 独立测试：`pkg/types/field_type_test.rs`、`pkg/types/field_type_5_aster_unit_test.rs`、`pkg/types/field_type_test.go`。本任务是纯文档分析，按计划未运行 Cargo 或测试二进制；验证限于源码、调用图、对照文件和文档结构。
