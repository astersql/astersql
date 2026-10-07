# `pkg/expression/builtin_cast_vec.rs`

## 文件定位

本文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs:175-176` 以私有模块 `builtin_cast_vec_kernel` 挂载。它是 Go `pkg/expression/builtin_cast_vec.go` 的独立 Rust 向量化 CAST 内核：以 `Vec<Option<ScalarValue>>` 表示一列，逐行完成 7 类求值类型之间 Go 已向量化的 49 种转换。

当前接线边界很重要：仓库内 `cast_column` 的直接 Rust 调用者只有 `builtin_cast_vec_3_aster_unit_test.rs`、`builtin_cast_vec_test.rs` 和 `builtin_cast_bench_test.rs`；`lib.rs:724-726` 也只在 `#[cfg(test)]` 下提供测试别名。因此该文件已有完整可执行内核和测试覆盖，但尚未接入生产表达式对象的 `vecEval*` 主链，不能把它描述成现有 SQL 执行路径已经调用的实现。

## 核心职责

- `SUPPORTED_CASTS` 固定允许的 49 个 `(EvalKind, EvalKind)` 组合，`cast_column` 在执行前拒绝矩阵外签名。
- `cast_column` 负责列级容量预分配、NULL 原样传播、声明源类型与实际值类型一致性检查，再把非空行交给 `cast_one`。
- `cast_one` 按源值变体分派给 `cast_int`、`cast_real`、`cast_decimal`、`cast_string`、`cast_time`、`cast_duration` 或 `cast_json`；各函数再按目标 `EvalKind` 执行具体转换。
- `CastSpec` 携带目标 `FieldType` 以及 UNION、unsigned、boolean、YEAR、FLOAT32、binary、parse-to-JSON 等 Go CAST 分支需要的元数据。
- `CastContext` 集中持有类型/时间上下文、当前时间、warning/strict 策略和警告列表；数值、字符串、时间和 JSON 辅助函数复用 `astersql-types` 的精度、舍入与解析规则。

## 主要符号

- `EvalKind`：`Int/Real/Decimal/String/Time/Duration/Json` 七类物理求值类型。Go 的 `ETDatetime` 与 `ETTimestamp` 在这里都落入 `Time`，具体 MySQL 类型码保存在 `CastSpec.target`。
- `ScalarValue`：上述七类单行值的 tagged union；`kind` 用于运行时一致性检查，`as_*` 与 `numeric_string` 主要服务断言和观察。
- `CastSpec`：转换规格。`new` 生成默认 MySQL 目标类型，`decimal/string/datetime/duration` 设置精度、长度或 FSP，`target_type` 区分 DATE、DATETIME、TIMESTAMP、YEAR 等类型码。
- `CastContext`：`warning` 将可策略化的非法时间/时长转换成 `NULL + warning`，`strict` 返回 `CastError`；`with_now` 使 Duration→Time/Year 等依赖当前日期的行为可确定。
- `CastError`：本地字符串错误包装，实现 `Display` 和 `Error`。
- `supported_casts`、`cast_column`：文件的主要可调用入口；前者暴露签名表，后者执行整列转换。
- `target_field`、`target_fsp`、`produce_decimal`、`produce_string`、`produce_real`：把规格映射为类型库要求的目标字段，并统一应用 unsigned、flen、decimal/FSP、字符集及浮点宽度约束。
- `parse_duration_value`、`parse_time_value`、`real_time_text`：统一时长/时间解析和 Real→Time 数字文本兼容规则。
- `json_to_real/json_to_int/json_to_decimal`、`time_from_json/duration_from_json/json_from_time`：JSON 类型码与标量/时态值之间的桥接。
- `decimal/json/time/duration`：会对非法字面量 `expect` 的测试构造辅助函数，不应被当作面向不可信生产输入的解析 API。

## 执行流程

1. 调用者构造 `CastSpec` 与 `CastContext`，将输入编码为 `&[Option<ScalarValue>]`。
2. `cast_column` 先以 `SUPPORTED_CASTS.contains` 检查签名；不支持时整列立即返回 `CastError`。
3. 函数按输入长度预分配输出。`None` 直接写入 `None`；`Some` 的实际 `kind()` 若与 `spec.source` 不同则立即报错，不继续处理后续行。
4. `cast_one` 按源变体选择七个 `cast_*` 之一。每个转换分支应用目标精度/长度、signedness、UNION 负数钳零、日期类型、FSP、binary padding 或 JSON 模式。
5. 成功值包装为 `Some(ScalarValue)`；可由上下文降级的非法时间/时长经 `CastContext::invalid` 变为 `None` 并累计警告，strict 模式则终止整列并返回错误。
6. `cast_column` 返回与输入等长且顺序一致的向量。它不会回滚此前已追加的临时输出，但错误时整个局部 `Vec` 被丢弃，调用者得不到部分结果。

各源族的关键分支如下：整数处理 YEAR、boolean JSON、unsigned 与 UNION；浮点处理 FLOAT32 文本、零时间和溢出边界；Decimal→Int 先以 `ModeHalfUp` 舍入；字符串可选择解析 JSON、生成 JSON string 或为 binary 类型生成 opaque JSON；Time/Duration 应用目标 FSP 并使用 `now` 完成相互转换；JSON 根据 `TypeCode` 选择字面量、数值、字符串或原生 time/duration 路径。

## 数据与状态

输入和输出均由调用方/当前函数拥有，不借用 Go `chunk.Column` 的临时列池；`Option` 是 NULL 位图，`ScalarValue` 是值载体。输出容量等于输入行数，时间复杂度为 O(n)，除字符串、JSON、Decimal 等值自身分配外，列容器额外空间为 O(n)。

`CastSpec` 在整列期间只读，保证每行使用同一源/目标声明。`CastContext` 是唯一可变共享状态：`warnings` 按行追加，`now` 为 Duration→Year/Time 和 JSON Duration→Time 提供统一基准，`type_context/time_context` 承载类型库的 location 与转换 flags。`target_field` 克隆 `FieldType` 后再合成 unsigned flag，不修改原规格。

时间值的 DATE 目标会显式把时分秒和微秒清零；FSP 经 `target_fsp` 钳制到 `0..=MaxFsp`。字符串到数字通常先 trim（Time/Duration 解析按对应类型库接口处理）；固定长 binary string 的零填充只在 `produce_string(..., pad_binary=true)` 路径发生。

## 依赖与调用关系

上游直接证据：`pkg/expression/lib.rs:175-176` 挂载模块；`lib.rs:441-445` 挂载两个独立测试模块；测试别名位于 `lib.rs:724-726`。精确仓库检索未发现生产 Rust 调用者。`builtin_cast_bench_test.rs` 直接从私有内核导入 `cast_column` 做基准。

下游主要依赖来自 `types-dependency`（`pkg/types`）：`FieldType/MyDecimal/Time/Duration/BinaryJSON`、`Produce*WithSpecifiedTp`、`StrTo*`、`ParseTime/ParseDuration`、浮点到整数转换及 JSON 类型码。文件还直接使用 `chrono`、`chrono-tz` 记录当前时间，使用 `rust_decimal::ToPrimitive` 将 Decimal 数字结果转为基础数值；这些依赖均由 `pkg/expression/Cargo.toml` 声明。MySQL 类型码和 flags 经 `crate::mysql` 使用。

逻辑调用链为 `cast_column -> cast_one -> cast_* -> 类型库解析/产出函数`。RustCodeGraph 能定位 `cast_column`（356）、`cast_one`（387）和 `CastSpec`（139），但本地索引的 `callers/callees` 对这些符号未返回边，因此上述边由文件内显式调用和跨仓库精确引用检索核验。

## 错误处理与边界

- 矩阵外签名、输入值类型与 `spec.source` 不一致始终返回 `CastError`，不受 warning 模式影响。
- `CastContext::invalid` 只覆盖显式走该入口的解析/转换失败，主要是时间与时长非法值；warning 模式追加消息并产生 NULL，strict 模式直接报错。Decimal/字符串/数值溢出及多数 `Produce*` failure 使用 `?` 返回错误，不会统一降级为 warning。
- NULL 永远不调用转换函数，因此既不产生 warning 也不依赖目标类型。
- `target_fsp` 会钳制超界 FSP；目标 flen/decimal 的截断、范围和字符集规则交由 `Produce*WithSpecifiedTp`。
- UNION 且目标 unsigned 时，多条转换路径将负数钳为零；非 UNION 的 signed/unsigned 重解释必须结合 `source_unsigned` 与目标 flag 判断。
- JSON→数值仅接受 literal、数值和 string 类型码；数组、对象及不匹配类型产生截断错误。JSON→Time/Duration 的某些不匹配类型可按上下文变 NULL。
- `String→Json` 的 binary 源生成 `Opaque`，普通源在 `parse_to_json=true` 时解析 JSON，否则创建 JSON string。JSON→String 特意不做固定 binary 零填充。
- `decimal/json/time/duration` 辅助构造器遇到非法测试字面量会 panic，这是测试便利接口的明确边界。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`cast_column` 是同步逐行执行；输入只读，输出局部拥有，出错时随栈展开释放。

`CastContext` 需要 `&mut` 是因为会累计 warning，因此同一个实例不能在多个并发求值中无同步共享；最安全的粒度是每个语句/求值任务独占上下文，或由上层显式合并警告。`with_now` 应在一次求值前固定，避免同一列的 Duration/JSON Duration 转换跨越时间边界。`BinaryJSON`、`String`、`MyDecimal` 在需要时克隆或新建，没有手工资源回收要求。

## 与 Go 版本的对应关系

Go 文件 `pkg/expression/builtin_cast_vec.go` 为每个签名提供独立 `builtinCast*Sig.vecEval*`，通过 `chunk.Column` 的 typed slice、NULL bitmap 和临时列池批量求值；Rust 将这些签名收敛为 `SUPPORTED_CASTS + cast_one + 七个 cast_*`，用枚举保持显式分支。源文件注释和 Rust 测试均把 49 个源/目标组合定义为对齐目标。

保留的 Go 语义包括：NULL 传播；UNION unsigned 负数钳零；Int YEAR/boolean JSON；Real→Time 的 3/4 位数字左补零规则；Decimal→Int half-up 舍入；目标 flen/precision/FSP；DATE 清零时间部分；binary string 零填充；`ParseToJSON` 与 opaque binary JSON；Time/Duration/JSON 时态互转及依赖当前时间的 YEAR/Time 转换。

结构差异包括：Rust 不使用 `chunk.Column` 缓冲池，warning 由本地字符串列表表示，而非完整 Go `EvalContext` 的 statement warning 机制；七类 `EvalKind` 合并 Go 的 datetime/timestamp 物理枚举差异；Rust 当前没有生产表达式签名对象的接线。`pkg/expression/builtin_cast_vec_test.go` 的向量化/标量一致性框架和 Real→Time、UNION unsigned 回归用例，是语义基线而非 Rust 运行时接线证据。

## 扩展指南

新增源/目标组合时，应同时修改 `SUPPORTED_CASTS` 和对应源族 `cast_*` 的目标分支；Rust 的穷尽匹配会提示现有源族缺少的新 `EvalKind`，但不会自动保证 Go 新增签名已进入矩阵。若新增元数据语义，优先扩展 `CastSpec`，并在 `target_field` 或专用辅助函数集中处理，避免七个转换器产生不一致分支。

涉及时间/时长的扩展必须明确：目标 MySQL 类型码、FSP 舍入、时区/location、非法值是 warning 还是错误，以及是否依赖固定 `now`。涉及 unsigned/UNION 时要覆盖负数、边界值和源 unsigned 位；涉及字符串/JSON 时要区分普通字符串、binary opaque、parse-to-JSON 和 JSON→String 不填零的特例。性能修改应保持单次列预分配与 O(n) 遍历，并关注每行字符串格式化/JSON clone 的分配成本。

测试逻辑不得放入本生产文件。应同步扩展独立的 `pkg/expression/builtin_cast_vec_3_aster_unit_test.rs`（签名矩阵、行为和 NULL），必要时在 `builtin_cast_vec_test.rs` 加精确 Go 回归，在 `builtin_cast_bench_test.rs` 加性能覆盖，并核对 `builtin_cast_vec_test.go` 与 `builtin_cast_vec.go`。若要投入生产，还需在表达式构建/求值层增加真实调用接线及端到端测试；仅令 `cast_column` 单测通过不足以证明 SQL 主链已使用它。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/expression/builtin_cast_vec.rs`（1192 行、79 个符号）；`node --file` 分段读取全文件；`query cast_column/cast_one/CastSpec` 分别定位主要入口、分派器和规格类型；`callers/callees` 无输出，故以精确引用检索补证。
- Rust 源与装配：`pkg/expression/builtin_cast_vec.rs`、`pkg/expression/lib.rs:175-176,441-445,724-726`、`pkg/expression/Cargo.toml`。
- 独立 Rust 测试：`pkg/expression/builtin_cast_vec_3_aster_unit_test.rs` 覆盖 49 签名、NULL、Real→Time、UNION unsigned、舍入与 JSON 模式；`pkg/expression/builtin_cast_vec_test.rs` 覆盖 binary padding 特例；`pkg/expression/builtin_cast_bench_test.rs` 是直接基准调用者。
- Go 对照：`pkg/expression/builtin_cast_vec.go` 的 49 个 `vecEval*` 实现及 `pkg/expression/builtin_cast_vec_test.go` 的向量化/标量一致性、Real→Time 和 UNION unsigned 回归。
- 人工复核：公开符号与内部辅助函数清单、49 项矩阵、所有七个源族分派、错误降级边界、生产接线现状均与上述文件交叉核对；本任务为纯文档分析，按任务要求未运行 Cargo。
