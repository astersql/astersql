# `pkg/expression/builtin_other.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-expression`（`pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs:249-250` 以私有模块 `builtin_other_kernel` 编译，并仅在测试配置下通过 `expression_other` 重导出（`pkg/expression/lib.rs:808-811`）。它是 Go `pkg/expression/builtin_other.go` 中一组“其他”标量函数的可独立运行 Rust 语义内核，覆盖 `IN`、用户变量、`VALUES()`、`BIT_COUNT()`、`ROW()` 和 `GET_PARAM()`。

当前接线边界必须特别注意：仓库搜索到这些公开 Rust 符号的直接使用者是 `builtin_other_test.rs` 与 `builtin_other_22_aster_unit_test.rs`；生产表达式框架另有 `pkg/expression/builtin.rs` 以及向量化文件。因此，本文件已经进入 crate 编译边界，但不能据此声称它已替代 Go 完整函数注册、protobuf 下推或 session context 接线。

## 核心职责

- 以 `Value`、`FieldType`、`EvalType` 提供足以表达本组内置函数的本地类型模型，并用 `OtherError` 统一返回参数、转换、偏移和参数索引错误。
- 由 `InPredicate::build` 折叠严格常量、去重、缓存可哈希类型，记录运行时参数与 NULL；由 `InPredicate::eval` 实现 SQL 三值逻辑及按类型比较。
- 由 `Session` 模拟本组函数所需的会话状态：大小写不敏感的用户变量、变量类型、只读变量集合、当前 INSERT 行和计划缓存参数。
- 由 `GetVarExpr`、`ValuesFunction`、`bit_count`、`get_param`、`RowFunction` 分别保留对应 Go 控制流的核心行为。

它不负责 SQL AST 注册、通用 `Expression` trait、chunk 执行、protobuf 标量签名、告警上下文或函数向量化；这些属于相邻实现或尚未完成的集成层。

## 主要符号

- `Result<T>` / `OtherError`：模块错误边界。错误变体包括参数数量不符、值转换失败、行列越界、INSERT 值过长及计划缓存参数越界。`UnsupportedInType` 和 `UnsupportedValuesType` 当前已声明但本文件没有构造它们；Rust 的穷举 `EvalType` 分派使当前入口不会落入 Go 的 default 分支。
- `EvalType`、`MysqlType`、`FieldType`：求值类型、少量 MySQL 物理类型标志及 collation 元数据。`FieldType::bit`、`float32`、`string`、`hybrid_string` 构造特殊路径所需类型。
- `Value`：模块统一 SQL 值。`Null` 表示 SQL NULL；`eval_type`、`to_mysql_string`、`convert_to` 承担本地类型识别和转换。
- `Expr`：`Constant`、`Context`、`Column` 三类 IN 参数。只有 `Constant` 可进入哈希折叠；`Context` 对齐 Go `ConstOnlyInContext`，构建时验证类型但仍在运行期求值。
- `ConstantCache`：按 Int/String/Real/Decimal/Time/Duration 分桶的常量缓存；Json 和 VectorFloat32 使用 `None`，逐项比较。
- `InPredicate`：IN 的构建结果，保存压缩参数、非常量参数下标、NULL 标记、类型化缓存及跳过计划缓存原因。
- `Session` / `GetVarExpr`：用户变量及相关会话数据模型；`GetVarExpr::build` 可把已标只读的变量折叠成常量。
- `ValuesFunction`：保存 INSERT 列偏移与返回字段类型，`eval` 从 `Session::curr_insert_values` 取值并规范化。
- `bit_count`：MySQL 整数强制转换后统计 u64 补码中的置位数。
- `get_param`：按有符号索引读取 `Session::plan_cache_params`，非 NULL 值转 MySQL 文本。
- `RowFunction`：只保存 ROW 参数类型；`eval` 故意 panic，因为 ROW 应在表达式重写阶段展平。

## 执行流程

`IN` 的主流程分成构建和逐行求值：

1. `InPredicate::build` 要求至少两个参数；对 BIT 字段候选列表中的负整数严格常量进行剪枝，必要时记录 `Bit Column in (...)` 作为跳过计划缓存原因。
2. 对支持缓存的类型创建 `ConstantCache`。严格常量在构建期求值：NULL 只保留一次，非 NULL 去重并写入类型化集合；`Context` 只做目标类型验证，和列引用一起保留为运行时参数。
3. `InPredicate::eval` 先求针值；针值为 NULL 直接返回 `None`。随后先查常量缓存；若未命中，只遍历保留的运行时参数（无有效缓存时遍历全部候选）。任一相等返回 `Some(true)`，没有相等但见过 NULL 返回 `None`，否则返回 `Some(false)`。
4. `eval_in` 是全常量便捷入口，组装 `Expr::Constant` 后复用上述构建和求值流程。

其他入口均为同步调用：`set_user_var` 规范化名称并保存自有值；`GetVarExpr::build` 决定常量折叠或运行时读取；`ValuesFunction::eval` 检查空行、偏移、NULL 后按字段类型取值；`bit_count` 先处理 NULL/溢出特例再计位；`get_param` 校验索引并字符串化；`RowFunction::eval` 始终 panic。

## 数据与状态

`InPredicate` 的 `args` 在构建后已压缩，`non_constant_args` 中保存的是压缩后下标；二者必须同步更新。`has_null` 只记录严格常量列表中的 NULL，运行时 NULL 由每次 `eval` 局部累计。整数缓存的 `HashMap<i64, bool>` 以 i64 位型为键，以 bool 保存候选是否无符号，从而区分 `-1_i64` 与 `u64::MAX`。浮点缓存通过 `canonical_float_bits` 合并 `+0.0/-0.0`；NaN 不进入集合，也不会相等。字符串缓存使用 `collate::GetCollator(...).Key(...)`，因此比较语义由 `FieldType::collation` 决定。

`Session` 是拥有数据的普通结构，不借用输入行：`set_user_var` 克隆值，防止后续行缓冲修改污染用户变量。名称统一转小写。传入 NULL 时函数直接返回且不修改已有映射；它既不会删除旧值，也不会新建变量。`curr_insert_values` 和 `plan_cache_params` 是调用方负责装载、清理的公开向量，本文件没有语句生命周期钩子。

## 依赖与调用关系

- crate 接线：`pkg/expression/lib.rs` 声明私有 `builtin_other_kernel`；测试模块 `builtin_other_test` 与 `builtin_other_aster_unit_test` 通过测试专用 `expression_other` 重导出调用本文件。
- 下游内部调用：`eval_in -> InPredicate::build -> ConstantCache::{for_type,insert}`，随后 `InPredicate::eval -> ConstantCache::contains/values_equal`；转换辅助链包括 `int_repr`、`real_value`、`decimal_value`、`collation_key` 和 `json_values_equal`。
- 外部依赖：`std::collections` 提供缓存与会话映射；`rust_decimal` 支持 Decimal 转换/哈希；`serde_json` 表示 JSON；`thiserror` 派生错误；字符串比较经 `crate::collate` 转到 `collate-dependency`。这些依赖均在 `pkg/expression/Cargo.toml` 声明。
- 上游现状：RustCodeGraph 定位了本文件的主要定义，但 `callers/callees` 未返回跨文件调用边；仓库文本引用确认直接消费者限于独立测试。`pkg/expression/builtin_other_vec.rs` 自有一个同名 `bit_count(i64)`，不是对本文件 `bit_count(&Value)` 的调用。

## 错误处理与边界

- 构建 IN 或 VALUES 时参数数量不符返回 `OtherError::ArgumentCount`；列引用超出输入行返回 `ColumnIndex`。
- 不兼容的本地值转换返回包含原值和目标类型的 `Conversion`。文本转整数是刻意宽松的 MySQL 风格：先解析 i64，再解析 f64 并截断，否则回退为 0；与完整 Go statement context 的告警行为不等价。
- IN 的整数比较显式处理有符号/无符号组合；负有符号值不与相同位型的无符号值相等。JSON 数字跨整数/浮点编码时使用 `1e-8` 容差，递归用于数组与对象。
- `VALUES()` 在当前 INSERT 行为空时返回 NULL；偏移越界返回 `ValuesOffset`；整数路径中超过 8 字节的二进制值返回 `InsertValueTooLong`；FLOAT 返回值显式压到 f32 再升为 f64；hybrid 字符串走文本转换。
- `BIT_COUNT()` 对非有限/超出 i64 的实数、无法落入 i64 的 Decimal，以及可解析为越界浮点的文本返回 64；普通负数按二进制补码计数。NULL 返回 `Ok(None)`。
- `GET_PARAM()` 对负索引和越界统一返回 `ParamIndex`，参数为 NULL 时返回 `Ok(None)`。
- `RowFunction::eval` 的 panic 是契约而非可恢复错误；调用前必须保证表达式重写已展平 ROW。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、异步任务、文件句柄或网络资源，所有求值均同步完成。`InPredicate` 构建完成后，求值方法只读其缓存和参数；只要不通过外部可变封装修改，它可安全共享。Go `baseInSig` 对新增字段有“执行期间必须线程安全或不可变”的约束，Rust 结构通过 `&self` 求值体现了相同意图。

`Session` 的修改方法需要 `&mut self`，读取方法使用 `&self`；它本身没有跨线程同步策略。若未来把它接入真实 session 或跨线程共享，必须由上层保证语句/会话隔离及同步。变量值、常量缓存和表达式均拥有或克隆其数据，没有悬垂借用；代价是字符串、JSON、向量和表达式值可能发生克隆。

## 与 Go 版本的对应关系

- `InPredicate::build/eval` 对应 `inFunctionClass.verifyArgs`、各 `builtinInXXXSig.buildHashMapForConstArgs/evalInt`：保留 BIT 负常量剪枝、`ConstStrict` 去重、`ConstOnlyInContext` 验证、NULL 三值逻辑、整数符号规则和 collation key。JSON/Vector 与 Go 一样不使用常量哈希。
- `Session::{set_user_var,get_user_var}` 与 `GetVarExpr` 汇总对应多种 `builtinSet*VarSig`、`BuildGetVarFunction`、`convertReadonlyVarToConst` 和 `builtinGet*VarSig`。Rust 用一个 `Value` 枚举合并 Go 的多签名及 session property reader；未建模 collation、statement context、可选求值属性和完整时间错误处理。
- `ValuesFunction` 对应 `valuesFunctionClass` 及各 `builtinValues*Sig`，保留空行 NULL、偏移错误、二进制整数长度、FLOAT 精度和 hybrid 字符串分支；Rust 以单个枚举分派替代 Go 的类型化签名。
- `bit_count` 对应 `builtinBitCountSig.evalInt`：保留 NULL、补码计位和转换溢出返回 64。Rust 自行模拟文本/数值转换，而不是复用 Go 的 `EvalInt` 和 statement flags。
- `RowFunction` 对应 `rowFunctionClass/builtinRowSig`，保留“可构建但不应执行”的 panic 契约。
- `get_param` 对应 `getParamFunctionClass/builtinGetParamStringSig`，保留计划缓存参数按索引读取和字符串返回；Go 通过 EvalContext 取参数，Rust 从本地 `Session` 向量读取。

Go 文件还包含 SLEEP、命名锁等本 Rust 文件未覆盖的函数；反之，本 Rust 文件的简化类型/会话模型不能被当成完整 Go 框架的等价替换。

## 扩展指南

- 新增 `EvalType` 时必须同时审查 `Value::eval_type/convert_to`、`ConstantCache::for_type/insert/contains`、`validate_value_for_type`、`values_equal` 和 `ValuesFunction::eval` 的穷举分支；若类型不可安全哈希，应像 JSON/Vector 一样使用逐项比较。
- 修改 IN 折叠时保持三个不变量：针值始终为 `args[0]`；`non_constant_args` 指向压缩后的下标；NULL 去重不能丢失三值逻辑。还要同步整数 signedness、collation、NaN/零和 plan-cache-sensitive BIT 测试。
- 扩展用户变量或 VALUES 时，不要把测试逻辑内嵌到生产文件；应更新同目录独立测试 `builtin_other_test.rs`，复杂 Go 对齐边界可同时补充 `builtin_other_22_aster_unit_test.rs`。涉及真实表达式框架接线时，还需检查 `lib.rs`、`builtin.rs` 和 session/expression trait，而不能只改本地 `Session`。
- 修改转换规则要与 `builtin_other.go` 和 `builtin_other_test.go` 对照，特别检查 statement warning、溢出、二进制字面量、时间类型及 JSON 数字语义。当前轻量模型无法表达的行为应显式标注，而不是静默简化。
- 若让 `RowFunction::eval` 可执行，将违反 Go 的重写阶段契约；应先定位 ROW 展平链路并新增独立回归测试，而不是直接移除 panic。

## 验证依据

- 源码全量阅读：`pkg/expression/builtin_other.rs`（1043 行），核对所有模块级类型、函数、impl 与错误分支；文件没有条件编译项。
- RustCodeGraph：`status` 显示索引含 11467 文件、307296 节点和 1848419 条边；`query InPredicate/ValuesFunction/bit_count/get_param/RowFunction` 定位到本文件及相关测试；`node InPredicate` 确认其字段；针对限定符号的 `callers/callees` 没有返回跨文件调用边。
- crate 与入口：`pkg/expression/Cargo.toml`；`pkg/expression/lib.rs:249-254,558-571,808-816`。
- Rust 测试：`pkg/expression/builtin_other_test.rs`；补充边界证据 `pkg/expression/builtin_other_22_aster_unit_test.rs`。覆盖 BIT_COUNT、IN 缓存/NULL/signedness/collation/各类型、用户变量所有权和只读折叠、VALUES 类型分支、GET_PARAM 越界以及 ROW panic。
- Go 对照：`pkg/expression/builtin_other.go`；`pkg/expression/builtin_other_test.go`。重点核对 `inFunctionClass`、各 `builtinIn*Sig`、SET/GET_VAR、`valuesFunctionClass`、`builtinBitCountSig`、`getParamFunctionClass` 和 `rowFunctionClass`。
- 结构验证使用任务指定命令，要求本文件存在且恰有十一个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
