# `pkg/expression/pb_to_expr_runtime.rs`

## 文件定位

[`pb_to_expr_runtime.rs`](./pb_to_expr_runtime.rs) 是 `astersql-expression` crate 中把 TiPB 线载表达式还原为本地形式化表达式树的运行时边界。crate 根在 [`lib.rs`](./lib.rs) 中以 `#[path = "pb_to_expr_runtime.rs"]` 挂载该文件，并公开再导出 `PBToExprs`、`PBToExpr`、`FieldTypeFromPB`、`PbTypeToFieldType` 和 `PBSignatureFunctionName`；因此调用方不需要知道私有模块名。

这个方向与表达式下推的序列化方向相反：输入是 `tipb::Expr`、`tipb::FieldType` 以及列类型数组，输出是实现本 crate 表达式接口的 `ExprBox`（实际节点为 `Column`、`Constant` 或 `ScalarFunction`）。它处在 TiDB/TiKV、TiFlash 共享协议与 SQL 表达式运行时之间，不负责执行表达式，也不负责决定哪些表达式可以下推。

[`Cargo.toml`](./Cargo.toml) 将当前目录定义为 `astersql-expression` crate；本文件直接依赖的协议、时间和 protobuf 能力分别来自 `tipb`（启用 `protobuf-codec`）、`chrono-tz` 和 `protobuf`。同一 manifest 还把 [`pb_to_expr_runtime_test.rs`](./pb_to_expr_runtime_test.rs) 注册为独立测试目标，符合测试与生产源码分离的仓库约定。

## 核心职责

1. `PBToExprs` 保持输入顺序批量转换表达式，并在任一元素失败时立即返回错误。
2. `PBToExpr` 按 `tipb::ExprType` 分派，把列引用、空值、数值、字符串、字节、位串、时间、JSON、枚举、向量和标量函数恢复成本地节点。
3. `PbTypeToFieldType` 完整复制类型码、标志、显示长度、小数位、字符集、排序规则和枚举元素；`FieldTypeFromPB` 是其兼容别名。
4. `PBSignatureFunctionName` 把 `ScalarFuncSig` 的 Debug 名称转换为 AST 函数名，使协议签名能通过 `NewFunctionBase` 进入本地函数注册表。
5. 对需要语境的值维持 Go 语义：字符串恢复排序规则，`TIMESTAMP` 从 UTC 转换到构建上下文时区，`ValueList` 展开为常量参数，空列表折叠为整型假值 `0`。

本文件不是薄门面：协议解码、类型恢复、递归建树和大规模签名映射都在这里完成。它也不使用旁边 `distsql_builtin.rs` 中同名的旧式/并行实现；当前 crate 对外导出明确来自 `pb_to_expr_runtime` 模块。

## 主要符号

- `fn new_field_type(tp: u8) -> types::FieldType`：内部小工具，经 `types::NewFieldType` 为无完整 TiPB 元数据的常量创建默认类型。
- `fn decode_error(kind: &str, value: &[u8]) -> crate::Error`：内部错误工厂，保留值类别和十六进制载荷，供多种解码分支统一构造错误。
- `fn field_type_from_datum(value: &types::Datum) -> types::FieldType`：调用 `InferParamTypeFromDatum` 推断 `ValueList` 中每个展开常量的参数类型。
- `pub fn PBToExprs(context, expressions, field_types) -> Result<Vec<ExprBox>, crate::Error>`：公开批量入口；迭代调用 `PBToExpr` 并由 `collect` 保证顺序与短路错误传播。
- `pub fn PBToExpr(context, expression, field_types) -> Result<ExprBox, crate::Error>`：公开单节点入口和主要分派器。标量函数分支会递归调用自身。
- `pub fn FieldTypeFromPB(field_type) -> types::FieldType`：公开兼容入口，直接委托 `PbTypeToFieldType`。
- `pub fn PbTypeToFieldType(field_type) -> types::FieldType`：公开字段元数据转换器；排序规则编号通过 `collate::ProtoToCollation` 转成本地名称。
- `pub fn PBSignatureFunctionName(signature: &str) -> Option<&'static str>`：`#[doc(hidden)]` 的公开映射入口。先处理强规则族和类型后缀比较，再回退到命名表。
- `fn function_name_for_named_signature(signature) -> Option<&'static str>`：内部精确表，覆盖信息函数、加密、网络、JSON、正则、向量、全文检索等签名，并继续回退。
- `fn function_name_for_date_or_string_signature(signature) -> Option<&'static str>`：内部日期、时间、字符串和向量前缀/精确匹配表；无法识别时返回 `None`。

文件没有模块级可变状态、结构体、枚举、trait、`impl` 或条件编译项。大映射表 `NAMES` 是函数内部的静态切片，不在运行时修改。

## 执行流程

批量入口的路径很短：`PBToExprs` 遍历 `expressions`，对每项调用 `PBToExpr(context, expression, field_types)`，成功结果依次进入 `Vec`；首个错误终止收集。

`PBToExpr` 的主要流程如下：

1. 读取 `expression.get_tp()` 并进入类型分支。
2. `ColumnRef` 用 `codec::DecodeInt` 解出列偏移，再把偏移安全转换为 `usize`，检查 `field_types` 边界，克隆对应 `FieldType` 后构造 `Column { Index, RetType }`。
3. 字面量分支使用 codec 或类型库恢复 `Datum`。`Null`、`Bytes`、`MysqlBit` 等可直接构造；整数、浮点、十进制、Duration、JSON、Enum 和向量需要验证并解码载荷；字符串还附加协议中的排序规则和显示宽度。
4. `MysqlTime` 先恢复字段类型和 packed uint，设置时间类型与 FSP，再调用 `FromPackedUint`。若类型是 `TIMESTAMP` 且会话位置不是 UTC，调用 `ConvertTimeZone(UTC, location)`；其他时间类型不做此转换。
5. `ScalarFunc` 预分配参数数组并按子节点顺序处理。普通子节点递归进入 `PBToExpr`；`ValueList` 使用 `codec::Decode(..., 1)` 一次解出多个 Datum，并为每项推断字段类型。空 `ValueList` 立即返回 `LONG_LONG` 类型、值为 `0` 的 `Constant`。
6. 标量参数准备好后，把 `expression.get_sig()` 格式化为 Debug 名称，经 `PBSignatureFunctionName` 映射到 AST 名称；最后以协议返回类型调用 `NewFunctionBase` 构造函数节点。这里特意不用会立刻做常量折叠的路径，以保持下推表达式形状。
7. 已知分支之外的 `ExprType` 会触发 `panic!`，信息明确要求输入必须是 `ScalarFunc`；成功分支统一包装为 `Ok(result)`。

签名映射有三层优先级：`PBSignatureFunctionName` 先匹配 Cast、比较、算术、逻辑等高频规则；随后精确查找 `NAMES`；最后处理日期/时间/字符串等前缀族。先精确、后宽泛的局部顺序很重要，例如 `InsertUTF8`、`InstrUTF8` 和 `Substring2ArgsUTF8` 不能被更早的宽前缀误捕获。

## 数据与状态

输入 `tipb::Expr` 是借用的协议树，转换不会修改原树。`PBToExpr` 为返回节点分配拥有所有权的 `Box`：字节载荷会按分支复制到 `Vec<u8>`，字段类型会转换或克隆，子表达式会递归形成拥有型参数数组。`field_types` 只作为列偏移到返回类型的只读映射。

`Datum` 是字面量的主要状态载体。十进制除值外保留 precision/fraction；字符串保留 collation 与 flen；枚举同时依赖数值载荷和 `FieldType.elems`；时间值保留类型和 FSP，并可能根据 `BuildContext::GetEvalCtx().Location()` 改变时区表示。`ValueList` 展开后不保留列表包装节点，而是变成多个独立 `Constant` 参数。

函数名映射返回 `&'static str`，引用的是 `ast` 常量或源码中的静态字符串，不产生缓存或动态注册。整个文件没有全局可变状态，也没有跨调用积累的数据。

## 依赖与调用关系

上游方面，`lib.rs` 是确定的装配入口并公开再导出五个 API。RustCodeGraph 对目标文件给出的 “used by” 文件包括 `lib.rs`、[`pb_to_expr_runtime_test.rs`](./pb_to_expr_runtime_test.rs)、`builtin_registry_aster_unit_test.rs`、`distsql_builtin.rs` 和 `pkg/ddl/storage_class.rs`；其中源码级直接调用证据主要来自独立测试与内建函数注册表测试，生产调用者可通过 crate 的公开再导出使用这些入口。图查询还识别到 `PBToExprs -> PBToExpr` 和 `PBToExpr -> PBToExpr` 的批量/递归边。

下游关系以 `PBToExpr` 为中心：

- `crate::codec::{DecodeInt, DecodeUint, DecodeFloat, DecodeDecimal, DecodeOne, Decode}` 负责协议载荷解码。
- `types` 提供 `Datum`、`FieldType`、时间、枚举、向量以及参数类型推断。
- `collate::ProtoToCollation` 恢复排序规则；`chrono_tz::UTC` 与 `BuildContext` 的求值位置共同决定 `TIMESTAMP` 转换。
- `Column`、`Constant` 和 `NewFunctionBase` 构造正式表达式节点；`PBSignatureFunctionName`/`ast` 把协议签名桥接到本地函数注册表。
- `errors::New` 把底层解码、时间和枚举错误统一到 crate 的 `Error`。

Cargo 边界证据来自 [`Cargo.toml`](./Cargo.toml)：`tipb` 固定到明确 revision 并启用 protobuf codec，`types-dependency`、`codec-dependency`、`collate-dependency` 等本地 crate 经 `lib.rs` 再导出为本文件使用的模块。这些依赖决定线载格式、字段元数据和排序规则语义，不应在本文件内另建平行表示。

## 错误处理与边界

- 列偏移的线载解码错误直接传播；负偏移在 `usize::try_from` 处转为 `negative column offset` 错误；超出 `field_types` 则返回 `column offset ... is out of range`，不会像 Go 切片索引那样意外越界。
- int、uint、float、decimal、duration、JSON 和向量的格式错误通过 `decode_error` 返回带原始十六进制载荷的错误。`MysqlTime` 的 packed 值解码直接传播，时间恢复/时区转换错误转成 crate 错误。
- JSON 解出 Datum 后还检查 `KindMysqlJSON`；枚举数值非零时必须能在字段元素表中解析；这些是格式解码成功后的语义校验。
- 非空 `ValueList` 解码错误会终止整个标量函数；空列表是合法边界，直接折叠为假常量。该提前返回意味着同一标量节点其余孩子不会继续转换，保持 Go 路径的边界行为。
- 未知 ScalarFuncSig 映射为 `None`，随后返回 `FUNCTION <signature> does not exist`；`NewFunctionBase` 自身的参数、类型或注册错误继续向上传播。
- 非已处理字面量、非 `ScalarFunc` 的 ExprType 是调用契约违例，会 `panic!` 而非返回错误。安全扩展新 TiPB ExprType 时必须先在该总分派中增加明确分支。
- `PBSignatureFunctionName` 使用前缀和子串规则，规则顺序属于兼容性边界；过宽的新规则可能把原本应进入精确表的签名映射错。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或外部句柄。所有函数只借用调用方提供的上下文/协议值，并在当前调用栈同步完成转换；因此并发属性由传入的 `BuildContext`、底层类型以及调用者保证，本文件不引入额外同步约束。

资源生命周期以递归栈和拥有型表达式树为主。子节点转换成功后移入参数 `Vec`，错误时 Rust 自动释放已经构造的节点；批量转换同理会释放已收集的前缀。向量的函数名显示为 `ZeroCopyDeserializeVectorFloat32`，但返回值被装入拥有型 Datum；本文件不保留对输入 `expression.get_val()` 的长期借用。`ValueList` 解码会创建 Datum 集合，再移动进常量节点。

递归深度与协议表达式树深度一致，本文件没有显式深度上限。大列表会按元素数分配常量节点，标量参数 `Vec` 初始容量只按 children 数预估，展开 `ValueList` 时可能继续增长；扩展时需关注深树栈使用和大列表分配，但当前源码没有并行化或流式处理。

## 与 Go 版本的对应关系

直接 Go 对照是 [`distsql_builtin.go`](./distsql_builtin.go) 中的 `PBToExprs`、`PBToExpr`、`convertTime`、`decodeValueList`、各字面量转换器，以及文件前部的 `PbTypeToFieldType`。两版共同保持：按输入顺序转换；列引用从载荷取 offset 和字段类型；字面量恢复成 `Constant`；`TIMESTAMP` 从 UTC 转到会话位置；空 ValueList 返回假；普通孩子递归；字段类型复制完整元数据。

Rust 版本把 Go 的多个 `convert*` 辅助函数内联进 `PBToExpr` 分支，并用 `Result`、安全整数转换和 `.get(index)` 把负数/越界列偏移变成显式错误。Go 的标量函数通过 `newDistSQLFunctionBySig(ctx, expr.Sig, ...)` 直接按枚举分派；当前 Rust 路径先把签名 Debug 名映射为 AST 名称，再调用 `NewFunctionBase`。因此 `PBSignatureFunctionName` 及其覆盖完整性是 Rust 特有的迁移桥梁。

非空 ValueList 也存在实现表达差异：Go 的 `decodeValueList` 构造未显式携带 `RetType` 的常量，Rust 对每个 Datum 调用 `InferParamTypeFromDatum`。两者意图都是把列表扁平加入标量参数；修改时应以现有测试和实际求值类型语义为准，不能仅机械复制结构。

Go 测试 [`distsql_builtin_test.go`](./distsql_builtin_test.go) 的 `TestPBToExpr` 验证残缺 Datum/ValueList 错误和空列表边界，`TestEval` 覆盖解码后实际求值，`TestPBToExprWithNewCollation` 覆盖新旧排序规则编号。Rust 独立测试聚焦当前正式运行时 API，并额外把所有 639 个非 `Unspecified` 协议签名作为映射完整性不变量。

## 扩展指南

新增 TiPB 字面量类型时，首先在 `PBToExpr` 的 `ExprType` 匹配中添加分支，并明确三件事：线载 codec、Datum kind、返回 `FieldType` 来源。若值依赖会话时区、排序规则或 SQL mode，应从 `BuildContext`/字段元数据读取，而不是使用进程全局默认值。同步在独立的 [`pb_to_expr_runtime_test.rs`](./pb_to_expr_runtime_test.rs) 增加合法载荷、残缺载荷和类型元数据断言；不要把测试嵌回生产文件。

新增 `ScalarFuncSig` 时，应先判断它属于现有前缀族、精确 `NAMES` 表还是日期/字符串回退表，并把最具体规则放在宽泛规则之前。必须保持 `every_existing_pb_signature_has_a_formal_function_mapping` 通过，并为容易被前缀遮蔽的名称增加独立断言；同时确认目标 AST 名称已在 `NewFunctionBase` 的注册路径可构造。只让映射函数返回某个字符串并不能证明函数实现存在。

修改字段类型转换时需同步核对 Go `PbTypeToFieldType`、`FieldTypeFromPB` 的其他使用点和排序规则转换；类型码、flag、flen、decimal、charset、collation、elems 任一遗漏都可能改变求值或比较语义。修改时间分支时要同时覆盖 UTC 与非 UTC `TIMESTAMP`，并避免把 `DATE`/`DATETIME` 当作时区瞬时值转换。

性能敏感改动应关注递归深度、协议字节复制、ValueList 展开时的额外分配及签名表线性查找。若改成索引表或生成代码，仍需维持静态返回生命周期、匹配优先级和未知签名返回 `None` 的契约。兼容性风险主要来自协议 enum 增长、函数别名大小写/拼写变体和排序规则编号变化。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 [`pb_to_expr_runtime.rs`](./pb_to_expr_runtime.rs) 已索引并识别 13 个符号。
- RustCodeGraph `files --filter pkg/expression/pb_to_expr_runtime.rs` 与 `node --file ...`：确认文件全长 778 行、模块符号和 `used by` 文件；分段读取覆盖 1–778 行。
- RustCodeGraph `query PBToExpr`、`query PBToExprs`、`query PbTypeToFieldType`、`query PBSignatureFunctionName`：确认目标定义、签名以及 Go/并行 Rust 同名候选；带 `--file` 的 `callers/callees PBToExpr` 确认批量调用、递归调用和下游解码/类型/上下文依赖。图结果存在跨语言同名噪声，本文只采用与目标文件或直接源码一致的边。
- 源码装配与 crate 边界：[`lib.rs`](./lib.rs) 的模块挂载/再导出；[`Cargo.toml`](./Cargo.toml) 的 package、依赖和独立测试目标。
- Go 对照：[`distsql_builtin.go`](./distsql_builtin.go) 的 `PBToExprs`、`PBToExpr`、`convert*`、`decodeValueList`；[`distsql_builtin_test.go`](./distsql_builtin_test.go) 的 `TestPBToExpr`、`TestEval`、`TestPBToExprWithNewCollation`。
- Rust 测试：[`pb_to_expr_runtime_test.rs`](./pb_to_expr_runtime_test.rs) 的字段/字面量、列/列表/标量、639 个签名、前缀优先级、残缺载荷与越界列测试；`builtin_registry_aster_unit_test.rs` 还抽查多类签名到 AST 名称的映射。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求文档存在且固定二级标题恰好为 11 个；最终还需人工检查唯一产物、链接、源码事实和 diff。
