# `pkg/parser/charset/encoding_base.rs`

## 文件定位

本文件是 `astersql-parser-charset` crate 中 ASCII/Binary 编码实现共用的基础层，由 `pkg/parser/charset/lib.rs` 的 `encoding_base` 模块通过 `include!("encoding_base.rs")` 纳入并整体再导出。crate 边界和依赖声明位于 `pkg/parser/charset/Cargo.toml`；其中 `astersql-errors`、`astersql-parser-mysql`、`astersql-parser-terror` 提供类型化 parser 错误，`encoding`/`encoding_rs` 是字符转换相关依赖。

当前接线范围必须与完整 Go 包区分：crate 根的 `Encoding` 只有 `Nop`，`EncodingRef` 只有 `Ascii` 和 `Bin`，所以本文件的 `EncodingBase` 目前实际由 `encoding_ascii.rs` 和 `encoding_bin.rs` 持有。GBK、GB18030、Latin1、UTF-8 的 Rust 文件使用另一组实现和 `encoding.rs` 中的公共 trait，并未通过本基座接线。

## 核心职责

- `EncodingBase` 保存底层转换器工厂 `enc` 和指回具体编码对象的 `self_encoding`，模拟 Go `encodingBase` 的匿名嵌入与 `self` 回指。
- `is_valid`、`transform` 和 `foreach` 组成通用处理链：具体编码决定字符切片边界，基座负责选择编码/解码方向、逐片转换、识别错误，以及按 `Op` 位标志截断、替换或收集字节。
- `to_upper`、`to_lower` 和默认 `mb_len` 提供可复用的默认行为。
- `ERR_INVALID_CHARACTER_STRING`、`generate_encoding_err` 生成与 parser/MySQL 错误码相连的错误；`REPLACEMENT_BYTES` 和 `begin_with_replacement_char` 辅助区分合法输入的 U+FFFD 与转换器产生的替换字符。
- `hack_slice`/`hack_string` 提供字符串和字节切片之间的无复制视图，但 Rust API 将可变性和 UTF-8 前置条件显式收紧。

## 主要符号

- `ERR_INVALID_CHARACTER_STRING: LazyLock<Box<terror::Error>>`：延迟构造 `terror::ClassParser.NewStd(mysql::ErrInvalidCharacterString)`，供错误实例化使用。
- `EncodingBase { enc, self_encoding }`：`enc: Encoding` 创建 encoder/decoder；`self_encoding: Option<EncodingRef>` 负责动态派发 `name`、`peek`、`foreach`。
- `EncodingBase::new` 只设置底层编码器并把回指留为 `None`；`set_self` 必须在使用依赖回指的方法前完成初始化。ASCII/Binary 分别在 `init_encoding_ascii`、`init_encoding_bin` 中完成这一接线。
- `mb_len` 固定返回 `0`；`to_upper`/`to_lower` 使用 Rust Unicode 大小写映射。
- `is_valid` 以 `OP_FROM_UTF8` 调用具体编码的 `foreach`，遇到第一个 `ok == false` 时回调返回 `false`，从而短路并返回不合法。
- `transform` 清空调用方缓冲或按输入长度创建局部缓冲，记录首个错误，并解释 `OP_SKIP_ERROR`、`OP_TRUNCATE_TRIM`、`OP_TRUNCATE_REPLACE`、`OP_COLLECT_FROM`、`OP_COLLECT_TO`。成功时返回 `TransformResult::Owned`。
- `foreach` 按 `OP_FROM_UTF8` 选择 encoder 与 UTF-8 `peek`，否则选择 decoder 与具体编码 `peek`；每个片段写入固定四字节栈缓冲并调用回调。
- `REPLACEMENT_BYTES`/`begin_with_replacement_char`：检查输出是否以 U+FFFD 的 UTF-8 字节 `EF BF BD` 开头。
- `generate_encoding_err`：把非法字节逐个格式化为两位大写十六进制，再传给 parser 标准错误。
- `hack_slice`：安全、只读的 `str::as_bytes` 借用；`hack_string`：不校验 UTF-8 的 `unsafe from_utf8_unchecked` 借用。

## 执行流程

1. `init_encoding_ascii` 或 `init_encoding_bin` 构造具体编码对象，以 `EncodingBase::new(Encoding::Nop)` 建立基座，再通过 `set_self` 写入对应的 `EncodingRef`，最后放入 `OnceLock`。
2. 校验路径调用 `EncodingBase::is_valid`。它派发到具体实现的 `foreach`；ASCII 按字节检查并把一个非 ASCII UTF-8 序列作为整体非法片段，首个非法片段即停止。Binary 当前覆盖自己的恒真 `is_valid`，不走基座。
3. ASCII `transform` 对合法输入直接返回借用切片；非法输入进入 `EncodingBase::transform`。该函数先清空目标缓冲，再令具体编码的 `foreach` 逐片调用内部 `collect` 闭包。
4. `collect` 在非法片段上最多生成一次错误。Trim 位使遍历立即结束；Replace 位写入 `?` 后继续；否则再按 CollectFrom/CollectTo 的优先级追加源片段或转换结果。
5. 基座自己的 `foreach` 用于需要真正 encoder/decoder 驱动的通用路径：方向位决定 `peek` 与转换器，每轮仅处理一个字符片段。转换器报错，或解码输出 U+FFFD 且该替换字符并非原输入时，回调收到 `ok = false`。
6. 遍历结束后，`transform` 在记录过且未跳过错误时返回 `Err`；否则复制缓冲内容形成 `TransformResult::Owned`。

## 数据与状态

`EncodingBase` 的长期状态只有复制型 `enc` 和可选静态句柄 `self_encoding`。初始化完成后，ASCII/Binary 实例存放于各自的全局 `OnceLock`，通过 `encoding_by_ref` 取得共享只读 `EncodingView`。`self_encoding == None` 不是可运行状态：`is_valid`、`transform`、`foreach`、私有 `name` 都会以 `expect("encodingBase.self must be initialized")` panic。

每次 `transform` 调用拥有独立的局部缓冲和 `first_error`；传入的 `ByteBuffer`（即 `Vec<u8>`）会先被清空。每次 `foreach` 创建独立的 boxed transformer、索引和 `[u8; 4]` 栈缓冲，不保存跨调用的流状态。回调看到的 `from` 和 `to` 都只在该次调用期间有效，不能越过借用生命周期保存。

`Op` 是位集合而非互斥枚举。当前收集逻辑在两个收集位同时存在时优先 `OP_COLLECT_FROM`；替换分支写入 `?` 后立即返回，因此该非法片段不会再执行 CollectFrom/CollectTo。

## 依赖与调用关系

直接上游是 `encoding_ascii.rs::EncodingAscii` 与 `encoding_bin.rs::EncodingBin`：两者持有 `EncodingBase` 并在初始化时设置回指；ASCII 还把默认 `mb_len`、大小写和非法输入的 `transform` 委托给基座。`lib.rs` 为两种具体类型实现 `EncodingView`，使 `encoding_by_ref` 能回调其 `name`、`peek`、`foreach`。

直接下游包括 crate 根的 `Encoding::{new_encoder,new_decoder}`、`Transformer::transform`、`Transformer::rune_error_is_last_input`、`encoding_utf8_impl().peek`、`encoding_by_ref`、`ByteBuffer`、`TransformResult` 和全部 `OP_*` 标志；错误路径依赖 `terror`、`mysql` 和 `errors::ErrorArg`。

RustCodeGraph 将本文件识别为 15 个符号，并报告它被包括 `encoding_ascii.rs`、`encoding_bin.rs`、`lib.rs` 及 charset 测试在内的 19 个文件使用；但对本文件精确执行 `callers/callees` 未返回可用边，所以上述调用关系又由 `rg` 的局部引用和相邻源码核实。仓库当前没有 `hack_slice`、`hack_string` 的 Rust 调用者，二者只是 crate 再导出的公共兼容 API。

## 错误处理与边界

- 未调用 `set_self` 就使用依赖具体编码的操作会 panic；这是初始化不变量，而不是可恢复输入错误。
- `transform` 只保留首个非法片段的错误。`OP_SKIP_ERROR` 抑制错误对象，但不会改变 trim/replace/collect 行为；`OP_TRUNCATE_TRIM` 在首个非法片段停止；`OP_TRUNCATE_REPLACE` 用单字节 `?` 替换并继续。
- `foreach` 的宽度来自 `peek`，然后直接切片 `src[index..index + width]`。现有 UTF-8 helper 会把宽度限制到剩余长度；新增具体编码必须保证非空输入时返回 `1..=remaining_len`，否则会停滞或越界。
- 固定输出缓冲只有四字节，因此新增 transformer 必须保证单个输入字符最多写四字节，或先重构缓冲策略。
- 解码时单凭 U+FFFD 输出不足以判错；`rune_error_is_last_input` 用来保留原输入本来就是替换字符的合法场景。crate 根的默认 transformer 返回 `false`，目前完整的 GB18030 状态检查存在于另一套实现，未接入本基座。
- `hack_string` 对非 UTF-8 字节调用会违反 `str` 不变量，调用者必须在调用前证明 UTF-8 合法；空输入直接返回静态空字符串。`hack_slice` 返回不可变借用，不能复现 Go `[]byte` 的可写别名能力。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道、事务或 I/O。全局错误使用 `LazyLock`，具体编码使用相邻模块的 `OnceLock`，均由标准库保证线程安全的一次初始化。初始化后，基座经静态共享引用读取；`EncodingView` 和 transformer 的实际并发能力仍取决于 crate 根接口及具体实现。

转换器和临时输出均限定在单次调用内。传入目标缓冲由调用方所有，基座只在调用期间独占借用并先清空；`TransformResult::Owned` 会复制最终切片，所以不会借用传入缓冲。`hack_slice`/`hack_string` 不分配，返回值生命周期绑定输入，调用者不得让底层存储先行失效或（通过其他不安全别名）在共享 `str` 存活期间修改字节。

## 与 Go 版本的对应关系

Rust 的 `EncodingBase`、`is_valid`、`transform`、`foreach`、替换字符检测、错误格式化和两个 hack 函数逐项对应 `encoding_base.go` 的 `encodingBase`、`IsValid`、`Transform`、`Foreach`、`beginWithReplacementChar`、`generateEncodingErr`、`HackSlice`、`HackString`。核心 Op 分支次序、只记录首错、每字符四字节缓冲及 decoder 对合法 U+FFFD 的特殊判断均保留。

存在几项可见差异。Go 的 `encodingBase` 可承载 `golang.org/x/text/encoding.Encoding` 和包内任意 `Encoding`，当前 Rust crate 根基座只有 `Nop`、ASCII/Binary 两种回指，完整编码尚未统一接线。Go `Transform` 即使有错误也同时返回已生成字节；当前 Rust `Result` 的 `Err` 不携带该输出，调用方只能得到错误。Go 传入非空 `bytes.Buffer` 后可直接借用其 `Bytes()`；Rust 成功路径总是 `to_vec()` 成为 Owned。Go 的 `HackSlice` 暴露可写无复制切片，而 Rust 安全版本只暴露 `&[u8]`；Rust 的 `hack_string` 通过 `unsafe` 把 UTF-8 责任明确交给调用者。

Go `encoding_test.go` 覆盖 GBK/GB18030 的编解码、替换、首错和原始 U+FFFD 边界；这些是语义参照，但当前 Rust 本基座的直接测试只覆盖 ASCII/Binary 的已接线范围，不能据此声称完整多编码路径已由该基座实现。

## 扩展指南

若新增只需复用当前基座的具体编码，应先扩展 `lib.rs::EncodingRef` 与 `encoding_by_ref`，提供满足 `EncodingView` 的静态实例，并保证初始化时调用 `set_self`。若编码不是恒等转换，还必须扩展 `Encoding`/`Transformer` 工厂，验证 encoder/decoder 的方向、最大单字符输出、非法输入报告和合法 U+FFFD 判定；不能仅添加名称分支。

修改 Op 行为时，应保持 `transform` 中“首错记录 → trim/replace → collect”的 Go 顺序，并为 SkipError、Trim、Replace、CollectFrom、CollectTo 及组合冲突增加独立测试。错误格式或类别变化要同步检查 `generate_encoding_err`、MySQL 错误码和 Go `generateEncodingErr`。

测试应继续放在独立文件，不嵌入生产源。基座/ASCII 直接回归宜扩展 `pkg/parser/charset/encoding_ascii_test.rs`；跨编码行为可扩展 `encoding_test.rs` 或对应编码的 `*_test.rs`；对照语义参考 `encoding_test.go`。新增 `hack_string` 测试必须只传有效 UTF-8，并单独验证空输入和生命周期/零拷贝预期，不能用未定义行为作为负例。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/parser/charset` 找到目标及 34 个相邻文件；`node --file pkg/parser/charset/encoding_base.rs --offset 1 --limit 260` 返回完整 193 行和 15 个符号；`query` 定位 `EncodingBase`、`begin_with_replacement_char`、`generate_encoding_err`、`hack_string`。精确 callers/callees 无结果，未把缺失图边当作不存在调用。
- Rust 源与装配：`pkg/parser/charset/encoding_base.rs`、`pkg/parser/charset/lib.rs`、`pkg/parser/charset/encoding.rs`、`pkg/parser/charset/encoding_ascii.rs`、`pkg/parser/charset/encoding_bin.rs`。
- crate 配置：`pkg/parser/charset/Cargo.toml`，确认 crate 名、`lib.rs` 入口、字符转换和错误依赖以及 Go 包映射元数据。
- Go 对照：`pkg/parser/charset/encoding_base.go`、`pkg/parser/charset/encoding.go`、`pkg/parser/charset/encoding_ascii.go`、`pkg/parser/charset/encoding_bin.go`。
- 测试证据：`pkg/parser/charset/encoding_ascii_test.rs` 验证默认方法、合法输入零拷贝、非法 ASCII 替换和回调短路；`pkg/parser/charset/charset_1_aster_unit_test.rs` 验证 ASCII 替换与 Binary 零拷贝；`pkg/parser/charset/encoding_test.go` 提供多编码错误、替换及合法 U+FFFD 的 Go 语义参照。同目录未发现 `encoding_base_test.rs`，辅助函数和非 SkipError 分支缺少本文件专属 Rust 测试。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核只新增本说明、没有修改 Rust/Go/Cargo/只读总计划。
