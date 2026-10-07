# `pkg/parser/charset/encoding_gb18030.rs`

## 文件定位

本文件是 `astersql-parser-charset` crate 的 GB18030 编码实现，源码入口是
[`encoding_gb18030.rs`](encoding_gb18030.rs)。`lib.rs` 以
`pub mod encoding_gb18030` 声明模块并以 `pub use encoding_gb18030::*`
公开其符号；`encoding.rs::FindEncoding` 在字符集名等于
`CharsetGB18030` 时返回静态实例 `ENCODING_GB18030_IMPL`。因此它位于“字符集名
解析 -> 统一 `Encoding` trait -> GB18030 编解码”链路的具体实现端，而不是 SQL
解析器入口本身。

该 crate 的边界由 `pkg/parser/charset/Cargo.toml` 定义，包名为
`astersql-parser-charset`，库入口是 `lib.rs`。本文件直接使用 crate 内的统一编码
抽象和 GB18030 大小写表，并依赖外部 `encoding_rs = 0.8.35` 完成实际字符集转换。
Cargo 元数据把对应 Go 包标为 `pkg/parser/charset`，未为本实现设置条件 feature；
源码也没有条件编译项。

## 核心职责

- `EncodingGb18030` 实现统一的 `encoding.rs::Encoding` trait，提供名称、类型、
  字符边界探测、多字节长度、合法性、逐字符遍历、整体转换和大小写转换。
- `peek` 按 GB18030 的 1/2/4 字节形状切出下一个输入单元；非法或不完整输入
  每次只消费首字节，使上层可以选择截断、替换或继续扫描。
- `foreach_gb18030` 统一编码与解码方向的分块循环；`transform_gb18030` 再把通用
  `Op` 位标志解释成收集源/目标、遇错截断、以 `?` 替换、是否返回错误等策略。
- `CustomGb18030Encoder` 与 `CustomGb18030Decoder` 提供 Go 风格构造入口；解码器
  额外记录最近输入是否以 U+FFFD 的 GB18030 编码 `84 31 A4 37` 结尾。
- `convert_bytes_to_u32` / `convert_u32_to_bytes` 提供 GB18030 映射值使用的无符号
  大端整数与变长字节序列互转。当前 Rust 跨文件直接使用点是
  `pkg/util/collate/gb18030_bin.rs::encode_char` 对 `convert_u32_to_bytes` 的调用。

## 主要符号

- `EncodingGb18030`：无字段标记类型；其 `Encoding` 实现承载全部标准接口行为。
- `ENCODING_GB18030_IMPL: EncodingGb18030`：进程内共享的不可变静态实例，
  `encoding.rs::FindEncoding` 通过 `&'static dyn Encoding` 返回它。
- `peek(src: &[u8]) -> &[u8]`：公开的零拷贝边界探测函数。空输入返回空切片；
  ASCII 返回一个字节；前导字节 `0x81..=0xFE` 后接合法 GBK 续字节时返回两字节，
  符合“四字节前导 + 数字 + 前导 + 数字”形状时返回四字节，其余返回首字节。
- `decode_gb18030(chunk) -> Option<Vec<u8>>`：私有单块解码器。先明确拒绝
  `0x80`，再调用 `encoding_rs::GB18030.decode_without_bom_handling_and_without_replacement`；
  无替换解码失败时返回 `None`。
- `encode_gb18030(chunk) -> Option<Vec<u8>>`：私有单块编码器。先严格解析 UTF-8，
  再调用 `encoding_rs::GB18030.encode`，仅在 `had_errors == false` 时返回结果。
- `foreach_gb18030(src, op, callback)`：私有遍历核心。`OP_TO_UTF8` 分支使用
  `peek` 后解码；另一分支使用 `encoding.rs::utf8_chunk` 后编码。回调收到
  `(源块, 目标块, 是否成功)`，返回 `false` 可提前终止。
- `transform_gb18030(dest, src, op)`：私有整体转换核心。它记录第一个非法块，
  调用 `encoding.rs::invalid_action` 决定截断或追加 `?`，最后交给
  `encoding.rs::finish_transform` 更新 `dest` 并决定返回 `Ok` 或 `EncodingError`。
- `Encoding` 实现：`Name`/`Tp` 分别返回 `gb18030` 和 `EncodingTpGB18030`；
  `Peek`、`Foreach`、`Transform` 委托给本文件核心函数；`IsValid` 从 UTF-8 编码
  方向遍历并在首个失败处停止；`ToUpper`/`ToLower` 委托
  `encoding_gb18030_data.rs::gb18030_case`。
- `CustomGb18030Encoder`：无状态包装器，`transform` 固定采用 `OpEncode`，
  `reset` 当前为空操作。
- `CustomGb18030Decoder { rune_error_is_last_input }`：有一个布尔状态；
  `transform` 固定采用 `OpDecode`，随后按原始输入是否以 `84 31 A4 37` 结尾更新状态，
  `reset` 将其清为 `false`。
- `new_custom_gb18030_encoder` / `NewCustomGB18030Encoder` 与对应 decoder 函数：
  分别提供 Rust 风格和 Go 风格命名的公开构造入口。
- `convert_bytes_to_u32`：按大端顺序折叠任意长度输入；空切片得到 `0`。
  `convert_u32_to_bytes`：去掉 `u32::to_be_bytes()` 的前导零；数值 `0` 特意保留
  一个零字节。

## 执行流程

统一查找与转换主流程如下：

1. 调用方以 `FindEncoding(CharsetGB18030)` 取得 `ENCODING_GB18030_IMPL` 的 trait
   引用；`Name` 和 `Tp` 可用于确认具体实现。
2. `Transform` 进入 `transform_gb18030`，按输入长度预分配输出，并调用
   `foreach_gb18030`。
3. 解码方向（`op & OP_TO_UTF8 != 0`）先由 `peek` 划定一个 GB18030 候选块，
   再由 `decode_gb18030` 做无替换解码。编码方向先由 `utf8_chunk` 划定并验证一个
   UTF-8 字符，再由 `encode_gb18030` 转为 GB18030。
4. 成功块按 `OP_COLLECT_FROM` 选择复制源块，否则复制目标块。失败块只在首次出现
   时保存 `(CharsetGB18030, from)`，随后 `invalid_action` 根据 `Op`：TRIM 停止，
   REPLACE 写入 `?` 并继续，没有这两个标志则继续但不追加替代字节。
5. `finish_transform` 清空并重写调用方的 `dest`；存在非法块且没有
   `OP_SKIP_ERROR` 时返回包含首个非法块和已生成输出的 `EncodingError`，否则返回输出。

`IsValid` 并不检查一段已经编码的 GB18030 字节，而是固定用 `OP_FROM_UTF8` 检查
“UTF-8 输入能否完整编码为 GB18030”。回调在首个 `ok == false` 时返回 `false`，
因此它是短路校验。`Foreach` 则把方向和停止时机完整交给调用方。

自定义包装器的流程更窄：encoder 的 `transform` 固定走 `OpEncode`；decoder 固定走
`OpDecode`，并在转换调用结束后更新末尾替换字符标志。它们不是
`encoding_rs::Encoder`/`Decoder` 的流式适配器，也不接受 `at_eof` 或固定容量目标切片。

## 数据与状态

`EncodingGb18030` 和 `CustomGb18030Encoder` 都不保存可变状态。
`ENCODING_GB18030_IMPL` 因而可安全作为全局共享 `Encoding` trait 对象；trait 自身要求
`Sync`。每次转换的 `offset`、`output` 和首错信息均为栈上局部状态，结果使用独立
`Vec<u8>` 所有权，不借用临时目标缓冲。

`CustomGb18030Decoder` 唯一持久状态是 `rune_error_is_last_input: bool`。构造与
`reset` 都将其置为 `false`；每次 `transform` 后，它仅反映该次完整 `src` 是否以
`[0x84, 0x31, 0xA4, 0x37]` 结尾，不累积跨调用的部分序列。调用方若需要跨缓冲流式
解码，不能把该标志理解为自动拼接状态。

大小写状态和 GB18030-2022 补充映射不存放在本文件：大小写由
`encoding_gb18030_data.rs::gb18030_case` 提供；补充 Unicode↔GB18030 映射也由该数据
模块维护。`convert_*` 只做表示转换，不校验字节序列是否是合法 GB18030 码位。

## 依赖与调用关系

上游接线：

- `pkg/parser/charset/lib.rs` 声明并公开再导出本模块，也在那里挂载相关独立测试文件。
- `pkg/parser/charset/encoding.rs::FindEncoding` 是统一运行时入口；
  `IsSupportedEncoding` 把 `gb18030` 纳入支持集合，`CountValidBytes` 等通用函数可通过
  trait 的 `Foreach` 间接使用本实现。
- RustCodeGraph 对目标文件给出的直接跨文件使用者是
  `pkg/util/collate/gb18030_bin.rs`。其中 `encode_char` 先查
  `unicode_to_gb18030()`，命中后调用本文件 `convert_u32_to_bytes` 生成排序键字节；
  未命中再回退 `encoding_rs::GB18030`。

下游依赖：

- `crate::encoding::*`：`Encoding`、`EncodingTpGB18030`、`Op` 及操作位、
  `utf8_chunk`、`invalid_action`、`finish_transform`、`EncodingError`。
- `crate::encoding_gb18030_data::gb18030_case`：GB18030 特殊大小写规则。
- `encoding_rs::GB18030`：无替换解码与标准编码算法。
- 标准库：`std::str::from_utf8` 验证编码方向输入；`Vec` 管理转换结果。

`Cargo.toml` 没有可选 feature 改变上述路径。`encoding_rs` 是本文件实际使用的外部
依赖；`astersql-errors` 等 crate 依赖通过统一错误层间接参与，但本文件不直接构造
外部错误类型。

## 错误处理与边界

- 空输入：`peek` 返回原空切片；遍历循环不执行；转换最终得到空输出。
- ASCII：`peek` 返回单字节。`0x80` 和 `0xFF` 也以单字节非法块前进，其中
  `decode_gb18030` 明确拒绝 `0x80`，避免采用 WHATWG 将其映射为欧元符的语义。
- 两字节：首字节必须为 `0x81..=0xFE`，第二字节必须是 `0x40..=0x7E` 或
  `0x80..=0xFE`；`0x7F` 被排除。
- 四字节：形状必须是 `81..FE 30..39 81..FE 30..39`。不完整或形状错误时，
  `peek` 只返回首字节，所以替换模式会逐字节重新同步。
- `MbLen` 在输入少于两个字节或首字节不是多字节前导时返回 `0`。若恰有 2 或 3
  字节、第二字节是 `0x30..=0x39`，当前实现会继续访问索引 2/3 而发生越界 panic；
  调用者必须保证传入完整候选字符，扩展时应为该现状补独立回归测试后再决定是否
  修正。Go 对照函数也只先检查长度小于 2，具有相同的调用前置条件风险。
- 编码方向遇到非法 UTF-8 时，`utf8_chunk` 返回首字节并标记失败；遇到
  `encoding_rs` 无法表示的字符时也产生失败。解码方向使用“无替换”API，避免库内部
  静默吞掉非法序列。
- 统一错误只保留第一个非法源块，但 `EncodingError::output()` 保留按照 `Op` 已生成
  的完整或截断输出。`OP_SKIP_ERROR` 只抑制最终错误，不会把失败块变成合法块。
- `CustomGb18030Decoder::transform` 无论内部转换成功与否，都会根据整个 `src` 的
  后缀更新状态；该状态不是对“确实成功解码了 U+FFFD”的证明。
- 整数转换不做长度上限检查：超过四字节的输入会按 `u32` 溢出语义移位折叠；正常
  用途应只传入最多四字节的 GB18030 值。

## 并发与资源生命周期

静态 `ENCODING_GB18030_IMPL` 无内部状态，且 `Encoding: Sync`，可被多个线程并发
共享。每次 `Transform`、`Foreach`、`ToUpper`、`ToLower` 都只使用调用栈局部变量或
只读的编码/大小写数据，没有锁、通道、后台任务、事务、文件句柄或网络资源。

`CustomGb18030Decoder` 的布尔字段需要 `&mut self` 才能转换或重置，因此 Rust 借用
规则阻止同一实例被无同步地并发修改；不同实例互不影响。所有临时输出在函数返回时
按所有权正常释放。回调只在 `foreach_gb18030` 调用期间借用当前源块和临时目标块，
不得把这些切片保存到调用结束之后；接口的借用期和 `to` 的临时 `Vec` 生命周期共同
约束了这一点。

性能上每个字符块的 `encode_gb18030`/`decode_gb18030` 都可能分配一个 `Vec`，整体
转换还会把块复制进最终输出；`Vec::with_capacity(src.len())` 只保证按源长度预留，
编码膨胀时仍可能扩容。任何流式或低分配优化都必须保持 `Op`、首错和逐块重同步语义。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/charset/encoding_gb18030.go`；相关码表位于
`encoding_gb18030_data.go`。对应关系如下：

- Rust `EncodingGb18030` / `ENCODING_GB18030_IMPL` 对应 Go
  `encodingGB18030` / `EncodingGB18030Impl`；Rust 用静态无状态实例实现 trait，
  Go 通过嵌入 `encodingGBK`/`encodingBase` 和 `init` 设置 `self`。
- Rust `peek` 与 Go `peek` 的字节形状、非法时消费一字节及排除 `0x7F` 的规则一致。
  Rust 用 `decode_gb18030` 额外显式拒绝 `0x80`；Go 自定义 decoder 则把 `0x80`
  输出为 U+FFFD。
- Go 的主 `Transform` 来自嵌入的 `encodingBase` 并通过 `x/text/transform` 的
  transformer 工作；Rust 把等价的遍历与错误策略直接实现为
  `foreach_gb18030`/`transform_gb18030`，使用拥有所有权的 `Vec` 返回值。
- Go custom encoder/decoder 是 `golang.org/x/text/encoding` 的流式包装器，处理
  `ErrShortDst`、`ErrShortSrc`、`atEOF`、已消费/已写入计数，并在补充映射命中时
  特判。Rust custom 类型只提供整片 `Vec` 转换，当前没有这些流式合同。
- Go decoder 的 `runeErrorIsLastInputFlag` 仅在遍历实际遇到
  `84 31 A4 37` 时置位，并在每次调用开始时清零；Rust 以输入后缀判断，在调用结束
  后更新。两者在一次完整、成功的整片输入上目的相同，但错误返回或分片输入时不应
  假设状态完全等价。
- Go 的 `convertBytesToUint32` 只显式支持 1 至 4 字节，其他长度返回 `0`；Rust 使用
  fold，可接受任意长度。两端的 `convertUint32ToBytes` / `convert_u32_to_bytes` 都输出
  去前导零的大端表示，`0` 均输出单个零字节。
- Go 注释声明实现满足 GB18030-2022，并通过 `unicodeToGB18030` /
  `gb18030ToUnicode` 补充表修正库行为。Rust 的补充表在数据模块中，排序规则明确先
  查该表；本文件的通用编解码直接依赖 `encoding_rs`。现有 Rust 测试验证欧元符、ḿ、
  麻将符号及 U+FFFD 序列的结果，但不能据此宣称所有 Go 流式边界已经移植完成。

## 扩展指南

- 修改字符边界规则时，入口是 `peek` 和 `Encoding::MbLen`。必须同步覆盖空输入、
  1/2/3 字节截断、`0x7F`、`0x80`、`0xFF`、合法两字节及合法四字节；特别应为
  `MbLen` 的短数字续字节输入建立独立回归测试。
- 修改编码/解码算法或 GB18030-2022 映射时，应同时审查 `decode_gb18030`、
  `encode_gb18030`、`encoding_gb18030_data.rs` 和
  `pkg/util/collate/gb18030_bin.rs::encode_char`，避免转换结果与排序键采用不同映射。
- 修改错误策略时，应优先保持 `encoding.rs` 中所有编码共享的 `Op` 合同；本文件
  应只负责报告 `(from, to, ok)`，不要另建与 `invalid_action`/`finish_transform`
  冲突的替换或错误模型。
- 若要真正对齐 Go 流式 custom transformer，需要明确设计目标容量不足、输入不足、
  `at_eof`、消费计数、跨调用不完整序列和 reset 行为，而不能仅扩展当前整片
  `Vec` API。
- 测试逻辑必须继续放在独立文件。优先扩展
  `encoding_gb18030_2_aster_unit_test.rs`（边界与 GB18030-2022 样例）和
  `encoding_test.rs::test_encoding_gb18030`（Go 对齐转换矩阵）；大小写/补充映射则扩展
  `charset_1_aster_unit_test.rs` 或 `encoding_gb18030_data_test.rs`。同步核对 Go
  `encoding_test.go::TestEncodingGB18030`，不要把测试内嵌回生产源文件。
- 兼容风险集中在 MySQL/TiDB 对 `0x80` 的特殊处理、非法块重新同步方式、U+FFFD
  的合法编码和特殊大小写规则；性能风险集中在逐字符分配及最终复制。任何优化都应
  以相同字节级用例验证输出和首错片段。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；目标目录中确认
  `encoding_gb18030.rs`、数据模块、独立测试和 Go 对照均已索引。
- RustCodeGraph `node --file pkg/parser/charset/encoding_gb18030.rs`：读取全部 243 行，
  核对所有常量、结构、函数、trait 实现及无条件编译事实；图报告目标文件的直接跨文件
  使用者为 `pkg/util/collate/gb18030_bin.rs`。
- RustCodeGraph `explore` / `callers` / `callees`：确认
  `transform_gb18030 -> foreach_gb18030 -> decode_gb18030/encode_gb18030`，以及
  `Encoding::{Foreach,IsValid,Transform}` 到这些内部函数的委托关系。
- `pkg/parser/charset/lib.rs` 与 `pkg/parser/charset/encoding.rs`：核对模块公开、测试
  挂载、`Encoding` trait、`Op` 位标志、`FindEncoding`、`utf8_chunk`、
  `invalid_action` 和 `finish_transform`。
- `pkg/parser/charset/Cargo.toml`：核对 crate 名称、库入口、Go 包元数据和
  `encoding_rs` 版本；未发现影响目标实现的 feature。
- Go 对照：`pkg/parser/charset/encoding_gb18030.go`、
  `encoding_gb18030_data.go`、`encoding_base.go`；调用证据还包括
  `pkg/lightning/mydump/charset_convertor.go`、`reader.go` 和
  `pkg/util/collate/gb18030_bin.go`。这些 Go 调用点说明上游设计意图，不代表同名 Rust
  custom API 当前已有等量调用者。
- Rust 独立测试：`encoding_gb18030_2_aster_unit_test.rs` 覆盖查找、2022 映射、
  1/2/4 字节往返、Peek/MbLen 和 U+FFFD；`encoding_test.rs` 覆盖 Go 对齐的合法、
  非法、替换及往返矩阵；`charset_1_aster_unit_test.rs` 与
  `encoding_gb18030_data_test.rs` 覆盖补充映射和特殊大小写。
- Go 测试：`pkg/parser/charset/encoding_test.go::TestEncodingGB18030` 提供对应转换
  矩阵，包括 `0x80`、U+FFFD 编码及补充字符。
- 按任务约束，本次为纯文档分析，未运行 Cargo 或代码测试；最终使用任务指定命令
  校验本文恰含 11 个固定二级章节，并人工复核文件定位、运行方式与安全扩展入口。
