# [`pkg/expression/builtin_string_vec.rs`](./builtin_string_vec.rs)

## 文件定位

本文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），提供从 Go `pkg/expression/builtin_string_vec.go` 移植而来的字符串内建函数“行无关向量内核”。crate 根在 `pkg/expression/lib.rs:265-266` 以私有模块 `builtin_string_vec_kernel` 装入它；只有测试配置在 `pkg/expression/lib.rs:826-829` 通过 `string_vec` 重导出。因此，当前代码会参与库编译，但其公开项并不是 crate 对外 API，直接调用者也仅见于独立测试。

它与 Go 文件的架构位置并不完全等价：Go 各 `builtin*Sig.vecEvalString/vecEvalInt` 直接读取 `chunk.Chunk`、调用子表达式并写入 `chunk.Column`；本文件把这一层压缩为 `Vec<Value>` 输入、`EvalOutput` 输出，以 `StringBuiltin` 选择算法。文件头注释和 RustCodeGraph 对 `eval_rows` 的调用边都表明，这是供后续表达式签名类型接线的计算内核，不应描述为已经接入 SQL 执行主链的完整实现。

## 核心职责

- 用 `StringBuiltin` 枚举统一表示大小写、截取、定位、填充、编码转换、进制转换、Base64、格式化、翻译等字符串内建变体，并在 `eval_row` 中集中分派。
- 用 `eval_rows` 保持“一输入行对应一输出值”的批处理约束，同时跨行汇总 `EvalWarning`；遇到致命 `EvalError` 时立即终止整批。
- 显式区分二进制和 UTF-8 路径。二进制函数以字节位置和字节长度工作；UTF-8 函数通常经 `String::from_utf8_lossy` 转为字符序列，以字符位置工作（见 `map_utf8`、`left_utf8`、`substring*_utf8`）。
- 复刻关键 MySQL/TiDB 边界：NULL 传播、1 基位置、负位置、`max_allowed_packet`、`result_flen`、未知 locale/非法编码告警、严格字符集模式，以及 Go 单 rune 大小写映射。
- 提供与真实 chunk/表达式系统解耦的最小数据模型，便于独立验证移植语义；它没有负责函数签名构造、参数类型推导、chunk 缓冲复用或执行上下文接线。

## 主要符号

- `Value`：单元格值模型，包含 `Null`、`Bytes(Vec<u8>)`、`Int(i64)`、`Real(f64)`、`Decimal(String)`。`From<&str>` 只做 UTF-8 字节复制，不携带字段类型或 collation 元数据。
- `EvalWarning`：非致命诊断，包括包大小超限、未知 locale 和非法编码。
- `EvalError`：致命错误，包括缺参、类型不匹配、包大小超限、未知字符集及格式化错误；实现 `Display` 和 `std::error::Error`。
- `EvalConfig`：每批求值配置。默认 `max_allowed_packet` 为 64 MiB，`result_flen` 为 `MAX_BLOB_WIDTH`（16,777,215），并保存截断降级与严格模式开关。
- `EvalOutput`：返回 `values` 和共享的 `warnings`。告警没有行号，调用者只能知道本批出现了哪些告警。
- `TrimDirection`：公开枚举，但当前分派实际通过 `trim3` 的整数方向值解释 `LEADING`/`TRAILING`/`BOTH`；本文件中没有使用该类型。
- `StringBuiltin`：分派标签。携带状态的变体包括 `Locate*Utf8`/`InstrUtf8`/`FindInSet`/`Strcmp` 的 collation、`Convert` 的源/目标字符集、`SubstringIndex` 的 unsigned 标志，以及 `Char` 的字符集。
- `eval_rows(&StringBuiltin, &[Vec<Value>], &EvalConfig) -> Result<EvalOutput, EvalError>`：唯一公开批入口；预分配结果列，逐行调用私有 `eval_row`。
- `eval_row`：核心 `match` 分派器，把每个枚举变体转入专用函数或 `map_bytes`/`map_utf8` 通用路径。
- `arg`、`bytes`、`int`：参数提取和动态类型检查边界；缺参、错型在这里统一成为 `EvalError`。
- `packet_overflow`：包大小策略中心；按配置选择致命错误，或追加告警并要求调用函数返回 NULL。
- `find_bytes`、`split_bytes`、`join_bytes`、`repeat_prefix`、`trim_pattern`、`substring_start`：供多个内建函数复用的字节/位置算法。

## 执行流程

1. 调用者构造一个 `StringBuiltin`、若干 `Vec<Value>` 行以及 `EvalConfig`，进入 `eval_rows`（`pkg/expression/builtin_string_vec.rs:228`）。
2. `eval_rows` 为结果按行数预分配容量，新建整批共享的告警列表，并按输入顺序调用 `eval_row`；任何一行返回 `Err` 都通过 `?` 立即结束，之前计算的值不会作为部分结果返回。
3. `eval_row` 根据变体选择具体内核。例如 `Length` 与 `CharLengthBinary` 共用字节长度实现，`Locate2Utf8` 将 collation 转成是否大小写不敏感，`ExportSet3/4/5` 以不同 arity 进入同一函数。
4. 专用内核先通过 `bytes`/`int` 提取参数。多数函数只要必需参数为 NULL 就返回 `Value::Null`；例外包括 `QUOTE(NULL)` 返回字节串 `NULL`、`CONCAT_WS` 跳过分隔符之后的 NULL、`CHAR` 跳过 NULL 整数、`MAKE_SET` 跳过命中位上的 NULL。
5. 二进制路径直接切片、搜索或拼接字节；UTF-8 路径先有损解码，再按 `char` 计数或变换。`go_simple_uppercase`/`go_simple_lowercase` 特别避免 Rust 完整 Unicode 映射产生多字符展开，以匹配 Go 的简单 rune 映射。
6. 可能扩张输出的 `repeat`、`space`、`concat`、`concat_ws`、`insert_*`、`pad_*`、`to_base64`、`from_base64` 会在分配前调用 `packet_overflow`；填充和重复还检查 `result_flen` 或 blob 上限。
7. 全部行成功后返回 `EvalOutput { values, warnings }`。输出顺序与输入顺序一致，且每一行恰有一个值。

按算法族看，定位/截取函数统一使用 SQL 1 基位置并在内部换算；填充函数区分字节长度和字符长度；`FORMAT` 将小数位钳制到 0..=30 后调用 `mysql::locale_format::FormatByLocale`；`CONVERT`/`CHAR` 使用 `encoding_rs`；Base64 使用标准字母表并按 76 字符插入换行；`TRANSLATE` 反向建立映射，使 `from` 中重复字符的第一次出现最终生效，超出 `to` 的映射表示删除。

## 数据与状态

本文件没有全局可变状态。两个模块常量是 `MAX_BLOB_WIDTH` 和 `FORMAT_MAX_DECIMALS`；其余状态均由参数传入或在一次求值中局部创建。

`Value::Bytes` 同时承载文本和任意二进制数据，语义由 `StringBuiltin` 变体决定。UTF-8 路径使用有损解码，非法序列会成为替换字符，但这类路径不会自动生成 `InvalidEncoding` 告警；只有显式字符集转换的 `convert` 和 `char_value` 会记录该告警。`Decimal` 以字符串保存，`round_decimal` 自行执行十进制舍入，并假设输入是结构正确的十进制文本。

`EvalConfig` 在整批内只读，`warnings` 在逐行过程中追加。因为 `eval_rows` 在错误时丢弃整个 `EvalOutput`，错误前积累的告警不会交付给调用者。大多数函数返回新 `Vec<u8>`，即使是大小写 binary 恒等或越界 `INSERT` 也会复制输入，输入行从不被原地修改。

## 依赖与调用关系

上游方面，RustCodeGraph 对 `eval_rows` 的 `Called by` 只找到 `pkg/expression/builtin_string_vec_24_aster_unit_test.rs` 中的 `assert_one`、`vector_packet_limits_and_null_rules_match_go` 和 `vector_variable_arity_and_base64_match_go`；源码搜索还显示 `pkg/expression/builtin_string_vec_test.rs` 的 Go 对照测试入口调用它。没有发现生产表达式签名调用该入口。`pkg/expression/lib.rs` 的模块可见性也与这一事实一致。

下游方面，`eval_rows -> eval_row` 是稳定主边；`eval_row` 再调用本文件的专用内核。跨模块依赖只有：

- `crate::collate`：`Locate*Utf8`/`InstrUtf8` 判断 CI collation，`FindInSet` 生成比较 key，`Strcmp` 使用 collator 比较。
- `crate::mysql::locale_format::FormatByLocale`：实现 `FORMAT` 的分组和 locale 规则。
- `base64`、`hex`、`encoding_rs`：分别处理 Base64、十六进制和字符集编解码；这些依赖均在 `pkg/expression/Cargo.toml` 声明。
- Rust 标准库 `HashMap`：实现二进制和 UTF-8 `TRANSLATE`；`fmt`：实现错误显示。

同目录的 `builtin_string_vec_generated.rs` 是另一模块，不由本文件调用；Go 版的 chunk 评估、子表达式求值和列缓冲管理也没有在这里形成调用边。

## 错误处理与边界

- 参数缺失和类型错误是致命错误：`arg` 返回 `MissingArgument`，`bytes`/`int` 返回带参数索引的 `TypeMismatch`。
- NULL 通常传播，但各函数遵循自身 SQL 契约。新增变体时不能机械套用统一 NULL 规则，应对照 Go 对应 signature。
- 超出 `max_allowed_packet` 时，`truncate_as_warning` 或 `ignore_truncate_error` 任一为真，就追加 `AllowedPacketOverflow` 并由调用函数返回 NULL；否则返回同名 `EvalError`。`result_flen` 超限通常直接返回 NULL，不追加告警。
- `encoding` 接受 `utf8`/`utf8mb4`/`ascii` 别名，未知标签为致命 `UnknownCharset`。实际解码/编码替换产生 `InvalidEncoding` 告警；`CHAR` 在 `strict_mode` 下还返回 NULL。
- 无效十六进制和 Base64 数据返回 NULL，不是错误。未知 locale 回退到 locale 格式化器结果并追加告警；locale 格式化器本身的错误转成 `EvalError::Format`。
- 位置和长度运算包含若干防溢出设计，如 `wrapping_neg`、`wrapping_add`、`u128` 尺寸计算及钳制；`SUBSTRING` 的负长度最终产生空串。新增算术必须保持这些极值语义。
- UTF-8 有损路径会改变非法字节；二进制路径必须保持原始字节。选择错误变体会造成位置、长度、大小写和输出内容差异。
- `pad_binary` 在包检查时把负目标长度先按 `u64` 解释，因此可能先触发超包策略，再进入 `target < 0` 分支；这是当前代码事实，若要改变需先与 Go 极值行为和测试核对。

## 并发与资源生命周期

所有入口只借用 `StringBuiltin`、行切片和 `EvalConfig`，不持有锁、不启动线程或异步任务、不访问网络/文件，也没有事务与 channel 生命周期。局部 `Vec`、`String`、`HashMap` 和编码缓冲在单次调用结束时释放；返回的 `EvalOutput` 完全拥有其值与告警。

因此，同一不可变 `StringBuiltin`/`EvalConfig` 可由上层并发调用而不在本文件共享状态，但本文件没有自己声明或管理并行执行。批内严格顺序执行，告警顺序即触发顺序。主要资源风险是由输入规模、行数以及字符串变换产生的内存分配；包大小检查覆盖若干扩张函数，却不是所有函数的统一内存预算机制，例如 `replace`、`translate_*` 和 `export_set_row` 没有调用 `packet_overflow`。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/expression/builtin_string_vec.go` 为每个 concrete signature 提供 `vecEvalString` 或 `vecEvalInt`，并以 `vectorized() bool` 宣告能力。Rust 的 `StringBuiltin` 变体大体一一对应这些 signature：例如 `Repeat` 对 `builtinRepeatSig`、`Locate3Utf8` 对 `builtinLocate3ArgsUTF8Sig`、`FormatWithLocale` 对 `builtinFormatWithLocaleSig`、`TranslateBinary/Utf8` 对相应 translate signature。

保留的关键语义包括 binary/UTF-8 双路径、chunk NULL 规则、1 基位置、collation 比较、包大小限制、FORMAT locale、字符集转换和 Base64 折行。`pkg/expression/builtin_string_vec_24_aster_unit_test.rs` 用真实 `eval_rows` 覆盖 NULL、多字节边界、包限制、binary/UTF-8 位置、编码/翻译/格式化、可变参数与 Base64，并包含更大的函数矩阵；`pkg/expression/builtin_string_vec_test.rs` 保存 Go `builtin_string_vec_test.go` 的 case/benchmark 结构，四个测试入口会转调该真实 parity suite，并另测 Go 简单大小写映射。

仍需明确的差异是：Rust 没有 `EvalContext`、`chunk.Chunk`/`chunk.Column`、子表达式向量求值、列缓冲复用及 signature 类型；`Value` 的类型系统也远小于 TiDB `types`。Rust `builtin_string_vec_test.rs` 的大量 case 定义仍是迁移草稿数据，benchmark 入口不执行性能测试。因此当前验证证明的是内核案例对齐，而不是完整 Go 向量执行框架或全量 case 自动驱动已经完成。

## 扩展指南

新增字符串内建函数时，先在 `StringBuiltin` 增加能表达必要静态状态的变体，再在 `eval_row` 添加分派，并将算法放在独立私有函数中。需要复用的参数验证应走 `arg`/`bytes`/`int`；可能扩张结果时同时核对 `max_allowed_packet`、`result_flen`、blob 上限及 Go 的告警/错误分支；文本算法必须明确选择字节还是 rune/字符语义，并确认 collation 是否参与比较。

修改现有函数时，应逐段对照 `pkg/expression/builtin_string_vec.go` 的同名 signature，而不是只根据 SQL 函数直觉重写。重点风险包括 NULL 例外、unsigned 参数、极值溢出、空 needle/delimiter/pad、非法编码、locale 回退、重复 translate 源字符及 Go 简单 Unicode 大小写规则。

测试逻辑必须继续放在独立文件。优先扩展 `pkg/expression/builtin_string_vec_24_aster_unit_test.rs` 的真实内核矩阵；如果变更 Go case 映射或桥接入口，再同步 `pkg/expression/builtin_string_vec_test.rs`，并参考 Go `pkg/expression/builtin_string_vec_test.go`。若未来把内核接入生产 signature，还需修改 `pkg/expression/lib.rs` 或实际调用模块并增加覆盖真实 chunk/执行上下文的独立测试，而不能把测试内嵌进本源文件。

性能扩展应减少逐行重复分配和有损解码，同时保持结果所有权与 Go 语义；在没有真实 chunk 接线前，不应把此内核的分配行为宣称为 Go 向量实现的性能等价物。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/expression/builtin_string_vec.rs`；`files --filter` 将其识别为 1534 行、171 个符号的 Rust 文件；`node --file ... --offset/--limit` 分三段读取完整实现。
- RustCodeGraph：`query eval_rows --kind function` 唯一定位到 `pkg/expression/builtin_string_vec.rs:228`；`node eval_rows` 给出 `eval_rows -> eval_row`、`eval_rows -> EvalOutput`，并列出直接测试调用者。单独的 `callers/callees` 命令未额外产生可用输出，故未据此推断生产调用。
- 模块与 crate：读取 `pkg/expression/lib.rs:262-266`、`:826-829`，以及完整 `pkg/expression/Cargo.toml`，确认私有模块、测试重导出、crate 名和直接依赖。
- Go 对照：读取/搜索 `pkg/expression/builtin_string_vec.go`，确认 concrete signature 的 `vecEvalString`/`vecEvalInt` 和 `vectorized()` 结构；读取/搜索 `pkg/expression/builtin_string_vec_test.go` 的四组测试/benchmark 入口。
- Rust 测试：读取 `pkg/expression/builtin_string_vec_24_aster_unit_test.rs` 和 `pkg/expression/builtin_string_vec_test.rs` 的真实 parity suite 调用、边界案例与 Go case 草稿。未运行 Cargo，符合本纯文档任务要求。
- 交付结构以任务指定命令检查，要求本文恰有 11 个固定二级标题；人工复核同时确认本文没有把测试可达内核写成已接入生产执行链。
