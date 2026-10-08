# `pkg/util/table-filter/parser.rs`

## 文件定位

[`parser.rs`](parser.rs) 是 `astersql-util-table-filter` crate 的规则文本解析层。crate 入口 [`lib.rs`](lib.rs) 将本模块公开为 `parser` 并再导出其符号；上层 [`table_filter.rs`](table_filter.rs) 的 `Parse` 和 [`column_filter.rs`](column_filter.rs) 的 `ParseColumnFilterRules` 分别用它把字符串参数或 `@file` 中的行转换成 `tableRule`、`columnRule`。本文件只负责语法解析、匹配器构造和来源位置错误，不负责最终的“后写规则优先”排序或实际匹配；规则反转与匹配发生在上述两个上层文件。

[`Cargo.toml`](Cargo.toml) 定义本目录为独立库 crate `astersql-util-table-filter`，库入口是 `lib.rs`，直接依赖 `regex = "1.11"` 和 `regex-syntax = "0.8"`。其中本文件通过同 crate 的 `newRegexpMatcher` 间接使用正则实现，而不是直接导入这两个 crate。

## 核心职责

- `tableRulesParser::parse` 识别空行、注释、否定前缀 `!`、文件导入前缀 `@`，并把一条 `schema.table` 规则拆成两个 `matcher` 后追加到 `rules`。
- `columnRulesParser::parse` 使用同一套模式语法解析单列规则，并在保存前调用 `matcher::toLower`，落实列名始终大小写不敏感的约定。
- `matcherParser::parsePattern` 在显式正则 `/.../`、双引号标识符、反引号标识符和普通通配模式之间分派。
- `matcherParser::parseWildcardPattern` 把 glob 风格的 `*`、`?`、`[...]` 转成锚定正则；完全没有通配符时保留为成本更低的 `stringMatcher`。
- 两个 `import_file` 逐行读取外部规则文件，将文件名与行号写入错误上下文，并通过 `can_import = false` 禁止递归导入。

## 主要符号

- `pub struct tableRulesParser { rules, matcher_parser }`：表规则的有状态收集器。`parse(&mut self, line, can_import)` 是主要入口；`import_file` 是私有文件读取路径。
- `pub struct columnRulesParser { rules, matcher_parser }`：列规则的有状态收集器，接口形状与表规则解析器一致，但单个模式不允许未转义的 `.`，且结果统一转小写。
- `pub struct matcherParser { fileName, lineNum }`：保存当前来源位置。`errorf` 与 `annotatef` 是公开的定位错误构造器；`parsePattern`、`parseWildcardPattern` 是公开的模式解析函数。
- `matcherParser::wrap_error`：生成 `at <file>:<line>: ...` 前缀。
- `matcherParser::regexp_matcher`：调用 [`matchers.rs`](matchers.rs) 的 `newRegexpMatcher`，并把编译失败包装为 `invalid pattern`。
- `find_quoted_end(bytes, delimiter, doubled_escape)`：定位 `/.../`、`"..."`、反引号模式的结束位置。斜杠模式用反斜杠跳过下一字节；标识符用成对定界符表示转义；空的成对定界串被视为不完整。
- `character_class_end(bytes)`：寻找 glob 字符类的 `]`，拒绝空类、缺少右括号以及反斜杠转义字母数字。

本文件没有模块级常量、trait、枚举、条件编译项或异步入口。三个解析器类型及其若干方法虽然是 `pub`，真实生产调用仍集中在同 crate 的表/列过滤入口。

## 执行流程

表规则的主流程如下：

1. `table_filter::Parse` 以 `<cmdline>:1` 创建 `tableRulesParser`，对每个参数调用 `parse(arg, true)`。
2. `parse` 仅裁掉首尾空格和制表符；空行和裁剪后以 `#` 开头的行直接成功返回。
3. 行首 `!` 将规则标记为否定并移除一个前缀；行首 `@` 在允许导入时转入 `import_file`，否则报告递归导入错误。前缀判定只执行一次，因此 `!@file` 是名为 `@file` 的否定模式，不是导入。
4. 第一次 `parsePattern(line, true)` 解析 schema。通配解析遇到未转义 `.` 时停止并把剩余切片返回；显式正则或引号模式则在闭合定界符处停止。
5. 调用者要求剩余文本以 `.` 开头，再以相同规则解析 table；若仍有剩余字符则拒绝整行。
6. 成功后追加 `tableRule { schema, table, positive }`。上层在全部参数解析完毕后反转规则列表，由匹配层实现 last-rule-wins。

列规则流程相同，但 `parsePattern(line, false)` 不把 `.` 当分隔符，未转义的点会报错；成功的匹配器先 `toLower()` 再写入 `columnRule`。上层同样在解析完后反转列表。

模式分派与通配转换的关键规则是：`/.../` 原样交给正则编译；双引号中的 `""` 还原为 `"`，反引号中的双反引号还原为单反引号；`*` 变成 `.*`，`?` 变成正则的单字符点号，`[!...]` 变成否定正则类 `[^...]`，而 glob 的 `[^...]` 会转义 `^` 以保持其字面含义。生成的通配正则加 `(?s)^` 与 `$`，因此匹配完整字符串且 `.` 可跨换行。

文件导入时，两个 `import_file` 都打开路径、暂存旧来源、把来源切换到导入文件第 1 行、用 `BufReader::lines()` 逐行解析并递增行号，正常结束后恢复旧来源。导入行调用 `parse(line, false)`，所以第二层 `@file` 会在实际文件和行号处失败。

## 数据与状态

- `rules: Vec<tableRule>` / `Vec<columnRule>` 只在一行完整解析成功后追加，因此语法失败不会留下半条规则；但此前已成功解析的规则仍保留在解析器中。
- `matcher_parser.fileName` 与 `lineNum` 是可变诊断状态。命令行参数全部共享 `<cmdline>:1`；导入文件从 1 开始逐行递增。
- `parsePattern` 返回 `(Box<dyn matcher>, &str)`：匹配器拥有解析结果，剩余输入仍借用原始行，避免复制尾部文本。
- `parseWildcardPattern` 同时维护 `literal` 和 `wildcard` 字节缓冲。只要出现 `*`、`?` 或字符类，就选择正则路径；否则用 `stringMatcher`。转义符本身不强制切换到正则路径。
- 解析按 UTF-8 字符串的字节遍历。非 ASCII 字节允许作为普通模式内容并原样复制；错误消息中非法 ASCII 特殊字符按单字节字符显示。
- 如果导入中的读取或解析提前返回错误，当前实现不会执行函数尾部的来源恢复，解析器会保留导入文件位置。生产入口随后立即传播错误并丢弃解析器，所以正常 API 不观察该状态；若未来复用解析器继续解析，应先修复或显式处理这个状态约束。

## 依赖与调用关系

已由 RustCodeGraph 的目标文件节点和仓库直接引用交叉核对的主链为：

```text
table_filter::Parse
  -> tableRulesParser::parse
     -> matcherParser::parsePattern
        -> matcherParser::parseWildcardPattern
        -> find_quoted_end
        -> matcherParser::regexp_matcher
           -> matchers::newRegexpMatcher
        -> character_class_end

column_filter::ParseColumnFilterRules
  -> columnRulesParser::parse
     -> matcherParser::parsePattern
     -> matcher::toLower
```

`@file` 分支从两个 `parse` 分别进入各自的 `import_file`，再以 `can_import = false` 回调同一个解析入口。规则数据类型 `tableRule`、`columnRule`、trait `matcher`、`stringMatcher` 与 `newRegexpMatcher` 均来自 [`matchers.rs`](matchers.rs)。文件 I/O 仅依赖标准库 `File`、`BufReader` 和 `BufRead::lines`。

RustCodeGraph 将 [`parser_test.rs`](parser_test.rs)、[`table_filter.rs`](table_filter.rs)、[`column_filter.rs`](column_filter.rs) 和匹配器模块列为本文件的直接使用者；精确图查询没有返回稳定的逐方法 callers/callees 输出，因此上述方法级边又用同目录符号引用搜索核验。仓库外部通常通过 `lib.rs` 的再导出调用 `Parse` 或 `ParseColumnFilterRules`，而不是直接操作这些解析器。

## 错误处理与边界

- 所有可预期解析失败统一返回 `FilterError`，文本以前述来源位置开头。I/O 与正则编译错误经 `annotatef` 保留底层错误文字。
- 缺失模式、缺失 schema/table 分隔点、模式后杂字符、不完整正则/引号、行尾反斜杠、非法特殊字符、非法字符类和对字母数字的保留转义均有独立错误分支。
- `//`、`""` 与空反引号模式被 `find_quoted_end` 明确当作“不完整”，与 Go 正则定义中要求至少一个内容单元的行为一致。
- `character_class_end` 只验证 glob 类的结构边界；例如 `[!]` 会被转换为 `[^]$`，随后由正则编译器返回更具体的无闭合 `]` 错误。这一区分由 `parser_test.rs` 固定。
- 行尾只裁剪空格与制表符，不裁剪其他 Unicode 空白；整行注释仅在裁剪后第一个字符是 `#` 时成立，模式后的 `#` 是非法特殊字符。
- 文件名取自 `@` 后的完整已裁剪行，因此路径两端的空格/制表符无法保留。文件不能打开、行读取失败或嵌套导入都会返回当前来源位置错误。
- `String::from_utf8(...).expect(...)` 依赖输入原本是有效 UTF-8 且转换只复制/插入 ASCII 字节这一不变量；在该不变量下不会成为用户可触发的错误路径。

## 并发与资源生命周期

解析器由 `&mut self` 串行更新规则和来源位置，没有锁、原子变量、线程、异步任务或通道。类型本身不提供跨线程共享语义；调用方应为每次解析创建独立实例，或在外部同步可变访问。

导入文件的 `File` 被 `BufReader` 拥有，函数返回时依靠 Rust RAII 关闭，包括错误返回路径。每一行的临时 `String` 只活到该轮解析结束，规则匹配器拥有其所需内容。正则与字面量匹配器装箱进入规则向量，随后所有权转移到 `tableFilter` 或 `ColumnFilterRules`。文件读取是同步、逐行且不设显式行长上限；超长行会增加内存使用，但不会产生后台资源。

## 与 Go 版本的对应关系

直接对照文件是 [`parser.go`](parser.go)。Rust 保留了 Go 的两类规则解析器、位置上下文、四种模式语法、glob 到正则转换、禁止递归导入、列匹配器小写化以及主要错误消息。`table_filter_test.go`、`column_filter_test.go` 与对应 Rust 独立测试沿用了匹配、失败和导入用例。

实现层面的主要差异如下：

- Go 用预编译正则寻找 `/.../`、引号和字符类边界；Rust 用 `find_quoted_end`、`character_class_end` 手工扫描，并由 [`parser_test.rs`](parser_test.rs) 专门验证空定界模式及字符类的 Go 兼容边界。
- Go 将共同的 `importFile` 放在 `matcherParser` 并接收解析回调；Rust 在表/列解析器中各有一个同形私有 `import_file`。
- Go 使用 `bufio.Scanner`，Rust 使用 `BufRead::lines()`。因此 Rust 没有 Scanner 默认 token 大小限制，并要求输入行是有效 UTF-8；这两项并非完全等价，若过滤文件可能包含极长行或非 UTF-8 字节，应增加独立兼容测试再决定行为。
- Go 的 `errors.Annotatef` 建立错误链；Rust 的 `FilterError` 目前保存格式化后的单个字符串，因此可见错误文本对齐，但不保留可供程序遍历的底层错误类型链。
- Go 正常导入结束后先恢复外层来源，再检查 `Scanner.Err()`；Rust 对 `lines()` 的读取错误在恢复前立即返回。两者在失败后复用内部解析器时都有状态细节，公开 `Parse`/`ParseColumnFilterRules` 路径则直接返回并销毁解析器。

## 扩展指南

- 新增规则前缀或行级语法时，修改两个 `parse` 的共同分支，并同时扩展 [`table_filter_test.rs`](table_filter_test.rs)、[`column_filter_test.rs`](column_filter_test.rs) 及 Go 对照测试；避免只更新一种规则类型导致语法漂移。
- 新增模式形式时，入口是 `matcherParser::parsePattern`；新增 glob 元字符或转义语义时，入口是 `parseWildcardPattern`，通常还需调整 `character_class_end` 或新增专用扫描器。必须验证字面量优化路径和正则路径产生相同匹配边界。
- 调整正则转换时保持 `(?s)^...$`、`[!]`/`[^]` 差异、ASCII 字母数字转义保留规则以及非 ASCII 字节处理；这些会直接影响与 Go 的兼容性和正则编译错误文本。
- 扩展 `@file` 能力时，应优先抽取两个重复的 `import_file`，并用作用域守卫确保任何错误路径都恢复 `fileName`/`lineNum`。若允许递归导入，还必须增加深度/循环检测与路径解析策略，不能只把 `can_import` 改成 `true`。
- 测试逻辑必须继续放在独立文件。解析扫描器的局部边界放在 [`parser_test.rs`](parser_test.rs)；公开表规则行为放在 [`table_filter_test.rs`](table_filter_test.rs)；公开列规则行为放在 [`column_filter_test.rs`](column_filter_test.rs)。需要保持 Go 语义时同步检查同名 `.go` 测试。
- 性能风险主要来自把原本可用 `stringMatcher` 的模式误判为正则、无界长导入行以及重复编译大量等价正则；兼容风险主要集中在错误文本、Unicode、转义、字符类和文件读取差异。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 20 个索引文件；`files --filter pkg/util/table-filter` 确认目标源码、Go 对照与测试均在索引中；`node --file pkg/util/table-filter/parser.rs --offset 1 --limit 500` 读取目标文件 358 行和 19 个符号；`query` 确认 Rust/Go 两侧的 `tableRulesParser`、`columnRulesParser`、`matcherParser`、`parsePattern`、`parseWildcardPattern` 及两个私有扫描函数。精确 callers/callees 查询未产出稳定结果，方法级调用边用同目录直接引用搜索补证。
- 源码与 crate 边界：[`parser.rs`](parser.rs)、[`matchers.rs`](matchers.rs)、[`table_filter.rs`](table_filter.rs)、[`column_filter.rs`](column_filter.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。本包没有 `doc.go`。
- Go 对照：[`parser.go`](parser.go)、[`table_filter_test.go`](table_filter_test.go)、[`column_filter_test.go`](column_filter_test.go)。
- Rust 独立测试：[`parser_test.rs`](parser_test.rs) 验证空定界模式和字符类错误边界；[`table_filter_test.rs`](table_filter_test.rs) 与 [`column_filter_test.rs`](column_filter_test.rs) 覆盖公开解析、匹配、错误、导入及递归导入行为。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅执行任务指定的 11 章节结构检查，并人工复核本文没有把推测写成已支持行为。
