# `pkg/dumpformat/sqlfile/sql.rs`

## 文件定位

本文件是 `astersql-dumpformat-sqlfile` crate 的单值 SQL 字面量编码层。crate 入口 `pkg/dumpformat/sqlfile/lib.rs` 将本模块声明为私有模块 `sql`，再以 `pub use sql::append_value` 公开唯一函数；同 crate 的 `writer.rs::Writer::write` 用它编码 `INSERT ... VALUES` 中的每个字段。上层生产入口是 `dumpling/export/writer_util.rs::writeSQLFile`：该函数构造 `Writer`，把查询行交给 `Writer::write`，最终形成 Dumpling 的 SQL 数据文件。

该文件只负责“一个值如何写入已有字节缓冲区”，不负责 INSERT 前缀、行括号、字段逗号、语句切分、文件轮转、底层 I/O 或类型推断。这些边界分别由 `pkg/dumpformat/sqlfile/writer.rs` 与 `dumpling/export/writer_util.rs` 承担。

## 核心职责

- `append_value` 根据 `is_null` 与 `astersql_dumpformat::FieldKind`，向调用者提供的 `Vec<u8>` 末尾追加恰好一个 SQL 值。
- NULL 固定编码为未加引号的 `NULL`；`FieldKind::Number` 原样追加；`FieldKind::Bytes` 编码为小写十六进制 `x'...'`；`FieldKind::String` 加单引号并按配置选择 MySQL 反斜杠转义或 SQL 单引号加倍。
- 保留任意已有 `dst` 前缀，只做追加，不清空、不返回新缓冲区，也不执行 UTF-8 校验。该契约可由 `append_value(dst: &mut Vec<u8>, val: &[u8], ...)` 的签名和函数内全部 `push`/`extend_from_slice` 操作确认。

## 主要符号

`pub fn append_value(dst: &mut Vec<u8>, val: &[u8], is_null: bool, kind: FieldKind, escape_backslash: bool)` 是本文件唯一的模块级符号，也是公开 API：

- `dst`：可增长的输出缓冲区，函数在其尾部原地追加。
- `val`：调用者已准备好的原始字段字节；函数不拥有也不修改输入。
- `is_null`：优先级高于 `kind` 和 `val`。为 `true` 时立即追加 `NULL` 并返回，即便 `val` 非空也不会读取其内容。
- `kind`：定义在 `pkg/dumpformat/kind.rs`，只有 `Number`、`String`、`Bytes` 三个穷举分支。
- `escape_backslash`：只影响 `String`；对 NULL、数字与二进制值没有作用。

文件内常量 `HEX = b"0123456789abcdef"` 位于 `Bytes` 分支的局部作用域，用每个输入字节的高、低半字节索引字符表，保证输出为两位、小写且零填充的十六进制。

## 执行流程

1. `append_value` 先检查 `is_null`。若为真，追加 `NULL` 并提前返回。
2. `FieldKind::Number` 直接追加 `val`，不添加引号、不解析数值，也不校验字面量是否合法。
3. `FieldKind::Bytes` 先追加 `x'`，再把每个字节展开为两个小写十六进制字符，最后追加单引号；空输入因此得到 `x''`。
4. `FieldKind::String` 先追加起始单引号，然后逐字节处理：
   - `escape_backslash == true` 时，NUL、换行、回车、反斜杠、单引号、双引号和 `0x1a` 分别转成 `\\0`、`\\n`、`\\r`、`\\\\`、`\\'`、`\\"`、`\\Z`，其余字节原样追加。
   - `escape_backslash == false` 时，仅把单引号写两次，其余字节原样追加。
5. 字符串分支追加结束单引号后返回。函数没有独立返回值，结果由 `dst` 的增长体现。

在整行路径中，`writer.rs::Writer::write` 先写行级标点，再逐列调用 `append_value`；因此本函数输出中不含列间逗号或元组括号。

## 数据与状态

本文件没有静态可变状态、结构体或持久状态。唯一变化是对借用的 `Vec<u8>` 追加数据；Rust 的独占可变借用保证一次调用期间其他安全 Rust 代码不能同时修改同一缓冲区。

输出增量长度取决于分支：NULL 为 4 字节；数字为 `val.len()`；二进制为 `2 * val.len() + 3`；字符串至少为 `val.len() + 2`，每个需要转义的字节再增加 1 字节。算法对输入长度是 O(n)，额外显式工作空间为 O(1)，但 `Vec` 容量不足时可能由标准库扩容。

`FieldKind` 不是在这里推导的。Dumpling 上层通过 `dumpling/export/writer_util.rs::columnKinds` 形成列分类，`Writer` 保存该分类并把对应项传入本函数；分类错误会直接改变生成的 SQL 表示。

## 依赖与调用关系

- 直接类型依赖：`astersql_dumpformat::FieldKind`，由相邻 `pkg/dumpformat` crate 的 `kind.rs` 定义。`pkg/dumpformat/sqlfile/Cargo.toml` 只声明这一项路径依赖，没有 feature 或第三方依赖。
- crate 装配：`pkg/dumpformat/sqlfile/lib.rs` 声明 `mod sql` 并公开再导出 `append_value`。
- 直接生产调用者：`pkg/dumpformat/sqlfile/writer.rs::Writer::write`，它为每列传入原始字节、空值标记、列类型和 `Config::escape_backslash`。
- 应用主链：`dumpling/export/writer_util.rs::WriteInsertSQL` → `writeSQLFile` → `SQLWriter::write` → `append_value`。`writeSQLFile` 还负责特殊注释、指标、文件大小阈值、迭代器错误和关闭 writer，这些不属于本文件。
- 直接跨 crate 使用证据：`dumpling/export/parity_test.rs` 通过公开路径 `astersql_dumpformat_sqlfile::append_value` 验证转义契约。

RustCodeGraph 对 `sql.rs::append_value` 给出的调用者包含 `writer.rs::write` 和 `writer_test.rs::escaping_and_binary_bytes`；文件级关系还显示 `sql.rs` 被 `writer.rs`、`writer_test.rs` 使用。本函数内部只调用 `Vec`/slice 的基础追加操作，没有业务层被调用函数。

## 错误处理与边界

`append_value` 不返回 `Result`，也不会报告无效 SQL。`Number` 分支完全信任调用者提供的字节，因此空值、非数字文本或已含 SQL 标点都会被原样写入；安全扩展时不能把未经约束的外部文本误分类为 `Number`。字符串按字节而非 Unicode 标量处理，非 UTF-8 字节会被保留，这与数据库驱动返回原始字段字节的用途一致。

重要边界包括：`is_null` 覆盖其余参数；空二进制值为 `x''`；关闭反斜杠转义时只加倍单引号，NUL、换行、回车、反斜杠、双引号和 `0x1a` 均保持原字节；开启时上述七类字节均使用 MySQL 风格反斜杠序列。函数不负责 SQL mode 是否允许反斜杠转义，调用者必须让 `escape_backslash` 与目标恢复环境匹配。

内存分配失败会按 Rust/标准分配器的进程级策略处理，不是本 API 的可恢复错误。底层写入错误只会在后续 `Writer::write`/`close` 写向 `std::io::Write` 时产生和传播。

## 并发与资源生命周期

本函数同步执行，不创建线程、任务、锁、通道、事务或文件句柄。输入切片只在调用期间借用；输出缓冲区由调用者拥有并在返回后继续使用。它没有全局状态，因此不同线程可各自对不同缓冲区并行调用；同一 `Vec<u8>` 的并发修改需要由调用者在函数外同步，安全 Rust 的 `&mut` 规则会阻止无同步的共享写入。

资源生命周期由上层控制：`Writer` 复用自己的行缓冲区，Dumpling 的 `writeSQLFile` 在行循环结束后调用 `sw.close()` 写入结尾。`append_value` 本身没有 flush/close 语义，也不会保留对 `dst` 或 `val` 的引用。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/dumpformat/sqlfile/sql.go`。Rust `append_value` 对应 Go `AppendValue`，分支语义一致：NULL 优先、数字原样、字节使用小写 `x'%x'`、字符串由 `escapeBackslash` 选择两种转义模式。Go 将字符串逻辑拆为 `appendEscaped` 与 `appendEscapedBackslash`，Rust 为保持单文件直观性把两段逻辑内联到 `FieldKind::String` 的单次循环中；这是结构差异，不是行为删减。

API 形态上的差异是 Go 返回扩展后的 `[]byte`，Rust 通过 `&mut Vec<u8>` 原地修改且返回 `()`；Go 的 `default` 承担字符串分支，Rust 对三种 `FieldKind` 穷举匹配，新增枚举变体时会产生编译期未穷尽错误。Rust `Bytes` 手工查表编码，Go 使用 `fmt.Appendf`，预期字节结果相同。

对应测试为 Go 的 `pkg/dumpformat/sqlfile/writer_test.go` 与 Rust 的独立文件 `pkg/dumpformat/sqlfile/writer_test.rs`。两边都覆盖完整行框架、NULL、数字、空/非空二进制、两种字符串转义和语句切分；Rust 还直接调用 `append_value` 检查 `[0, 255] -> x'00ff'`，并额外覆盖宽度不匹配、写错误与关闭幂等性等 writer 边界。

## 扩展指南

- 新增或改变字段类别时，先更新 `pkg/dumpformat/kind.rs::FieldKind`，再在本函数添加明确编码分支，并同步检查 `dumpling/export/writer_util.rs::columnKinds`；不能用数字原样分支代替新类型的转义或格式验证。
- 改动字符串转义集合时，应保持 `sql.go::AppendValue`/`appendEscapedBackslash` 的兼容语义，分别为 `escape_backslash` 开、关补充用例，尤其覆盖 NUL、控制字符、单双引号、反斜杠、非 ASCII 和空输入。
- 改动二进制编码时，应保留每字节两位、小写、零填充及空值 `x''` 的契约，除非 Go 与恢复端协议同时变更。
- 测试应继续放在独立的 `pkg/dumpformat/sqlfile/writer_test.rs`，不要内嵌进生产源文件；跨 crate 的公开 API/Go-Rust 对齐可同步更新 `dumpling/export/parity_test.rs`。
- 性能修改要关注热路径的单次遍历、缓冲复用和扩容次数；可预留容量，但不能改变已有 `dst` 前缀或输出字节。兼容性修改还应验证生成 SQL 在目标 MySQL/TiDB SQL mode 下可恢复。

## 验证依据

- RustCodeGraph：`status` 确认索引包含本仓库；`files --filter pkg/dumpformat/sqlfile` 确认模块文件；`query append_value --kind function --json` 将目标定位为 `sql.rs::append_value`；`node --file` 阅读 `sql.rs`、`writer.rs`、`writer_test.rs`、`pkg/dumpformat/kind.rs` 与 `dumpling/export/writer_util.rs`；`explore` 确认 `append_value` 的直接调用者为 `writer.rs::write` 和 `writer_test.rs::escaping_and_binary_bytes`。
- crate 与入口：`pkg/dumpformat/sqlfile/Cargo.toml`、`pkg/dumpformat/sqlfile/lib.rs`、根 `Cargo.toml` 的 workspace member，以及 `dumpling/export/Cargo.toml` 的路径依赖。
- Go 对照：`pkg/dumpformat/sqlfile/sql.go`、`pkg/dumpformat/sqlfile/writer.go`、`pkg/dumpformat/sqlfile/writer_test.go`。
- Rust 测试与应用证据：`pkg/dumpformat/sqlfile/writer_test.rs`、`dumpling/export/parity_test.rs`、`dumpling/export/writer_util.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有十一个固定二级章节，并人工复核本文只描述上述源码与测试能够支持的现状。
