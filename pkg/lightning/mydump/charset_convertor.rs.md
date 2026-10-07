# `pkg/lightning/mydump/charset_convertor.rs`

## 文件定位

本文件属于 `astersql-lightning-mydump` crate，crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，模块由 [`lib.rs`](./lib.rs) 中的 `mod charset_convertor; pub use charset_convertor::*;` 接入并公开。它位于 Lightning/mydump 的 CSV 输入链路中：[`region.rs`](./region.rs) 的 `openCSVParser` 根据 `DataDivideConfig.charset` 和 `invalid_char_replacement` 创建 `CharsetConvertor`，随后把它传给 [`csv_parser.rs`](./csv_parser.rs) 的 `NewCSVParser`。

该文件不是通用的连接字符集实现，也不负责自动探测编码；它只把已配置的 dump 源字符集与 Rust UTF-8 `String` 互相转换。当前直接支持 `binary`、`utf8`/`utf8mb4`、`ascii`、`gb18030`、`gbk`、`latin1` 六类名称，其中 `latin1` 有意按 Windows-1252 处理。

## 核心职责

1. `Charset::parse` 把用户配置字符串规范化为受控枚举，拒绝未知字符集。
2. `NewCharsetConvertor` 保存源字符集和非法输入替换串，并完成构造期校验。
3. `CharsetConvertor::Decode` 把 dump 字节解码为 Rust UTF-8 `String`；GB18030、GBK 和 Latin1 走 `encoding_rs`，非法字节产生的 U+FFFD 再替换成配置串。
4. `CharsetConvertor::Encode` 把 UTF-8 文本编码回源字符集，供 CSV 解析器把分隔符、包围符、行终止符转换为输入文件中的实际字节，也供测试构造源编码数据。
5. 对 GB18030 合法表示的 U+FFFD（字节 `84 31 A4 37`）做专门保护，避免把真实数据误认成解码错误。

这里的“直通”仍受 Rust 类型约束：`Binary`、`Utf8Mb4`、`Ascii` 的 `Decode` 使用 `String::from_utf8`，因此输入必须是合法 UTF-8；它不是任意二进制到字符串的无校验转换。

## 主要符号

- `pub enum Charset`：源字符集的闭集，变体为 `Binary`、`Utf8Mb4`、`Ascii`、`Gb18030`、`Gbk`、`Latin1`。它是 `Copy` 值类型，不携带动态编解码器。
- `Charset::parse(&str) -> Result<Charset, MydumpError>`：私有配置解析器；先 `trim`，再做 ASCII 小写化；`utf8` 是 `Utf8Mb4` 的别名，未知名称产生 `MydumpError::Configuration`。
- `Charset::encoding(self) -> Option<&'static Encoding>`：把需要转换的变体映射到 `encoding_rs::{GB18030, GBK, WINDOWS_1252}`；其余三个变体返回 `None`，表示不调用 `encoding_rs`。
- `pub struct CharsetConvertor`：仅保存 `source_character_set` 与 `invalid_char_replacement`。字段私有，实例可 `Clone`，没有可变的流式解码状态。
- `pub fn NewCharsetConvertor(&str, &str) -> Result<CharsetConvertor, MydumpError>`：兼容 Go 命名的公开构造器，依次调用 `Charset::parse`、`initDecoder` 和 `initEncoder`。
- `pub fn new_charset_convertor(...)`：符合 Rust 命名习惯的公开薄别名，直接转发给 `NewCharsetConvertor`。
- `initDecoder`、`initEncoder`：公开的 Go 对齐方法；Rust 版本不创建编解码器对象，两者当前都只调用私有 `validate`。
- `validate`：穷尽匹配所有枚举变体。由于外部不能构造未知变体，它目前不会返回错误，构造错误实际来自 `Charset::parse`。
- `precheck(&[u8]) -> bool`：仅当输入非空且 `Charset::encoding()` 为 `Some` 时启用转换。
- `Decode(&[u8]) -> Result<String, MydumpError>` / `decode`：源字节到 UTF-8 文本的主实现及蛇形别名。
- `Encode(&str) -> Result<Vec<u8>, MydumpError>` / `encode`：UTF-8 文本到源编码字节的主实现及蛇形别名。

本文件没有 trait、模块级可变状态、条件编译项或异步入口；唯一的函数内常量是 GB18030 合法 U+FFFD 的四字节表示 `ENCODED_REPLACEMENT_CHARACTER`。

## 执行流程

构造流程如下：

1. `NewCharsetConvertor` 调用 `Charset::parse`；配置会去除两端空白并忽略 ASCII 大小写。
2. 保存枚举与替换串的自有副本。
3. 顺序调用 `initDecoder`、`initEncoder`。当前两步只做穷尽枚举校验，未分配资源。

解码流程如下：

1. `Decode` 调用 `precheck`。空输入或 `Binary`/`Utf8Mb4`/`Ascii` 走 `String::from_utf8`；无效 UTF-8 转成 `MydumpError::Encoding`。
2. GBK、GB18030、Latin1 取得静态 `encoding_rs::Encoding`。
3. 普通转换路径调用 `Encoding::decode`，再把结果中的 U+FFFD 全部替换成 `invalid_char_replacement`。
4. GB18030 路径先扫描合法 U+FFFD 的固定四字节序列。序列之间的片段按普通路径解码和替换，而该合法序列直接写入真正的 U+FFFD；因此只有解码错误生成的 U+FFFD 被替换。

编码流程如下：

1. `Binary`/`Utf8Mb4`/`Ascii` 直接复制 `str::as_bytes()`。
2. 其他字符集调用 `Encoding::encode`。
3. 若 `had_errors` 为真，丢弃带替代结果并返回 `MydumpError::Encoding`；否则返回拥有所有权的编码字节。

在完整 CSV 链路中，[`region.rs`](./region.rs) 的 `openCSVParser` 构造转换器；[`csv_parser.rs`](./csv_parser.rs) 的 `encodeSpecialSymbols` 在解析前调用 `Encode` 转换三个控制符，`CsvParser::ReadColumns` 和 `ReadRow` 则通过私有 `decode` 对每个字段调用 `Decode`。因此字段边界按源编码字节识别，字段值最终以 UTF-8 字节写入 `Datum::Bytes`。

## 数据与状态

`CharsetConvertor` 的状态在构造后不再改变：字符集是 `Copy` 枚举，替换串是拥有所有权的 `String`。每次调用都独立创建返回缓冲区；`Decode` 返回新的 `String`，`Encode` 返回新的 `Vec<u8>`。`encoding_rs` 编码对象是静态引用，不保存在实例中，也没有跨调用的增量转换状态。

重要不变量包括：

- `source_character_set` 只能来自 `Charset::parse` 所允许的变体。
- 需要转换的字符集与静态编码一一对应：GB18030→`GB18030`、GBK→`GBK`、Latin1→`WINDOWS_1252`。
- 非转换路径的 `Decode` 必须生成合法 Rust UTF-8；`Encode` 的输入因类型为 `&str` 已保证 UTF-8。
- GB18030 合法 U+FFFD 的四字节序列必须原样表示为 U+FFFD，而不能套用无效字节替换串。
- 替换串本身是 UTF-8 文本，可以为空或包含多个 Unicode 标量；代码不限制其长度。

## 依赖与调用关系

直接依赖只有两组：

- `encoding_rs`：由本 crate 的 [`Cargo.toml`](./Cargo.toml) 以 `encoding_rs = "0.8"` 声明，提供 GB18030、GBK、Windows-1252 编解码。
- `crate::MydumpError`：定义在 [`common.rs`](./common.rs)，本文件使用其中的 `Configuration` 与 `Encoding` 变体；错误通过 `?` 继续传播到 CSV/region 调用者。

RustCodeGraph 对 `NewCharsetConvertor` 的调用关系显示，生产入口是 `pkg/lightning/mydump/region.rs::openCSVParser`；直接测试入口包括 `charset_convertor_test.rs::TestCharsetConvertor`、`TestInvalidCharReplace`、`valid_gb18030_replacement_character_is_preserved` 和 `csv_parser_test.rs::TestCharsetConversion`，跨 crate 的使用还包括 `pkg/executor/test/loadremotetest/one_csv_test.rs::load_csv_decodes_gbk_and_latin1_input`。

模块内下游关系为：`new_charset_convertor → NewCharsetConvertor → Charset::parse/initDecoder/initEncoder`，`initDecoder/initEncoder → validate`，`Decode → precheck/Charset::encoding/Encoding::decode`，`Encode → Charset::encoding/Encoding::encode`。`lib.rs` 的公开再导出使依赖该 crate 的 executor、session、ingestor 等 crate 可以引用这些 API，但本次核验到的直接生产调用点仍是 mydump 的 CSV 打开路径。

## 错误处理与边界

- 未知字符集在构造阶段返回 `MydumpError::Configuration("unknown charset …")`；不会退回默认编码。
- `Binary`、`Utf8Mb4`、`Ascii` 解码无效 UTF-8 时返回 `MydumpError::Encoding`。这比 Go 字符串的字节直通更严格，调用方不能把任意 binary 字节假定为可接受文本。
- GB18030、GBK、Latin1 解码使用 `encoding_rs` 的替换式 API。代码忽略其 `had_errors` 标志，改为将生成的 U+FFFD 替换成配置串。
- GB18030 的合法 U+FFFD 有专门切段处理；新增或替换编解码库时必须保留这一差异，否则真实 U+FFFD 会被误改写。当前保护逻辑只针对明确验证过的 GB18030 四字节表示。
- 编码不可表示的字符不会静默保留 `encoding_rs` 的替代字节，而是返回 `MydumpError::Encoding("text is not representable in …")`。
- 空输入返回空结果；替换串为空会删除非法序列对应的 U+FFFD；两种行为均没有额外拒绝逻辑。
- `Decode` 的替换以解码后 U+FFFD 为判据，除专门处理的 GB18030 序列外，不应推断它能区分所有“数据中原有 U+FFFD”和“错误产生 U+FFFD”的情况。

## 并发与资源生命周期

该类型不启动线程、任务或通道，不持有文件、锁、事务、网络连接或可变全局资源。构造器只分配替换串；每次转换的临时缓冲区随调用结束释放。两个字段均可安全只读共享所需的基础类型组成，源码没有显式实现或禁止 `Send`/`Sync`，实际 auto trait 由字段决定。

`Clone` 会复制替换串和枚举，因此克隆实例之间不存在共享的可变转换状态。当前 API 的转换方法只接收 `&self`，同一实例并发只读调用不会在本文件内产生竞态；性能代价主要来自每次转换的输出分配、U+FFFD `replace`，以及 GB18030 特殊路径对输入窗口的扫描。

## 与 Go 版本的对应关系

直接对照文件是 [`charset_convertor.go`](./charset_convertor.go)，测试对照是 [`charset_convertor_test.go`](./charset_convertor_test.go)。Rust 保留了 Go 的公开类型/方法名、构造顺序、六类字符集、Windows-1252 形式的 Latin1，以及“非法解码内容替换成配置串”的主要意图。

关键实现差异如下：

- Go 使用 `config.Charset` 并在实例中保存 `*encoding.Decoder`/`*encoding.Encoder`；Rust 使用本地闭集 `Charset`，按调用取静态 `encoding_rs` 引用，`initDecoder`/`initEncoder` 仅为兼容结构而保留。
- Go `precheck` 还检查 nil receiver、decoder、encoder；Rust 的 `&self` 不可能为空，且不保存编解码器，所以只检查输入非空和是否需要转换。
- Go 对 Binary/UTF8MB4/ASCII 返回原始 Go `string`；Rust `Decode` 必须构造合法 UTF-8 `String`，无效字节会报错。
- Go 编码器返回其库的编码错误；Rust 通过 `encoding_rs::encode` 的 `had_errors` 转为 `MydumpError::Encoding`。
- Rust 增加了 GB18030 合法 U+FFFD 的显式保护及独立回归测试；这是为避免便捷解码 API把合法字符与错误替换字符合并，不是简单照抄 Go 的 decoder 对象布局。
- Rust 同时提供 Go 风格大写 API 和蛇形别名，便于现有移植调用与惯用 Rust 调用共存。

## 扩展指南

新增字符集时至少同步以下位置：`Charset` 变体、`Charset::parse` 的配置别名、`Charset::encoding` 的库映射，以及 `validate` 的穷尽分支。若新编码不能由单个静态 `encoding_rs::Encoding` 表示，不应勉强塞入现有 `Option` 模型；应先设计明确的编解码策略，并同步审视 `precheck`、错误映射和实例状态。

修改解码替换逻辑时，应在独立的 [`charset_convertor_test.rs`](./charset_convertor_test.rs) 中覆盖：正常往返、非法序列、合法替换字符、空输入、多字符/空替换串和直通字符集的无效 UTF-8。不得把测试内嵌到生产源文件。涉及 CSV 行为时还应同步 [`csv_parser_test.rs`](./csv_parser_test.rs)；涉及用户可见 LOAD DATA 行为时应扩展 `pkg/executor/test/loadremotetest/one_csv_test.rs` 的对应场景。

需要特别评估三类风险：兼容性上，Go 对 binary/ASCII 的字节语义与 Rust UTF-8 `String` 约束不同；正确性上，不能用全局 U+FFFD 替换破坏合法字符；性能上，当前 GB18030 特殊路径执行窗口扫描、分段解码和字符串替换，大输入上的改动应避免额外的重复遍历与分配。若改变公开大写方法，应保留或有计划地迁移 `region.rs`、`csv_parser.rs` 及外部 crate 调用者。

## 验证依据

- 源码：[`charset_convertor.rs`](./charset_convertor.rs) 全部 170 行；主要证据符号为 `Charset`、`Charset::parse`、`Charset::encoding`、`NewCharsetConvertor`、`CharsetConvertor::{initDecoder,initEncoder,validate,precheck,Decode,Encode}`。
- crate/模块：[`Cargo.toml`](./Cargo.toml) 确认 crate 名、`lib.rs` 入口和 `encoding_rs = "0.8"`；[`lib.rs`](./lib.rs) 确认模块接入、公开再导出及独立测试文件挂载。
- 直接调用链：[`region.rs`](./region.rs) 的 `openCSVParser`；[`csv_parser.rs`](./csv_parser.rs) 的 `encodeSpecialSymbols`、`NewCSVParser`、`CsvParser::ReadColumns`、`ReadRow` 和私有 `decode`。
- 错误定义：[`common.rs`](./common.rs) 的 `MydumpError::{Configuration,Encoding}`。
- Rust 单元测试：[`charset_convertor_test.rs`](./charset_convertor_test.rs) 覆盖 GB18030/UTF-8 往返、非法字节替换、未知字符集和合法 U+FFFD；[`csv_parser_test.rs`](./csv_parser_test.rs) 的 `TestCharsetConversion` 覆盖转换器进入 CSV 字段解析。
- Rust 跨 crate 测试：`pkg/executor/test/loadremotetest/one_csv_test.rs::load_csv_decodes_gbk_and_latin1_input` 覆盖 GB18030 与 Windows-1252 输入进入 `Datum::Bytes`。
- Go 对照：[`charset_convertor.go`](./charset_convertor.go) 与 [`charset_convertor_test.go`](./charset_convertor_test.go)；用于核对原始构造、编解码和非法字符替换语义。
- RustCodeGraph：已运行 `status`（索引包含 11,467 文件、307,296 节点、1,848,419 边）、`files --filter pkg/lightning/mydump`、目标文件 `node --file`、`query CharsetConvertor`/`query new_charset_convertor`、`explore` 及构造器/编解码入口的 callers/callees 查询；图结果确认上述直接生产入口和测试调用者。精确 callers/callees 命令未额外打印边，故调用细节以 `explore` 结果及对应源码节点交叉核对。
