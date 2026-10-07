# `pkg/parser/charset/encoding_utf8.rs`

## 文件定位

本文件属于 `astersql-parser-charset` crate；crate 根由 `pkg/parser/charset/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/parser/charset/lib.rs` 通过 `pub mod encoding_utf8` 装入并通过 `pub use encoding_utf8::*` 再导出本模块 API。它是字符集抽象 `pkg/parser/charset/encoding.rs::Encoding` 的 UTF-8 实现层，负责完整 UTF-8（MySQL `utf8mb4`）和限制为至多三字节的历史 `utf8`/utf8mb3 语义。

该文件不是 SQL 解析入口，也不做字符集名称查找。上游先经 `encoding.rs::FindEncoding` 获得 `&'static dyn Encoding`：`CharsetUTF8MB4` 与 `CharsetUTF8` 默认都指向本文件的 `ENCODING_UTF8_IMPL`；需要拒绝四字节字符的路径必须显式调用 `EncodingUTF8MB3StrictImpl()`。例如 `pkg/types/datum.rs` 在 utf8mb3 场景使用严格实例，`pkg/session/runtime/dispatch.rs` 则通过 `FindEncoding(CharsetUTF8)` 使用普通实例。

## 核心职责

- 以同一类型 `EncodingUtf8` 和字段 `strict_mb3` 表达两种策略：普通实例接受合法的一至四字节 UTF-8，严格实例还拒绝宽度为四字节的码点。
- 实现 `Encoding` 所要求的名称、类型、字符窥视、多字节长度、整段校验、逐字符遍历、转换和 Unicode 大小写转换。
- 为 ASCII 编码的非法高位字节分组提供 `peek_utf8`。该函数只根据首字节估算宽度，不验证续字节；`pkg/parser/charset/encoding.rs` 的 ASCII `Foreach` 直接调用它。
- 在转换中保留合法输入的快速路径；遇到非法片段时复用 `encoding.rs::invalid_action` 与 `finish_transform`，从而遵循公共 `Op` 位标志定义的截断、替换、错误收集和跳过错误策略。

## 主要符号

- `pub struct EncodingUtf8 { strict_mb3: bool }`：无可变运行时状态的策略对象。字段私有，外部只能使用模块提供的静态实例。
- `pub static ENCODING_UTF8_IMPL`：`strict_mb3 = false` 的公开静态实例，也是 `FindEncoding(CharsetUTF8MB4 | CharsetUTF8)` 的目标。
- `static ENCODING_UTF8_MB3_STRICT_IMPL`：`strict_mb3 = true` 的模块私有静态实例。
- `pub fn EncodingUTF8MB3StrictImpl() -> EncodingRef`：返回严格实例的公开访问器；`EncodingRef` 是 `&'static dyn Encoding`。
- `pub(crate) fn peek_utf8(src: &[u8]) -> &[u8]`：按首字节区间选择 1、2、3 或 4 字节，并以剩余长度截断。空输入原样返回。`0x80..=0xdf` 会被分为两字节，`0xf0..=0xff` 会被分为四字节，因此它是分组函数而非合法性判定函数。
- `EncodingUtf8::valid_chunk`：调用 `encoding.rs::utf8_chunk` 取得一个已验证的 UTF-8 分块；严格模式再增加 `chunk.len() <= 3` 约束。
- `impl Encoding for EncodingUtf8`：公开行为面。`Name` 固定返回 `CharsetUTF8MB4`；`Tp` 区分 `EncodingTpUTF8` 和 `EncodingTpUTF8MB3Strict`；其余方法见后续流程。

## 执行流程

1. 查找阶段：`encoding.rs::FindEncoding` 对 `utf8mb4`/`utf8` 返回 `ENCODING_UTF8_IMPL`；utf8mb3 语义的调用方显式取得 `EncodingUTF8MB3StrictImpl()`。
2. 快速校验：`Transform` 首先调用 `IsValid`。普通模式使用 `std::str::from_utf8` 验证整段输入；严格模式在此基础上遍历 `chars()`，要求每个字符的 `len_utf8()` 不超过 3。
3. 合法快路径：若整段有效，`Transform` 直接返回 `src.to_vec()`，不清空、不写入传入的 `dest`。`encoding_utf8_test.rs::utf8_valid_transform_preserves_destination_buffer` 明确锁定了这一契约。
4. 非法慢路径：`Foreach` 从偏移 0 开始调用 `valid_chunk`。底层 `utf8_chunk` 对合法字符返回完整码点切片；非法或不完整序列只返回首字节并标记失败。严格模式把合法的四字节块改标为失败，但仍以完整四字节块交给回调。
5. 回调处理：有效块追加到局部 `output`；第一个无效块被记录为 `(Name(), bytes)`；`invalid_action` 根据 `Op` 决定停止、追加 `?` 或继续扫描。
6. 收尾：`finish_transform` 用局部输出重建 `dest`。若记录过非法块且未设置 `OP_SKIP_ERROR`，返回携带首个非法片段和已生成输出的 `EncodingError`；否则返回输出。
7. 辅助行为：`Peek` 委托给 `peek_utf8`；`MbLen` 仅对合法且宽度大于 1 的首字符返回宽度，ASCII、非法和不完整序列返回 0；`ToUpper`/`ToLower` 使用 Rust 标准库的 Unicode 大小写映射。

## 数据与状态

模块的持久数据只有两个只读静态 `EncodingUtf8` 值，差异完全由布尔字段 `strict_mb3` 决定。方法不缓存扫描位置或转换结果；`Foreach` 的 `offset`、`Transform` 的 `output` 与 `first` 都是单次调用的栈上/局部所有权状态。

输入和分块以借用切片传递，`Peek`、`utf8_chunk` 和 `valid_chunk` 不复制字节。`Transform` 的返回类型是拥有所有权的 `Vec<u8>`：合法快路径复制一次输入；慢路径预分配 `src.len()` 容量并逐块写入。四字节码点在严格模式中作为一个非法单元处理，而非法引导字节或不完整序列由 `utf8_chunk` 逐字节处理。这一分组差异会影响替换字符数量和错误中记录的字节片段。

`Name` 对普通和严格实例都返回 `utf8mb4`，因此错误文案中的编码名不能用来区分两种策略；应使用 `Tp()` 判断实际类型。

## 依赖与调用关系

本文件以 `use crate::encoding::*` 使用同 crate 的 `Encoding`、`EncodingRef`、`EncodingTp`、字符集/类型常量、`Op`、`EncodingError`、`utf8_chunk`、`invalid_action` 和 `finish_transform`，没有直接使用 `Cargo.toml` 中的第三方编码库。实际 UTF-8 校验和大小写转换依赖 Rust 标准库。

RustCodeGraph 的精确边显示：`valid_chunk -> utf8_chunk`，本文件 `Foreach -> valid_chunk`，本文件 `Transform -> IsValid/Foreach/invalid_action/finish_transform/Name`；`EncodingUTF8MB3StrictImpl -> ENCODING_UTF8_MB3_STRICT_IMPL`。图对 trait 对象的动态调用方不能全部解析，因此还以源码引用核验：`encoding.rs::FindEncoding -> ENCODING_UTF8_IMPL`，ASCII `Foreach -> encoding_utf8::peek_utf8`，`pkg/types/datum.rs -> EncodingUTF8MB3StrictImpl()`。

crate 入口 `lib.rs` 另有 `encoding_utf8_impl() -> Utf8Encoding`，它只是轻量 `peek` 门面，并不是本文件的 `EncodingUtf8` 静态实例。`encoding_test.rs::test_ascii_foreach_uses_utf8_peek_width` 同时测试该轻量门面的分组规则和 ASCII 路径；不能据此推断轻量类型实现了本文件的校验或转换逻辑。

## 错误处理与边界

- 空输入：`peek_utf8` 返回空切片；`IsValid` 为真；`Foreach` 不回调；`Transform` 走合法快路径并返回空向量。
- 不完整或非法 UTF-8：`IsValid` 返回假；慢路径通过 `utf8_chunk` 每次至少消耗一个字节，因此不会停滞或越界。
- 合法的 Unicode 替换字符 `U+FFFD` 是三字节有效 UTF-8，不应与解码非法字节产生的错误标记混淆；Go/Rust 验证用例均覆盖这一点。
- utf8mb3：合法四字节码点（例如 emoji）在 `IsValid` 中失败，并在 `Foreach` 中作为一个四字节非法块交给回调。普通 utf8mb4 实例接受它。
- `Peek` 有意不验证首字节和续字节。例如孤立续字节可与下一字节组成一个两字节“观察块”；真正合法性由 `IsValid`/`utf8_chunk` 判定。
- 非法转换是否返回错误由 `Op` 控制。`OP_TRUNCATE_TRIM` 令遍历停止，`OP_TRUNCATE_REPLACE` 追加 `?`，`OP_SKIP_ERROR` 仅抑制最终错误而不改变非法判定。错误只保存第一个非法块，但 `output` 可包含此前及后续处理结果。
- `MbLen` 使用普通 `utf8_chunk`，没有应用 `strict_mb3`，所以严格实例对合法四字节字符仍会报告长度 4；严格拒绝行为由 `IsValid`、`Foreach` 和 `Transform` 承担。扩展者不应把 `MbLen > 0` 当作严格模式下的合法性结论。

## 并发与资源生命周期

`Encoding` trait 要求 `Sync`，两个实例是不可变的进程期静态值，因此可安全地被多个线程共享，无锁、无原子变量、无通道、无后台任务，也没有外部句柄或显式清理阶段。

每次 `Transform` 都创建独立局部缓冲，只有在慢路径收尾时才修改调用方提供的 `dest`；合法快路径保持 `dest` 原状。回调由 `Foreach` 同步调用，其借用分块只在调用期间有效；回调返回 `false` 会立即结束扫描。调用方若需跨回调保存内容，必须复制字节，不能保存临时借用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/charset/encoding_utf8.go`。Rust 的 `EncodingUtf8 { strict_mb3 }` 合并了 Go 的 `encodingUTF8` 与嵌入它的 `encodingUTF8MB3Strict` 两个类型；两个 Rust 静态实例对应 Go 的 `EncodingUTF8Impl` 和 `EncodingUTF8MB3StrictImpl`。Rust 不需要 Go `init()` 中设置 `encodingBase.self` 的自引用接线。

行为对应如下：两侧 `Name` 都报告 `CharsetUTF8MB4`；普通模式接受完整 UTF-8，严格模式拒绝宽度大于 3 的字符；`Peek` 都只按首字节阈值分组并在输入不足时返回剩余部分；`Foreach` 都同步逐字符回调并允许提前停止；`Transform` 都对合法输入直接返回而不改写目标缓冲，对非法输入再进入公共转换策略。

实现机制有所不同：Go 使用 `unicode/utf8.DecodeRune` 和 `encodingBase`/`encoding.Nop`，Rust 使用 `std::str::from_utf8`、`chars().len_utf8()` 及 `encoding.rs::utf8_chunk`。Go 普通 `Transform` 可返回原 `src` 切片；Rust 的返回类型要求拥有所有权，因此合法快路径返回内容相同的 `Vec<u8>`。Rust `ToUpper`/`ToLower` 是 trait 所需的本地实现，Go UTF-8 文件没有对应方法。上述差异不应被误写为零拷贝等价。

Go 的 `encoding_test.go::TestEncodingValidate` 与 Rust 的 `encoding_test.rs` 对齐验证 ASCII、中文、emoji、非法 `0xff/0xfe/0xfd` 和 `U+FFFD`；Rust 独立测试 `encoding_utf8_test.rs` 额外固定了合法快路径保持 `dest` 不变的所有权适配语义。

## 扩展指南

- 修改 UTF-8 分块或非法序列策略时，先判断应改 `peek_utf8`（仅观察宽度）还是 `encoding.rs::utf8_chunk`（真实校验）。二者用途不同，合并会改变 ASCII 非法片段聚合或 UTF-8 替换数量。
- 新增模式相关规则应集中在 `EncodingUtf8::valid_chunk` 与 `IsValid`，并同步检查 `Tp`、静态实例和公开访问器；必须保证 `Foreach` 每轮消费非空块。
- 修改转换语义时复用公共 `invalid_action`/`finish_transform`，保持 `OpEncode`、`OpReplaceNoErr` 等标志在所有编码间的一致含义。特别保留“合法输入不改写 `dest`”契约，除非同时审查所有调用方。
- 扩展查找行为需修改 `encoding.rs::FindEncoding`，而不是只添加本文件静态值。若希望 `CharsetUTF8` 默认采用严格实例，需要审计当前依赖非严格映射的解析器、session 和类型转换调用方，这是兼容性变更。
- 测试必须放在独立文件。直接行为优先补充 `pkg/parser/charset/encoding_utf8_test.rs`；跨编码/查找行为补充 `encoding_test.rs`；同步核对 Go 的 `encoding_utf8.go` 与 `encoding_test.go`，覆盖空输入、截断序列、孤立续字节、过长/非法引导字节、U+FFFD、三/四字节边界、回调提前停止及各 `Op` 分支。
- 性能敏感点是 `IsValid` 的整段扫描、严格模式的第二次字符遍历以及非法慢路径的分配。优化时需用测试证明错误分块、首次错误内容和 `dest` 生命周期没有变化。

## 验证依据

- 目标源码：`pkg/parser/charset/encoding_utf8.rs`，核对了 `EncodingUtf8`、两个静态实例、`EncodingUTF8MB3StrictImpl`、`peek_utf8`、`valid_chunk` 及完整 `Encoding` 实现。
- crate 与模块：`pkg/parser/charset/Cargo.toml`、`pkg/parser/charset/lib.rs`；确认 crate 名、入口文件、模块公开方式，以及轻量 `Utf8Encoding` 与完整实现的区别。
- 公共依赖：`pkg/parser/charset/encoding.rs`；确认 `Encoding` trait、`EncodingRef`、`EncodingTp`、`Op`、`FindEncoding`、`utf8_chunk`、`invalid_action`、`finish_transform` 及 ASCII 对 `peek_utf8` 的调用。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；查询了 `EncodingUtf8`、`EncodingUTF8MB3StrictImpl`、`peek_utf8`、`valid_chunk`，并核对 callers/callees。已确认的关键边为 `Foreach -> valid_chunk -> utf8_chunk`、`Transform -> IsValid/Foreach/invalid_action/finish_transform`；trait 动态调用方以源码引用补证。
- Rust 测试：`pkg/parser/charset/encoding_utf8_test.rs`、`pkg/parser/charset/encoding_test.rs`；确认合法快路径的 `dest` 契约、utf8mb4/utf8mb3 边界、非法替换和 ASCII Peek 分组。
- Go 对照：`pkg/parser/charset/encoding_utf8.go`、`pkg/parser/charset/encoding_test.go`；确认类型映射、首字节分组、RuneError 判定、严格三字节限制和验证用例意图。
- 上游引用：`pkg/types/datum.rs`、`pkg/session/runtime/dispatch.rs`，以及 `rg` 得到的直接符号引用；用于确认严格实例与普通查找路径在应用中的真实接线。
- 本任务是只读分析加文档，不运行 Cargo，也未据此声称运行时测试通过。交付结构以任务指定命令验证，内容另经人工复核，确保能回答文件为何存在、如何运行以及如何安全扩展。
