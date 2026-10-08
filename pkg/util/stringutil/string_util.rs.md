# `pkg/util/stringutil/string_util.rs`

## 文件定位

本文件是 `astersql-util-stringutil` crate 的主体实现；crate 入口 `pkg/util/stringutil/lib.rs` 只公开 `string_util` 模块，`pkg/util/stringutil/Cargo.toml` 则把它登记为 Go 包 `pkg/util/stringutil` 的 Rust 迁移。它提供三组底层能力：SQL/字符串转义解析、SQL `LIKE` 模式编译与匹配、以及标识符/标签/UTF-8/ASCII 等无 I/O 字符串辅助函数。

这些能力位于 SQL 表达式和排序规则之下，而不是直接处理 SQL 请求。直接使用证据包括：`pkg/util/collate/bin.rs` 的二进制及 rune 级通配模式，`pkg/util/collate/{general_ci,unicode_0400_ci_impl,unicode_0900_ai_ci_impl,gbk_chinese_ci,gb18030_chinese_ci}.rs` 的定制排序权重匹配，`pkg/planner/core/expression_rewriter.rs` 的精确 `LIKE` 判定，`pkg/planner/core/memtable_infoschema_extractor.rs` 与 `memtable_predicate_extractor.rs` 的内存表谓词，以及 `pkg/expression/builtin_ilike.rs`、`builtin_ilike_vec.rs` 的 ILIKE ASCII 归一化。`StringerStr` 还由 `pkg/util/breakpoint/breakpoint.rs` 用作字符串化键。

## 核心职责

1. `Unquote*` 按 TiDB/Go 的字节语义拆除成对单/双引号并处理反斜杠转义；其中 `UnquoteBytes`/`UnquoteCharBytes` 保留 Go `string` 可包含非法 UTF-8 的能力，`&str` 适配器只服务合法 UTF-8 调用方。
2. `CompilePattern*` 把 `LIKE` 文本编译成平行的“权重”和“类型”数组，类型由 `PatMatch`、`PatOne`、`PatAny` 表示；`DoMatch*` 用线性 glob 回退算法执行匹配，`CompileLike2Regexp` 则生成锚定的正则文本。
3. `Escape`、`BuildStringFromLabels`、`GetTailSpaceCount`、`Utf8Len`、`TrimUtf8String`、`ConvertPosInUtf8` 和 `EscapeGlobQuestionMark` 完成稳定格式化及字节/字符位置换算。
4. `LowerOneString*` 只折叠 ASCII，并在 ILIKE pattern 中保护 escape 字节；`StringerFunc`、`MemoizedStringer`、`StringerStr` 提供 Go `fmt.Stringer` 形状的本地表达。

文件自身明确标注若干迁移边界：`StringUtilError`、`GoStringer`、`SQLMode`/`ModeANSIQuotes` 和 `regexp_quote_meta` 是本 crate 的局部替代，不等同于已接入 `pingcap/errors`、统一跨包 stringer、parser mysql 类型或正则 crate。

## 主要符号

- 错误与转义：`ErrSyntax: &str` 是唯一错误文本；`StringUtilError { message }` 实现 `Display`/`Error`；`UnquoteChar`、`Unquote` 是 UTF-8 `String` 适配器，`UnquoteCharBytes`、`UnquoteBytes` 是精确的任意字节实现。
- LIKE 编译：`PatMatch = 1`、`PatOne = 2`、`PatAny = 3` 是编译结果标签；`CompilePattern`/`CompilePatternInner` 按 Unicode `char` 工作，`CompilePatternBinary`/`CompilePatternInnerBinary` 按字节工作。
- LIKE 执行：`DoMatch` 使用严格 `char` 相等；`DoMatchCustomized<F>` 允许 collator 注入字符比较器；`DoMatchBinary` 按字节比较；私有 `doMatchInner` 统一游标与 `%` 回退；`IsExactMatch` 判断编译结果是否完全无通配符。
- 正则转换：`CompileLike2Regexp(str_, escape)` 将同一编译结果映射为 `^...$`；私有 `regexp_quote_meta` 只转义 Rust 源码列出的正则元字符。
- Stringer：`StringerFunc<F>` 调闭包生成字符串；`GoStringer` 是局部 trait；`MemoizedStringer<F>` 通过 `RefCell<String>` 缓存非空结果；`MemoizeStr` 构造该包装；`StringerStr` 返回内部字符串副本。
- 格式与 UTF-8：`Escape` 根据 `ModeANSIQuotes` 选择双引号或反引号并将内部同类引号加倍；`BuildStringFromLabels` 按 key 排序；`GetTailSpaceCount` 只数尾部 ASCII 空格；`Utf8Len` 数首字节前导 1；`TrimUtf8String` 从头删除指定字符数并返回字节数；`ConvertPosInUtf8` 把字节前缀长度换成从 1 开始的字符位置。
- ASCII 与 glob：`IsUpperASCII`、`IsLowerASCII`、`IsNumericASCII` 分类单字节；`LowerOneString` 原地小写；`LowerOneStringExcludeEscapeChar` 同时返回实际 escape；`EscapeGlobQuestionMark` 仅在 `?` 前加反斜杠；`Copy` 返回独立 `String`。

## 执行流程

反引号流程从 `UnquoteBytes` 开始：先验证长度、首尾引号相等且属于单/双引号；无反斜杠且内部不含同类引号时直接复制内部字节。否则循环调用 `UnquoteCharBytes`：普通 ASCII 消耗一字节，合法多字节 UTF-8 整段复制，非法高位字节按单字节保留；反斜杠分支把 `b/n/r/t/Z/0` 映射为控制字节，把 `\` 和引号还原，把 `\%`/`\_` 连同反斜杠保留，未知转义则丢弃反斜杠。引号不闭合、空输入、首字符等于当前引号或孤立反斜杠均返回 `StringUtilError::syntax()`。

LIKE 流程先由 `CompilePatternInner` 或二进制版本扫描模式。escape 后的字符强制标为 `PatMatch`；`_` 标为 `PatOne`；`%` 标为 `PatAny`，连续 `%` 合并，并将 `%_` 规范化成 `_%`。随后 `doMatchInner` 同步推进输入游标与模式游标：精确项调用传入 matcher，`_` 消耗一个字符/字节，`%` 记录最近回退位置；后续失败时回到该 `%` 并让它多吞一个单位。游标同时耗尽即成功，没有可用 `%` 回退即失败。`pkg/util/collate/bin.rs` 直接使用默认/二进制版本，各 CI collator 则经 `DoMatchCustomized` 注入权重比较。

ILIKE 流程位于调用方：`pkg/expression/builtin_ilike.rs::normalize_ilike` 先用 `LowerOneString` 折叠 value；pattern 的 escape 是字母时改用 `LowerOneStringExcludeEscapeChar`，否则普通折叠，再交给 collator 编译/匹配。planner 的 `expression_rewriter.rs` 编译常量 pattern 后用 `IsExactMatch` 判断能否按精确条件处理；内存表提取器用 `CompilePattern`/`DoMatch` 或 `CompileLike2Regexp` 生成可执行筛选。

辅助流程均为局部转换：`BuildStringFromLabels` 排序后逐项写入并删除末尾逗号；`MemoizedStringer::String` 只在缓存非空时命中，闭包返回空串会在下次重算；`TrimUtf8String` 反复依据首字节宽度 `drain`；`ConvertPosInUtf8` 对指定字节前缀做 lossy UTF-8 字符计数并加一。

## 数据与状态

LIKE 编译结果由两个等长向量共同表示：`pat_weights[i]` 保存字面字符/字节，`pat_types[i]` 决定该位置是精确、单单位还是任意长度匹配。`doMatchInner` 不修改它们，只维护 `c_idx`、`p_idx` 以及最近 `%` 的 `next_c_idx`、`next_p_idx`；因此每次调用没有跨请求共享状态。

多数函数只持有栈上游标和新分配的 `String`/`Vec`。唯一内部可变状态是 `MemoizedStringer.result: RefCell<String>`：空串既代表“未缓存”也代表“计算结果为空”，所以空结果不会缓存。`BuildStringFromLabels` 接收只读 `HashMap`，输出顺序通过 key 排序稳定化；`LowerOneString*` 和 `TrimUtf8String` 则显式原地修改调用方缓冲区。

`SQLMode` 当前是 `i64` 别名，`ModeANSIQuotes` 固定为 `0x00000004`；这是为保存 Go 分支结构的局部类型/常量，不是 parser mysql 类型的共享定义。

## 依赖与调用关系

本 crate 的 `Cargo.toml` 没有外部依赖，文件仅使用标准库的 `RefCell`、`HashMap` 和 `fmt::Write`。`lib.rs` 对外暴露 `pub mod string_util`，并在 `cfg(test)` 下从独立的 `string_util_test.rs` 与 `migration_aster_unit_test.rs` 装配测试，符合“生产逻辑与测试分文件”。

主要下游调用为：`Unquote -> UnquoteBytes -> UnquoteCharBytes -> Utf8Len`；`CompilePattern -> CompilePatternInner`；`CompilePatternBinary -> CompilePatternInnerBinary`；`CompileLike2Regexp -> CompilePattern -> regexp_quote_meta`；`DoMatch -> DoMatchCustomized -> doMatchInner`；`DoMatchBinary -> doMatchInner`；`LowerOneStringExcludeEscapeChar -> IsLowerASCII/IsUpperASCII/Utf8Len`。

主要上游调用为：collate 的 binary/derived binary pattern 使用编译与默认匹配，general/Unicode/GBK/GB18030 CI pattern 使用定制 matcher；expression ILIKE 标量与向量路径使用 ASCII helper；planner 表达式重写和内存表提取使用 LIKE 编译、精确性判断和正则转换；breakpoint 使用 `StringerStr`。Cargo 清单还显示 expression、planner core、memtable extractors、breakpoint 等通过路径依赖引用 `astersql-util-stringutil`，证明它是 workspace 内共享基础 crate，而不是孤立示例。

## 错误处理与边界

可恢复错误只出现在 Unquote 家族，且统一为消息 `invalid syntax`。`UnquoteBytes` 是需要 Go 任意字节兼容性的首选 API；`Unquote(&str)` 假设合法 UTF-8 输入在反转义后仍合法，并用 `expect` 固化该不变量。未知反斜杠转义不是错误，这是 TiDB 与 `strconv.UnquoteChar` 的刻意差异。

部分 API 依赖调用方前置条件而会 panic：`TrimUtf8String` 要求输入始终是合法且非空的 UTF-8，并且 `trimmed_num` 不超过字符数；`ConvertPosInUtf8` 要求 `pos` 非负且不超过字节长度，允许切在多字节字符中；LIKE 执行要求权重、类型数组长度和 matcher 索引彼此一致。`Utf8Len` 只是按前导位计数，不验证合法性，测试明确记录 `0xff` 得到 8。

`LowerOneStringExcludeEscapeChar` 只折叠 ASCII；非 ASCII 通过 `Utf8Len` 跳过，输入缓冲区应来自合法 UTF-8。`Escape` 不校验标识符合法性，只负责包围及双写引号；`BuildStringFromLabels` 不转义 key/value 中的 `,` 或 `=`；`CompileLike2Regexp` 只生成文本，本文件不编译或执行正则。上述限制不应在扩展时被误当作已完成的上层校验。

## 并发与资源生命周期

本文件没有线程、async、goroutine、channel、锁、事务或外部 I/O。绝大多数结果完全归调用方所有，临时向量和字符串在函数返回后按 Rust 所有权正常释放；LIKE 编译结果可由调用方跨多次匹配复用。

`MemoizedStringer` 明确不是并发安全类型：`RefCell` 提供单线程运行时借用检查且使该结构不具备 `Sync` 语义；重入或重叠可变借用也会 panic。若未来需要跨线程缓存，不能只给现有类型附加不安全标记，应另行选择 `Mutex`/`OnceLock` 并保留“空结果不缓存”的 Go 行为，且新增独立测试。

原地修改函数借用 `&mut [u8]` 或 `&mut String`，借用期间由类型系统排除同一缓冲区的并发别名写入；文件本身没有全局可变状态。调用方 `builtin_ilike.rs` 的模式缓存使用 `RwLock`，但那是上游生命周期管理，不属于本文件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/stringutil/string_util.go`，Rust 基本保留其函数分组、模式标签、`%_ -> _%`/连续 `%` 规范化、Russ Cox glob 回退、标签排序、ASCII 位运算以及 escape 保护分支。`pkg/util/stringutil/string_util_test.go` 的 Unquote、LIKE、正则、精确匹配、标签、glob、memoize 表格已移植到独立 Rust 测试；Rust 还把 Go benchmark 的正负 LIKE 输入保留为普通断言。

关键语言差异是 Go `string` 可保存任意字节，而 Rust `&str` 必须是 UTF-8。因此 Rust 新增 `UnquoteBytes`/`UnquoteCharBytes` 作为精确兼容路径，并让 `Unquote`/`UnquoteChar` 做 `String` 适配。Go 的 `fmt.Stringer`、`errors.Trace`、`mysql.SQLMode`、`regexp.QuoteMeta` 与 `hack.Slice` 分别由局部 `GoStringer`、`StringUtilError`、类型别名/常量、`regexp_quote_meta` 与 `to_owned` 表达；它们保持当前行为形状，但不代表统一依赖已经接线。

Rust 的 `ConvertPosInUtf8` 使用 `String::from_utf8_lossy(prefix).chars().count()` 模拟 Go `utf8.RuneCountInString` 对截断多字节后缀计一个 `RuneError` 的行为，独立测试以 `ConvertPosInUtf8("你好", 4) == 3` 固化。`MemoizedStringer` 用 `RefCell` 表达 Go 闭包捕获的可变字符串，同时保持“仅缓存非空结果”。

## 扩展指南

修改转义规则时，应同时更新 `UnquoteCharBytes`（真实字节语义）及必要的 `&str` 适配器，并在 `string_util_test.rs::test_unquote` 和 `migration_aster_unit_test.rs::migration_unquote_matches_go_escape_and_raw_byte_behavior` 增加合法、非法、控制字节和非法 UTF-8 用例；还要核对 Go 的 `UnquoteChar`/`Unquote`，不要仅让 UTF-8 正例通过。

修改 LIKE 语法或优化时，Unicode 与 binary 两个 `CompilePatternInner*` 必须同步，`doMatchInner` 要继续支持 collate 的定制 matcher 并维持线性回退特性。至少同步 `test_pattern_match`、`test_compile_like_2_regexp`、`test_is_exact_match`、两个 benchmark 语义断言和迁移对齐测试，并检查 collate、planner 精确 LIKE、内存表及 ILIKE 调用方。尤其要保留自定义 escape、连续 `%`、`%_`、Unicode 单字符与负向长 `%` 链用例。

修改 ASCII/UTF-8 helper 时，应同步 `migration_utf8_and_ascii_mutation_match_go`、`test_convert_pos_in_utf8_inside_multibyte_character`，并检查标量/向量 ILIKE。若接入统一错误、SQL mode、stringer 或 regex 类型，应先评估公开签名和 workspace Cargo 依赖，不能把当前局部占位静默解释为完全等价。所有 Rust 测试仍应留在同目录独立测试文件，不嵌入本生产源文件。

性能风险主要在 `CompilePattern`/`DoMatchCustomized` 每次收集完整 `Vec<char>`、正则转换逐字符分配、标签排序，以及任何破坏 `doMatchInner` 单回退点算法的修改；兼容风险集中在 Go 任意字节语义、escape 尾字符、ASCII-only ILIKE 和截断 UTF-8 前缀计数。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/stringutil` 识别 `string_util.rs` 为含 55 个符号的目标文件。
- RustCodeGraph `node --file pkg/util/stringutil/string_util.rs`：完整读取 1–805 行，并报告 expression、planner 与迁移测试等使用文件；精确 `query` 核对了 `Unquote`、`CompilePattern`、`DoMatch`、`Escape`、`BuildStringFromLabels`、`LowerOneStringExcludeEscapeChar`、`EscapeGlobQuestionMark`。
- 调用边核验：`pkg/util/collate/bin.rs`、`general_ci.rs`、`unicode_0400_ci_impl.rs`、`unicode_0900_ai_ci_impl.rs`、`gbk_chinese_ci.rs`、`gb18030_chinese_ci.rs`；`pkg/expression/builtin_ilike.rs`、`builtin_ilike_vec.rs`；`pkg/planner/core/expression_rewriter.rs`、`memtable_infoschema_extractor.rs`、`memtable_predicate_extractor.rs`；`pkg/util/breakpoint/breakpoint.rs`。
- crate/装配核验：`pkg/util/stringutil/Cargo.toml`、`pkg/util/stringutil/lib.rs`，以及 expression、planner core、memtable extractors、breakpoint 的 Cargo 路径依赖声明。
- Go 与测试核验：`pkg/util/stringutil/string_util.go`、`string_util_test.go`、`string_util_test.rs`、`migration_aster_unit_test.rs`。测试证据覆盖非法语法/非法 UTF-8、控制转义、Unicode 与 binary LIKE、自定义 escape、正则元字符、精确模式、稳定标签顺序、ANSI 引号、截断 UTF-8、ASCII escape 保护、glob 和非空 memoize。
- 本任务是纯文档分析，未运行 Cargo；最终结构验证要求本文恰有十一个规定的二级标题，并另行检查唯一新增生产物、源码/Go/Cargo/测试均未改动。
