# `pkg/parser/util/escape.rs`

## 文件定位

`escape.rs` 是 parser 工具层的 MySQL 字符串反斜杠转义映射器。它不负责识别完整字符串字面量，而只接收已经由上游识别出的“反斜杠后一个字节”，返回应写入结果缓冲区的字节序列。源码通过三条装配路径使用：`pkg/parser/util/lib.rs` 将其作为 `astersql-parser-util` crate 的 `escape` 模块公开并再导出；`pkg/parser/lib.rs` 以 `#[path = "util/escape.rs"] pub mod util` 将同一实现纳入 parser crate；`pkg/parser/ast/util.rs` 也以路径模块纳入并再导出给 AST crate。根 crate 还在 `pkg/lib.rs` 的 `parser::util` 门面中再导出独立工具 crate。

该文件位于 SQL 文本进入语法分析之前的字节处理边界。`pkg/parser/lexer.rs::Scanner::scanString` 在扫描引号字符串时识别反斜杠并由 `handleEscape` 调用它；`pkg/parser/ast/base.rs::convertBinaryStringLiterals` 在把不可打印字符串字面量改写为十六进制形式时也调用它，以保证改写前后的字节语义一致。

## 核心职责

- 实现 `pkg/parser/util/escape.go::UnescapeChar` 的逐字节 Rust 对照逻辑，保持 MySQL 字符串字面量规则一致。
- 把 `\n`、`\0`、`\b`、`\Z`、`\r`、`\t` 映射为对应的单个控制字节。
- 对 LIKE 通配符 `\%` 和 `\_` 保留反斜杠，返回两个字节，避免它们过早变成未转义通配符。
- 对引号、反斜杠和未知转义采用统一默认规则：去掉调用者已经消费的前导反斜杠，只返回当前输入字节。

本文件不判断引号边界、不读取后续字节、不解释字符编码，也不判断 `NO_BACKSLASH_ESCAPES`。这些职责属于调用者；例如 `Scanner::scanString` 只在 `!self.sqlMode.HasNoBackslashEscapesMode()` 时进入 `handleEscape`。

## 主要符号

文件只定义一个公开函数，没有模块级常量、类型、trait、`impl` 或条件编译项。

- `pub fn UnescapeChar(b: u8) -> Vec<u8>`：输入 `b` 是反斜杠后的单个原始字节。输出长度恒为 1 或 2；`b'%'`、`b'_'` 返回 `[b'\\', b]`，其余输入返回一个字节。函数保留 Go 风格名称，以便迁移代码和 Go 对照保持直观；所在 crate 的 `lib.rs` 通过 `#![allow(non_snake_case)]` 接受这一命名。

精确映射为：`n -> 0x0a`、`0 -> 0x00`、`b -> 0x08`、`Z -> 0x1a`、`r -> 0x0d`、`t -> 0x09`、`%/_ -> 反斜杠加原字节`，其他 248 个 `u8` 值原样返回。`Z` 的结果是 ASCII 26，即 MySQL 兼容的 Windows 文本 EOF 控制字符。

## 执行流程

1. 上游扫描器发现一个反斜杠，并确认当前 SQL mode 允许反斜杠转义。
2. 上游读取或查看反斜杠后的一个字节，将该字节传给 `UnescapeChar`；函数本身不接收反斜杠。
3. `match` 先处理六种控制字符转义，每种分配并返回一个单字节 `Vec<u8>`。
4. 若输入是 `%` 或 `_`，返回反斜杠与原字符组成的双字节向量。
5. 其余输入走默认分支，仅返回输入字节，相当于删除前导反斜杠。
6. 调用者把返回向量追加到自己的缓冲区：`Scanner::handleEscape` 追加到 `self.buf`；`convertBinaryStringLiterals` 追加到局部 `content`，之后再逐字节编码为 `0xHH...`。

重要不变量是“每次调用恰好处理反斜杠后的一个字节”，而不是一段 UTF-8 字符或完整转义序列。因此调用者必须保证反斜杠后仍有字节；`scanString` 在调用前检查 EOF，`convertBinaryStringLiterals` 检查 `position + 1 < content_bytes.len()`。

## 数据与状态

函数只处理 `u8` 和新建的 `Vec<u8>`，不读取或修改全局状态，也不缓存结果。输入覆盖完整的 0 到 255 字节域；这使它适用于尚未进行字符集解码的 SQL 原始字节。输出拥有自己的内存，长度仅可能为 1 或 2，随后通常被调用者的缓冲区消费。

字符集、引号种类、SQL mode、当前扫描位置和目标缓冲区均不在本模块保存。尤其是非 ASCII 输入不会在这里进行 UTF-8 解码或合法性校验，而是由默认分支按单字节保留。`pkg/parser/util/migration_aster_unit_test.rs::unescape_char_matches_go_for_every_byte` 穷举全部 `u8`，验证了这一全域行为。

## 依赖与调用关系

`UnescapeChar` 的实现仅依赖 Rust 标准库的 `Vec` 和字节字面量，不依赖第三方 crate。`pkg/parser/util/Cargo.toml` 声明独立包 `astersql-parser-util`，没有 `[dependencies]`，并用 `package.metadata.porting.go-package = "pkg/parser/util"` 记录 Go 来源。

直接上游证据如下：

- `pkg/parser/lexer.rs::Scanner::handleEscape -> util::UnescapeChar`：SQL lexer 的字符串扫描路径。
- `pkg/parser/ast/base.rs::convertBinaryStringLiterals -> crate::util::UnescapeChar`：AST 文本保存/恢复时，对不可打印字符串内容先还原转义，再输出十六进制字面量。
- `pkg/lib_test.rs::independent_parser_config_modules_are_wired -> crate::parser::util::UnescapeChar`：验证根门面的公开接线。
- `pkg/parser/util/escape_test.rs::test_unescape_char` 与 `pkg/parser/util/migration_aster_unit_test.rs::unescape_char_matches_go_for_every_byte`：独立工具 crate 内的直接测试调用。

RustCodeGraph 的文件节点报告 `escape.rs` 被 `pkg/parser/lexer.rs`、`pkg/parser/ast/base.rs`、`pkg/lib_test.rs` 使用；精确 `callers` 查询未生成函数级边，因此上述函数级调用关系又以精确符号搜索和相邻源码核验。函数内部没有下游函数调用，RustCodeGraph 的 `callees` 结果为空。

## 错误处理与边界

函数签名没有 `Result` 或 `Option`，也不会 panic；对所有 `u8` 输入都给出确定结果。未知转义不是错误，而是按 Go/MySQL 兼容规则删除反斜杠并保留输入字节。尾随反斜杠无法由该函数单独表达，因为输入参数要求已有后续字节；是否调用以及尾随反斜杠如何处理由扫描器负责。

最容易误改的边界是 `%` 与 `_`：它们必须保留反斜杠，而不能归入默认分支。另一个边界是大小写敏感：只有大写 `Z` 映射到 26，小写 `z` 走默认分支。NUL、退格和 Ctrl-Z 使用数值字节而不是 Unicode 字符抽象。函数也不会自行应用 `NO_BACKSLASH_ESCAPES`；若调用者在该模式下仍调用它，会改变原始 SQL 字节语义。

## 并发与资源生命周期

函数是无状态纯映射，没有锁、原子变量、通道、任务、文件句柄、事务或异步生命周期。每次调用独立分配一个最多两字节的 `Vec<u8>`，返回后其所有权交给调用者；调用者通过 `extend` 复制进长期缓冲，临时向量随后释放。因此并发调用彼此不影响，也不要求同步。

该分配行为位于逐转义字符的热路径。若未来优化为固定容量容器、借用切片或直接写入调用者缓冲区，必须同时评估所有装配路径和调用签名，且以基准或等价测试证明没有改变 `%/_` 的双字节结果及全部 256 个输入的语义。

## 与 Go 版本的对应关系

Rust `UnescapeChar` 与 `pkg/parser/util/escape.go::UnescapeChar` 分支逐项对应：Go 的 `byte` 对应 Rust 的 `u8`，Go 的 `[]byte` 对应 Rust 的 `Vec<u8>`，`switch` 对应 `match`。六种控制字符、两个 LIKE 通配符以及默认分支的结果完全相同，没有 Rust 特有的额外转义或错误路径。

`pkg/parser/util/escape_test.go::TestUnescapeChar` 与 Rust 的 `pkg/parser/util/escape_test.rs::test_unescape_char` 使用相同代表性用例，覆盖控制字符、LIKE 通配符、自转义字符和未知字符。Rust 额外由 `migration_aster_unit_test.rs::unescape_char_matches_go_for_every_byte` 在测试内复刻 Go 分支并穷举全部 256 个输入，防止默认分支或稀有字节在迁移中偏离。当前证据表明这是已接线实现，而不是桩或仅供占位的门面。

## 扩展指南

- 新增或修改 MySQL 转义时，首要修改点是 `UnescapeChar` 的 `match`；应先确认 Go `escape.go` 和目标 MySQL 兼容语义，再同步 Go/Rust 独立测试，不能只为 Rust 添加简化规则。
- 必须在 `pkg/parser/util/escape_test.rs::test_unescape_char` 添加可读的代表性回归项，并在 `migration_aster_unit_test.rs` 的 Go 基准函数确有上游变化时同步它；测试逻辑继续放在独立测试文件，不嵌入 `escape.rs`。
- 改动返回类型、命名或可见性前，应检查三种源码装配路径以及 `lexer.rs`、`ast/base.rs`、根 facade 的调用，避免只让独立 `astersql-parser-util` crate 通过而破坏 parser/AST 的路径包含。
- 若规则依赖 SQL mode、上下文或多字节序列，应优先把判断留在拥有扫描状态的调用者，而不是在这个单字节纯映射函数中引入隐式状态。
- 兼容性风险主要是 SQL 字面量值或 LIKE 模式语义变化；性能风险主要是每个转义产生临时小向量。任何优化都应保持输出顺序、长度和全字节等价性。

## 验证依据

- 源实现：`pkg/parser/util/escape.rs`，确认唯一生产符号及完整 `match` 分支。
- crate 与模块装配：`pkg/parser/util/Cargo.toml`、`pkg/parser/util/lib.rs`、`pkg/parser/lib.rs`、`pkg/parser/ast/util.rs`、根 `Cargo.toml` 与 `pkg/lib.rs`。
- 直接调用：`pkg/parser/lexer.rs::Scanner::scanString` / `handleEscape`、`pkg/parser/ast/base.rs::convertBinaryStringLiterals`、`pkg/lib_test.rs::independent_parser_config_modules_are_wired`。
- Go 对照：`pkg/parser/util/escape.go::UnescapeChar` 与 `pkg/parser/util/escape_test.go::TestUnescapeChar`。
- Rust 独立测试：`pkg/parser/util/escape_test.rs::test_unescape_char`；全字节迁移验证：`pkg/parser/util/migration_aster_unit_test.rs::unescape_char_matches_go_for_every_byte`。
- RustCodeGraph：`status` 显示本地索引包含 11,467 个文件；`files --filter pkg/parser/util` 找到目标与对照文件；`query UnescapeChar` 和 `node --file pkg/parser/util/escape.rs` 核对定义；文件节点给出三个使用文件；带 `--file pkg/parser/util/escape.rs` 的 `callers` 与 `callees` 均为空，函数级调用边改由精确源码搜索补证。
- 本任务为纯文档分析，按计划不运行 Cargo；最终使用任务文件给定命令检查目标存在且固定二级标题恰好为 11 个，并人工复核本文未把 SQL mode、编码或扫描职责误归于本函数。
