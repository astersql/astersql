# `pkg/expression/builtin_cast.rs`

## 文件定位

`builtin_cast.rs` 是 `astersql-expression` crate 内的一套可执行、类型自包含的标量 CAST 内核。`pkg/expression/lib.rs:173-174` 通过 `#[path = "builtin_cast.rs"] mod builtin_cast_kernel;` 将它作为私有模块编入 crate；它没有从 crate 根公开再导出。crate 根对生产表达式代码公开的同名 `BuildCastFunction` 来自 `expression_builtin::formal_registry`（`pkg/expression/lib.rs:374-381`），并非本文件的函数。因此，本文件当前不是完整 SQL 执行主链中 Go CAST 签名体系的替代入口，而是一个与 Go 语义对齐、由独立测试和基准直接驱动的 Rust 标量内核。

直接使用者是 `pkg/expression/builtin_cast_test.rs`（再导出内核供 `builtin_cast_4_aster_unit_test.rs` 使用）和 `pkg/expression/builtin_cast_bench_test.rs`。Cargo 边界由 `pkg/expression/Cargo.toml` 定义；本文件直接使用其中的 `chrono`、`rust_decimal`、`serde_json`、`thiserror`，基准另使用开发/普通依赖 `rand`。

## 核心职责

- 用 `FieldKind`、`MysqlType`、`FieldType` 描述 CAST 所需的最小 SQL 类型元数据，包括长度、精度、无符号、二进制、可空、JSON 解析、混合类型、字符集/排序规则和数组元素类型。
- 用 `Value` 统一承载整数、浮点、DECIMAL、字符串/字节、日期时间、时长、JSON、`VectorFloat32` 和数组运行时值，并由 `Expression::eval` 延迟执行转换。
- `BuildCastFunctionWithCheck` 构造 CAST 节点、传播可空性、验证 JSON→ARRAY 的构造条件，并在数值目标下调用 `TryPushCastIntoControlFunctionForHybridType` 处理 IF/CASE/ELT 的混合类型分支。
- `cast_value` 按目标 `FieldKind` 分派到数值、字符串、时间、时长、JSON、向量和数组转换；NULL 无条件透传。
- `CastContext` 收集非致命警告并提供当前日期及 `max_allowed_packet`；`CastError` 表示不支持的类型对或无法继续的解析/结构错误。
- `WrapWithCastAs*` 与字段长度辅助函数构造常用隐式 CAST 元数据，尽量对应 Go 文件中的包装入口。

本文件不实现 Go 版的 `functionClass`/`builtinFunc` 类型矩阵、protobuf 下推签名、chunk 行访问、会话 `StatementContext`、常量折叠或字符集转换器；这些能力仍属于 `builtin.rs`/`formal_registry` 等生产实现。

## 主要符号

- 常量：`UNSPECIFIED_LENGTH`、`MAX_DECIMAL_WIDTH`、`MAX_FSP`、三个日期/时间显示宽度常量和 `MAX_LONG_BLOB_WIDTH`，用于字段推导与范围限制。
- 类型元数据：`FieldKind` 是求值族；`MysqlType` 保留更细的 MySQL 类型码；`FieldType` 及其 `new`、`array`、`with_*` 构造器固定目标元数据。
- 时间和值：`MysqlTime` 保留 DATE/DATETIME/TIMESTAMP 及 FSP；`MysqlDuration` 允许带符号且小时超过 24，但 `new` 将范围限制为 838:59:59.999999；`Value` 是运行时联合体。
- 表达式：私有 `ExprNode::{Constant, Cast, Control}` 与公开 `Expression`。`Expression::constant`/`control` 构造节点，`Expression::eval` 递归求值；`ControlKind::{If, Case, Elt}` 只覆盖 CAST 下推所需的控制流形态。
- 构建入口：`BuildCastFunction`、`BuildCastFunction4Union`、`BuildCastCollationFunction`、`BuildCastFunctionWithCheck`、`TryPushCastIntoControlFunctionForHybridType`。
- 核心分派：`cast_value`，以及 `to_int`、`to_real`、`to_decimal`、`to_string`、`to_time`、`to_duration`、`to_json`、`to_vector`、`to_array`。
- 解析/规格化辅助：整数前缀解析、`parse_float`/`parse_decimal`、`produce_decimal`、字符串长度处理、时间和时长解析/舍入、JSON 数值转换、向量解析/格式化。
- 公共辅助：`ConvertJSON2Tp`、`CanImplicitEvalInt`/`CanImplicitEvalReal`、全部 `WrapWithCastAs*`、`adjustRetFtForCastString`、`minimalDecimalLenForHoldingInteger`、`setDataTypeDouble`、`floatLength`、`decimalPrecisionToLength`。

公开符号仅表示 Rust 模块可见性；由于 `builtin_cast_kernel` 本身是 crate 私有模块，它们目前不是 crate 的对外公共 API。

## 执行流程

1. 调用者先用 `Expression::constant` 或 `Expression::control` 建立源表达式和源 `FieldType`，再选定目标 `FieldType`。
2. `BuildCastFunction` 或 UNION/排序规则包装入口进入 `BuildCastFunctionWithCheck`。源可空时清除目标的 `not_null`；ARRAY 目标必须来自 JSON 且声明 `element_type`，否则返回 `CastError::InvalidArray`。
3. 对 Int/Real 目标，`TryPushCastIntoControlFunctionForHybridType` 检查 IF、CASE、ELT 的结果分支，把非 BIT 的 hybrid 分支先包成目标 CAST，并同步控制节点的结果类型；条件/index 参数不被包装。
4. 构造出的 `ExprNode::Cast` 保存源表达式、目标元数据和 `in_union`。真正转换延迟到 `Expression::eval`：先递归求源值，再调用 `cast_value`。
5. `cast_value` 先透传 NULL，再按目标类型分派。数值路径处理舍入、整数字符串前缀、精度钳制和 unsigned/UNION 差异；字符串路径按字符或二进制字节截断/补零；时间路径解析紧凑或分隔格式并按 FSP 舍入；ARRAY 路径逐个把 JSON 元素递归转为元素类型。
6. 可恢复问题通过 `CastContext::warn` 追加 `CastWarning` 并返回钳制/截断后的值；无法表示的来源类型、非法结构或无法解析的时间/向量等通过 `CastError` 返回。

RustCodeGraph 对 `builtin_cast.rs::BuildCastFunction` 的调用轨迹确认其下游是 `BuildCastFunctionWithCheck`；后者调用 `TryPushCastIntoControlFunctionForHybridType`。对 `builtin_cast.rs::cast_value` 的轨迹确认八类目标分派，并确认其上游为 `Expression::eval`、`to_array` 和 `ConvertJSON2Tp`。

## 数据与状态

`Expression` 同时保存 `field_type` 与 `target`；普通常量和控制节点初始化时二者相同，CAST 节点二者都设置为最终目标。源表达式保留在 `Box<Expression>` 内，因此构建后类型元数据与 UNION 标志固定，求值时只读取表达式树。

`CastContext` 是每次求值的可变状态：`warnings` 按发生顺序累积；`current_date` 用于把 TIME/DURATION 合成 DATETIME；`max_allowed_packet` 限制定长二进制补零。默认日期是 1970-01-01，默认包上限为 64 MiB。调用者若复用同一上下文，警告也会跨多次 `eval` 累积，文件不会自动清空。

`FieldType` 的 `element_type` 只对 ARRAY 有意义；`parse_to_json` 决定字符串/字节转 JSON 时是解析 JSON 文本还是生成 JSON string；`binary` 会把 charset/collation 改为 `binary` 并令 `MysqlType` 为 `String`。`in_union` 只存于 CAST 节点，主要改变负数转 unsigned 以及 unsigned Real/Decimal 的行为：UNION 对齐时负数归零，而普通 CAST 产生环绕结果并记录警告。

本文件没有全局可变状态、缓存、注册表或外部 I/O。

## 依赖与调用关系

- crate 装配：`pkg/expression/lib.rs` 私有挂载 `builtin_cast_kernel`；测试模块同样在 `lib.rs` 的 `#[cfg(test)]` 区域挂载。
- 直接上游：`builtin_cast_test.rs` → `builtin_cast_4_aster_unit_test.rs` 调用 `BuildCastFunction`、`Expression::eval` 和元数据辅助；`builtin_cast_bench_test.rs` 调用本内核的行式 Int→Int 路径，并与 `builtin_cast_vec.rs` 的列式 `cast_column` 对照。
- 内部主链：`BuildCastFunction*` → `BuildCastFunctionWithCheck` →（可选）`TryPushCastIntoControlFunctionForHybridType` → `Expression::eval` → `cast_value` → 各目标转换函数。
- 递归边：`to_array` 对 JSON array 的每个非 NULL 元素再次调用 `cast_value`，源类型固定为 JSON、目标为数组元素类型。
- 外部库：`chrono` 负责无时区日期时间与舍入后的时间运算；`rust_decimal` 负责 DECIMAL 表示/舍入/数值转换；`serde_json` 负责 JSON 与向量字面量解析；`thiserror` 派生错误显示。
- Go 对照：`pkg/expression/builtin_cast.go` 提供生产版本；`pkg/expression/builtin_cast_test.go` 与 `builtin_cast_bench_test.go` 提供语义和性能意图。

对仓库 Rust 引用的搜索只发现测试和基准显式使用 `builtin_cast_kernel`。生产文件中的未限定 `BuildCastFunction` 解析到 crate 根再导出的 `formal_registry::BuildCastFunction`，不能据名称相同推断其调用本文件。

## 错误处理与边界

`CastWarning` 覆盖截断、溢出、负数转 unsigned、有符号溢出和包大小溢出。典型可恢复路径包括：字符串仅解析数值前缀并记 `Truncated`；有限/非有限浮点或 DECIMAL 越界时钳制并记 `Overflow`；定长二进制补零超过包限制时记 `AllowedPacketOverflow` 并返回 NULL。

`CastError` 用于无法继续的情形：类型对不支持、非法整数/DECIMAL/时间/时长/JSON/向量、ARRAY 结构不合法。NULL 在 `cast_value` 分派前透传，不触发目标转换。`MysqlDuration::new` 强制小时不超过 838、分秒不超过 59、微秒不超过 999999、FSP 不超过 6。时间 FSP 被限制到 0..6；DECIMAL 的实现受 `rust_decimal::Decimal` 能力约束，并非 Go `types.MyDecimal` 65 位精度的等价存储。

需要特别注意的当前差异：`BuildCastFunction`/`BuildCastFunction4Union` 对构造错误使用 `expect`，只有 `BuildCastFunctionWithCheck` 向调用者返回错误；`_is_explicit_charset` 参数尚未参与逻辑；本文件的 `CanImplicitEvalInt/Real` 以 hybrid 元数据判断，而 Go 版明确识别 `DAYNAME`；排序规则构建器也没有 Go 版的“非字符串直接返回、相同 collation 返回、binary VarString 规避补零”等完整分支。这些都应视为当前实现事实，不能宣称与 Go 全量等价。

## 并发与资源生命周期

所有表达式、值和字段元数据都由所有权值、`Box`、`Vec`、`String` 管理，没有裸指针、锁、线程、异步任务、通道、事务或文件/网络句柄。构建函数消费 `Expression`，避免共享可变表达式树；需要复用时由调用者显式 `clone`。

求值唯一的共享式可变入口是调用者传入的 `&mut CastContext`，Rust 借用规则保证一次只能有一个可变求值者。若多线程求值，应为每个执行流准备独立上下文，或在更外层自行同步；否则警告顺序和当前日期语义会混合。临时 JSON、字符串、数组和向量在返回值或错误离开作用域时自动释放。

## 与 Go 版本的对应关系

Rust 的 `BuildCastFunction*`、`WrapWithCastAs*`、`TryPushCastIntoControlFunctionForHybridType`、`ConvertJSON2Tp` 及字段长度辅助，名称和意图直接对应 `pkg/expression/builtin_cast.go`。Rust 把 Go 按源/目标 `EvalType` 展开的众多 `builtinCastXAsYSig` 合并为 `Value` + `cast_value` 的目标分派；`CastContext` 则以精简警告列表模拟 Go `StatementContext` 的部分行为。

已保留的主要语义包括：源可空则目标可空；普通与 UNION unsigned 转换差异；字符长度与二进制字节长度的区别；DECIMAL 舍入/钳制；日期时间和时长 FSP；`parse_to_json`；JSON array 元素转换；字符串↔`VectorFloat32` 支持路径；混合类型 IF/CASE/ELT 结果分支提前 CAST；字段显示长度推导。

仍未覆盖或明显简化的 Go 能力包括：每个签名的 protobuf code 与 TiKV 下推、向量化签名、真实 charset/collation 校验、BIT/binary literal 特例、session SQL mode 与细粒度错误策略、时区、零日期、完整 MySQL 时间解析、常量折叠、Go ARRAY 的目标类型限制、完整 JSON 转换，以及 Go 的精密 DECIMAL 范围。`pkg/expression/builtin_cast_4_aster_unit_test.rs` 只覆盖选定对齐案例，不能作为全量等价证明。

## 扩展指南

- 新增目标类型：扩展 `FieldKind`、`Value`、`FieldType::new`、`kind_of` 和 `cast_value`，再实现独立 `to_*` 路径；同步检查所有 `WrapWithCastAs*`、字符串长度推导和 ARRAY 元素递归是否需要支持。
- 新增来源类型：在每个允许的目标转换函数中显式加入来源分支；不允许的组合继续返回 `CastError::Unsupported`，不要用字符串中转悄悄扩大语义。
- 修改数值规则：同时检查普通/UNION、signed/unsigned、NaN/Infinity、溢出钳制和 warning 顺序；参考 Go 的对应 `builtinCastXAsYSig`，并在 `builtin_cast_4_aster_unit_test.rs` 增加独立回归测试。
- 修改字符/二进制行为：重点核对字符数与字节数、固定宽度零填充、`max_allowed_packet`、charset/collation 元数据；若要补齐 Go 行为，应先明确本内核是否将接入生产 `formal_registry`，不要形成第三套隐式入口。
- 修改时间或 DECIMAL：记录 `chrono`/`rust_decimal` 与 TiDB 类型能力的差距，避免把库限制误当 MySQL 规则；同步覆盖边界值、FSP 进位、负时长和精度溢出。
- 测试必须继续放在独立文件：标量语义放 `pkg/expression/builtin_cast_4_aster_unit_test.rs`（由 `builtin_cast_test.rs` 挂载）；性能对照放 `builtin_cast_bench_test.rs`；Go 语义基准参考 `builtin_cast_test.go`。不要把测试内嵌回本源文件。

## 验证依据

- 源码全读：`pkg/expression/builtin_cast.rs`（1759 行），核对常量、类型、公开构建器、转换分派、警告/错误和元数据辅助。
- RustCodeGraph：`status` 显示索引可用（11467 文件、307296 节点、1848419 边）；精确 `node builtin_cast.rs::BuildCastFunction`、`node builtin_cast.rs::BuildCastFunctionWithCheck`、`node builtin_cast.rs::cast_value`、`node builtin_cast.rs::TryPushCastIntoControlFunctionForHybridType` 验证核心调用边。索引对 `Expression::eval` 的限定名查询未命中，且散列 ID 存在歧义，因此其余事实以源码和限定路径搜索复核。
- crate/入口：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`；确认直接库依赖、私有模块挂载、测试挂载，以及生产公开 CAST 入口来自 `formal_registry`。
- Rust 测试：`pkg/expression/builtin_cast_test.rs`、`pkg/expression/builtin_cast_4_aster_unit_test.rs`、`pkg/expression/builtin_cast_bench_test.rs`；覆盖字符/二进制、数值 warning、DECIMAL、时间/时长、JSON/ARRAY、向量、NULL、元数据和行/列路径对照。
- Go 对照：`pkg/expression/builtin_cast.go`、`pkg/expression/builtin_cast_test.go`、`pkg/expression/builtin_cast_bench_test.go`；核对构建器、类型矩阵、包装器、控制流下推、字段长度、数组及测试意图。
- 跨文件搜索：只有测试/基准显式引用 `builtin_cast_kernel`；生产侧未限定 `BuildCastFunction` 来自 crate 根 `formal_registry` 再导出。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；交付验证仅执行任务指定的 11 章节结构检查并人工复核上述事实链。
