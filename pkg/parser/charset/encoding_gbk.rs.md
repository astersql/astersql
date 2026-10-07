# `pkg/parser/charset/encoding_gbk.rs`

## 文件定位

本文件是 `astersql-parser-charset` crate 的 GBK 编码实现。crate 边界由 `pkg/parser/charset/Cargo.toml` 定义，入口 `pkg/parser/charset/lib.rs` 声明 `pub mod encoding_gbk` 并通过 `pub use encoding_gbk::*` 导出本文件的公开项。通用查找入口 `pkg/parser/charset/encoding.rs::FindEncoding` 在字符集名为 `CharsetGBK` 时返回静态实例 `ENCODING_GBK_IMPL`。

它位于 SQL 文本和字符数据处理的底层字符集层，不负责词法或语法分析本身。常规消费者经 `FindEncoding("gbk")` 获得 `&'static dyn Encoding`；另一路公开入口 `NewCustomGBKEncoder` 被 `pkg/util/collate/gbk_bin.rs::encode_char` 用来生成 GBK binary 排序键。

## 核心职责

- 用 `EncodingGbk` 实现通用 `Encoding` trait，包括名称、类型、字符边界探测、合法性检查、遍历、编解码和大小写转换。
- 在 `encoding_rs::GBK` 之上补齐 Go/TiDB 兼容规则：拒绝字节 `0x80`、拒绝 CP936 用户自定义区字节对，并在编码方向拒绝欧元符 `U+20AC`。
- 通过 `foreach_gbk` 和 `transform_gbk` 解释通用 `Op` 位标志，使严格截断、问号替换、是否返回错误、收集源侧或目标侧数据等行为与同 crate 的编码框架一致。
- 提供无状态的 `CustomGbkEncoder`，供排序规则等只需要 UTF-8 到 GBK 编码的调用方直接使用。
- 实现 MySQL GBK 特殊大小写表，并模拟 Go 的单码点大小写回退，避免 Rust 完整 Unicode 映射把一个字符展开成多个码点。

## 主要符号

- `pub struct EncodingGbk`：零大小类型，承载 `Encoding` trait 实现；没有实例字段。
- `pub static ENCODING_GBK_IMPL: EncodingGbk`：全局共享的 GBK 实例，由 `FindEncoding` 引用。
- `pub(crate) fn peek_gbk(src: &[u8]) -> &[u8]`：空输入返回空片；ASCII 首字节取 1 字节，其他首字节最多取 2 字节。它只划分候选字符，不证明候选合法。
- `decode_gbk(chunk) -> Option<Vec<u8>>`：拒绝 `0x80` 和私用区字节对，再调用 `encoding_rs::GBK.decode_without_bom_handling_and_without_replacement`；成功时返回 UTF-8 字节。
- `is_gbk_private_use_pair(chunk) -> bool`：识别 Go `simplifiedchinese.GBK` 未映射、但 WHATWG/CP936 表可能映射到 Unicode 私用区的两字节范围。
- `encode_gbk(chunk) -> Option<Vec<u8>>`：先要求输入块是合法 UTF-8，再显式拒绝 `€`，最后用 `encoding_rs::GBK.encode`，仅在 `had_errors == false` 时成功。
- `foreach_gbk(src, op, callback)`：统一逐字符驱动器；回调参数依次是源块、转换后块和合法标志，回调返回 `false` 可提前终止。
- `transform_gbk(dest, src, op) -> Result<Vec<u8>, EncodingError>`：统一转换入口，记录首个非法块，按 `Op` 决定复制哪一侧和遇错后的动作，并用 `finish_transform` 收尾。
- `GBK_UNCHANGED_CASE_RANGES` 与 `gbk_case`：实现 MySQL GBK 大小写转换中的“不变化”区间。
- `go_simple_upper` / `go_simple_lower`：将 Rust 大小写迭代器约束为 Go `unicode.ToUpper` / `ToLower` 的单码点语义，并处理会展开的特殊码点。
- `pub struct CustomGbkEncoder`、`new_custom_gbk_encoder`、`NewCustomGBKEncoder`：无状态编码器及 Rust/Go 风格构造入口；`transform` 固定使用 `OpEncode`，`reset` 是空操作。

## 执行流程

1. 调用方通常用 `FindEncoding(CharsetGBK)` 取得 `ENCODING_GBK_IMPL`，或直接构造 `CustomGbkEncoder`。
2. `IsValid` 以 `OP_FROM_UTF8` 调用 `foreach_gbk`。编码方向先由 `encoding.rs::utf8_chunk` 切出一个 UTF-8 标量；非法 UTF-8 块、GBK 不可表示字符和欧元符都会令 `ok` 为 `false`。回调在首个错误处停止，因此 `IsValid` 只回答整段是否可编码。
3. `Transform` 委托 `transform_gbk`。若 `op` 含 `OP_TO_UTF8`，`peek_gbk` 按 1/2 字节切块后调用 `decode_gbk`；否则按 UTF-8 标量切块后调用 `encode_gbk`。
4. 合法块根据 `OP_COLLECT_FROM` 选择保留原块，否则追加转换块。非法块首次出现时保存 `(CharsetGBK, from)`，然后 `encoding.rs::invalid_action` 根据操作位停止、追加 `?` 或继续。
5. `encoding.rs::finish_transform` 清空并重写调用方的 `dest`；若记录过非法块且未设置 `OP_SKIP_ERROR`，返回携带首个非法块和已生成输出的 `EncodingError`，否则返回输出。
6. `ToUpper` / `ToLower` 逐 Unicode 标量调用 `gbk_case`：特殊不变区间原样输出，其余使用 Go 风格简单映射。
7. `pkg/util/collate/gbk_bin.rs::encode_char` 为每个字符新建 `CustomGbkEncoder`，调用其 `transform`；不可编码时由该上游转成单字节 `?`，再用于比较或构造排序键。

一个容易忽略的边界是 `peek_gbk`：任何非 ASCII 首字节都会与下一字节组成候选块（若存在），即使首字节是非法的 `0x80`。因此输入 `aa\x80ab` 的非法块是 `\x80a`，替换后剩余 `b`，得到 `aa?b`；Rust 与 Go 对照测试都固定了这一行为。

## 数据与状态

`EncodingGbk`、`CustomGbkEncoder` 和全局 `ENCODING_GBK_IMPL` 都不保存可变状态。转换的局部状态只有源偏移、预分配的 `output`、首个非法块 `first` 以及大小写结果字符串。`transform_gbk` 初始容量取 `src.len()`，但 GBK 与 UTF-8 的字节宽度不同，必要时 `Vec` 会继续扩容。

`Op` 是定义在 `pkg/parser/charset/encoding.rs` 的位标志。方向位决定 GBK→UTF-8 还是 UTF-8→GBK；收集位决定追加原块或目标块；错误动作位决定截断或追加 `?`；`OP_SKIP_ERROR` 只控制最终是否返回错误，不改变已经执行的替换/截断。`EncodingError` 保存字符集名、首个非法字节块和当时规则生成的完整输出，调用方可通过 `output()` 读取错误输出。

大小写兼容数据是只读常量 `GBK_UNCHANGED_CASE_RANGES`。该表与 Go 文件中的 `GBKCase` 区间一一对应，不在运行时构建，也不依赖区域设置。

## 依赖与调用关系

- crate 内上游：`pkg/parser/charset/encoding.rs::FindEncoding` 引用 `ENCODING_GBK_IMPL`；`CountValidBytes`、字符集转换及所有持有 `EncodingRef` 的代码可经 trait 间接调用本实现。
- crate 外直接上游：RustCodeGraph 的调用边显示 `pkg/util/collate/gbk_bin.rs::encode_char -> NewCustomGBKEncoder -> CustomGbkEncoder::transform -> transform_gbk`，用于 `gbk_bin` 的字符编码和排序键。
- crate 内下游：本文件使用 `encoding.rs` 提供的 `Encoding`、`EncodingTpGBK`、`Op` 与各操作位、`utf8_chunk`、`invalid_action`、`finish_transform`、`EncodingError`、`CharsetGBK`。
- 外部下游：`encoding_rs::GBK` 提供实际码表和无替换编解码。`pkg/parser/charset/Cargo.toml` 将其固定为 `encoding_rs = "0.8.35"`。
- 装配关系：`pkg/parser/charset/lib.rs` 公开本模块和符号，并以独立文件 `encoding_gbk_test.rs` 挂载专属测试；通用 GBK 行为还由 `encoding_test.rs` 和 `encoding_gb18030_2_aster_unit_test.rs` 覆盖。

## 错误处理与边界

- 空输入：`peek_gbk` 返回空片，遍历循环不执行，转换得到空输出。
- 不完整或非法 GBK：高位尾字节可能形成长度 1 的候选块；`encoding_rs` 无替换解码失败后走统一非法动作。`0x80` 无条件非法。
- CP936 私用区：`is_gbk_private_use_pair` 在调用 `encoding_rs` 前拒绝这些两字节组合，避免 WHATWG 映射与 Go 码表不一致。
- 非法 UTF-8：`utf8_chunk` 最少消费 1 字节并标记失败，保证遍历能前进，不会死循环。
- 不可编码 Unicode：`encoding_rs` 的 `had_errors` 令本块失败；`€` 即便底层可能提供 CP936/WHATWG 映射，也被显式拒绝以对齐 Go/TiDB。
- 操作策略：`OpEncode` / `OpDecode` 在首个非法块处截断并返回错误；`OpEncodeReplace` / `OpDecodeReplace` 以 `?` 继续但仍返回错误；带 `OP_SKIP_ERROR` 的组合保留相同输出策略但返回 `Ok`。
- `MbLen` 只检查前两个字节是否落在 GBK 双字节范围，返回 2 或 0；它不调用完整码表，因此不能替代 `IsValid`。
- `dest` 不是增量追加缓冲：`finish_transform` 会清空再写入，即使随后返回 `EncodingError`，错误对象和 `dest` 都含规则生成的输出。

## 并发与资源生命周期

`Encoding` trait 要求 `Sync`，而 `EncodingGbk` 无字段，因此全局静态实例可被多线程只读共享。所有遍历偏移、错误记录和输出缓冲都属于单次调用栈，不存在锁、通道、后台任务、文件句柄或事务。

`CustomGbkEncoder::transform` 使用 `&mut self` 保留了编码器接口形状，但类型本身无字段，`reset` 也不做任何操作；实例无需跨请求保存。解码和编码会为每个成功字符块创建临时 `Vec<u8>`，`transform_gbk` 再复制到聚合输出；这是本实现最直接的分配成本。若优化为复用缓冲或流式状态，必须保持块边界、错误输出和 Go 兼容例外不变。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/charset/encoding_gbk.go`：Go 的 `encodingGBK` 加 `encodingBase` 对应 Rust 的 `EncodingGbk` 加通用 `Encoding`/`Op` 框架；Go 的 `EncodingGBKImpl` 对应 `ENCODING_GBK_IMPL`；`GBKCase` 对应 `GBK_UNCHANGED_CASE_RANGES`；Go 的 `customGBKEncoder` 对应 `CustomGbkEncoder`。

两端都保留 TiDB 的关键兼容行为：GBK 字符宽度探测、欧元符编码失败、`0x80` 解码异常、非法字符替换以及 MySQL 特殊大小写区间。Rust 还显式过滤 CP936 用户自定义区，因为 `encoding_rs` 按 WHATWG GBK 索引可能把它们映射为 Unicode 私用码点，而 Go `golang.org/x/text/encoding/simplifiedchinese.GBK` 相应表项未映射。

实现形态并非逐类型复制：Go 通过 `encoding.Encoder` / `Decoder` 的流式 `Transform(dst, src, atEOF)` 返回消费计数，Rust 的公开自定义编码器一次接收完整切片并返回 `Result<Vec<u8>, EncodingError>`；Rust 的 `reset` 因无状态而为空。Rust 的 `go_simple_upper` / `go_simple_lower` 还专门处理标准库完整大小写映射可能多码点展开的问题，以复现 Go 单 rune 映射；`encoding_gbk_test.rs` 验证了 `ß`、`U+1F80` 和 `U+0130`。

## 扩展指南

- 调整 GBK 字节合法范围或码表差异时，优先修改 `decode_gbk` / `is_gbk_private_use_pair`，并同步 `pkg/parser/charset/encoding_test.rs` 的解码替换表和对应 Go 测试 `encoding_test.go`；不要仅修改 `MbLen`，因为它只是长度辅助。
- 调整 Unicode→GBK 兼容例外时修改 `encode_gbk`，同时覆盖严格、替换和跳过错误三类 `Op`。排序键也依赖这一路径，应同步检查 `pkg/util/collate/gbk_bin_test.rs`。
- 调整遍历或错误语义时修改 `foreach_gbk` / `transform_gbk`，并核对通用辅助 `utf8_chunk`、`invalid_action`、`finish_transform` 的契约；尤其保持“首个非法块”、回调提前停止、`dest` 覆写和错误中携带输出的行为。
- 增删 MySQL GBK 大小写例外时同步维护 `GBK_UNCHANGED_CASE_RANGES` 与 Go `GBKCase`，扩充独立测试 `encoding_gbk_test.rs`，并确认不会引入多码点映射。
- 若给 `CustomGbkEncoder` 增加缓存或流式状态，`reset` 必须真正恢复初始状态，并重新评估 `Sync`/`Send`、跨调用复用和错误后的状态；测试逻辑仍应放在独立 `*_test.rs` 文件，不应内嵌进本源文件。
- 任何底层库升级都要重新核对 `0x80`、欧元符、CP936 私用区和不可表示字符，因为这些正是 `encoding_rs` 与 Go 行为可能分叉的边界。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/charset` 确认目标源、模块入口、Go 对照及独立测试均已索引。
- RustCodeGraph 源码与内部流：`node --file pkg/parser/charset/encoding_gbk.rs` 核对了全部 256 行；`explore` 给出 `foreach_gbk -> peek_gbk/decode_gbk/encode_gbk`，以及 `Transform -> transform_gbk`、`ToUpper/ToLower -> gbk_case` 等边。
- RustCodeGraph 上游证据：目标文件显示被 `pkg/util/collate/gbk_bin.rs` 使用；调用查询和该文件节点共同确认 `encode_char -> NewCustomGBKEncoder`。`encoding.rs` 节点确认 `FindEncoding(CharsetGBK) -> ENCODING_GBK_IMPL`。
- crate 与装配：读取了 `pkg/parser/charset/Cargo.toml`、`pkg/parser/charset/lib.rs`、`pkg/parser/charset/encoding.rs`；确认 crate 名、`encoding_rs` 版本、公开再导出、测试挂载及通用错误/操作位语义。
- Go 对照：读取了 `pkg/parser/charset/encoding_gbk.go` 和 `pkg/parser/charset/encoding_test.go`，核对 `EncodingGBKImpl`、`Peek`、`MbLen`、`GBKCase`、自定义编解码器及替换用例。
- Rust 测试：读取了 `pkg/parser/charset/encoding_gbk_test.rs`、`encoding_test.rs`、`encoding_gb18030_2_aster_unit_test.rs`，并检查 `pkg/util/collate/gbk_bin_test.rs` 的直接消费者边界；覆盖往返、替换、合法性、欧元符、`0x80`、字符分块和简单大小写映射。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前另以任务指定命令验证本文恰好具有 11 个固定二级章节，并人工复核未把推测写成已实现事实。
