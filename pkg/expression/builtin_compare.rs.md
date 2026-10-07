# `pkg/expression/builtin_compare.rs`

## 文件定位

本文件是 `astersql-expression` crate 中的标量比较语义内核，源码由 `pkg/expression/lib.rs` 以 `builtin_compare_kernel` 私有模块装入，再以 `pub(crate) use builtin_compare_kernel as builtin_compare` 在 crate 内提供统一路径。它覆盖比较类型决议、七种比较算子、`COALESCE`、`GREATEST`/`LEAST`、`INTERVAL` 和比较常量精化；直接依赖由 `pkg/expression/Cargo.toml` 声明的 `chrono`、`rust_decimal` 与 `serde_json`。

当前接线边界需要特别说明：对 `pkg/expression/**/*.rs` 的精确引用搜索只发现 `builtin_compare_7_aster_unit_test.rs` 和 `builtin_compare_test.rs` 直接调用本文件 API；SQL 函数名注册、表达式构建和向量化实现分别仍位于 `builtin.rs`、其他表达式模块以及 `builtin_compare_vec*.rs`。因此它是已纳入 crate 的 Rust 标量语义/移植对照内核，但现有证据不足以称其已独立承接完整 SQL 执行主链。

## 核心职责

1. `get_base_cmp_type`、`get_accurate_cmp_type` 与 `resolve_type_for_between` 根据 MySQL 字段类型、表达式形态和特殊类型选择共同求值类型，避免 DECIMAL 列与字符串常量比较时丢失精度，并保留 JSON、向量、时间和时长的专用路径。
2. `compare_datums` 把运行时 `Datum` 转为指定 `CompareType`，统一执行 `<`、`<=`、`>`、`>=`、`=`、`!=`、`<=>`，同时保持 SQL NULL 三值逻辑和有符号/无符号整数语义。
3. `coalesce`、`greatest`、`least` 与 `interval_int`/`interval_real` 实现多参数比较函数的短路、类型聚合、NULL 处理和线性/二分搜索选择。
4. `refine_compared_constant` 与 `refine_unsigned_comparison` 为范围构建准备更精确的整型常量或恒真/恒假结果，复刻 Go 版常量精化的关键局部规则。

## 主要符号

- 类型元数据：`EvalType` 表示比较阶段的求值类型；`MysqlType` 是参与决议的 MySQL 类型子集；`FieldType` 保存类型、unsigned、NOT NULL、hybrid、flen/decimal；`ExpressionMeta` 以 `ExpressionKind` 和 `binary_literal` 补足决议所需的表达式形态。
- 运行时值：`Datum` 覆盖 NULL、整数、实数、Decimal、字符串、日期时间、Duration、JSON、Float32 向量和用于验证求值顺序的 `Error`；`CompareError` 是本内核的错误载体。
- 比较配置：`CompareType` 决定统一转换目标，`Op` 表示七种算子，`Collation` 提供 binary、`utf8mb4_bin`、`utf8mb4_general_ci` 三条字符串路径，`TemporalMode` 控制极值函数是否按日期或日期时间归一。
- 公开行为入口：`get_base_cmp_type`、`get_accurate_cmp_type`、`resolve_type_for_between`、`fix_flen_and_decimal_for_greatest_and_least`、`compare_int_values`、`compare_datums`、`coalesce`、`greatest`、`least`、`interval_int`、`interval_real`、`refine_compared_constant`、`refine_unsigned_comparison`。
- 辅助实现：`aggregate_compare_type`、`cast_datum`、`parse_datetime`、`normalize_temporal`、`extremum`、`compare_json`、`compare_vector` 和 `compare_non_null` 负责聚合、转换及各类型的具体次序。

## 执行流程

类型决议先由 `FieldType::eval_type` 得到两侧基础类型，`get_base_cmp_type` 按“字符串族、整型/hybrid、DECIMAL 与字符串、数值组合、时间与 YEAR、最后 REAL”的顺序合并；`get_accurate_cmp_type` 再依次覆盖向量、JSON、日期时间、Duration、DECIMAL 列对字符串常量以及时态列对常量的例外。`resolve_type_for_between` 对三个参数逐次合并，并为 Duration、其他时态值和全整型/二进制字面量修正结果。

普通比较从 `compare_datums` 进入：先传播任一 `Datum::Error`，再处理 NULL；普通算子遇 NULL 返回 `Ok(None)`，`NullEq` 则返回非 NULL 布尔值。非 NULL 两侧经 `cast_datum` 转换后交给 `compare_non_null`，后者按 `CompareType` 分派到整数混比、浮点/Decimal、排序规则字符串、时间、Duration、JSON 或向量比较，最后由 `Op` 将 `Ordering` 映射为布尔值。

`coalesce` 先计算所有参数的聚合类型，再从左到右跳过 NULL，首个非 NULL 值转换后立即返回；首个有效值之后的错误不会求值。`extremum` 是 `greatest`/`least` 的共享实现：空参数报错，按参数顺序在首个 NULL 或错误处结束；时态模式将值归一为字符串，否则先统一类型再顺序选择极值。

`interval_int`/`interval_real` 在目标为 NULL 时返回 `-1`。只要任一边界可空就线性寻找首个严格大于目标的非 NULL 边界；全部边界非 NULL 时使用 `partition_point` 二分，前提是调用者提供已排序边界。

常量精化从 `refine_compared_constant` 开始：把常量解释为 Decimal；整数目标对 `<`/`>=` 使用 ceiling，对 `<=`/`>` 使用 floor，分数相等比较标记 exceptional，YEAR 额外执行两位年份扩展。`refine_unsigned_comparison` 仅在左侧为 unsigned 且 NOT NULL、右侧为非正有符号常量时按算子折叠结果，否则返回 `Keep`。

## 数据与状态

所有状态都由参数和值对象显式传递；本文件没有全局可变变量、缓存或会话对象。`FieldType` 和 `ExpressionMeta` 是类型推断快照，`Datum` 是拥有所有权的运行时值，`RefinedConstant` 是优化阶段的常量表示。`cast_datum` 对字符串、JSON、向量等拥有型数据进行克隆；Decimal 和日期时间等可复制值直接复制。

关键不变量包括：`compare_non_null` 只接收已转换且非 NULL 的同类值；`compare_null` 至少一侧必须是 NULL；二分 `INTERVAL` 路径要求边界全部非 NULL 且有序；`extremum` 的直接模式在访问 `args[0]` 前已拒绝空参数；`fix_flen_and_decimal_for_greatest_and_least` 分别取所有参数的最大 flen 和最大 decimal。

## 依赖与调用关系

上游装配证据是 `pkg/expression/lib.rs` 的 `#[path = "builtin_compare.rs"] mod builtin_compare_kernel` 与 crate 内再导出。RustCodeGraph 的符号查询能定位 `compare_datums` 到本文件第 913 行；调用边命令未返回可用边，因而又用仓库精确引用搜索核验，直接调用者局限于 `builtin_compare_7_aster_unit_test.rs` 和 `builtin_compare_test.rs`。向量版本 `builtin_compare_vec.rs` 拥有独立的 `interval_int`/`interval_real` 等实现，不调用本标量入口。

内部主要调用链为：`get_accurate_cmp_type -> get_base_cmp_type`；`coalesce -> aggregate_compare_type -> cast_datum`；`greatest/least -> extremum -> normalize_temporal` 或 `cast_datum -> compare_non_null`；`compare_datums -> cast_datum -> compare_non_null`；`compare_non_null -> compare_int_values/compare_strings/compare_json/compare_vector`；`refine_compared_constant -> constant_decimal -> convert_integral -> adjust_year`。

外部库边界很窄：`chrono` 解析、格式化和比较日期时间，`rust_decimal` 保持十进制精度并完成取整/数值转换，`serde_json::Value` 提供 JSON 值树。文件没有网络、存储、事务或执行器依赖。

## 错误处理与边界

可恢复错误统一返回 `CompareError`：不支持的类型转换、Decimal/Real 越界、非法日期时间、比较类型与值不匹配、空的 `GREATEST`/`LEAST` 都会失败。`Datum::Error` 用于表现参数求值失败并验证短路顺序；`compare_datums` 总是先检查左侧再检查右侧，`coalesce` 只传播首个有效值之前的错误，`extremum` 在遇 NULL 后立即返回 NULL，因此之后的错误不可见。

部分转换有意采用兼容性回退而非错误：字符串转整数、实数或 Decimal 失败分别得到 0、0.0 或 Decimal 零；`normalize_temporal` 的日期模式解析失败保留原字符串。浮点比较的不可比较结果由 `partial_cmp(...).unwrap_or(Ordering::Equal)` 视为相等；向量用 `f32::total_cmp` 给 NaN 和有符号零稳定次序。字符串 `Utf8Mb4GeneralCi` 路径通过去尾空格和 Unicode 小写实现近似规则，不能据此推断已覆盖 Go 排序规则库的全部权重语义。

JSON 比较先按 Null、Bool、Number、String、Array、Object 排序；数组递归逐项比较后比较长度，对象先按键排序再递归比较键和值。该实现是本文件声明的局部次序，扩展时必须用 Go 版 JSON 比较测试验证兼容性。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部句柄。公开函数仅借用切片/值并返回拥有型结果，天然可重入；并发安全取决于调用者传入的数据是否被安全共享，而本文件自身不保存跨调用状态。

资源生命周期局限于函数栈和临时分配：类型聚合会创建类型向量，大小写不敏感字符串比较会分配小写副本，JSON 对象比较会创建并排序键值引用数组，字符串/JSON/向量转换可能克隆数据。对大参数列表、深层 JSON 或大向量扩展功能时，应关注这些 O(n) 分配、递归深度和逐元素比较成本。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/expression/builtin_compare.go`。Rust 的 `get_base_cmp_type` 对应 Go `getBaseCmpType`，`get_accurate_cmp_type` 对应 `GetAccurateCmpType`；`coalesce`、`greatest`、`least` 和 `interval_*` 合并了 Go 中按求值类型展开的多组 signature；`CompareType` 与 `Op` 以共享执行代码表达 Go 的“类型 × 算子”签名矩阵。`compare_datums` 对应各 `builtinLT*Sig` 至 `builtinNullEQ*Sig` 的核心标量结果语义。

常量精化只对齐 Go `compareFunctionClass.refineArgs`、`RefineComparedConstant` 和 `refineArgsByUnsignedFlag` 的局部结果规则。Go 版还包含 BuildContext、计划缓存防护、可变常量移除、Duration 的 NullEQ 特殊改写、数值常量到 datetime 的精化、表达式替换、protobuf signature 选择及 JSON cast 标志；这些上下文和接线不在本 Rust 文件中，不能把 `refine_*` 当作 Go 优化流程的完整替代。

测试对照方面，`builtin_compare_7_aster_unit_test.rs` 覆盖 Go 语义内核的 NULL、求值顺序、类型决议、所有算子、复杂类型和精化路径；`builtin_compare_test.rs` 另覆盖构建上下文排序规则快照、NULL 前错误顺序和关联 Duration 列例外。Go 的 `builtin_compare_test.go` 则是更完整的原始行为基线，包含 `TestCompareFunctionWithRefine`、`TestCompare`、`TestCoalesce`、`TestIntervalFunc`、`TestGreatestLeastFunc` 及 unsigned/nullable 精化用例。

## 扩展指南

- 新增比较类型时，应同步扩展 `EvalType`/`MysqlType`/`CompareType`、`FieldType::eval_type`、类型决议、`Datum`、`datum_compare_type`、`cast_datum` 和 `compare_non_null`；若可参与极值，还要审查 `aggregate_compare_type` 的优先级。
- 新增或修改算子时，应更新 `Op`、`compare_datums` 的 Ordering 映射、常量取整方向和 unsigned 折叠规则，并与 Go 的 `compareFunctionClass.generateCmpSigs`、对称算子及各 signature 对齐。
- 修改 NULL 或错误顺序时，必须同时检查 `coalesce`、`extremum` 与 `compare_datums`，并在独立测试文件中加入“错误位于 NULL/首个有效值前后”的回归用例；不要把 Rust 单元测试嵌入本生产文件。
- 修改排序规则、JSON、时间或向量次序时，先确认 Go 对应 helper 的真实语义；当前 `Utf8Mb4GeneralCi` 和 JSON 实现是局部内核，存在与完整 TiDB collator/JSON 比较器偏离的兼容风险。
- 修改 `INTERVAL` 时保持“可空边界走线性、全非 NULL 有序边界走二分”的分支，并在 `builtin_compare_7_aster_unit_test.rs` 同时覆盖 nullable、mixed-sign、目标 NULL 和边界等值。
- 如果要把本内核接到更多生产路径，应从 `pkg/expression/lib.rs` 的模块边界、`builtin.rs` 的函数注册/构建逻辑和现有向量实现入手，先补直接生产调用与端到端测试，不能仅依据当前 crate 内再导出推断运行时已接通。

## 验证依据

- 源码全貌：`pkg/expression/builtin_compare.rs`（1092 行），逐项核对全部枚举、结构、公开函数、私有 helper 和错误分支；文件无条件编译项、全局可变状态或内嵌测试模块。
- crate 与模块：`pkg/expression/Cargo.toml` 确认 crate 名称、`lib.rs` 入口以及 `chrono`、`rust_decimal`、`serde_json` 依赖；`pkg/expression/lib.rs` 确认 `builtin_compare_kernel` 装入、crate 内别名和两个独立测试模块。
- RustCodeGraph：`status` 显示索引含 11467 个文件、307296 个节点、1848419 条边；`query compare_datums --kind function` 定位本文件符号。`files --filter pkg/expression/builtin_compare` 未匹配且精确 callers 查询未产出可用结果，故未据此虚构调用边，改用 `rg` 的精确符号引用补证。
- Rust 测试：`pkg/expression/builtin_compare_7_aster_unit_test.rs` 与 `pkg/expression/builtin_compare_test.rs`；额外查阅 `pkg/expression/lib.rs` 的测试装配位置，确认测试与生产源文件分离。
- Go 对照：`pkg/expression/builtin_compare.go` 与 `pkg/expression/builtin_compare_test.go`，重点核对 `getBaseCmpType`、`GetAccurateCmpType`、`compareFunctionClass.refineArgs`、`refineArgsByUnsignedFlag`、`getFunction`、`generateCmpSigs` 及 Coalesce/Greatest/Least/Interval/比较 signature 测试。
- 调用范围复核：对 `pkg/expression/**/*.rs` 精确搜索本文件公开入口，只发现上述独立 Rust 测试的直接调用；`builtin_compare_vec.rs` 的同名 interval 函数为独立实现。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付结构以任务指定的 11 个固定二级标题命令验证。
