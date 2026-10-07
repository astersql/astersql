# `pkg/parser/charset/encoding_ascii.rs`

## 文件定位

本文件是 `astersql-parser-charset` crate 中一套公开的 ASCII 编码实现。crate 入口 `pkg/parser/charset/lib.rs` 通过 `include!("encoding_ascii.rs")` 将其放入 `encoding_ascii` 模块，并以 `pub use encoding_ascii::*` 再导出。`pkg/parser/charset/Cargo.toml` 指定该 crate 的入口为 `lib.rs`，Go 包对应关系为 `pkg/parser/charset`。

当前 crate 内还存在一套位于 `pkg/parser/charset/encoding.rs` 的私有 `EncodingAscii`。通用 `FindEncoding(CharsetASCII)` 返回的是那套私有静态实例，而不是本文件的 `ENCODING_ASCII_IMPL`。本文件的真实接线是：独立测试直接使用其公开 API，`lib.rs` 为它实现 `EncodingView`，`EncodingBase` 再可通过 `EncodingRef::Ascii` 回调本实例。因此阅读或扩展时不能把通用 `FindEncoding` 的调用者直接算作本文件调用者。

## 核心职责

- 用 `EncodingAscii` 表达 Go `encodingASCII`：ASCII 只接受 `0x00..=0x7f`，空输入和 ASCII 输入保持原字节语义。
- 用 `ENCODING_ASCII_IMPL` 与 `init_encoding_ascii` 建立进程内共享实例，并设置 `EncodingBase` 所需的自引用句柄。
- 提供名称、类型、多字节长度、大小写、首字符探测、整段合法性校验、转换和逐字符遍历 API。
- 对合法输入提供借用原切片的零复制快路径；对非法输入复用 `EncodingBase::transform` 的替换、截断、收集和错误策略。
- 按 Go 行为将高位字节用 UTF-8 首字节宽度聚成一个非法片段，避免把同一多字节字符的续字节逐个报告。

本文件不解析 SQL、不访问数据库或网络，也不维护会话状态；它只处理传入的字节切片与字符串。

## 主要符号

- `ENCODING_ASCII_IMPL: OnceLock<EncodingAscii>`：全局单例容器。初始化成功后只读；重复设置的错误被有意忽略，因此 `init_encoding_ascii` 可重复调用而不替换首个实例。
- `init_encoding_ascii()`：以 `Encoding::Nop` 构造 `EncodingBase`，写入 `EncodingRef::Ascii` 自引用，再尝试发布到 `OnceLock`。这是使用 `encoding_by_ref(EncodingRef::Ascii)` 前的初始化前提。
- `EncodingAscii { encoding_base: EncodingBase }`：ASCII 实现及其公共基座。字段公开，当前正常构造路径是 `init_encoding_ascii`。
- `name() -> &'static str`：返回 crate 根常量 `CHARSET_ASCII`，即 `"ascii"`。
- `tp() -> EncodingTp`：返回 crate 根轻量枚举的 `EncodingTp::Ascii`。
- `mb_len(&str) -> usize`：委托给基座，当前恒为 `0`，表示 ASCII 没有多字节字符长度。
- `to_upper(&str)` / `to_lower(&str)`：委托基座的 Rust Unicode 大小写转换；参数是合法 UTF-8 字符串，而非任意字节。
- `peek(&[u8]) -> &[u8]`：空输入原样返回，非空输入返回首字节切片，不校验该字节是否为 ASCII。
- `is_valid(&[u8]) -> bool`：线性扫描；发现首个大于 `0x7f` 的字节立即返回 `false`。
- `transform(dest, src, op)`：合法时返回 `TransformResult::Borrowed(src)` 且不触碰 `dest`；非法时委托 `EncodingBase::transform`。
- `foreach(src, op, callback)`：按 ASCII/UTF-8 探测宽度切片，向回调传递相同的 `from`、`to` 和合法标志；回调返回 `false` 时立即停止。当前实现不读取 `_op`。

## 执行流程

初始化流程如下：调用 `init_encoding_ascii`；创建持有 `Encoding::Nop` 的 `EncodingBase`；将其 `self_encoding` 设为 `Some(EncodingRef::Ascii)`；最后写入 `ENCODING_ASCII_IMPL`。基座需要该回指来通过 `encoding_by_ref` 派发回本文件的 `EncodingView::foreach` 和 `name`。

校验流程是 `is_valid` 从头检查字节。空切片自然合法；任一高位字节使其立即失败。`transform` 先调用该校验：全合法时借用并返回原输入，不分配输出，也不清空调用方缓冲；存在高位字节时进入 `EncodingBase::transform`。

非法转换中，基座先选择或创建目标缓冲并清空它，再通过本实例的 `foreach` 分块。`foreach` 对 ASCII 字节产生宽度为 1、`ok=true` 的片段；对高位字节调用 `encoding_utf8_impl().peek`，仅根据首字节选择最多 2、3 或 4 字节，产生 `ok=false` 的整体片段。基座回调依据 `Op` 决定记录首个错误、停止、写入 `?`，或收集 `from`/`to` 字节。若没有未抑制的错误则返回自有缓冲，否则返回带字符集名与非法片段的错误。

遍历本身支持提前终止：一旦回调返回 `false`，后续字节不再检查。`encoding_ascii_test.rs` 用 `"a中b"` 证明回调只收到合法的 `a` 和整体非法的三字节 `中` 后便可停止。

## 数据与状态

持久状态只有 `OnceLock` 中的一个 `EncodingAscii`。实例内的 `EncodingBase` 保存两项配置：底层恒等编码器 `Encoding::Nop`，以及 `Some(EncodingRef::Ascii)` 回指。初始化后这些配置在正常路径中不再改变。

输入字节由调用方拥有。`peek`、`foreach` 的分片和合法 `transform` 的 `Borrowed` 结果都借用输入，不复制也不延长其生命周期。只有非法转换通过基座生成 `Owned(Vec<u8>)`。`dest: Option<&mut ByteBuffer>` 仅在非法慢路径被使用；合法快路径必须保留其原内容。

字符边界不是完整 UTF-8 校验结果。`Utf8Encoding::peek` 只根据首字节阈值选择宽度，并以剩余长度截短；因此形如 `e2 28 a1` 的错误序列仍会被归为一个三字节非法片段。这是与 Go 对齐的错误分组不变量。

## 依赖与调用关系

直接上游包括 `pkg/parser/charset/encoding_ascii_test.rs` 的 `ascii()` 和 `pkg/parser/charset/charset_1_aster_unit_test.rs`，它们调用 `init_encoding_ascii` 后读取单例；`pkg/parser/charset/lib.rs` 的 `EncodingView for EncodingAscii` 将动态派发的 `name`、`peek`、`foreach` 转到本文件固有方法。RustCodeGraph 也确认 `init_encoding_ascii` 的生产定义实例化 `EncodingAscii` 并引用 `ENCODING_ASCII_IMPL`。

直接下游包括 crate 根的 `CHARSET_ASCII`、`EncodingTp`、`Encoding`、`EncodingRef`、`TransformResult`、`ByteBuffer`、`Op` 与 `encoding_utf8_impl`，以及 `pkg/parser/charset/encoding_base.rs` 的 `EncodingBase`。非法 `transform` 会进一步使用基座的 `encoding_by_ref`、`EncodingView::foreach`、错误构造及操作位处理。

Cargo 依赖中，与这条链直接相关的是外部 `encoding = "0.2.33"` 及内部 `astersql-errors`、`astersql-parser-mysql`、`astersql-parser-terror`；它们主要由 crate 根和基座封装。本文件没有直接导入网络、异步运行时或存储依赖。

需注意并行接线：`pkg/parser/charset/encoding.rs::FindEncoding` 使用该文件自己的私有 ASCII 静态对象。它与本文件保持相似行为并由 `encoding_test.rs` 覆盖，但不是本文件的直接调用边。若未来统一两套实现，必须同时审查 trait 类型、错误类型、返回所有权和测试入口，不能只替换同名符号。

## 错误处理与边界

- 空输入：`peek` 返回空切片，`is_valid` 返回 `true`，`transform` 借用原空切片，`foreach` 不调用回调。
- 边界字节：`0x00` 与 `0x7f` 合法；`0x80` 起非法。独立测试显式覆盖这些值。
- 非法片段：高位首字节按 UTF-8 探测宽度聚组，但不验证续字节；切片宽度被剩余输入长度限制，因而不会越过末尾。
- 回调终止：`foreach` 立即返回，不将其视为错误，也不访问剩余输入。
- 合法转换：不修改可选目标缓冲，返回借用结果。这是可观察的 API 契约和性能特征。
- 非法转换：错误、替换或截断由 `Op` 控制。基座仅保留首个未被 `OP_SKIP_ERROR` 抑制的错误；错误字节按大写十六进制写入 parser 错误参数。
- 初始化错误：`encoding_by_ref(EncodingRef::Ascii)` 在单例未初始化时会 `expect("ASCII initialized")` 触发 panic。直接构造一个没有设置 `self_encoding` 的 `EncodingAscii` 并走非法慢路径，也会在基座派发时 panic。
- `to_upper`/`to_lower` 接受 `&str`，沿用 Unicode 大小写规则；它们不是任意 ASCII 字节清洗接口。

## 并发与资源生命周期

`OnceLock` 提供线程安全的一次发布与共享读取；多个线程并发调用 `init_encoding_ascii` 时只有一个实例会写入，其他 `set` 失败结果被丢弃。发布后的实例只通过共享引用读取，方法内部没有可变全局状态、锁持有、后台任务或通道。

每次非法转换可能创建一个局部 `ByteBuffer`，或暂借调用方的 `dest`；基座会在使用它前清空。转换器是单次调用内创建并销毁的 `Box<dyn Transformer>`。所有借用切片仅在输入有效期间存活，回调不能从签名中安全地取得超出调用期的可变所有权。

时间复杂度通常为 `O(n)`。合法 `transform` 虽需一次完整校验，但之后零复制；非法路径会再次遍历并分配输出，因此仍为线性时间但可能扫描两遍。额外空间在合法路径为 `O(1)`，非法路径为 `O(n)`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/charset/encoding_ascii.go`。`EncodingASCIIImpl`/`init`、`encodingASCII`、`Name`、`Tp`、`Peek`、`IsValid`、`Transform` 和 `Foreach` 均在本文件有对应物。两侧都以 Nop 编码器构造基座、补上 self 回指、只接受最大 ASCII 字节、在合法转换时返回原输入，并用 UTF-8 Peek 宽度聚合非法片段。

Rust 的表现形式差异包括：Go 全局指针与包初始化函数被 `OnceLock` 和显式 `init_encoding_ascii` 取代；Go 的嵌入方法由显式委托 `mb_len`、`to_upper`、`to_lower` 取代；Go `[]byte`/`*bytes.Buffer` 的返回关系由 `TransformResult::Borrowed/Owned` 和 `Option<&mut Vec<u8>>` 明确表达；回调使用泛型闭包而非函数值。

`pkg/parser/charset/encoding_test.go::TestEncodingValidate` 给出 ASCII 空串、普通文本、`Ê`、中文和混合输入的期望。Rust 的 `encoding_test.rs::test_encoding_validate` 保留了相同表格意图，不过它通过通用 `FindEncoding` 覆盖并行实现；本文件自身由 `encoding_ascii_test.rs` 专门验证。两套测试都应继续保持一致。

## 扩展指南

若修改 ASCII 合法范围或错误分组，应首先修改 `is_valid` 与 `foreach`，确保两者对同一字节序列达成一致；同步更新 `pkg/parser/charset/encoding_ascii_test.rs`，并评估 `encoding_test.rs` 与 Go `encoding_test.go` 的共同用例。不要把 Rust 单元测试写回生产源文件。

若修改转换策略，应优先确认行为属于 ASCII 特例还是 `EncodingBase::transform` 的通用规则。合法快路径必须继续保持借用原输入且不清空 `dest`；非法路径的操作位、首错语义和替换输出应在基座的独立测试或现有 ASCII 测试中覆盖。

若增加新的全局使用入口，调用前必须保证 `init_encoding_ascii` 已执行，或者把初始化协议收敛为安全的惰性访问器。不要暴露可构造但缺少 `self_encoding` 的半初始化实例。

若计划消除 `encoding_ascii.rs` 与 `encoding.rs` 的重复实现，需要单独设计统一方案：核对两个 `EncodingTp`、两个编码 trait/引用类型、两个错误返回模型和 `FindEncoding` 的所有调用者。最小验证至少应同时覆盖 `encoding_ascii_test.rs`、`encoding_test.rs` 和 Go 对照用例，避免只让一条入口语义正确。

性能相关修改应保留线性扫描、切片借用和按需分配特征；对 `foreach` 的宽度计算必须以剩余输入为界，避免越界，并保留回调提前停止能力。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点与 1,848,419 条边，可覆盖本任务的 Rust/Go 源和测试。
- RustCodeGraph `node --file pkg/parser/charset/encoding_ascii.rs`：核对了全部 121 行、单例、类型及 9 个公开方法，并确认索引报告的直接使用测试文件。
- RustCodeGraph `node init_encoding_ascii` 与 `explore`：确认初始化函数实例化 `EncodingAscii`、引用单例，并由两个 Rust 测试入口调用；确认 `peek`、`is_valid`、`transform`、`foreach` 的内部调用关系。
- RustCodeGraph 读取 `pkg/parser/charset/lib.rs`：核对模块 include/再导出、`EncodingView` 适配、`EncodingRef::Ascii` 派发、操作位、缓冲及 UTF-8 Peek 规则。
- RustCodeGraph 读取 `pkg/parser/charset/encoding_base.rs`：核对非法慢路径的缓冲清理、字符遍历、首错、替换/截断、回指初始化前提和错误格式。
- RustCodeGraph 读取 `pkg/parser/charset/encoding.rs`：核对通用 `FindEncoding` 使用并行私有实现，而非本文件单例；同时核对通用测试链的行为边界。
- `pkg/parser/charset/Cargo.toml`：核对 crate 名称、`lib.rs` 入口、内部 parser 错误依赖、`encoding` 依赖及 Go 包元数据。
- Go 对照 `pkg/parser/charset/encoding_ascii.go` 与 `encoding_test.go`：核对全局实例、self 回指、ASCII 上界、合法快路径、UTF-8 宽度分组和表格化边界结果。
- Rust 测试 `pkg/parser/charset/encoding_ascii_test.rs`、`encoding_test.rs` 与 `charset_1_aster_unit_test.rs`：核对公开方法、零复制、目标缓冲保持、替换结果、非法分组和回调提前停止。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的命令验证文档恰有 11 个固定二级标题。
