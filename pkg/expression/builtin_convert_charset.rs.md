# `pkg/expression/builtin_convert_charset.rs`

## 文件定位

本文件属于 `astersql-expression` crate。crate 根在 `pkg/expression/lib.rs`，通过 `#[path = "builtin_convert_charset.rs"] mod builtin_convert_charset_kernel;` 将它作为私有内核模块挂入；对应独立单元测试由同一文件中的 `#[cfg(test)]` 模块加载 `pkg/expression/builtin_convert_charset_test.rs`。`pkg/expression/Cargo.toml` 声明的直接下游依赖是 `parser-charset-dependency`（实际包 `astersql-parser-charset`），转换最终委托给该包的 `FindEncoding` 与 `Encoding::Transform`。

截至本次分析，仓库内对本文件公开符号的精确引用只出现在 `pkg/expression/builtin_convert_charset_test.rs` 和 `pkg/expression/builtin_control_9_aster_unit_test.rs`；模块本身也没有从 `lib.rs` 公开再导出。因此它当前是已挂载、可被 crate 内测试验证的字符集转换与包装决策内核，并非已经接入 Rust `ScalarFunction` 构建/执行链的完整生产实现。完整的运行时接线仍可在 Go 对照 `pkg/expression/builtin_convert_charset.go` 中看到。

## 核心职责

文件承担三组相互关联但边界清晰的职责：

1. `encode_to_binary` / `decode_binary` 实现单值 UTF-8 到目标字符集字节、以及 binary 字节到目标字符集文本（UTF-8 字节）的转换。
2. `encode_to_binary_rows` / `decode_binary_rows` 为可空行集合提供批量路径，保留 Go 向量执行在失败、warning 和 strict mode 下的特殊结果语义。
3. `conversion_property`、`is_legacy_charset` 与 `handle_binary_literal` 复刻 Go `HandleBinaryLiteral` 的分类和决策部分，判断表达式应保持不变、包 `to_binary`，还是包带目标 collation 的 `from_binary`。

它不负责构造真实表达式节点、设置 protobuf scalar-function code、读取会话 SQL mode、执行常量折叠或把 warning 写入真实 statement context；Rust 侧分别以 `WrapperAction`、`DecodeOptions` 和 `WarningContext` 表达这些决策/状态，调用方接线尚未出现在生产路径。

## 主要符号

- 内部函数与字符集常量：`INTERNAL_FUNC_TO_BINARY`、`INTERNAL_FUNC_FROM_BINARY`、`CHARSET_*` 和 `COLLATION_GBK_CHINESE_CI` 使用与 Go/MySQL 对应的字符串名；`MAX_BYTES_TO_SHOW = 6` 约束错误展示长度。注意 `CHARSET_BIG5`、`CHARSET_SHIFT_JIS`、`CHARSET_EUC_KR` 只是名称常量，当前 `pkg/parser/charset/encoding.rs::FindEncoding` 并没有对应编码实现，会按未知字符集回退为 binary。
- `ConvertError { displayed_bytes, from_charset, to_charset }`：转换失败的结构化错误。私有构造器 `ConvertError::new` 调用 `format_bytes`，将输入前 6 字节格式化为大写十六进制，超长时加 `...`；`Display` 生成 `cannot convert string ... from ... to ...`。
- `encode_to_binary(input, charset)`：以 `OpEncode` 调用 `FindEncoding(charset).Transform`；错误中的源字符集固定为 `utf8mb4`，目标为参数 `charset`。
- 私有 `decode_lossy(input, charset)`：以 `OpDecode` 转换，返回“已产生的字节及是否报错”。失败时不丢弃编码器已写入的合法前缀。
- `DecodeOptions`：由 `cannot_convert_as_warning` 和 `strict_mode` 两个布尔量组成，决定解码失败是错误、warning 后 NULL，还是 warning 后有损值。
- `WarningContext`：本地 `Vec<ConvertError>` warning 收集器，只模拟本文件所需的 SQL warning 载体，并非会话级 statement context。
- `decode_binary`：单值解码入口，返回 `Result<Option<Vec<u8>>, ConvertError>`；外层 `Result` 表示硬错误，内层 `Option` 表示 SQL NULL。
- `encode_to_binary_rows` / `decode_binary_rows`：输入输出均逐行保留 `None`，前者遇到首个非 NULL 转换错误就结束整批；后者逐行收集 warning 并保留 Go 向量路径的失败行为。
- `FunctionProperty::{None, BinaryAware, Auto}` 与三个 `PROPERTY_*` 表：按函数名定义包装策略。`conversion_property` 是大小写敏感的精确匹配，未登记名称也返回 `None`。
- `ExpressionCollation { charset, collation }`：包装决策所需的目标字符集/校对规则快照。
- `WrapperAction::{Unchanged, ToBinary, FromBinary { target, cannot_convert_as_warning }}`：`handle_binary_literal` 的纯决策结果；它描述下一步应构造什么包装，不直接创建表达式。

## 执行流程

编码单值时，`encode_to_binary` 先按字符集名选择编码器，预分配与 UTF-8 输入长度相同的 `Vec<u8>`，再执行 `OpEncode`。成功返回目标编码字节；失败将编码器错误统一映射为 `ConvertError`。`encode_to_binary_rows` 对每个 `Option<String>` 调用该入口，NULL 原样传递，`collect` 保证首个错误使整个调用返回 `Err`。

解码单值时，`decode_binary` 先调用 `decode_lossy`。无错误时返回 `Ok(Some(decoded))`；有错误时构造 binary 到目标字符集的 `ConvertError`。若 `cannot_convert_as_warning == false`，立即返回硬错误；否则先把错误加入 `WarningContext`，严格模式返回 `Ok(None)`，非严格模式返回编码器在非法序列前已经产出的合法前缀。

批量解码 `decode_binary_rows` 逐行处理：NULL 直接追加 NULL；成功行追加完整解码值；失败且不允许 warning 时终止整批；允许 warning 时追加一次 warning，严格模式追加 NULL，非严格模式追加原始 binary 单元。这里“追加原始单元”而非单值入口的“合法前缀”是刻意保留的 Go 向量路径差异，由 `vector_decode_error_paths_match_go_row_behavior` 覆盖。

包装决策从 `handle_binary_literal` 开始：

1. `None` 类函数始终 `Unchanged`。
2. `BinaryAware` 类函数仅当实参不是 legacy charset 时返回 `ToBinary`。
3. `Auto` 类在“非 binary 实参、binary 结果”时，对非 legacy 实参返回 `ToBinary`；在“binary 实参、非 binary 结果、且表达式不是 NULL 类型”时返回 `FromBinary`，目标 collation 来自 `result_collation`，而 `explicit_cast` 被传为 `cannot_convert_as_warning`；其他组合保持不变。

## 数据与状态

转换数据均由调用者所有：输入使用借用的 `&str`/`&[u8]`，输出使用新 `Vec<u8>`。批量接口接收不可变切片并构造等序输出，不在输入行上原地修改。`ExpressionCollation` 和 `ConvertError` 拥有自己的 `String`，`WrapperAction::FromBinary` 会克隆目标 collation，因而动作结果不借用调用者状态。

唯一显式可变状态是调用者传入的 `&mut WarningContext`。每个允许降级的失败追加一个 `ConvertError`，函数不会自动清空既有 warning；连续调用的 warning 数量会累积。硬错误路径在追加 warning 之前返回。批量解码可能在若干成功行或 warning 行之后遇到硬错误，但由于结果向量没有暴露给调用者，只有先前已经写入的 `WarningContext` 可能成为可见副作用。

`PROPERTY_NONE`、`PROPERTY_BINARY_AWARE`、`PROPERTY_AUTO` 是只读静态切片，没有 Go `init()` 建表带来的可变全局状态。代价是每次 `conversion_property` 进行线性 `contains` 查找；当前表规模较小，但扩展大量函数名时应重新评估查找成本。

## 依赖与调用关系

下游主链为 `encode_to_binary`/`decode_lossy` → `parser_charset_dependency::FindEncoding` → 所选编码器的 `Transform(..., OpEncode/OpDecode)`。`pkg/parser/charset/encoding.rs::FindEncoding` 当前明确支持 utf8/utf8mb4、GBK、Latin1、ASCII、GB18030，其余名称返回 binary 编码器；RustCodeGraph 也确认 `FindEncoding` 的调用者包含本文件的 `encode_to_binary` 与 `decode_lossy`。

文件内部调用边包括：`encode_to_binary_rows` → `encode_to_binary`；`decode_binary`/`decode_binary_rows` → `decode_lossy` 和 `ConvertError::new`；`ConvertError::new` → `format_bytes`；`handle_binary_literal` → `conversion_property`/`is_legacy_charset`。

上游方面，`pkg/expression/lib.rs` 私有挂载该模块及独立测试；精确仓库搜索没有发现 Rust 生产模块调用这些 API。测试上游是 `pkg/expression/builtin_convert_charset_test.rs`，以及通过独立 `#[path]` 再次加载目标文件的 `pkg/expression/builtin_control_9_aster_unit_test.rs`。因此不能据此声称 SQL 规划或执行已使用这些 Rust API；Go 的 `BuildToBinaryFunction`、`BuildFromBinaryFunction` 和 `HandleBinaryLiteral` 才展示了预期应用主链。

## 错误处理与边界

- 错误展示使用原始输入的前 6 字节，而不是 Unicode 字符数；字节以无分隔的大写十六进制呈现，超长追加 `...`。这与 Go 的 `maxBytesToShow` 意图一致，但 Rust `ConvertError` 不是 Go `dbterror` 的错误码/stack 包装。
- `FindEncoding` 对空名和未知名回退 binary，所以这些情况通常按字节直通而非报“不支持字符集”。独立测试用 `big5` 验证了这一事实；Big5/SJIS/EUC-KR 常量的存在不代表有真实编解码支持。
- `decode_lossy` 依赖编码器在错误前保留已写入目标缓冲区。单值非严格 warning 返回此前缀；批量非严格 warning 则刻意返回原始单元，两者不可合并成同一个结果策略。
- NULL 只存在于批量行的 `Option` 和单值解码结果中；单值编码入口不接收 NULL。`handle_binary_literal` 通过独立布尔量 `expression_is_null` 阻止给 binary NULL 包 `from_binary`。
- `conversion_property` 大小写敏感，`"sha2"` 命中而 `"SHA2"`/未知名称落到 `None`。新增别名时必须显式加入对应表。
- `explicit_cast` 只有生成 `FromBinary` 动作时才进入 `cannot_convert_as_warning`；它不影响 `ToBinary` 或 `Unchanged`。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。静态属性表和字符集常量只读；转换缓冲区、结果向量、错误和动作值均为调用栈/返回值所有，函数结束后按 Rust 所有权正常释放。

并发调用本身不共享内部可变状态。`WarningContext` 通过独占 `&mut` 借用传入，Rust 类型系统阻止同一个实例被无同步并发写入；若上层需要跨任务共享 warning，必须在本文件之外选择同步策略。与 Go 向量实现复用 `bufAllocator`/`bytes.Buffer` 不同，Rust 行批量路径目前为每个单值编码分配输出，并在批量解码中为每行建立 `decoded` 缓冲区，正确性测试没有给出大批量性能或峰值内存证据。

## 与 Go 版本的对应关系

Rust 的函数名常量、6 字节错误摘要、编码/解码方向、legacy charset 集合、三类函数属性和 `HandleBinaryLiteral` 分支直接对应 `pkg/expression/builtin_convert_charset.go`。`PROPERTY_*` 的名字集合复刻 Go `convertActionMap`；未知名称在 Go map 查询和 Rust 查询中都归为 `funcPropNone`/`FunctionProperty::None`。Go 和 Rust 的 `FindEncoding` 也都只把 utf8/utf8mb4、GBK、Latin1、ASCII、GB18030 映射到真实实现，其他名称回退 binary。

关键行为对应为：Go `builtinInternalToBinarySig.evalString/vecEvalString` 对应 Rust 单值/行编码；Go `builtinInternalFromBinarySig.evalString/vecEvalString` 对应 Rust 单值/行解码；Go `isLegacyCharset` 对应 Rust `is_legacy_charset`；Go `HandleBinaryLiteral` 的条件判断对应 Rust `handle_binary_literal`。

迁移并不完整。Go 文件还实现了 `functionClass` 参数验证、`baseBuiltinFunc`、返回 `FieldType`、`ScalarFunction` 构造、`tipb` code、`FoldConstant`、真实 `BuildContext`/SQL mode/warning context，以及 expression clone/share 约束；Rust 文件只保留可独立测试的转换和决策模型。Go `constant_test.go::TestConstantFoldingCharsetConvert` 验证了真实表达式构造与常量折叠，而当前 Rust 测试只验证内核函数，不能替代该主链证据。

## 扩展指南

- 新增真实编码支持时，应先在 `pkg/parser/charset` 的 `FindEncoding` 及独立编码器测试中完成实现，再决定是否在本文件增加名称常量；仅添加 `CHARSET_*` 常量会继续走 binary 回退。
- 修改转换失败策略时，同时检查 `decode_lossy`、`decode_binary` 和 `decode_binary_rows`。尤其要保留单值非严格“已解码前缀”与向量非严格“原始单元”的差异，并同步 `pkg/expression/builtin_convert_charset_test.rs` 和 `pkg/expression/builtin_control_9_aster_unit_test.rs` 中的独立测试。
- 新增或调整函数分类时修改恰好一个 `PROPERTY_*` 表，并覆盖大小写、未知名称、legacy/non-legacy charset、binary/non-binary 结果和 NULL 表达式。重复或跨表名称会由当前优先级（`BinaryAware` 先于 `Auto`）决定，宜避免这种隐含冲突。
- 把内核接入生产表达式链时，应在其他生产文件中实现或复用表达式节点构造、返回类型/collation、常量折叠、会话 warning 与 strict mode、protobuf pushdown 等能力，不应把这些上下文强塞进本文件的纯值模型。还需补与 Go `TestConstantFoldingCharsetConvert` 同意图的独立 Rust 回归测试。
- 性能扩展应关注批量路径的逐行分配和属性表线性扫描；任何缓存/复用都必须保持返回缓冲区所有权、错误前缀和 warning 顺序不变。测试必须继续放在独立 `*_test.rs` 文件，不能内嵌到生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；`node --file pkg/expression/builtin_convert_charset.rs --offset 1 --limit 400` 完整读取 383 行源文件；`query` 精确定位 `conversion_property`、`handle_binary_literal`、`decode_binary`、`encode_to_binary`；`node encoding.rs::FindEncoding` 验证支持表、binary 回退及目标文件调用边。对目标自由函数执行 `callers/callees` 未返回结果，因此上游接线改用精确仓库搜索核验，未把缺失图边当成“无人调用”的唯一证据。
- crate 与模块：`pkg/expression/Cargo.toml`（crate 名、`lib.rs`、`parser-charset-dependency`、`autotests = false`）和 `pkg/expression/lib.rs`（私有模块与 `#[cfg(test)]` 独立测试挂载）。目标包没有 `doc.go`，因此无额外包契约可读。
- Rust 测试：`pkg/expression/builtin_convert_charset_test.rs` 验证未知编码 binary 回退、函数名大小写敏感、单值非严格失败返回合法前缀；`pkg/expression/builtin_control_9_aster_unit_test.rs` 验证 GBK/GB18030 往返、NULL 行、错误/warning/strict 分支、包装动作和向量失败保留原始单元。
- Go 对照：`pkg/expression/builtin_convert_charset.go` 验证完整 function class、标量/向量执行、warning/strict 分支、属性表及 `HandleBinaryLiteral`；`pkg/expression/constant_test.go::TestConstantFoldingCharsetConvert` 验证 Go 生产表达式的字符集转换与常量折叠；`pkg/parser/charset/encoding.go` 与 Rust `pkg/parser/charset/encoding.rs` 核对编码映射和未知名回退。
- 人工复核结论：本文件存在是为了把内部 `to_binary`/`from_binary` 的转换语义及二进制字面量包装规则移植成 Rust 可测试内核；当前执行从纯函数入口进入 parser charset 编码器，失败经 options 分流；安全扩展必须同步编码器、三条错误策略和独立测试，并把尚缺的生产表达式接线视作范围外事实，而非当前已支持能力。
