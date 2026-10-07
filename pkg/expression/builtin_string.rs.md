# `pkg/expression/builtin_string.rs`

## 文件定位

本文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs:261-262` 以私有模块 `builtin_string_kernel` 装配。它不是 SQL 表达式框架的完整函数类实现，而是从 Go `pkg/expression/builtin_string.go` 抽出的“求值无关”字符串语义内核：输入已经是 Rust 字符串、字节串或整数，输出普通值或 `Option`，不直接持有 `BuildContext`、`EvalContext`、`Expression`、行数据、类型元数据、告警上下文或 protobuf 签名。

当前运行时接线是局部的。`pkg/expression/builtin.rs` 的 `CoreBuiltin` 构造与求值路径直接调用本模块的 `charLength`、`hexInt`、`hexString`、`rpadUtf8`、`insertUtf8`、`substringBytes`、`substringUtf8`、`trimLeft`、`trimRight` 和 `trimBoth`；文件中的其他公开函数目前主要由独立测试调用，不能据此认定全部 44 项规格都已经接入 SQL 执行主链。

## 核心职责

- 用 `STRING_FUNCTION_SPECS` 保存 44 个字符串函数的名称、参数数量范围、返回值族、包大小敏感性，以及是否有 binary/UTF-8 两套签名；`stringFunctionSpec` 和 `verifyStringFunctionArgs` 提供大小写无关的查找与参数个数校验。
- 实现两类边界：binary 路径按字节处理，UTF-8 路径按 Unicode 标量值（Rust `char`，对应 Go `rune`）处理。典型成对实现包括 `leftBytes`/`leftUtf8`、`substringBytes`/`substringUtf8`、`lpadBytes`/`lpadUtf8`、`insertBinary`/`insertUtf8`、`translateBinary`/`translateUtf8`。
- 保留 SQL NULL 与限制条件的核心语义。无法由普通返回值表达的 NULL、非法输入、未知字符集、溢出或 `max_allowed_packet` 超限通常合并为 `None`；真正的 SQL 错误/警告转换仍应由上层表达式包装器完成。
- 提供编码、格式化和选择类算法，包括 HEX/UNHEX、CHAR/ORD/QUOTE、FORMAT locale、Base64、FIND_IN_SET/FIELD/ELT/MAKE_SET/EXPORT_SET、TRANSLATE 和 binary `WEIGHT_STRING`。

## 主要符号

- 常量与规格：`FORMAT_MAX_DECIMALS` 将 FORMAT 小数位限制为 30；`INVALID_BYTE` 是 binary TRANSLATE 的删除哨兵；私有 `MAX_BLOB_WIDTH` 限制 LPAD/RPAD 为 16,777,216。`StringReturnKind`、`StringFunctionSpec` 和 `STRING_FUNCTION_SPECS` 描述构造期信息，但不创建真正的函数类或返回类型。
- 基础字符串族：`length`/`ascii`、`concat`/`concatWS`、`repeat`/`space`、`lowerUtf8`/`upperUtf8`、`strcmp`、`replace`、`reverseBytes`/`reverseRunes`。
- 切片与定位族：私有 `substringBounds` 统一 MySQL 一基正负位置；其上是 `substringBytes`/`substringUtf8`。`substringIndex`、`locateBinary`/`locateUtf8` 和 `instrBinary`/`instrUtf8` 分别处理分隔符计数和一基查找结果。
- 编码与字节族：私有 `findEncoding` 配合 `convertCharset`；`hexString`/`hexInt`/`unhex`、`charFromInts`、`calcOrd`、`bin`、`oct`、`Quote` 完成字节或进制转换。
- 修剪与填充族：`trimLeft`、`trimRight`、`trimBoth` 反复剥离完整子串，`ltrim`/`rtrim` 只处理 ASCII 空格；LPAD/RPAD 各有字节和 UTF-8 版本。
- 集合族：`findInSet`、`fieldString`、`makeSet`、`elt`、`exportSet`。比较的简化入口是 `case_insensitive: bool`，不是 TiDB 完整 collator 对象。
- 数字与 Base64：`roundFormatArgs` 保留 Go 辅助函数的字符串进位行为；`formatByLocale` 经 `localeStyle`、`incrementDecimalDigits`、`groupInteger` 格式化。`base64NeededDecodedLength`/`base64NeededEncodedLength` 先预算，`fromBase64`/`toBase64` 再转换，`splitToSubN` 负责 76 字符换行。
- 尾部功能：`insertBinary`/`insertUtf8`、`loadFile`、`buildTranslateMap4UTF8`/`buildTranslateMap4Binary`、`translateUtf8`/`translateBinary`、`weightStringBinary`。`loadFile` 固定返回 `None`，明确不暴露服务端文件系统。

## 执行流程

上层 SQL 路径先在 `pkg/expression/builtin.rs` 的核心工厂中校验参数数量、插入类型转换、决定 binary/UTF-8 签名和返回 `FieldType`，随后 `CoreBuiltin::evalInt` 或 `evalString` 从行中求出参数并传播 SQL NULL，最后才调用本文件的纯算法。例如 SUBSTRING 根据输入 collation 选择 `CoreBuiltinKind::Substring(binary)`，求值时分别进入 `substringBytes` 或 `substringUtf8`；TRIM 在上层解析方向后分派到三个 trim 辅助函数。

本文件内部的一般流程是：先处理负值、零、空串、越界或 NULL 表示等早退条件；再把一基 SQL 位置或目标长度转换为 Rust 的零基半开区间；按 binary/UTF-8 路径建立字节或 `char` 视图；用预估容量构造拥有型结果；最后以普通值或 `Option` 返回。`repeat`、`space`、Base64 和 INSERT 会在分配前进行长度/包预算，LPAD/RPAD 会先执行 `MAX_BLOB_WIDTH` 检查。

TRANSLATE 先构造映射再单次扫描输入。映射按逆序插入，保证 `from` 中重复字符采用最左侧映射；当 `to` 较短时，多余来源字符映射为删除。FORMAT 先限制精度、提取连续数字、按下一位决定进位，再按 locale 风格对整数分组并选择小数分隔符；未知 locale 回退 `en_US` 风格并以布尔值 `false` 通知上层生成警告。

## 数据与状态

文件没有可变全局状态。函数规格是只读静态切片；每次调用只创建局部 `String`、`Vec` 或 `HashMap`。借用型 trim 与 `elt` 返回指向输入的切片，不延长资源生命周期；其他转换通常返回拥有型数据。

SQL NULL 在这里没有统一类型：`concat`、`concatWS` 和 `elt` 的 `Option` 直接表达 NULL；`repeat`、`space`、Base64、INSERT、LPAD/RPAD、UNHEX 和字符集转换还用 `None` 合并表达限制超出或非法输入。上层必须根据具体函数区分并补充 TiDB 的 NULL、错误与告警语义。`formatByLocale` 另以 `(String, bool)` 区分结果和 locale 是否已知。

大小写不敏感比较通过 `to_lowercase` 临时字符串实现；字符计数依赖 `str::chars()`，是 Unicode 标量数量而不是字形簇。binary 函数直接使用 `&[u8]`。这些选择会影响非 ASCII 输入、分配成本和与完整 collation 语义的兼容性。

## 依赖与调用关系

直接外部依赖只有 `base64::Engine` 与 `encoding_rs`，均由 `pkg/expression/Cargo.toml` 声明；标准库依赖为 `Ordering` 与 `HashMap`。本文件不依赖表达式 trait 或存储、会话和网络模块。

上游装配边为 `pkg/expression/lib.rs` → `builtin_string_kernel`。已核实的生产调用边来自 `pkg/expression/builtin.rs`：`CoreBuiltin::evalInt` → `charLength`；`CoreBuiltin::evalString` → HEX、RPAD、INSERT、SUBSTRING、TRIM 对应辅助函数。相应工厂将 `substring`/`substr`/`mid`、`trim`、`char_length`、`hex`、`rpad`、`insert` 注册进核心 builtin 表。LENGTH、ASCII、SPACE、FIND_IN_SET 等虽同样在核心工厂中存在，但当前求值代码在上层自行实现，并未调用这里的同名辅助函数。

测试边为 `pkg/expression/lib.rs` 的 `#[cfg(test)]` 模块装配：`builtin_string_test.rs` 保存从 Go 同名测试迁移的广泛场景；`builtin_string_25_aster_unit_test.rs` 直接导入本模块并集中覆盖纯内核；向量测试位于 `builtin_string_vec_test.rs`、`builtin_string_vec_generated_test.rs` 及其 AsterSQL 回归辅助文件。RustCodeGraph 能查询到本文件符号，但对所查关键符号未返回 callers/callees，因此生产调用边以模块装配和直接调用点读取为准。

## 错误处理与边界

- `checked_mul`、`checked_add` 和 `usize::try_from` 用于阻止长度预算溢出；发生溢出时相关 `Option` 函数返回 `None`。`bitLength` 使用饱和乘法。
- `substringBounds` 将位置 0、越界位置和非正请求长度变为空结果；负位置从尾部计算。LOCATE/INSTR 返回一基位置，未找到或非法起点返回 0，空 needle 遵循各自的一基规则。
- `unhex` 遇到非十六进制字符返回 `None`，奇数位输入把首位作为低值字节；Base64 解码忽略四类 ASCII 空白，非法编码返回 `None`。
- LPAD/RPAD 的目标长度为负或超过 `MAX_BLOB_WIDTH` 时返回 `None`；需要扩展但 pad 为空时返回空值。INSERT 的非法位置返回原值，负删除长度删除至尾部，结果超包返回 `None`。
- `convertCharset` 对未知字符集或若干非法编码路径返回 `None`。它是轻量语义辅助，不携带 Go 实现中的具体错误对象。
- `loadFile` 无条件返回 `None`，是安全边界而非待实现的文件读取逻辑。
- `weightStringBinary` 只实现 binary collation 的截断/填充；无显式长度时递归移除尾部空格。非 binary collation 的权重生成不在本文件中。

## 并发与资源生命周期

所有函数都是同步、无锁、无异步任务、无通道、无事务和无外部 I/O 的纯计算；输入通过不可变借用或调用方转移所有权传入，局部分配随返回值所有权或函数结束释放。因此同一函数可被多个线程并发调用，不存在模块内共享状态竞争。

主要资源风险是由输入长度驱动的内存和 CPU：UTF-8 分支经常收集完整 `Vec<char>`，大小写不敏感比较会分配小写副本，TRANSLATE 会建立 `HashMap`，CONCAT/REPEAT/PAD/Base64 会生成新缓冲区。只有部分函数执行 `max_allowed_packet` 或 `MAX_BLOB_WIDTH` 限制；调用未接线辅助函数时，上层仍须传入真实会话包限制并避免无界输入。

## 与 Go 版本的对应关系

Go 权威对照是 `pkg/expression/builtin_string.go`，测试对照是 `pkg/expression/builtin_string_test.go`。Rust 函数名大体保留 Go 的驼峰命名和边界算法，binary/UTF-8 双签名对应 Go 中按 `[]byte` 与 `[]rune` 分支的 builtin signature；`FORMAT_MAX_DECIMALS`、包大小检查、位置规则、Base64 换行、TRANSLATE 重复来源字符规则及 `LOAD_FILE` 返回 NULL 都有对应测试证据。

结构上两者并不等价：Go 文件定义完整的 function class、builtin signature、Clone、类型/长度/charset/collation 推导、行求值、错误与 statement warning、向量化标记及 protobuf code；Rust 本文件只保留纯算法和简化规格。Rust 的 `case_insensitive: bool` 不能替代 Go collator 的完整排序规则，`convertCharset` 和 FORMAT 错误通道也比 Go 简化。因此扩展时应复用这里的算法，但必须在 `builtin.rs` 或相应框架层补足类型系统、上下文和错误/告警接线。

独立 Rust 测试 `builtin_string_test.rs` 列出与 Go 同名的测试入口并覆盖 NULL、边界长度、二进制/多字节字符、签名和 locale；`builtin_string_25_aster_unit_test.rs` 则对本文件公开辅助函数给出直接断言。两者均与生产源文件分离，符合仓库测试组织要求。

## 扩展指南

新增或移植字符串 builtin 时，先确认它是否需要 binary/UTF-8 双路径、collation、会话 `max_allowed_packet`、错误/告警或返回类型推导。纯、可复用的字节/字符算法可放在本文件，并在 `STRING_FUNCTION_SPECS` 中准确记录参数范围和特性；SQL 可见接线应放在表达式工厂与求值层，不能只添加规格或 helper 就宣称已支持。

修改位置/长度算法时优先复用 `substringBounds` 一类统一边界转换，避免一基/零基和正负位置分支漂移。修改会分配内存的函数时，先用 checked arithmetic 预算，再应用真实包限制；不要把 `usize::MAX` 当成最终会话配置。新增字符集或 collation 行为时，应评估 `encoding_rs` 映射和 `case_insensitive` 简化是否足够，完整 SQL 语义应接入 crate 现有 charset/collator 设施。

测试应同步更新独立文件，而不是内嵌进 `builtin_string.rs`：纯算法回归放入 `builtin_string_25_aster_unit_test.rs`；Go 对齐场景放入 `builtin_string_test.rs`，并核对 `builtin_string_test.go`；若新增运行时接线，还应覆盖 `builtin.rs` 的构造、SQL NULL、返回类型/charset/collation、错误或 warning；向量路径变化则同步相应 `builtin_string_vec*_test.rs`。兼容性重点是多字节边界、collation、NULL 与警告的区分；性能重点是全量字符收集、重复小写转换、映射构建和大结果分配。

## 验证依据

- 源码与装配：`pkg/expression/builtin_string.rs`（常量、规格及全部辅助函数），`pkg/expression/lib.rs:261-262,823-824`（生产模块与测试重导出），`pkg/expression/builtin.rs`（核心工厂、注册表和已接线求值分支）。目标包不存在 `doc.go`。
- crate 边界：`pkg/expression/Cargo.toml` 的 `[lib] path = "lib.rs"`、`base64 = "0.22"`、`encoding_rs = "0.8.35"` 以及 `package.metadata.porting.go-package = "pkg/expression"`。
- Go 对照：`pkg/expression/builtin_string.go` 与 `pkg/expression/builtin_string_test.go`；Rust 测试：`pkg/expression/builtin_string_test.rs`、`pkg/expression/builtin_string_25_aster_unit_test.rs`，并参考 `builtin_string_vec_test.rs` 与 `builtin_string_vec_generated_test.rs` 的模块装配。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`query reverseBytes` 与 `query formatByLocale` 命中目标符号，精确 `callers`/`callees` 查询未返回边，因此调用关系由上述直接源码位置核验。最初 `files --filter pkg/expression/builtin_string` 未列出文件，故未把图输出当作唯一依据。
- 结构验证应确认本文存在且固定二级标题恰为 11 个；本任务是纯文档分析，按计划不运行 Cargo。
