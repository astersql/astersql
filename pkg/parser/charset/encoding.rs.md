# `pkg/parser/charset/encoding.rs`

## 文件定位

本文件是 `astersql-parser-charset` crate 的通用编码抽象与分派层。`pkg/parser/charset/Cargo.toml` 将 `lib.rs` 声明为 crate 入口；`lib.rs` 通过 `pub mod encoding` 纳入本文件，并再导出 `FindEncoding`、`CountValidBytes*` 和公开 `Op*` 组合。因此它位于 SQL 解析器、表达式和 datum 逻辑与各具体字符集实现之间：上游按名称获取统一的 `EncodingRef`，下游再由 UTF-8、GBK、GB18030、Latin1、ASCII 或 binary 实现执行实际校验与转换（依据：`encoding.rs::Encoding`、`encoding.rs::FindEncoding`、`lib.rs:208-230`）。

## 核心职责

- 定义受支持的字符集名、`EncodingTp` 类型标识和 `Op` 位标志，使编码、解码、截断、替换、错误收集可组合表达（`Charset*`、`EncodingTp*`、`Op*`）。
- 以 `Encoding` trait 规范具体编码的名称、类型、字符分块、校验、转换和大小写能力，并用 `EncodingRef = &'static dyn Encoding` 传递无所有权负担的全局实现引用。
- 完成字符集名到实现的分派，并在未知名称时保留 Go 版的 binary 回退语义（`FindEncoding`）。
- 提供合法前缀统计和各编码实现共享的错误收尾、非法字符处置与 UTF-8 分块辅助（`CountValidBytes*`、`finish_transform`、`invalid_action`、`utf8_chunk`）。
- 在本文件内实现 binary 透传和 ASCII 单字节语义；其他字符集通过文件顶部引入的静态实现接入。

## 主要符号

- `CharsetUTF8MB4`、`CharsetUTF8`、`CharsetGBK`、`CharsetLatin1`、`CharsetBin`、`CharsetASCII`、`CharsetGB18030`：查找所使用的精确小写名称；当前查找不做大小写归一化。
- `EncodingTp`：`#[repr(i8)]` 枚举，顺序对齐 Go `EncodingTp` 的 `None` 到 `Gb18030`；同时提供 Go 命名风格的 `EncodingTp*` 别名常量。
- `Op = i16` 及 `OP_*`：内部原子位。公开组合 `OpReplace(NoErr)`、`OpEncode(NoErr/Replace)`、`OpDecode(NoErr/Replace)` 决定方向、遇非法片段是停止还是追加 `?`，以及是否返回错误。
- `EncodingError`：保存静态编码名、首个非法片段和已产出输出；`output()` 允许调用者在错误情况下取回部分/替换结果，`Display` 生成 `invalid <encoding> character string: ...` 文案。
- `Encoding`：核心 trait，为 `Sync`；`Peek` 和 `Foreach` 保留字节切片视图，`Transform` 返回自有 `Vec<u8>` 或 `EncodingError`，`ToUpper`/`ToLower` 定义编码语义下的大小写。
- `IsSupportedEncoding`：仅判定七个精确常量名是否受支持；它与 `FindEncoding` 的“未知名回退 binary”是两种不同的 API 契约。
- `FindEncodingTakeUTF8AsNoop`：先调用 `FindEncoding`，若结果类型为 `Utf8` 则返回 binary，用于可安全跳过 UTF-8 校验的路径。
- `FindEncoding`：`utf8mb4`/`utf8` 共享 `ENCODING_UTF8_IMPL`，GBK、Latin1、ASCII、GB18030 各自分派，`binary`、空串和所有未知名都落到 `ENCODING_BIN_IMPL`。
- `CountValidBytes`、`CountValidBytesDecode`、`count_valid`：分别用 `OP_FROM_UTF8` 和 `OP_TO_UTF8` 调用 `Foreach`，只累加首个非法分块之前的源字节数。
- `EncodingBin`/`ENCODING_BIN_IMPL`：私有零状态静态实现，任意字节合法，逐字节遍历，转换复制原输入。
- `EncodingAscii`/`ENCODING_ASCII_IMPL`：私有零状态静态实现，只接受 `0x00..=0x7f`；高位字节借用 `encoding_utf8::peek_utf8` 按 Go UTF-8 Peek 宽度聚合为一个非法片段。

## 执行流程

1. 上游传入字符集名调用 `FindEncoding`。函数以精确 `match` 选择静态实现；不识别的名称安全地落入 binary 透传。对已知可不校验 UTF-8 的上游，`FindEncodingTakeUTF8AsNoop` 再将 `EncodingTp::Utf8` 替换为 binary。
2. 调用者通过 `IsValid`、`Peek`、`Foreach` 或 `Transform` 使用 trait。`Foreach` 的回调同时接收源片段、目标片段和合法性；返回 `false` 便中断遍历。
3. `CountValidBytes*` 是 `Foreach` 的薄封装：每遇合法块累加 `from.len()`，首个 `ok == false` 使回调返回 `false`，因而得到连续合法前缀而非全文合法字节总数。
4. binary 路径始终合法：`Peek` 取一字节，`Foreach` 按 `chunks(1)` 回调，`Transform` 直接返回 `src.to_vec()`。
5. ASCII `Transform` 先走 `IsValid` 快路，全 ASCII 时直接复制并不改动 `dest`。若存在高位字节，`Foreach` 按 UTF-8 Peek 宽度分块，合法 ASCII 原样追加；首个非法块被记录，`invalid_action` 根据 TRIM/REPLACE 停止或追加 `?` 继续。
6. ASCII 慢路最后调用 `finish_transform`：清空并用最终输出覆盖 `dest`；如果存在非法块且未设 `OP_SKIP_ERROR`，返回包含同一输出的 `EncodingError`，否则返回 `Ok(output)`。

## 数据与状态

本文件没有可变全局状态。具体实现以零大小静态单例存在，`EncodingRef` 是全局生命期只读 trait object。转换的可变数据都限于一次调用：`output: Vec<u8>`、首个非法片段 `first_invalid`、调用者提供的 `dest`和合法前缀计数器。`EncodingError` 拥有非法片段和输出缓冲，不借用调用者输入。

需特别注意 `dest` 契约：binary 转换和 ASCII 全合法快路不触碰 `dest`，只返回新 `Vec`；ASCII 非法慢路则由 `finish_transform` 覆盖 `dest`。这一差异被 `encoding_test.rs::test_noop_transform_preserves_destination_buffer` 显式锁定。

## 依赖与调用关系

- crate 边界：`pkg/parser/charset/Cargo.toml` 的 crate 名为 `astersql-parser-charset`，入口是 `lib.rs`；具体字符集实现使用 `encoding = 0.2.33` 和 `encoding_rs = 0.8.35` 等依赖，本文件自身直接依赖同 crate 的 `encoding_gb18030`、`encoding_gbk`、`encoding_latin1`、`encoding_utf8`。
- 模块接线：`lib.rs:208-230` 声明本模块并再导出主要查找/统计 API；ASCII 慢路还会调用 `encoding_utf8::peek_utf8`。
- parser 上游：`pkg/parser/lexer.rs` 用 `FindEncoding` 初始化 client/connection 编码，`pkg/parser/yy_parser.rs` 在规则应用时更换它们，`pkg/parser/ast/base.rs` 持有编码并用于节点文本转换。
- parser 之外：`pkg/expression/builtin_convert_charset.rs` 用 `FindEncoding(...).Transform(...)` 做字符集转换；`pkg/types/datum.rs` 使用 `FindEncodingTakeUTF8AsNoop`、`FindEncoding` 处理 datum 编码；`pkg/expression/collation.rs` 也使用 UTF-8-as-noop 路径。
- RustCodeGraph `explore` 显示 `FindEncoding` 的使用面包含 `yy_parser.rs::ApplyOn`、`builtin_convert_charset.rs::{decode_lossy, encode_to_binary}`、`types/datum.rs::findEncoding` 和多个 parser/charset 测试。精确 `callers`/`callees` 查询在当前索引中返回空边，因而本文档不将这些缺失的静态边当作“无调用者”，而是以上述源文件直接用法补证。

## 错误处理与边界

- 空字符集名、`binary` 和未知名均回退 binary，不返回查找错误；需区分“是否公式支持”时应先调 `IsSupportedEncoding`。
- `utf8` 和 `utf8mb4` 的 `FindEncoding` 结果都是通用 UTF-8 实现；严格 utf8mb3 是单独实现入口，测试会对 `CharsetUTF8` 显式选用 `EncodingUTF8MB3StrictImpl()`，不应误以为 `FindEncoding("utf8")` 自动强制三字节上限。
- `utf8_chunk` 在空输入时返回空切片且标记成功；对无效引导字节、长度不足或切片不是合法 UTF-8 时，仅消费首字节并标记失败，保证调用者可前进。
- `invalid_action` 中 TRIM 优先于 REPLACE；如果两个位都没设，它不修改输出但允许继续。公开预设都提供明确策略，自行组合原始位时必须考虑这个边界。
- 设置 REPLACE 不等于成功：未设 `OP_SKIP_ERROR` 时，可同时得到带 `?` 的输出和 `EncodingError`；`OpReplaceNoErr`、`OpEncodeNoErr`、`OpDecodeNoErr` 才抑制返错。
- ASCII 高位字节的错误粒度不是固定一字节；它按 Go UTF-8 Peek 宽度聚合，即使聚合出的序列本身不是合法 UTF-8。`test_ascii_foreach_uses_utf8_peek_width` 覆盖了该兼容性约束。

## 并发与资源生命周期

`Encoding: Sync` 是共享静态实现的并发契约；`EncodingRef` 可在多个线程间持有只读引用。本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源，也没有惰性初始化。输入切片仅在同步调用期间借用；返回值和错误内容均为自有内存，回调也不被保存。主要资源风险是转换时按输入规模分配 `Vec`；ASCII 慢路以 `src.len()` 预分配，但返错时 `dest` 与 `EncodingError.output` 同时保留输出内容，可产生额外复制。

## 与 Go 版本的对应关系

`pkg/parser/charset/encoding.go` 是主要对照。Rust 保留了七种字符集的查找集合、未知名回退 binary、UTF-8-as-noop 优化、`Encoding` 方法集、`EncodingTp` 顺序、`Op` 比特布局和通过 `Foreach` 计算合法前缀的流程。Rust 以 `match` 取代 Go `encodingMap`，但 `IsSupportedEncoding` 与 `FindEncoding` 的外部结果保持一致。

语言层差异包括：Go `Transform` 接收 `*bytes.Buffer` 并允许返回源切片别名，Rust 接收 `&mut Vec<u8>` 且总是返回自有 `Vec<u8>`；Go 的普通 `error` 在 Rust 中变为可查询部分输出的 `EncodingError`；Go `MbLen(string)` 在 Rust trait 中接收 `&[u8]`。此外，Rust 本文件内直接放置 binary/ASCII 实现，Go 将它们分在 `encoding_bin.go` 和 `encoding_ascii.go`。

`pkg/parser/charset/encoding_test.rs` 对齐 Go `encoding_test.go` 的 GBK/GB18030 往返、非法输入替换和多字符集校验，并额外锁定 Rust 的 `dest` 快路行为和 ASCII 错误分块语义。`encoding_gb18030_2_aster_unit_test.rs::test_encoding_lookup_and_fallback` 覆盖空串/未知名回退和 UTF-8-as-noop。

## 扩展指南

- 新增字符集时，同步增加 `Charset*`、`EncodingTp` 变体/别名、实现 `Encoding` 的独立源文件与静态实例，并更新 `IsSupportedEncoding` 和 `FindEncoding`。还需同步 `lib.rs` 的模块/再导出和 `Cargo.toml` 的必要依赖，以及 Go 对照的名称集合与类型顺序。
- 修改 `Op` 时必须保持现有比特数值，因为各具体实现使用按位判断。新策略应检查 `finish_transform`/`invalid_action` 与每个 `Foreach`/`Transform` 实现，并在独立测试文件中覆盖“输出内容 + 是否返错 + 合法前缀长度”三者。
- 修改 ASCII 时，必须保留 Go 的 Peek 错误分组，并更新 `pkg/parser/charset/encoding_test.rs`；修改具体编码则优先更新同目录的 `encoding_<name>_test.rs`。Rust 源与测试不要放在同一文件。
- 更改查找回退或 `utf8`/`utf8mb4` 映射前，检查 `pkg/parser/lexer.rs`、`pkg/parser/yy_parser.rs`、`pkg/parser/ast/base.rs`、`pkg/expression/builtin_convert_charset.rs`、`pkg/types/datum.rs` 的上游假设；这类改动可直接改变 SQL 文本解析、节点文本和显式字符集转换结果。
- 性能风险主要在额外分配/复制、字符分块颗粒和 UTF-8 校验。只有上游已保证 UTF-8 合法时才应使用 `FindEncodingTakeUTF8AsNoop`；否则会将非法输入当作 binary 透传。

## 验证依据

- RustCodeGraph 索引状态：`rustcodegraph status` 报告 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/parser/charset` 确认目标、Go 对照和独立测试均在索引中。
- 符号与源码查询：`query FindEncoding --kind function`、`query Encoding --kind trait`、`query CountValidBytes --kind function`、`query finish_transform`、`query invalid_action`、`query utf8_chunk`；`node --file pkg/parser/charset/encoding.rs` 覆盖全文 1-385 行。
- 调用关系查询：`explore "pkg/parser/charset/encoding.rs symbols callers callees encoding"` 返回 `FindEncoding` 的 parser/expression/types 使用面；对 `FindEncoding`、`CountValidBytes`、`finish_transform`、`invalid_action`、`utf8_chunk` 的精确 `callers`/`callees --json` 在当前索引中返回空数组，已用 `rg` 的直接调用点补齐且未把空边解读为无使用。
- 已读文件：`pkg/parser/charset/encoding.rs`、`pkg/parser/charset/Cargo.toml`、`pkg/parser/charset/lib.rs`、`pkg/parser/charset/encoding.go`、`pkg/parser/charset/encoding_test.rs`、`pkg/parser/charset/encoding_test.go`；还以直接搜索核对了 `pkg/parser/lexer.rs`、`pkg/parser/yy_parser.rs`、`pkg/parser/ast/base.rs`、`pkg/expression/builtin_convert_charset.rs`、`pkg/expression/collation.rs`、`pkg/types/datum.rs` 的调用点。
- 测试证据：`encoding_test.rs::{test_encoding,test_encoding_validate,test_ascii_foreach_uses_utf8_peek_width,test_noop_transform_preserves_destination_buffer,test_encoding_gb18030}` 与 Go `encoding_test.go::{TestEncoding,TestEncodingValidate,TestEncodingGB18030}` 对齐主要语义；查找回退还由 `encoding_gb18030_2_aster_unit_test.rs::test_encoding_lookup_and_fallback` 覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的 `test -f` 与固定十一章节 `rg -c` 命令验证文档结构。
