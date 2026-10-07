# `pkg/expression/distsql_builtin.rs`

## 文件定位

`distsql_builtin.rs` 位于 `astersql-expression` crate，源码由 `pkg/expression/lib.rs` 以私有模块 `distsql_builtin_kernel` 挂载。它把下推协议中的 `tipb::Expr` 重建为本文件自有的 `Expression::{Column, Constant, ScalarFunction}`，并用 `SIGNATURE_RECIPES` 保存 Go `getSignatureByPB` 的完整分派对应关系。

这个文件目前是移植对等内核，而不是 crate 对外的正式表达式运行时入口：`lib.rs` 对外再导出的 `PBToExpr`、`PBToExprs`、`PbTypeToFieldType` 来自 `pb_to_expr_runtime.rs`；本文件只在 `cfg(test)` 下经 `expression_distsql_builtin` 适配模块暴露给 `distsql_builtin_34_aster_unit_test.rs` 和 `distsql_builtin_test.rs`。因此，本文件能验证协议解码和 Go 分派表，但其 `BuiltinFuncDraft` 不会执行具体 builtin 求值。

crate 边界见 `pkg/expression/Cargo.toml`：直接相关依赖包括固定到 Git revision 的 `tipb`、`protobuf = 2.8.0`、`chrono`、`chrono-tz`，以及由 crate 根适配出的 `types`、`codec`、`collate` 模块。该 Cargo 文件关闭自动测试发现（`autotests = false`）；本文件的测试由 `lib.rs` 中的 `#[cfg(test)]` 模块显式挂载，而非独立 `[[test]]` 目标。

## 核心职责

1. `PbTypeToFieldType`/`FieldTypeFromPB` 将 TiPB 字段类型的类型码、flag、显示长度、小数位、字符集、排序规则和枚举元素复制到内部 `types::FieldType`。
2. `PBToExpr`/`PBToExprs` 解码列引用和各类字面量，递归展开标量函数子节点，并把 `ValueList` 展平为常量参数。
3. `getSignatureByPB` 通过 `SIGNATURE_RECIPES` 把 `tipb::ScalarFuncSig` 的 Debug 名匹配到 Go 构造器表达式字符串，同时保留构建上下文中的 `max_allowed_packet`。
4. `newDistSQLFunctionBySig` 收集参数求值类型，调用本地 `deriveCollation`，再生成携带返回类型、草稿 builtin 和 coercibility 的 `ScalarFunction`。
5. `codec` 子模块统一把底层编解码错误收敛为 `ExpressionError`；各 `convert*` 函数负责类型专用的 wire 数据转换和错误上下文。

职责边界必须注意：`SIGNATURE_RECIPES` 的 `constructor` 是用于逐项核对 Go switch 的静态字符串，不是 Rust 构造函数指针；`BuiltinFuncDraft` 只保存参数、返回类型、签名、构造器描述和包大小限制。本文件注释也明确“仅做内存重建，不执行求值”，正式运行时构建位于 `pb_to_expr_runtime.rs`，其标量函数路径调用 `NewFunctionBase`。

## 主要符号

- `ExpressionError(String)`：本文件统一错误类型；`external` 把任意 `Display` 错误转为字符串，`errors::errorf` 构造本地错误。
- `EvalContext` / `BuildContext`：只保存时区和 `max_allowed_packet`。与正式 crate 的 `BuildContext` trait 不同，它们是本文件的轻量对等模型。
- `Column`、`Constant`、`Expression`：本文件自有表达式树。`Constant::from_datum` 根据 `Datum::Kind()` 推断 `FieldType`，用于 `ValueList`；`Expression::get_type` 返回节点自身的返回类型。
- `BuiltinBaseDraft` / `BuiltinFuncDraft`：保存参数和返回类型，并记录 `ScalarFuncSig`、Go 构造器字符串和最大包大小；没有求值方法。
- `ScalarFunction`：保存草稿 builtin、函数名、返回类型和 coercibility；`set_coercibility` 在排序规则推导后写入状态。
- `SignatureRecipe` / `SIGNATURE_RECIPES`：565 项静态分派配方。独立测试校验数量、唯一性、首尾项以及代表性比较函数映射。
- `PbTypeToFieldType`：字段元数据转换主入口；`FieldTypeFromPB` 是直接别名包装。
- `getSignatureByPB`：按签名名查表并构造 `BuiltinFuncDraft`；未知签名通过 `ErrFunctionNotExists` 返回错误。
- `newDistSQLFunctionBySig`：草稿标量函数构建入口，负责参数 `EvalType` 收集、排序规则推导和 coercibility 设置。
- `PBToExprs` / `PBToExpr`：批量与单节点递归入口；前者保持顺序并在任一节点失败时立即返回。
- `convertTime`、`decodeValueList`、`convertInt`、`convertUint`、`convertString`、`convertFloat`、`convertDecimal`、`convertDuration`、`convertJSON`、`convertVectorFloat32`、`convertEnum`：协议载荷的类型专用转换函数。

文件还定义 `mysql`、`time`、`codec` 三个适配子模块，把 crate 已有类型/codec 暴露成接近 Go 源码的调用面。没有条件编译项；条件编译发生在 `lib.rs` 对测试适配模块和测试文件的挂载处。

## 执行流程

批量入口 `PBToExprs(ctx, pb_exprs, field_types)` 预分配结果数组，逐个调用 `PBToExpr`。转换错误直接经 `?` 返回；若单节点返回 `None`，则生成包含原 PB 节点的 `pb to expression failed` 错误；成功结果按输入顺序追加。

单节点入口 `PBToExpr` 的流程如下：

1. 取得 `EvalContext`，先按 `ExprType` 尝试字面量/列引用分派。
2. `ColumnRef` 用 `codec::decode_int` 解码列偏移，并以该偏移直接索引调用方传入的 `field_types`；列节点同时保存偏移和克隆后的字段类型。
3. `Null`、整数、无符号整数、字符串、字节、bit、浮点、decimal、duration、time、JSON、enum、float32 vector 分别构造 `Constant` 或调用相应 `convert*` 函数。成功后立即返回。
4. 若既不是已支持字面量也不是 `ScalarFunc`，代码按 Go 边界 `panic!("should be a tipb.ExprType_ScalarFunc")`，而不是返回可恢复错误。
5. 对标量函数逐个处理 children：普通 child 递归调用 `PBToExpr`；`ValueList` 使用 `decodeValueList` 一次解码并展平到参数数组。空 `ValueList` 立即把整个节点折叠为 `false`（`Longlong` 类型的整数 0），不会继续构造标量函数。
6. 参数收集完毕后，调用 `newDistSQLFunctionBySig(ctx, expr.sig, expr.field_type, args)`。
7. `getSignatureByPB` 先转换返回类型、构造公共 `BuiltinBaseDraft`、读取 `max_allowed_packet`，再以 `format!("{sig_code:?}")` 得到协议枚举名并对 565 项表做不区分 ASCII 大小写的线性查找。
8. `newDistSQLFunctionBySig` 从每个参数的返回类型提取 `EvalType`，调用 `deriveCollation`。当前实现只校验参数数与类型数相等，并按返回值是否为字符串给出 coercibility 4 或 5；随后创建 `ScalarFunction` 并返回 `Expression::ScalarFunction`。

类型专用转换中的关键分支包括：`convertTime` 只对 `TIMESTAMP` 且会话时区非 UTC 的值执行 UTC→会话时区转换；`convertJSON` 在 codec 解码后继续检查 `Datum::Kind()`；`convertEnum` 把数值 0 解释为空枚举，其他值按 PB 的 `elems` 查表；`convertDecimal` 把 wire 数据给出的 precision/fraction 写回 Datum；`convertVectorFloat32` 调用零拷贝反序列化接口。

## 数据与状态

本文件处理三层数据表示：TiPB wire 节点（`tipb::Expr`/`FieldType`）、内部值（`types::Datum`/`FieldType`）和本文件自有表达式树。转换过程中不修改输入 PB；字节载荷在字符串、bytes、bit 等路径会复制为 `Vec<u8>`，字段类型及其 `elems` 也被克隆到内部对象。

`BuildContext` 是不可变借用的配置容器，包含：

- `location: chrono_tz::Tz`，仅在 MySQL time/TIMESTAMP 解码时读取；
- `max_allowed_packet: u64`，由所有 `BuiltinFuncDraft` 记录，供 `Repeat`、`Rpad`、`Space`、`ToBase64` 等 Go 构造器配方对应的包大小语义核对。

`SIGNATURE_RECIPES` 是进程级只读静态切片，没有运行时注册、缓存或可变全局状态。`ErrFunctionNotExists` 也是零状态静态值，仅用于生成错误。递归构建的所有可变状态（参数向量、Datum、ScalarFunction coercibility）都由当前调用栈独占。

`Constant::from_datum` 为 `ValueList` 值推断默认类型：无符号整数会补 `UnsignedFlag`，时间使用值自身的 MySQL 类型，JSON/vector 使用专用类型码，未知 Kind 落到 `TypeUnspecified`。这一推断是 ValueList 常量的类型来源，与普通字面量从 PB `FieldType` 复制元数据的路径不同。

## 依赖与调用关系

模块装配链为：`pkg/expression/lib.rs` → 私有 `distsql_builtin_kernel` → 本文件。测试链为：`lib.rs` 的 `cfg(test) expression_distsql_builtin` 再导出内核符号 → `distsql_builtin_34_aster_unit_test.rs`；`distsql_builtin_test.rs` 又 `include!` 同一对等测试套件并追加历史边界测试。

RustCodeGraph 的直接调用边显示：

- `PBToExprs` 调用本文件 `PBToExpr`；测试也直接调用两者。
- `PBToExpr` 调用 `newDistSQLFunctionBySig` 以及所有类型专用 `convert*`/`decodeValueList` 函数。
- `newDistSQLFunctionBySig` 调用 `getSignatureByPB`、`deriveCollation`、表达式类型访问器和 coercibility setter。
- `getSignatureByPB` 调用 `PbTypeToFieldType`、`newBaseBuiltinFuncWithFieldType` 和上下文的 `get_max_allowed_packet`。
- `PbTypeToFieldType` 被 `FieldTypeFromPB`、签名构建、多个字面量转换函数以及对等测试调用。

图查询会因同名符号把 `pb_to_expr_runtime.rs::PBToExpr` 显示为候选相关节点；源码装配核验表明两套实现并行存在，并非前者调用本文件。crate 对外调用者实际进入 `pb_to_expr_runtime.rs`，该文件产生正式 `ExprBox`，并通过 `NewFunctionBase` 接入表达式注册/构建系统。

下游依赖主要是 crate 根提供的 `types`（Datum、FieldType、Time、Duration、Enum、VectorFloat32）、`codec`（整数/浮点/decimal/Datum 编解码）、`collate::ProtoToCollation`、`tipb` 协议类型及 `chrono_tz`。本文件不访问执行器、存储、网络或磁盘。

## 错误处理与边界

可恢复错误统一为 `ExpressionError`。底层 codec、时间转换、枚举解析和向量反序列化错误通过 `ExpressionError::external` 或带十六进制载荷的 `errors::errorf` 向上传播。`PBToExprs` 与递归 child 构建均为失败即停，不返回部分表达式数组。

已验证的重要边界如下：

- 未出现在 565 项配方表中的 `ScalarFuncSig`（测试使用 `Unspecified`）返回 `FUNCTION ... does not exist`。
- 截断的 int/uint/float/decimal/duration、损坏的列引用和 ValueList wire 数据返回错误。
- JSON 即便能解码，也必须得到 `KindMysqlJSON`，否则返回 `invalid Datum.Kind()`。
- 空 `ValueList` 返回布尔 false 常量；非空列表展平为多个参数。
- enum 数值 0 返回默认空枚举；非零值必须能在 `FieldType.elems` 中解析。
- `ColumnRef` 把已解码的 `i64` 直接转换为 `usize` 并使用 `field_types[offset]` 索引。负数转换或越界可能 panic；这是本文件保留的 Go 风格上游 PB 合法性前提。正式 `pb_to_expr_runtime.rs` 则显式检查负偏移和越界并返回错误，两者不可混为一谈。
- 未识别且非 `ScalarFunc` 的 `ExprType` 会 panic；这也是协议/上游合法性断言，而非普通错误返回。
- `deriveCollation` 当前只是对等草稿：除了参数元数据长度检查，它没有复刻 Go 的完整字符集、排序规则和隐式转换推导。新增依赖其精确语义的生产功能不能只修改本文件。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道、事务或外部句柄。所有构建对象都由值所有权和 `Vec` 管理，函数退出时自动释放；输入以共享借用传入，输出取得所需克隆数据的所有权。因此同一个 `BuildContext` 可被多个只读调用共享，但文件本身没有声明额外的线程安全保证，也没有并发协调职责。

资源消耗与输入树大小相关：普通表达式递归深度等于 PB 树深度；结果与参数向量按输入长度预分配。签名分派对 565 项静态表做线性搜索，每次标量函数重建都会格式化枚举名并扫描表。`ValueList` 会一次解码全部 Datum 并为每项构造 `Constant`，其内存规模随列表长度增长。当前 `max_allowed_packet` 只记录在草稿中，不在本文件执行大小限制。

## 与 Go 版本的对应关系

权威对照文件是 `pkg/expression/distsql_builtin.go`。Rust 保留了 Go 的主要控制流和命名：`PbTypeToFieldType`、`getSignatureByPB`、`newDistSQLFunctionBySig`、`PBToExprs`、`PBToExpr`、`convert*` 及 `decodeValueList` 均有直接对应项；`SIGNATURE_RECIPES` 把 Go 巨型 `switch sigCode` 的 case 与构造表达式压成数据表。

保持一致的语义包括：完整字段元数据复制；先构造 builtin 再推导 collation；递归 children；ValueList 展平及空列表折叠 false；TIMESTAMP 的时区转换；decimal precision/fraction；JSON Kind 校验；enum 0 特例；未知函数签名报错；畸形 wire 数据停止转换。

仍有明确迁移差异：

- Go `getSignatureByPB` 构造可执行的具体 `builtinFunc` 并设置 PB code；本文件只返回 `BuiltinFuncDraft`，构造器是字符串。
- Go `newDistSQLFunctionBySig` 使用完整 `deriveCollation` 和真实 `ScalarFunction`；本文件使用简化 `DerivedCollation` 与自有 `ScalarFunction`。
- Go 表达式实现可继续 `Eval`；本文件类型没有求值接口。
- Rust 本文件 `decodeValueList` 用 `Constant::from_datum` 推断返回类型；Go 对照只把 Datum 放入 `Constant`，其 `RetType` 为空。正式 Rust runtime 也单独推断参数类型，但使用 `types::InferParamTypeFromDatum`。
- 本文件的列边界保留直接索引；正式 Rust runtime 已加强为显式负数/越界错误。

Go 测试 `pkg/expression/distsql_builtin_test.go` 还覆盖真实表达式求值、大量函数和新排序规则开关；本文件的独立 Rust 测试只证明协议重建/分派对等，不足以证明 Go 运行时求值全集已由这个文件实现。

## 扩展指南

新增或变更 TiPB 签名时，先核对 `pkg/expression/distsql_builtin.go::getSignatureByPB` 的对应 case，再更新 `SIGNATURE_RECIPES`。必须同步维护 `distsql_builtin_34_aster_unit_test.rs` 中的数量、唯一性、首尾或代表性构造器断言，并检查正式运行时 `pb_to_expr_runtime.rs::PBSignatureFunctionName` 是否也需要新增 AST 名称映射；仅增加 recipe 不会让生产运行时获得新函数。

新增协议字面量类型时，应在 `PBToExpr` 的字面量 match 中接线独立 `convert*` 函数，明确其 `Datum::Kind`、返回 `FieldType`、wire 错误和空值行为。对应测试应放在独立测试文件 `pkg/expression/distsql_builtin_34_aster_unit_test.rs` 或 `pkg/expression/distsql_builtin_test.rs`，不要把测试嵌入本源文件；同时同步正式 `pb_to_expr_runtime.rs` 及其独立 `pb_to_expr_runtime_test.rs`，避免两套解码路径漂移。

修改字段类型转换时应覆盖所有七类元数据字段，并同时核对排序规则编号转换。修改 time、enum、JSON、vector 或 ValueList 时，要保留测试中的时区、enum 0、Kind 校验、损坏 wire 和空列表边界。

若要把本文件提升为生产实现，不能只把私有模块公开：必须用 crate 正式 `ExprBox`/`BuildContext`/`ScalarFunction` 取代草稿类型，将 recipe 映射接到真实 builtin 工厂，复刻完整 collation/coercibility 逻辑，并证明所有对外调用从 `pb_to_expr_runtime.rs` 安全迁移。主要风险是签名覆盖漂移、排序规则与隐式 cast 差异、列越界 panic、ValueList 类型推断差异，以及线性查表和大列表带来的性能成本。

## 验证依据

本说明基于以下直接证据：

- 源文件：`pkg/expression/distsql_builtin.rs`，重点符号为 `SIGNATURE_RECIPES`、`getSignatureByPB`、`newDistSQLFunctionBySig`、`PBToExprs`、`PBToExpr` 和全部 `convert*` 函数。
- crate/装配：`pkg/expression/Cargo.toml`；`pkg/expression/lib.rs` 中的 `distsql_builtin_kernel`、正式 `pb_to_expr_runtime` 再导出、`cfg(test) expression_distsql_builtin` 适配和两个测试模块挂载。
- 正式 Rust 运行时对照：`pkg/expression/pb_to_expr_runtime.rs`，用于确认 crate 对外 API、`ExprBox`/`NewFunctionBase` 接线以及更严格的列边界。
- Rust 独立测试：`pkg/expression/distsql_builtin_34_aster_unit_test.rs` 和 `pkg/expression/distsql_builtin_test.rs`，覆盖 565 项配方、FieldType 元数据、所有已支持字面量族、列/enum/ValueList 和畸形 wire 路径。
- Go 对照：`pkg/expression/distsql_builtin.go` 与 `pkg/expression/distsql_builtin_test.go`，用于核对函数结构、switch 分派、时区/排序规则、错误边界及真实求值覆盖。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；查询了 `PBToExpr`、`newDistSQLFunctionBySig`、`getSignatureByPB`，并检查它们的 callers/callees。图证据确认内部主链和测试调用；同名 runtime 节点由 `lib.rs` 与源码接线进一步消歧。

本任务为纯文档分析，未运行 Cargo 或代码测试。结构验证要求目标文档存在，并且以下固定章节恰好各出现一次：文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据。
