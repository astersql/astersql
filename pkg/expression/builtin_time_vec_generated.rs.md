# `pkg/expression/builtin_time_vec_generated.rs`

## 文件定位

该文件属于 `astersql-expression` crate；`pkg/expression/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `pkg/expression/lib.rs` 通过 `#[path = "builtin_time_vec_generated.rs"] mod builtin_time_vec_generated_kernel;` 将它作为私有模块编入。文件头说明它对应 Go 生成文件 `pkg/expression/builtin_time_vec_generated.go`，集中承载 `ADDTIME`、`SUBTIME` 和 `TIMEDIFF` 的列式求值移植。

目前它是一个自包含的 Rust 移植内核，而不是表达式主框架的完整接线：它自行定义 `Chunk`、`Column`、`Expression`、`BuiltinBase` 和各签名结构体。仓库搜索未发现生产 Rust 模块直接导入或构造这些签名；直接 Rust 调用位于独立测试 `builtin_time_vec_generated_28_aster_unit_test.rs` 和 `builtin_time_vec_generated_test.rs`。相对地，Go 的 `builtin_time.go` 会在函数类构建阶段按参数类型选择同名签名，`builtin_time_vec_generated.go` 再提供实际向量化方法。`distsql_builtin.rs` 中的同名构造器只是 Go 接线配方字符串，不是对本模块的 Rust 调用。

## 核心职责

- 用 `define_signatures!` 生成 30 个双参数签名壳，覆盖 ADDTIME/SUBTIME 的 datetime、date、duration、string 和恒 NULL 组合，以及 TIMEDIFF 的兼容类型组合。
- 先通过 `Expression::VecEvalTime`、`VecEvalDuration` 或 `VecEvalString` 整列求值参数，再逐行传播 NULL、进行时间运算，并将结果写成 `Column::Times`、`Durations` 或 `Strings`。
- 对字符串操作数执行 MySQL 风格的时长/日期时间判别和解析；ADDTIME/SUBTIME 根据签名决定输出类型，TIMEDIFF 只允许同类时间形态相减。
- 保存与 Go 向量路径相关的边界语义：零日期时间变 NULL、binary 字符串参数提前返回全 NULL、解析截断转警告、时差越界按上下文报错或警告并钳位。
- 通过所有签名的 `vectorized() -> true` 声明这些入口可走列式路径。这里的声明目前主要由 Rust 回归测试验证；尚未发现 Rust 优化器对这些本地签名的生产选择链。

## 主要符号

- `Chunk` 只保存 `rows`，`NumRows()` 是所有输出长度和循环次数的基准。
- `Column::{Empty, Times, Durations, Strings}` 是本文件的简化列模型。三种私有访问器会检查列类型，类型不符时返回 `EvalError::Message`。
- `FieldType { decimal, binary }` 保存分数秒精度和 binary 标志；前者参与 Duration 构造及 TIMEDIFF 字符串解析，后者控制 string/string 加减的短路。
- `EvalContext` 通过 `Arc<Mutex<Vec<String>>>` 收集警告，并用 `truncate_as_warning` 决定 TIMEDIFF 越界是警告钳位还是错误；它同时实现 `types_time::TimeContext`。
- `Expression` 提供三种列式求值方法和 `GetType`。默认求值方法明确报“不支持”；`LiteralExpression` 是本模块当前唯一实现，用于把现成列交给签名测试。
- `BuiltinBase` 保存两个 `Rc<dyn Expression>` 和返回 `FieldType`。`define_signatures!` 为每个签名生成仅含 `base` 的结构体及构造器。
- `DURATION_PATTERN`、`is_duration`、`get_fsp_for_time_add_sub` 和 `parse_duration_for_generated_path` 负责 ADDTIME/SUBTIME 的字符串时长识别、FSP 选择以及“警告 + NULL”转换。
- `time_op`、`duration_op`、`string_duration_op` 是加减运算核心；八个 `eval_*` 加减助手复用它们处理不同输入/输出列组合。
- `StringTemporal`、`convert_string_to_temporal`、`calculate_time_diff`、`calculate_duration_time_diff` 和七个 TIMEDIFF `eval_*` 助手处理字符串分类、同类约束与范围检查。
- `fill_null_time`、`fill_null_duration`、`fill_null_string` 生成与输入行数一致的恒 NULL 列；`impl_vectorized!` 为全部签名生成 `vectorized()`。

## 执行流程

1. 调用方构造 `BuiltinBase`，提供左右 `Expression`、参数 `FieldType` 和结果 `FieldType`，再构造一个具体签名。
2. 具体 `vecEvalTime`、`vecEvalDuration` 或 `vecEvalString` 只做薄派发。例如 `builtinAddDatetimeAndDurationSig::vecEvalTime` 调用 `eval_datetime_duration(..., true)`，对应 SUBTIME 签名传 `false`；TIMEDIFF 签名派发到各自的类型组合助手。
3. `eval_time_arg`、`eval_duration_arg`、`eval_string_arg` 请求子表达式整列求值，并用 `validate_column_rows` 强制列长等于 `Chunk::NumRows()`。这是对 Go 代码按索引遍历所有行的显式保护，避免 Rust `zip` 静默截短。
4. ADDTIME/SUBTIME 助手按行合并语义上的 NULL：任一输入为 `None` 时输出 `None`；datetime/date 的零值也输出 NULL。duration 以微秒 `i64` 保存，运算时包装成 `types_time::Duration`。
5. 字符串加减先用正则判定是否像 duration。左字符串若是 duration 就走 `Duration::Add/Sub`，否则按 DATETIME 解析后走 `Time::Add`；结果按是否含微秒设置 `MinFsp` 或 `MaxFsp`。date 输入会先把 `Time` 类型改为 `mysql::TypeDatetime`，然后输出格式化字符串。
6. string/string 加减在求值右参数前检查右表达式的 `FieldType.binary`；若为真，直接输出全 NULL。否则右字符串必须解析为 duration，再与左字符串表示的 duration 或 datetime 运算。
7. TIMEDIFF 的字符串先由 `convert_string_to_temporal` 分类：去除符号及小数后，整数部分长度至少 12 且能解析为 DATETIME 时视为 `Time`，否则按 `Duration` 解析；显式 time/duration 与字符串、或两个字符串，只在两侧分类一致时计算，混合类型返回 NULL。
8. `calculate_*_time_diff` 将差值交给 `handle_truncated_difference`。合法范围直接输出；越界时依据 `truncate_as_warning` 选择警告后钳位到 `types_time::MinTime/MaxTime`，或返回错误。

## 数据与状态

列中的 NULL 由 `Vec<Option<T>>` 表示，而不是单独 bitmap。时间值使用 `types_time::Time`，时长列只保存 `Duration.Duration` 的微秒 `i64`；需要运算时再补入 FSP。每次求值都会按 `Chunk::NumRows()` 新建输出 `Vec`，输入列只借用、不原地修改。

`EvalContext` 是唯一跨行可变状态：克隆上下文会共享同一个警告向量，因此多个运算产生的解析/截断警告会累计。`BuiltinBase` 和签名对象仅保存参数表达式及类型元数据，求值本身不缓存行状态。`DURATION_PATTERN` 是进程内惰性初始化的只读正则。

重要不变量包括：参数列长度必须精确匹配输入行数；输出列长度始终等于输入行数；恒 NULL 签名不求值参数；TIMEDIFF 的结果列始终是 duration 微秒；string/string TIMEDIFF 只有两侧同为 duration 或同为 datetime 才非 NULL。

## 依赖与调用关系

crate 边界由 `pkg/expression/Cargo.toml` 给出。本文件直接使用外部 `regex` 和内部路径依赖 `types-time`；还通过 crate 根的 `mysql` 常量区分 DATETIME 类型。标准库依赖为 `Rc`、`Arc`、`Mutex` 和 `LazyLock`。

Rust 上游证据是 `pkg/expression/lib.rs` 的模块声明和两个 `#[cfg(test)]` 测试模块。`builtin_time_vec_generated_28_aster_unit_test.rs` 直接构造各签名并调用 `vecEval*`；`builtin_time_vec_generated_test.rs` 复用该回归套件，并另测短列拒绝。仓库搜索没有发现测试之外的 Rust 构造调用，所以当前不能声称它已经接入 SQL 请求、优化器或执行器主链。

下游调用关系由 RustCodeGraph 可解析到辅助函数层。例如 `eval_datetime_duration` 调用 `eval_time_arg`、`eval_duration_arg`、列访问器、`Chunk::NumRows` 和 `time_op`；其他组合遵循相同结构。宏生成的签名没有在图索引中展开，因此其入口到辅助函数的边由源码薄派发和测试调用共同验证。

Go 主链对照更完整：`pkg/expression/builtin_time.go` 的 `addTimeFunctionClass.getFunction` 和 TIMEDIFF 函数类按参数类型构造同名签名并设置 protobuf code；`pkg/expression/builtin_time_vec_generated.go` 实现这些签名的 `vecEval*`；`pkg/expression/builtin_time_vec_generated_test.go` 用生成 case 表验证向量实现。`pkg/expression/distsql_builtin.rs` 保存的 `&builtin...Sig{base}` 是迁移配方文本，不会调用本文件。

## 错误处理与边界

- 子表达式求值失败、列类型错误、列长不匹配以及 `types_time` 加减溢出通常通过 `EvalResult` 立即向上传播。
- ADDTIME/SUBTIME 的右字符串若不符合 duration 正则，直接得到 NULL；符合正则但解析失败时，`parse_duration_for_generated_path` 追加警告并返回 NULL。
- `string_duration_op` 的左字符串解析失败也追加警告并返回 NULL。date 加减中的时间运算错误被转成警告和 NULL；datetime/duration、duration/duration 等路径的运算错误则向上传播。
- `convert_string_to_temporal` 自身不会吞掉解析错误或追加警告；TIMEDIFF 调用者使用 `?` 传播该错误。源码注释中“失败记警告并返回 None”与当前实现不一致，应以函数体为准。
- 零 datetime/date 被视为 NULL；普通 duration 的零值仍可参与运算。任一列值为 NULL 均逐行传播为 NULL。
- `handle_truncated_difference` 是严格模式边界：默认 `truncate_as_warning = true`，记录警告并返回钳位值；设为 `false` 时返回 `EvalError::Time`。
- 警告互斥锁若 poisoned，`append_warning`/`warnings` 会因 `expect` panic；这不是可恢复的 `EvalError` 路径。

## 并发与资源生命周期

`EvalContext` 的警告容器由 `Arc<Mutex<_>>` 保护，克隆后可共享警告状态；`DURATION_PATTERN` 由线程安全的 `LazyLock` 初始化。除此之外没有线程、异步任务、通道、锁顺序或事务资源。

签名保存 `Rc<dyn Expression>`，因此对象本身默认不是 `Send`/`Sync`，不能仅凭 Go 的线程安全签名清单推断 Rust 对象可跨线程或跨会话共享。`builtin_threadsafe_generated.rs` 中同名字符串属于另一套生成元数据，也不改变这里 `Rc` 的类型性质。

Rust 实现每次调用拥有输入借用和新输出向量，没有 Go 版本 `bufAllocator.get/put` 的临时列池及 `defer` 生命周期；离开函数后局部列自动释放。警告则随最后一个 `EvalContext` 克隆释放。

## 与 Go 版本的对应关系

签名集合和入口名与 `builtin_time_vec_generated.go` 一一对应，核心语义也保留：整列求值、NULL 合并、零时间处理、字符串时长判别、date 转 datetime、TIMEDIFF 同类相减、恒 NULL 签名，以及所有签名返回 `vectorized() == true`。

实现形态存在明确差异：Go 使用真实 `chunk.Chunk`/`chunk.Column`、`baseBuiltinFunc` 和 buffer allocator，并由函数类及 protobuf 接线进入生产表达式系统；Rust 文件使用本地简化门面、`Vec<Option<_>>` 和 `LiteralExpression`，目前没有生产构造者。Go 以 `terror.ErrorEqual` 区分可警告的截断错误；Rust 加减路径用本地分支选择警告或传播。Go 的列长度由 Chunk API 和索引循环隐含保证，Rust 额外用 `validate_column_rows` 显式拒绝不匹配列。

测试对应关系也分层存在：Go 的 `builtin_time_vec_generated_test.go` 是生成 case/benchmark；Rust 同名测试文件大量保存这些 case 的迁移草稿，但两个同名 `#[test]` 会调用 `run_generated_time_vector_parity_suite()` 执行真实内核回归。真正精炼的行为断言在 `builtin_time_vec_generated_28_aster_unit_test.rs`，覆盖 NULL/零值、解析警告、binary 短路、字符串分流、恒 NULL、TIMEDIFF 同类约束及全部 vectorized 标志。

## 扩展指南

新增参数组合或签名时，应同时更新 `define_signatures!`、对应的 `vecEval*` 薄入口、`impl_vectorized!` 列表，并选择或新增一个按输出类型正确写列的 `eval_*` 助手。若增加新列类型，还必须扩展 `Column`、类型访问器、`validate_column_rows` 和 `Expression` 求值接口，确保不会因 `zip` 截短而遗漏行。

修改字符串解析、FSP 或错误语义时，应优先对照 `builtin_time_vec_generated.go`、`builtin_time.go` 中的标量实现和 `types-time` API；需要明确每条错误路径是“错误”“警告 + NULL”还是“警告 + 钳位”。特别应避免依据过期注释改变 `convert_string_to_temporal` 的现有错误传播。

测试逻辑必须保持在独立文件中。行为回归应扩展 `builtin_time_vec_generated_28_aster_unit_test.rs`；生成 case/Go 同名入口或短列契约变化应同步 `builtin_time_vec_generated_test.rs`，并核对 Go 测试 `builtin_time_vec_generated_test.go`。若未来接入真实 Rust 表达式主链，还需用生产 `Expression`/Chunk 类型替代本地门面，并新增从函数构建、签名选择到列式执行的独立集成测试；仅让本文件单测通过不足以证明主链接线完成。

性能风险主要来自每次调用分配输出和参数列、每行字符串正则/解析以及共享警告锁；兼容风险集中在 MySQL 字符串分类、FSP、零时间、错误降级和 TIMEDIFF 范围。任何优化都应保留列长校验和逐行 NULL 语义。

## 验证依据

- RustCodeGraph：`status` 显示目标在索引中；`files --filter pkg/expression/builtin_time_vec_generated.rs` 报告该文件含 113 个符号；按文件分段读取覆盖 1–1375 行；`query eval_datetime_duration --kind function` 定位到第 466 行；`callees eval_datetime_duration` 验证其对 `NumRows`、列访问器、参数求值助手和 `time_op` 的调用。对宏生成签名执行查询/调用者检查时未得到 Rust 展开调用边，因此使用下列直接源码和测试补证。
- 源与 crate：`pkg/expression/builtin_time_vec_generated.rs`、`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`。
- Rust 入口/测试：`pkg/expression/builtin_time_vec_generated_28_aster_unit_test.rs`、`pkg/expression/builtin_time_vec_generated_test.rs`；仓库级搜索还核对了 `builtin_threadsafe_generated.rs` 和 `distsql_builtin.rs` 中同名元数据的性质。
- Go 对照：`pkg/expression/builtin_time_vec_generated.go`、`pkg/expression/builtin_time.go`、`pkg/expression/builtin_time_vec_generated_test.go`、`pkg/expression/generator/time_vec.go`。
- 人工复核结论：该文件存在是为了移植三类时间内建函数的生成式列执行矩阵；运行方式是“具体签名薄入口 → 类型组合助手 → 参数整列求值/长度校验 → 逐行运算 → typed Column”；安全扩展点及必须同步的独立测试如上一节所列。当前接线限制已明确标记，未将 Go 生产链或生成配方误认作 Rust 生产调用。
