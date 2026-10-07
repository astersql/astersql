# `pkg/parser/charset/encoding_table.rs`

## 文件定位

本文说明的真实源文件是 [`encoding_table.rs`](./encoding_table.rs)。它位于 `astersql-parser-charset` crate，crate 根由 `pkg/parser/charset/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定。`pkg/parser/charset/lib.rs` 通过 `pub mod encoding_table` 装入本模块，并用 `pub use encoding_table::*` 将其公开符号提升到 crate 根，因此调用方既可以使用模块路径，也可以从 `parser_charset` 根命名空间取得查找 API。

它是 HTML/WHATWG 编码标签与具体编解码器之间的适配表：输入用户或协议侧的编码标签，输出 `encoding` 0.2.33 crate 的静态编解码器引用和规范名称。它只负责“标签解析”，不实现 `pkg/parser/charset/encoding.rs` 中的字符集转换策略，也不包含 GBK、GB18030 等编码算法本身。

仓库检索目前只发现独立 Rust 测试直接调用此 API，未发现非测试 Rust 生产文件的直接调用点。因此它是 crate 的公开能力和编码实现的测试基准，但不能仅凭公开再导出断言其已进入 SQL 请求主链。

## 核心职责

- `lookup` 先按 HTML 标签规则裁剪首尾的制表符、换行、回车、换页和空格，再执行 Unicode 小写化。
- `lookup_normalized` 将规范化标签映射到 `encoding` crate 的 `EncodingRef` 及规范名称；同一编码的多个别名合并到同一个 `match` 分支。
- 未知标签返回 `None`，把 Go 的“`nil` 编码加空名称”表达为 Rust 的可选结果。
- `Lookup` 保留 Go 导出 API 的拼写，作为 `lookup` 的薄包装；crate 根的 `#![allow(non_snake_case)]` 允许这一兼容名称。
- 本文件不缓存、不初始化可变全局状态，也不执行实际编码或解码。返回的编解码器由外部 `encoding` crate 提供，调用方随后自行调用其 `encode`/`decode` 方法。

## 主要符号

- `pub struct LookupResult { encoding: ::encoding::types::EncodingRef, name: &'static str }`：一次成功查找的完整结果。两个字段均公开；结构体实现 `Clone + Copy`，复制只复制静态引用和静态字符串引用。
- `pub fn lookup(label: &str) -> Option<LookupResult>`：Rust 风格公开入口，负责裁剪和小写化，再委托给内部表查询。
- `pub fn Lookup(label: &str) -> Option<LookupResult>`：Go 风格公开兼容入口，不增加分支或状态，直接调用 `lookup`。
- `const fn entry(encoding: EncodingRef, name: &'static str) -> LookupResult`：内部构造辅助函数，使各匹配分支只声明具体编解码器与规范名。
- `fn lookup_normalized(label: &str) -> Option<LookupResult>`：私有静态匹配表。它要求参数已经规范化；命中时构造结果，未命中时提前返回 `None`。

本文件没有 trait、`impl`、模块级可变变量或条件编译项。RustCodeGraph 将上述结构体和函数识别为本文件的主要符号；文件总计包含 6 个索引符号。

## 执行流程

1. 调用方将原始标签传给 `Lookup` 或 `lookup`。`Lookup` 只转发到 `lookup`。
2. `lookup` 用 `trim_matches(['\t', '\n', '\r', '\u{000c}', ' '])` 仅删除 HTML 规定的五类首尾空白；字符串内部空白及其他 Unicode 空白不会被裁剪。
3. `to_lowercase` 创建小写化后的 `String`。测试 `lookup_uses_unicode_lowercase_like_go` 用 Kelvin 符号 `U+212A` 验证 Unicode 大小写处理能落到 `koi8-r` 标签，而不只是处理 ASCII 大写。
4. `lookup_normalized` 对规范化字符串执行穷举 `match`。例如 `utf8mb4` 返回 `UTF_8` 与规范名 `utf-8`，`GB_2312-80` 经小写化后返回 `GBK` 与规范名 `gbk`。
5. 每个命中分支用 `entry` 构造 `LookupResult`，最后包装为 `Some`；通配分支直接返回 `None`。
6. 调用方可使用 `result.name` 做规范名判断，或使用 `result.encoding` 调用外部编解码器。`encoding_test.rs` 分别用 `gbk` 和 `gb18030` 的结果生成预期字节并验证转换往返。

表内的重要特殊映射包括：`binary` 使用 `UTF_8` 编解码器但保留规范名 `binary`；ASCII、Latin-1 和若干同义标签按 WHATWG 规则映射到 `WINDOWS_1252`；`iso-2022-kr`、`iso-2022-cn` 等标签映射到 `whatwg::REPLACEMENT`；`utf-16` 映射到小端 `UTF_16LE`。

## 数据与状态

查找表由 `lookup_normalized` 的编译期字符串模式表达，而不是运行时 `HashMap`。Go 对照文件的 218 个标签经脚本核对均存在于 Rust 文件的字符串匹配项中；多个标签共享分支以减少重复结果值。

结果中的 `encoding` 是 `encoding::types::EncodingRef`，即指向外部 crate 提供的静态编码实现；`name` 是 `&'static str`。因此成功结果不借用输入 `label`，在输入释放后仍可使用，也不要求堆上保存表项。

唯一按调用产生的拥有型临时状态是 `lookup` 中 `to_lowercase` 返回的 `String`。表自身没有惰性初始化、锁、引用计数或可变缓存。`entry` 是 `const fn`，但当前调用发生在普通 `match` 执行路径中，其作用是统一构造而非建立单独的全局常量表。

## 依赖与调用关系

上游装配关系是 `pkg/parser/charset/lib.rs` → `encoding_table` 模块 → crate 根公开再导出。RustCodeGraph 的文件关系显示，本文件被 `encoding_table_test.rs`、`encoding_test.rs`、`encoding_gb18030_2_aster_unit_test.rs` 等测试文件使用；精确 `callers/callees` 命令未生成函数级调用边，因此又用仓库文本检索核实调用点。

仓库内已确认的直接调用者如下：

- `encoding_table_test.rs::lookup_uses_unicode_lowercase_like_go`：验证 Unicode 小写与 HTML 空白裁剪。
- `encoding_gb18030_2_aster_unit_test.rs::test_lookup_normalizes_html_labels`：验证 UTF8MB4、GBK 别名、GB18030 往返及未知标签。
- `encoding_test.rs::test_encoding` 与 `test_encoding_gb18030`：使用返回的外部 codec 生成 GBK/GB18030 预期字节。

下游依赖只有 Rust 标准库字符串操作和 Cargo 中声明的 `encoding = "0.2.33"`。具体常量来自 `::encoding::all`、`::encoding::all::whatwg`，引用类型来自 `::encoding::types::EncodingRef`。本文件不调用 crate 内的 `FindEncoding`，两者用途不同：前者接受 HTML 编码标签并返回通用 codec，后者服务 AsterSQL 自身字符集抽象。

## 错误处理与边界

API 不返回错误对象。合法标签返回 `Some(LookupResult)`，未知标签（包括空字符串、仅含可裁剪空白的字符串、拼写错误或表外标签）返回 `None`。调用方必须显式处理 `Option`；测试中的 `unwrap` 只用于已知固定标签，不代表生产调用可以忽略失败。

规范化只裁剪 `\t`、`\n`、`\r`、换页符和 ASCII 空格，刻意不使用 Rust 的通用 `trim`；新增空白处理时应先核对 Go `strings.Trim(label, "\t\n\r\f ")` 与 HTML 标签规范，避免扩大接受范围。`to_lowercase` 是 Unicode 小写化，现有独立测试覆盖 Kelvin 符号，但没有穷举所有 Unicode 多码点大小写转换，因此不应宣称所有非常规 Unicode 标签都与 Go 完全等价。

本函数不验证后续输入字节是否符合所选编码，编码/解码失败由返回的外部 codec 在实际 `encode`/`decode` 时处理。诸如 `binary` 使用 `UTF_8`、不受支持的 ISO-2022 变体使用 replacement codec，是表的显式兼容决策，而不是错误分支。

## 并发与资源生命周期

所有入口仅读取输入并访问静态编解码器引用，没有共享可变状态，因而本文件自身不需要锁、原子量、任务或通道。每次调用最多创建一个规范化 `String`，该字符串在 `lookup` 返回时释放；返回值不引用它。

`LookupResult` 的两个字段均指向静态数据，且结果可复制，所以这里没有显式关闭、归还或释放资源的协议。实际 codec 是否在内部维护状态取决于调用方随后创建和使用的编码器/解码器；本文件只返回静态 codec 描述，不持有一次转换的生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/charset/encoding_table.go`。Go `Lookup` 同样先执行 `strings.Trim(label, "\t\n\r\f ")` 与 `strings.ToLower`，再查询私有表。Go 用 `map[string]struct{ e encoding.Encoding; name string }` 保存 218 个标签；Rust 用 `match` 合并同值分支。脚本核对结果为 `go_labels=218` 且 `missing_in_rust=[]`。

返回模型不同：Go 返回 `(encoding.Encoding, string)`，未命中时依赖 map 零值形成 `(nil, "")`；Rust 用 `Option<LookupResult>` 把未命中显式化，并把规范名限制为静态字符串。Go 的 `encoding.Nop` 在 UTF-8 和 binary 标签中由 Rust 的 `encoding::all::UTF_8` 表示；其他 Go `x/text/encoding` 实现由 rust-encoding 中对应常量承接。这种库替换要求扩展时不仅比较规范名，还要用实际字节往返测试确认 codec 行为。

命名层面，Rust 同时提供惯用的 `lookup` 与兼容 Go 拼写的 `Lookup`。当前测试证明了关键标签、未知值和 Unicode Kelvin 符号路径，但不是对 218 个条目逐项做 codec 身份和行为的自动差分，因此“标签无缺失”不能替代所有编解码语义均已穷举验证。

## 扩展指南

新增或调整标签时，最可能修改的是 `lookup_normalized` 中对应的 `match` 分支；若新增 codec 类型，需要先确认 `encoding` 0.2.33 提供语义相符的静态 `EncodingRef`。保持别名、规范名和 codec 三者成组审查，不要只让标签能够命中。

同步修改要求如下：

- 对照更新 `encoding_table.go` 时，逐项核对标签集合、规范名称和实际 codec；若 Rust 有意偏离，须在文档和测试中写明原因。
- 在独立的 `encoding_table_test.rs` 添加规范化、别名或未命中回归测试；不要把测试嵌入生产源文件。
- 涉及具体编码字节行为时，在相邻独立测试（如 `encoding_test.rs` 或对应编码的 `*_test.rs`）增加严格编码/解码用例，而不只断言 `name`。
- 保持 `Lookup` 和 `lookup` 共用同一规范化路径，避免兼容入口与 Rust 入口产生差异。

兼容性风险主要是既有别名突然失效、规范名变化影响上层比较、或替换 codec 后同一字节序列行为改变。性能风险集中在每次查找都会分配小写化 `String` 以及大 `match` 的匹配成本；若要优化，必须保留 Unicode 小写和精确空白边界，并用基准或调用频率证据证明需要，不能直接改成仅 ASCII 的无分配逻辑。

## 验证依据

- 源码与符号：RustCodeGraph `node --file pkg/parser/charset/encoding_table.rs --offset 1 --limit 240` 读取完整 166 行文件；`query` 确认 `LookupResult`、`lookup`、`Lookup`、`entry`、`lookup_normalized` 的位置和签名。
- 图关系：RustCodeGraph `files --filter pkg/parser/charset` 确认模块文件集合；目标文件报告被 5 个测试文件使用。对精确符号执行 `callers/callees` 未得到边，故没有据此推断不存在调用者。
- crate 边界：读取 `pkg/parser/charset/Cargo.toml` 与 `pkg/parser/charset/lib.rs`，确认 crate 名、`encoding = "0.2.33"` 依赖、模块声明、公开再导出及独立测试装配。
- Go 对照：读取 `pkg/parser/charset/encoding_table.go`；提取其 218 个 map 标签并与 Rust 文件字符串匹配项比较，结果无缺失。
- 测试证据：读取 `pkg/parser/charset/encoding_table_test.rs`、`encoding_gb18030_2_aster_unit_test.rs` 和 `encoding_test.rs` 的相关用例；Go 侧 `encoding_test.go` 使用 `Lookup("gbk")`、`Lookup("gb18030")` 作为字节行为基准。
- 调用检索：用 `rg` 搜索 `encoding_table`、`Lookup(`、`encoding_table::lookup` 和 crate 路径；只确认到上述 Rust 测试调用与 `lib.rs` 装配，没有发现非测试 Rust 生产调用。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；交付前使用任务指定命令验证固定的 11 个二级章节，并人工复核所有本地路径和关键结论均有上述证据支撑。
