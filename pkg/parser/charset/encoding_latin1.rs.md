# `pkg/parser/charset/encoding_latin1.rs`

## 文件定位

该文件实现 `astersql-parser-charset` crate 中的 Latin1 编码策略，源码入口是 [`encoding_latin1.rs`](encoding_latin1.rs)。`Cargo.toml` 将本目录定义为 `astersql-parser-charset` crate，`lib.rs` 通过 `pub mod encoding_latin1` 声明模块并用 `pub use encoding_latin1::*` 重导出其公开符号。通用编码注册表位于 `encoding.rs::FindEncoding`：当字符集名为 `CharsetLatin1` 时，它返回本文件的 `ENCODING_LATIN1_IMPL`。

它处于 SQL 文本进入解析器前后的字节编码层，而不是完整的 ISO-8859-1/Windows-1252 转码器。`encoding_latin1.go` 明确这是 TiDB 向后兼容行为：Latin1 使用 UTF-8 方法，但对原始字节的查看、合法性与变换采用单字节透传语义。

## 核心职责

- 用 `EncodingLatin1` 实现 `encoding.rs::Encoding` trait 的全部契约，让 Latin1 能被通用编码 API 选中。
- 通过 `Name` 和 `Tp` 报告稳定的身份 `latin1` / `EncodingTp::Latin1`。
- 保留旧 TiDB 兼容性：`Peek` 每次只取一字节，`IsValid` 接受任意字节，`Transform` 不编解码也不使用操作位。
- 保留 Go 中通过嵌入 `encodingUTF8` 获得的方法语义：`MbLen` 和 `Foreach` 按 UTF-8 码点分块，`ToUpper` / `ToLower` 做 Unicode 大小写映射。

因此，“整段输入是合法 Latin1”与“遍历时当前分块是合法 UTF-8”是两个不同的判定维度：`IsValid` 恒为 `true`，但 `Foreach` 仍可向回调传入 `ok = false`。

## 主要符号

- `pub struct EncodingLatin1`：无字段的零大小标记类型，实现 `Encoding: Sync`。
- `pub static ENCODING_LATIN1_IMPL: EncodingLatin1`：全局共享实例；`encoding.rs::FindEncoding` 将 `CharsetLatin1` 映射到该静态值。
- `Name(&self) -> &'static str`：返回 `encoding.rs::CharsetLatin1`，值为 `"latin1"`。
- `Tp(&self) -> EncodingTp`：返回 `EncodingTpLatin1`，即 `EncodingTp::Latin1`。
- `Peek<'a>(&self, src: &'a [u8]) -> &'a [u8]`：空输入返回原空切片，非空输入返回 `&src[..1]`；返回值借用原输入。
- `MbLen(&self, src: &[u8]) -> usize`：调用 `encoding.rs::utf8_chunk`；首个分块是合法且宽度大于 1 时返回 2–4，否则返回 0。
- `IsValid(&self, _: &[u8]) -> bool`：恒返回 `true`，包括空切片和任意非 UTF-8 字节。
- `Foreach(&self, src, op, callback)`：忽略 `op`，重复用 `utf8_chunk` 取下一分块，将同一分块同时作为 `from` 和 `to` 传给回调，并转交 UTF-8 合法性标志；回调返回 `false` 即提前结束。
- `Transform(&self, dest, src, op) -> Result<Vec<u8>, EncodingError>`：忽略 `dest` 和 `op`，总是 `Ok(src.to_vec())`。
- `ToUpper(&self, src: &str) -> String` / `ToLower`：分别调用 Rust `str::to_uppercase` / `str::to_lowercase`，输入在类型层面已保证是 UTF-8。

本文件没有条件编译项、自定义错误类型或内部辅助函数。

## 执行流程

1. 上游把字符集名传给 `encoding.rs::FindEncoding`。
2. 精确命中 `CharsetLatin1` 时，查找函数返回 `&ENCODING_LATIN1_IMPL` 作为 `EncodingRef = &'static dyn Encoding`；未知名称不会进入本实现，而会回退到 binary。
3. 调用者按用途使用 trait 方法：
   - 要求字符边界时，`Peek` 强制取一字节。
   - 要求整段合法性时，`IsValid` 无条件通过。
   - `CountValidBytes` 等通用路径会间接调用 `Foreach`；后者从偏移 0 开始，通过 `utf8_chunk` 以 1–4 字节步进，直到输入耗尽或回调要求停止。非法/不完整 UTF-8 总是以单字节步进且 `ok = false`，所以不会卡住。
   - 编码、解码或替换路径调用 `Transform` 时，直接复制并返回输入，既不检查 `op` 也不改写调用者的 `dest`。

RustCodeGraph 显示 `FindEncoding` 的上游包括 `pkg/parser/yy_parser.rs::ApplyOn`、`pkg/parser/lexer.rs::{empty, reset}`、`pkg/expression/builtin_convert_charset.rs`中的转换路径以及 `pkg/types/datum.rs::findEncoding`。这些是通过通用查找间接到达 Latin1 实现，不是对 `EncodingLatin1` 类型的直接依赖。

## 数据与状态

`EncodingLatin1` 没有字段，`ENCODING_LATIN1_IMPL` 不持有码表、缓存、配置或可变状态。方法的数据流只由借用切片和局部值组成：

- `Peek` 返回指向原输入的切片，不分配。
- `MbLen` 只观察首个 UTF-8 分块。
- `Foreach` 仅维护局部 `offset`，分块同时作为源侧与目标侧视图，没有中间缓冲。
- `Transform` 为返回值分配一个与 `src` 等长的自有 `Vec<u8>`，而传入的 `dest` 保持原样。
- 大小写方法依据 Unicode 映射生成新 `String`，结果长度不保证与输入相等。

## 依赖与调用关系

直接依赖由 `use crate::encoding::*` 引入：`Encoding`、`EncodingTp`、`EncodingError`、`Op`、`CharsetLatin1`、`EncodingTpLatin1` 和 `utf8_chunk`。RustCodeGraph 的 `callees Foreach` 明确给出 `encoding_latin1.rs::Foreach -> encoding.rs::utf8_chunk`；`MbLen` 也在源码中调用同一辅助函数。

注册边由 `encoding.rs` 建立：它导入 `ENCODING_LATIN1_IMPL`，并在 `FindEncoding` 的 `CharsetLatin1` 分支返回该实例。`lib.rs` 再将模块与公开符号暴露给 crate 用户。直接测试调用者是 `encoding_latin1_test.rs`；集成式字符集查找证据还来自 `encoding_gb18030_2_aster_unit_test.rs::test_latin1_preserves_arbitrary_bytes`。

`Cargo.toml` 没有为本文件定义 feature 开关。本实现本身只使用 crate 内部抽象和 Rust 标准库；crate 中的 `encoding` / `encoding_rs` 等依赖由其他编码实现使用，不是该文件的直接下游。

## 错误处理与边界

- 空输入：`Peek` 返回空切片，`Foreach` 不调用回调，`MbLen` 返回 0，`IsValid` 返回 `true`，`Transform` 返回空 `Vec`。
- 任意字节：`IsValid` 全部接受；`Peek` 不解释高位字节；`Transform` 不产生 `EncodingError`。
- 非法 UTF-8：`Foreach` 会把当前非法字节单独交给回调并标记 `false`；`MbLen` 对其返回 0。这不改变 `IsValid` 的 Latin1 整段接受契约。
- `op` 无效：无论是编码、解码、截断或替换位，`Foreach` 和 `Transform` 都不分支处理；这是兼容行为，不应在未更新 Go 对照与回归测试时“修正”。
- `dest` 无效：Rust 实现不清空也不追加它；`encoding_latin1_test.rs::latin1_transform_is_noop_and_preserves_destination` 固定了这一边界。
- `Foreach` 的回调由调用者提供；返回 `false` 是正常的提前停止信号，不是错误。
- `ToUpper` / `ToLower` 只接受 `&str`，因而不能直接处理 `IsValid` 所允许的任意非 UTF-8 字节切片。

## 并发与资源生命周期

`Encoding` trait 要求 `Sync`，而 `EncodingLatin1` 无状态，因此 `ENCODING_LATIN1_IMPL` 可以被多线程以 `&'static dyn Encoding` 共享。本文件不使用锁、原子、线程、异步任务、通道、事务或 I/O 资源，也没有显式初始化/销毁阶段。

生命周期上，编码实例存活于整个进程；`Peek` 和 `Foreach` 产生的切片只在输入借用期内有效；`Transform` 和大小写方法返回独立所有权结果。`Foreach` 是同步回调，回调不能将借用分块保留到其生命周期之外。

## 与 Go 版本的对应关系

Rust `EncodingLatin1` 对应 `encoding_latin1.go::encodingLatin1`，Rust `ENCODING_LATIN1_IMPL` 对应 Go `EncodingLatin1Impl`。对齐点如下：

- `Name`、`Tp`、`Peek`、`IsValid` 的分支与返回值直接对应。
- Go 类型嵌入 `encodingUTF8`，因而继承 `MbLen`、`Foreach`、`ToUpper` 和 `ToLower`；Rust 不使用类型嵌入，而是在 trait impl 内显式实现这些方法。`MbLen` / `Foreach` 共用 `utf8_chunk` 来复现所需的 UTF-8 分块行为。
- Go 实例在 `init` 中设置嵌入基类的 `self` 指针；Rust 实现无基类自引用，不需要运行时初始化。
- Go `Transform` 直接返回 `src`，接口注释允许返回切片与输入别名；Rust 为满足当前 trait 的 `Result<Vec<u8>, EncodingError>` 返回类型而执行 `src.to_vec()`，所以字节值等价，但会分配且不与输入别名。两者都忽略 `dest` 和 `op`，且不报错。
- Go 依靠 `encoding.Nop` 嵌入对象支撑通用基类；Rust 此文件没有使用 crate 的外部 `encoding` 依赖。

`encoding_latin1_test.rs` 直接固定了从 Go 嵌入 UTF-8 方法提升而来的多字节分块、非法字节标记和 `dest` 不变行为；`encoding_gb18030_2_aster_unit_test.rs` 另外验证了通过 `FindEncoding` 查找后，Latin1 对 `[0xff, 0x80, b'a']` 的接受、单字节 `Peek` 与原样变换。Go 仓库中没有独立 `encoding_latin1_test.go`；通用 `encoding_test.go` 主要覆盖其他编码，Latin1 特有契约由实现本身和 Rust 独立回归测试补足。

## 扩展指南

- 调整 Latin1 的公开行为时，首先修改 `impl Encoding for EncodingLatin1`；如果改变名称、类型或注册规则，同步检查 `encoding.rs::{CharsetLatin1, EncodingTpLatin1, FindEncoding, IsSupportedEncoding}`。
- 若改变 UTF-8 分块规则，需先判断影响应仅限于 Latin1，还是要修改共享的 `encoding.rs::utf8_chunk`；后者会同时影响其他调用者，应做影响分析。
- 不要把 `IsValid == true` 简化成 `Foreach` 每块 `ok == true`；这会破坏从 Go `encodingUTF8.Foreach` 继承的兼容契约。
- 若希望 `Transform` 复用 `dest`、返回借用数据或解释 `op`，这不只是局部优化：它会改变当前 trait 所有权、Go 对照和已有 `dest` 不变测试，需同步设计并评估分配性能。
- 回归测试应继续放在独立的 `encoding_latin1_test.rs`，不嵌入生产源文件；经 `FindEncoding(CharsetLatin1)` 的注册集成行为可扩展到 `encoding_gb18030_2_aster_unit_test.rs` 或通用 `encoding_test.rs`。
- 至少保留空输入、ASCII、2/3/4 字节 UTF-8、非法引导字节、截断 UTF-8、回调提前停止、任意 Latin1 字节透传、`dest` 保持不变和 Unicode 大小写的测试面。
- 兼容性风险主要在 TiDB 历史 Latin1 语义而非编码标准本身；性能风险主要在 `Transform` 的等长分配和 Unicode 大小写扩张。

## 验证依据

- RustCodeGraph `status`：项目索引包含 7,032 个 Rust 文件和 4,415 个 Go 文件；本件分析时索引可用。
- RustCodeGraph `node --file pkg/parser/charset/encoding_latin1.rs`：核对了 `EncodingLatin1`、全局实例与九个 trait 方法的完整实现；`node EncodingLatin1` 再次确认类型与 impl 位置。
- RustCodeGraph `node --file pkg/parser/charset/encoding.rs`：核对 `Encoding` trait、`EncodingRef`、字符集/类型常量、`FindEncoding` 注册分支、`CountValidBytes` 间接路径和 `utf8_chunk` 边界。
- RustCodeGraph `callees Foreach`：返回 `encoding_latin1.rs::Foreach -> encoding.rs::utf8_chunk`；`explore "pkg/parser/charset/encoding_latin1.rs symbols callers callees Latin1 encoding"` 列出 `FindEncoding` 的解析器、表达式和 datum 上游以及 Latin1 相关测试。
- RustCodeGraph `node --file pkg/parser/charset/lib.rs`：核对模块声明、重导出与独立 `encoding_latin1_test.rs` 的 `#[cfg(test)]` 接线。
- `pkg/parser/charset/Cargo.toml`：核对 crate 名、`lib.rs` 入口、依赖与无 feature 开关。
- RustCodeGraph `node --file pkg/parser/charset/encoding_latin1.go`：核对历史兼容目的、UTF-8 嵌入、自引用初始化以及透传 `Transform`。`encoding.go` 用于核对 Go 接口、注册表、操作位与返回别名契约。
- RustCodeGraph `node --file pkg/parser/charset/encoding_latin1_test.rs` 与 `encoding_gb18030_2_aster_unit_test.rs`：核对多字节/非法 UTF-8 分块、透传任意字节、单字节 `Peek` 以及不改写 `dest` 的回归契约。`encoding_test.go` 用于确认 Go 通用编码测试的范围。
- 交付前按任务文件执行结构检查，确认文档存在且恰有 11 个规定的二级标题。本任务为纯文档分析，按计划不运行 Cargo。
