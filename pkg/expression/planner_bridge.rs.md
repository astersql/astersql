# `pkg/expression/planner_bridge.rs` 逻辑说明

## 文件定位

`pkg/expression/planner_bridge.rs` 是 `astersql-expression` crate 面向规划器的表达式桥接层。模块由 `pkg/expression/lib.rs:353-354` 以 `planner_bridge_kernel` 私有模块装入，并由 `pkg/expression/lib.rs:731-737` 选择性公开导出；它不是独立 crate，也不是 SQL 入口，而是把规划器重写阶段需要的表达式构造、类型推导和特殊 builtin 状态接入表达式运行时。

上游主链集中在 `pkg/planner/core/expression_rewriter.rs`：该文件调用本模块安装 GROUPING 元数据、构造或降级 FTS 表达式、创建 JSON_SUM_CRC32、精化比较常量、推导 BETWEEN/控制函数类型以及解析时间默认值。`pkg/planner/cardinality/selectivity.rs:241` 另以 `BuildFTSToILikeExpressionFromBuiltin` 将单列全文匹配临时替换为 ILIKE 树，以复用 TopN/直方图选择率路径。crate 边界由 `pkg/expression/Cargo.toml` 确认：包名为 `astersql-expression`，本文件直接使用的外部依赖包括 `chrono`、`chrono-tz`、`crc32fast`、`protobuf`、`tipb` 和 `types-dependency`。

## 核心职责

本文件承担六组彼此相关、但都服务于“规划期表达式语义落到运行时表达式对象”的职责：

1. 用 `PlannerBuiltinBase` 统一保存参数、返回类型、 protobuf 签名码、排序器、collation 信息和跨会话共享状态，并为本文件的三个特殊 builtin 提供通用 trait 行为。
2. 用 `BuiltinGroupingImplSig` 保存并执行规划器写入的 GROUPING mode/marks 元数据，同时支持 protobuf 元数据序列化和恢复。
3. 用 `BuiltinFtsMysqlMatchAgainstSig` 表示不能在普通表达式求值器直接执行的 MATCH...AGAINST，并提供受限的 ILIKE 回退树构造与选择率替代入口。
4. 用 `BuiltinJsonSumCrc32Sig` 校验 JSON 数组元素目标类型，把每个元素转换成稳定字符串后累计 CRC32。
5. 实现比较、BETWEEN、IF/IFNULL/CASE/COALESCE/LEAD/LAG 等表达式的规划期类型合并，以及整数列比较常量的边界精化。
6. 将字符串、整数或 `Datum` 默认值解析为指定 MySQL 时间类型，并区分语句当前时间与显式解析时区。

这些职责均可由 `planner_bridge.rs` 中的真实符号及其在 `builtin.rs`、`expression_rewriter.rs`、`selectivity.rs` 的调用点验证；文件并非占位或未接线门面。

## 主要符号

- `PlannerBuiltinBase`（私有）：保存 `args`、`return_type`、`pb_code`、`collator`、`collation_info`、`share_flag`。`new` 初始化排序器，`equal` 比较类型和参数语义，`safe_to_share` 委托 `builtin_threadsafe_generated_kernel::safeToShareAcrossSession`，`memory_usage` 汇总自身、类型和参数内存。
- `impl_collation_info!`（私有宏）：把 `CollationInfo` 的读写方法转发到各签名的 `base.collation_info`，减少三个 builtin 的重复实现。
- `BuiltinGroupingImplSig`（公开类型，构造器仅 crate 内可见）：`RwLock<GroupingSig>` 封装 GROUPING 状态。`SetMetadata` 将 `tipb::GroupingMode` 映射到内部 mode 并原子安装 marks；`builtinFunc` 实现提供初始化检查、元数据回读/恢复/序列化及 `evalInt`。
- `BuiltinFtsMysqlMatchAgainstSig`（私有）：用 `AtomicU8` 保存 modifier。其 `evalReal` 只允许首参数为 SQL NULL 的快速路径，其他普通求值均返回“必须使用全文索引”错误；`SafeToShareAcrossSession` 固定为 `false`。
- `BuiltinJsonSumCrc32Sig`（私有）：保留目标数组元素 `FieldType`。`converted_item` 按 string/int/real/datetime/duration 分支转换单个 JSON 元素；`evalInt` 对数组逐元素计算 `crc32fast::hash` 并以 wrapping addition 累加。
- `build_builtin`（crate 内）：被 `pkg/expression/builtin.rs:6357` 调用，只识别 `ast::Grouping` 与 `ast::FTSMysqlMatchAgainst`，未知名称返回 `None` 让通用构造链继续处理。
- FTS 公开入口：`SetFTSMysqlMatchAgainstModifier`、`ValidateFTSSearchStringForLikeFallback`、`BuildFTSToILikeExpression`、`BuildFTSToILikeExpressionFromBuiltin`。
- JSON 公开入口：`BuildJSONSumCrc32FunctionWithCheck`。
- 类型入口：`GetAccurateCmpType`、`ResolveType4Between`、`InferType4ControlFuncs`、`InferType4ControlFuncsVariadic`；其私有辅助包括 `base_cmp_type`、`max_length`、`set_decimal_from_args`、`set_flen_from_args`。
- 常量与时间入口：`RefineComparedConstant`、`GetTimeValue`；其私有辅助为 `clone_constant_with_type`、`try_convert_constant_int`。
- `PlannerGroupingMode`：对 `tipb::GroupingMode` 的公开再导出，供规划器侧使用同一枚举契约。

## 执行流程

GROUPING 流程从 `build_builtin` 创建带 unsigned BIGINT 返回类型的 `BuiltinGroupingImplSig` 开始。`pkg/planner/core/expression_rewriter.rs:4910` 在重写时调用 `SetMetadata`；内部 kernel 校验 mode 与 marks 后才成为已初始化状态。执行时 `evalInt` 先求 grouping id，NULL 原样传播，否则在读锁内调用 `GroupingSig::eval`。计划序列化通过 `metadata` 生成 `tipb::GroupingFunctionMetadata`，反序列化则由 `restoreGroupingModeAndMarks` 重建集合。

FTS 构造先由 `build_builtin` 验证至少两个参数、首参数为字符串或 NULL 常量、后续参数均为字符串列。规划器在 `expression_rewriter.rs:4292` 写入 modifier；直接表达式回退在 `expression_rewriter.rs:4404-4428` 先校验 token，再调用 `BuildFTSToILikeExpression`。自然语言模式把“列 × 空白分词”的谓词组成 DNF；布尔模式把 `+` 必含词按“每词跨列 OR、词间 AND”组织，把 `-` 排除词包装为 NOT，并仅在没有必含词时让可选词形成正向过滤。单谓词固定为 `IFNULL(column ILIKE '%term%' ESCAPE '\\', 0)`。选择率入口 `BuildFTSToILikeExpressionFromBuiltin` 只接受单列、常量搜索串，从签名读取 modifier 后复用同一构造器。

JSON_SUM_CRC32 流程由 `expression_rewriter.rs:3130` 调用 `BuildJSONSumCrc32FunctionWithCheck`。构造期继承源表达式可空性，要求目标是 JSON array、源是 JSON，拒绝 year/json/float/decimal 数组元素、非 UTF8MB4/bin 字符串和未指定长度的 char/binary BLOB。执行期要求实际值为 JSON 数组，对每个元素严格核对 JSON type code、转为目标类型的文本形式、计算 CRC32 并累加。

类型流程在通用 builtin 创建和 planner 重写两侧共用。`GetAccurateCmpType` 先做基础类型合并，再依次修正 vector、JSON、时间、duration、decimal-vs-string-constant 和时间列-vs-常量场景；`ResolveType4Between` 对三个参数折叠基础比较类型，再处理 temporal 和 binary literal；`InferType4ControlFuncsVariadic` 排除 NULL 参数后聚合字段类型，推导 flag/decimal/collation/charset/flen，并恢复可空性与 enum/set、datetime 细节。`RefineComparedConstant` 对整数目标执行转换；不等式按操作符选择 ceil/floor，等值比较识别永远不可能相等的非整数常量。

`GetTimeValue` 先选择解析 `TypeContext`（显式时区只影响解析），再分派字符串、`i64` 或 `Datum`。`current_timestamp`/`current_date` 从 `EvalContext::CurrentTime` 取得语句时间，后者清零时分秒；普通字符串和数字调用 types time parser，最终写入 `Datum::SetMysqlTime`。

## 数据与状态

`PlannerBuiltinBase.args` 和 `return_type` 是签名的主要不可变语义；`pb_code`、`collator`、`collation_info` 可由 `builtinFunc`/`CollationInfo` 接口更新。克隆会深克隆参数和字段类型，并依据返回类型重建 collator；`share_flag` 的当前数值被复制到新的 `Arc<AtomicU32>`，克隆体不与原对象共享同一原子实例。

GROUPING 元数据是唯一由 `RwLock` 保护的复合状态。`SetMetadata` 在写锁内交给内部 `GroupingSig::set_metadata`，读取、执行和序列化在读锁内完成。FTS modifier 使用 `Arc<AtomicU8>`，但 `Clone` 同样复制当前值到新的原子；由于签名明确返回不可跨 session 共享，原子主要保证本对象的并发读写安全，而不是授权会话间共享。JSON 数组目标类型在构造后保持不变。

类型推导、常量精化和 FTS/JSON 表达式构造主要使用局部值，不保留跨调用状态。`GetTimeValue` 读取 `EvalContext` 的语句当前时间；显式时区通过派生的 `TypeContext` 使用，不修改共享上下文。

## 依赖与调用关系

上游关系如下：

- `pkg/expression/builtin.rs:6357` → `build_builtin`，将两个特殊函数接入通用 `NewFunction` 构造链；同文件 `5245-5473` 调用比较精化和控制函数类型推导。
- `pkg/expression/aggregation/base_func.rs:602` → `InferType4ControlFuncs`，用于聚合相关返回类型合并。
- `pkg/planner/core/expression_rewriter.rs:3130,3761-3889,4292-4428,4496,4598-4680,4910,5535` → JSON、比较、FTS、BETWEEN、控制函数、GROUPING 和时间入口。
- `pkg/planner/core/fulltext_to_like.rs:29` → `BuildFTSToILikeExpression`，该文件是 planner 侧薄封装。
- `pkg/planner/cardinality/selectivity.rs:241` → `BuildFTSToILikeExpressionFromBuiltin`，用于统计估算替代。

下游依赖包括：crate 内 `builtin_grouping_kernel::GroupingSig` 与 `builtin_threadsafe_generated_kernel`；表达式核心的 `Expression`、`ScalarFunction`、`Constant`、`NewFunction`、CNF/DNF 组合器和 collation 推导；`types-dependency` 的 FieldType/Datum/BinaryJSON/time 转换；`tipb` 的 scalar function code 与 GROUPING protobuf；`crc32fast` 的哈希；`chrono`/`chrono-tz` 的时间和时区。`pkg/expression/Cargo.toml` 表明这些依赖均属于 `astersql-expression` 的正式依赖，不是测试专用接线。

RustCodeGraph 的文件节点将本文件标记为被 9 个已索引文件使用，并能定位上述主要符号；逐符号 `callers/callees` 在当前索引未输出边，因此具体行级调用关系由精确 `rg` 对所有 Rust 源补证，而非从空图结果推断。

## 错误处理与边界

GROUPING 的无效 mode、marks 或 poisoned 写锁通过 `Result<_, Error>` 返回；部分只读 trait 方法因签名无法返回错误而选择 `expect` 或 `Option::None`，因此锁中毒在 `Clone`、`groupingMetaInitialized` 会 panic，而元数据读取可降级为 `None`。未初始化 GROUPING 在执行 kernel 中会报错，NULL grouping id 则返回 SQL NULL。

FTS 的边界非常严格：搜索串必须是常量字符串或 NULL；匹配目标必须是字符串列；ILIKE 回退仅接受 ASCII 字母数字或非 ASCII 字符，布尔模式仅允许 token 开头的单个 `+/-`，拒绝通配符、孤立符号、query expansion、未知 mode、空列集合。空搜索和仅排除词返回常量 0。普通 `evalReal` 不模拟全文索引，只对 NULL 返回 NULL，否则明确报错。选择率替代另外拒绝多列、错误函数名、参数不足及错误签名。

JSON_SUM_CRC32 对构造期类型和运行时 JSON type code 双重校验；任一元素不能转换即终止。和式使用 `wrapping_add`，溢出按二进制补码回绕而不是返回错误。CRC 输入是转换后的字符串字节，因此格式变化会直接影响结果兼容性。

类型推导可能从 collation 合并返回错误。`InferType4ControlFuncsVariadic` 对空参数返回显式错误，而 Go 版本在该条件 panic。常量精化对普通求值/转换失败保守返回原常量；溢出和“等值永不成立”等情况通过第二个 `exceptional` 布尔值交给调用者决定恒真/恒假边界。`GetTimeValue` 对无法识别的泛型输入返回 NULL Datum，对 `Datum` 的非 NULL/string/int kind 返回 `invalid default value`，解析错误原样转换为 expression error。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或外部 I/O 资源。表达式对象的生命周期由 planner 构造的表达式树及其 `Box<dyn Expression>`/`Box<dyn builtinFunc>` 所有权管理；`Clone` 为计划树复制生成独立签名对象。

GROUPING 通过 `RwLock` 允许元数据安装和读取并发协调，但正确生命周期仍要求 planner 在执行前完成 `SetMetadata`。FTS modifier 和 share flag 使用顺序一致性 `Ordering::SeqCst`；然而 FTS 签名声明 `SafeToShareAcrossSession == false`，不能把原子字段误解为可跨会话复用保证。其他特殊 builtin 通过参数递归检查共享安全性。collator 由 `Box` 独占，原子计数对象由 `Arc` 管理，离开表达式树后自动释放。

## 与 Go 版本的对应关系

主要语义对照分散在多个 Go 文件，而不是同名 `planner_bridge.go`：

- GROUPING 对应 `pkg/expression/builtin_grouping.go` 的 `BuiltinGroupingImplSig`、`SetMetadata` 和三种 grouping 算法。Rust 把 mode/marks/initialized 聚合到带锁 kernel，并提供 protobuf bytes 序列化；核心 mode 与 marks 校验意图一致。
- FTS 签名对应 `pkg/expression/builtin_fts.go`，ILIKE 回退对应 `pkg/expression/fts_to_like.go`。自然语言/布尔 mode、query expansion 拒绝、必含/排除/可选组合以及单列选择率限制均对齐。Rust validator 直接按 token 字节验证其严格子集，扩展语法仍不在回退范围。
- JSON_SUM_CRC32 对应 `pkg/expression/builtin_json.go`。两者都要求 JSON array 并按目标元素类型转换后 CRC 累加；Go 构造路径还先调用 `TryPushCastIntoControlFunctionForHybridType`，Rust 当前实现未出现这一预处理，扩展混合类型时必须专门复核此差异。
- 比较和 BETWEEN 对应 `pkg/expression/builtin_compare.go` 的 `getBaseCmpType`、`GetAccurateCmpType`、`ResolveType4Between`、`RefineComparedConstant`；Rust 保留 vector/JSON/temporal/decimal precision、ceil/floor 与 exceptional 契约。
- 控制函数对应 `pkg/expression/builtin_control.go::InferType4ControlFuncs`。Rust 额外以二参数包装器加 variadic 实现承接 Go 的可变参数行为，并对空参数返回错误而非 panic。
- 时间解析对应 `pkg/expression/helper.go::GetTimeValue`。显式时区用于普通输入解析；当前时间来自会话语句时间，`pkg/expression/planner_bridge_test.rs::current_timestamp_uses_session_timezone_like_go` 固定验证显式解析时区不会移动 statement timestamp。

相关 Go 测试包括 `pkg/expression/builtin_grouping_test.go`、`builtin_json_test.go`、`fts_to_like_test.go`、`helper_test.go`、`builtin_compare_test.go` 和 `builtin_control_test.go`（后两者通过对应生产符号与测试集合继续追踪）。Go 是语义基线，但本文只描述当前 Rust 事实，不把 Go 中尚未出现在 Rust 调用链的能力写成已支持。

## 扩展指南

新增 planner 专用 builtin 时，应先判断能否复用通用 `builtin.rs` 实现；只有确需规划器私有状态时才扩展 `build_builtin`。新增签名需实现完整 `builtinFunc`/`CollationInfo` 契约，明确 PB code、返回类型、NULL 传播、内存统计、克隆语义和跨会话共享性，并把独立测试放在 `pkg/expression/*_test.rs`，不要内嵌到生产文件。

扩展 GROUPING mode 或元数据字段时，至少同步 `SetMetadata`、`groupingModeAndMarks`、`restoreGroupingModeAndMarks`、`metadata`、内部 `builtin_grouping_kernel::GroupingSig` 和 `pkg/expression/planner_bridge_test.rs`；还需检查 tipb protobuf 兼容性及计划 clone/序列化路径。marks 的遍历顺序若影响 protobuf 或结果位序，必须保持确定性契约。

扩展 FTS 回退语法时，应同时修改 validator、parser/组合逻辑、planner 的 graceful fallback 以及 `fts_to_like_test.go`/`planner_bridge_test.rs`。ILIKE 只是受限近似，通配符、词边界、ranking、query expansion 或多列选择率不能仅放宽校验后宣称兼容；需要评估结果正确性和统计估算偏差。

扩展 JSON 数组元素类型时，修改 `BuildJSONSumCrc32FunctionWithCheck` 的允许列表与 `converted_item` 必须成对进行，并与 Go 的格式化结果逐值核对，否则 CRC 会产生兼容性变化。混合类型控制函数还应评估 Rust 缺少 Go `TryPushCastIntoControlFunctionForHybridType` 预处理的影响。

修改类型推导时，应优先保持 `builtin_compare.go`/`builtin_control.go` 的分支顺序，因为先后次序决定 temporal、JSON、decimal 和 collation 的优先级；同步 `builtin.rs` 与 `expression_rewriter.rs` 的调用假设和独立回归测试。修改 `GetTimeValue` 时需分别覆盖 current timestamp/date、显式时区、零时间、数字输入、NULL 和非法 Datum，并警惕 statement timestamp 与解析时区的语义混淆。

## 验证依据

事实读取范围：`pkg/expression/planner_bridge.rs` 全部 1519 行、`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs` 的模块声明/导出、`pkg/expression/planner_bridge_test.rs`，以及直接调用文件 `pkg/expression/builtin.rs`、`pkg/expression/aggregation/base_func.rs`、`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/fulltext_to_like.rs`、`pkg/planner/cardinality/selectivity.rs`。

Go 对照读取范围：`pkg/expression/builtin_grouping.go`、`builtin_fts.go`、`fts_to_like.go`、`builtin_json.go`、`builtin_compare.go`、`builtin_control.go`、`helper.go`，以及相关 `*_test.go` 的测试入口。Rust 独立测试直接覆盖 `grouping_metadata_is_installed_atomically`、`fts_like_fallback_accepts_only_the_supported_token_subset`、`fts_like_fallback_uses_go_modifier_bit_layout`、`current_timestamp_uses_session_timezone_like_go`；其他入口的行为证据同时来自上述调用点和 Go 对照测试。

RustCodeGraph 验证包括：`status`（11467 files、307296 nodes、1848419 edges）、目标文件 `node --file` 全文分段读取、主要符号 `query`，以及 `callers/callees` 尝试。图能确认符号和文件级“used by 9 files”，但当前逐符号边查询为空，故再用精确 Rust 全库搜索验证行级调用关系。依任务约束未运行 Cargo；最终结构校验应确认本文存在且恰含规定的 11 个二级标题。
