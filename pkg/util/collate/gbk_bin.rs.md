# `pkg/util/collate/gbk_bin.rs`

## 文件定位

本文件实现 MySQL/TiDB 的 `gbk_bin` 排序规则，是 `astersql-util-collate` crate 中面向 GBK 字符集的二进制 Collator。crate 入口 `pkg/util/collate/lib.rs` 将本模块声明为 `gbk_bin` 并公开再导出；`pkg/util/collate/collate.rs::new_collator` 在新 collation 开启时把名称 `gbk_bin` 构造成 `gbkBinCollator`，`GetCollatorByID` 则先通过字符集元数据把 ID（测试确认是 87）解析成名称后走同一工厂。

它位于 SQL 字符串排序语义的基础设施层：上层只依赖 `Collator` trait，用本实现比较 GBK 字符串、生成可用于排序或索引的 key，并创建 LIKE 通配符匹配器。`pkg/util/collate/Cargo.toml` 表明该实现属于 `astersql-util-collate` 库；直接运行时依赖是本地 crate `astersql-parser-charset` 提供的 TiDB 定制 GBK 编码器。`full_collate` feature 不控制本文件，`gbk_bin` 始终属于 `new_collator` 的第一组实现。

## 核心职责

- `gbkBinCollator` 把 Unicode 标量逐字符编码为 GBK 字节，再按字节字典序实现区分大小写的 binary 比较；例如测试确认“中文”的 key 是 `D6 D0 CE C4`，而 `a` 大于 `A`。
- `Compare`、`Key`、`ImmutableKey` 和字节入口遵守 PAD SPACE：先删除末尾 ASCII 空格，内部空格、制表符等不被裁剪。只有 `KeyWithoutTrimRightSpace` 明确保留尾空格。
- 编码失败以及原始字节入口遇到非法 UTF-8 时统一产生问号字节 `0x3F`，使比较与 key 生成采用同一替换规则。
- `Pattern` 提供与 binary collation 相同的 rune 级通配符语义，具体编译和匹配状态委托给 `pkg/util/collate/bin.rs::derivedBinPattern`。

本文件不负责选择 collation、维护名称/ID 表、实现字符集编码表或实现通配符算法；这些职责分别属于 `collate.rs`、`parser_charset` 和 `bin.rs`/`stringutil`。

## 主要符号

- `pub struct gbkBinCollator`：零大小、无字段的公开 Collator 实现，派生 `Default`。它实现 `Collator: Send + Sync` 所需的全部方法，并通过 `as_any` 支持工厂测试和调用方的具体类型判断。
- `fn encode_char(ch: char) -> Vec<u8>`：内部编码原语。每次为单个字符创建 `parser_charset::NewCustomGBKEncoder()`，调用 `transform` 写入预分配容量为 2 的缓冲；任何错误都返回单字节 `?`。创建新编码器使一次字符转换不依赖此前调用的编码器状态。
- `Collator::Compare(&self, a, b) -> i32`：裁掉两侧字符串的尾空格，逐个 Unicode 字符编码并比较所得 `Vec<u8>`；首个不同字符立即返回 `-1` 或 `1`，共同前缀完全相同时由哪一侧先耗尽决定结果。
- `Collator::CompareBytes`：为 Go string 可携带非法 UTF-8 的兼容入口。它先按原始字节裁尾空格，再分别经 `encode_bytes` 转码，最后由 `compare_encoded_bytes` 比较整个 key。
- `Collator::Key` / `ImmutableKey`：两者都先裁尾空格；`ImmutableKey` 直接调用 `Key`，返回独立拥有的 `Vec<u8>`。
- `Collator::KeyBytes`：原始字节版本，裁尾空格后调用 `encode_bytes`，不会先做 lossy UTF-8 字符串转换。
- `Collator::KeyWithoutTrimRightSpace`：逐字符 `flat_map(encode_char)`，保留尾空格，是字符串 key 的实际编码循环。
- `Collator::MaxKeyLen`：返回 Unicode 字符数乘 2；依据是 GBK 中单个可编码字符最多占 2 字节。这是上界估算，不是实际 key 长度。
- `Collator::Pattern`、`Clone`、`as_any`：分别创建新的 `gbkBinPattern`、新的无状态 Collator 和提供 `Any` 视图。
- `fn encode_bytes(value) -> Vec<u8>`：用 `collate.rs::decodeRune` 顺序消费原始字节；非法 UTF-8 每次消费一个字节并追加 `?`，合法字符交给 `encode_char`。
- `fn compare_encoded_bytes(a, b) -> i32`：先生成两侧完整 GBK key，再把 Rust `Ordering` 规范化成 `-1/0/1`。
- `pub struct gbkBinPattern`：公开包装 `derivedBinPattern` 的状态对象；`Compile` 和 `DoMatch` 原样转发。
- `fn compare_length(a, b) -> i32`：保留自 Go 的内部长度比较辅助函数，以 `sign` 规范化差值；当前带 `allow(dead_code)` 且生产路径未调用。

文件内没有模块级常量、枚举、条件编译项或可变静态状态。

## 执行流程

名称入口的主链是：调用方请求 `GetCollator("gbk_bin")`（或用 ID 87 请求）→ `collate.rs::new_collator` 创建默认的 `gbkBinCollator` → 上层经 trait object 调用比较、key 或 pattern API。

字符串比较流程如下：

1. `Compare` 对两侧调用 `truncateTailingSpace`，只删除末尾字符 `' '`。
2. 两个字符迭代器同步前进；存在成对字符时，各自经 `encode_char` 得到 1～2 个 GBK 字节，或在不可编码时得到 `?`。
3. 按编码字节的词典序比较。首个不同编码决定结果；相同则继续。
4. 若一侧先耗尽，较短侧更小；同时耗尽则相等。因此 `a` 与 `a ` 在 PAD SPACE 后相等。

字符串 key 流程是 `Key` → `truncateTailingSpace` → `KeyWithoutTrimRightSpace` → 对每个字符调用 `encode_char` 并拼接。比较逐字符提前退出，而 key 会编码完整输入；两者仍使用相同的每字符编码及替换策略。

原始字节流程是 `CompareBytes`/`KeyBytes` → `truncateTailingSpaceBytes` → `encode_bytes`。`decodeRune` 对合法 UTF-8 返回字符并按该字符长度推进；对非法首字节返回 `invalid = true`、仅推进一个字节，`encode_bytes` 随即追加一个 `?`。`CompareBytes` 再比较两侧完整结果，所以不同非法字节（如 `FF` 与 `FE`）都会映射为相等的 `3F`。

通配符流程是 `Pattern` 创建空的 `gbkBinPattern` → `Compile` 把 pattern 和 escape 交给 `derivedBinPattern::Compile` → `DoMatch` 复用其 rune 级匹配状态。这里不对 pattern 或目标串做 GBK key 转换。

## 数据与状态

`gbkBinCollator` 是零大小无状态类型。每次 key 调用都返回新 `Vec<u8>`；`ImmutableKey` 的“不可变”是接口契约，不表示借用共享缓存。`Clone` 也只创建新的零状态实例。

瞬时数据包括 `encode_char` 的编码器与输出缓冲、`Compare` 的两个字符迭代器，以及 `encode_bytes` 的输出缓冲和字节游标。字符串 key 的容量由迭代收集动态增长；原始字节 key 初始容量等于输入字节数，但 GBK 编码结果需要时可以扩容。

`gbkBinPattern` 是本文件唯一持久保存调用间状态的对象，其 `inner` 在 `Compile` 时保存 pattern 字符和类型，之后供 `DoMatch` 只读使用。重新调用 `Compile` 会替换旧 pattern 状态。

关键不变量是：所有公开比较结果仅为 `-1/0/1`；`Key` 与 `ImmutableKey` 等价；字符串入口每个不可编码字符映射一个 `?`；字节入口每个非法 UTF-8 字节映射一个 `?`；PAD SPACE 只裁 ASCII 空格。

## 依赖与调用关系

上游直接接线为 `pkg/util/collate/lib.rs` 的模块声明/再导出，以及 `pkg/util/collate/collate.rs::new_collator` 的 `"gbk_bin"` 分支。`GetCollator`、`GetCollatorWithCollate` 和 `GetCollatorByID` 构成公开工厂链；新 collation 关闭时工厂返回 `derivedBinCollator`，不会进入本实现。RustCodeGraph 将目标文件识别为 22 个符号，并显示测试及同目录 binary 实现对它的引用；精确图边确认 `encode_bytes → decodeRune, encode_char`，`compare_encoded_bytes → encode_bytes`。

本文件的直接下游为：

- `parser_charset::NewCustomGBKEncoder`：提供 TiDB 定制 GBK 转码语义。
- `collate.rs::{truncateTailingSpace, truncateTailingSpaceBytes}`：提供字符串和原始字节的 PAD SPACE 预处理。
- `collate.rs::decodeRune`：区分合法 U+FFFD 与非法 UTF-8，并保证非法输入按单字节推进。
- `bin.rs::derivedBinPattern`：实际持有和执行 wildcard pattern。
- `collate.rs::sign`：仅被当前未使用的 `compare_length` 调用。

`pkg/util/collate/Cargo.toml` 还声明 `encoding_rs` 和 `dbterror`，但本文件没有直接引用它们；不要把 crate 级依赖误写成此文件的运行路径。

## 错误处理与边界

本文件的公开 trait 方法不返回 `Result`。`encode_char` 吞掉编码错误并用 `?` 替代，这是对齐 Go 的既定兼容行为，而不是把错误上抛。直接后果是不同不可编码字符可能生成同一个 key、比较为相等；独立测试特别确认 TiDB 定制 GBK 编码器拒绝 U+20AC（欧元符号），所以 `€` 与 `?` 比较相等。

原始字节入口不会 panic 或拒绝非法 UTF-8：`decodeRune` 每次至少推进一个字节，保证循环终止；每个非法字节都转换为 `?`。合法编码得到的 U+FFFD 则不是 `invalid`，会继续尝试 GBK 编码，因不可编码而同样落到 `?`，但经过的是不同分支。

空串、全空格串和裁剪后为空的输入会产生空 key；两个空结果比较相等。`Compare` 只接受合法 Rust `str`，要保留 Go 原始 string 的非法字节语义必须调用 `CompareBytes`/`KeyBytes`。

`MaxKeyLen` 使用 `chars().count() * 2` 后转换为 `i32`；它没有显式溢出处理。常规 SQL 字符串长度受上层限制，但若把该 API 用于极端超大内存字符串，转换边界需要调用方评估。`compare_length` 也先把 `usize` 转成 `isize` 再相减，不过当前没有调用者。

## 并发与资源生命周期

`Collator` 与 `WildcardPattern` trait 都要求 `Send + Sync`。`gbkBinCollator` 不保存编码器、缓存、锁或全局状态；`encode_char` 在每次调用栈上创建并销毁编码器，因此同一 Collator 实例可由多个线程只读共享，不存在内部状态竞争。

每次 `Compare`、`CompareBytes` 和 key 生成的迭代器及缓冲都归当前调用所有，函数返回后自动释放；没有文件句柄、网络连接、异步任务、通道或事务生命周期。`gbkBinPattern` 的编译需要 `&mut self`，匹配只需 `&self`：调用方必须在共享匹配器前完成编译，Rust 借用规则阻止编译与匹配同时可变访问。`Clone` 不共享任何内部资源；`Pattern` 每次也返回独立状态。

性能上，`Compare` 可在首个差异处短路，但每比较一对字符会创建两个编码器和两个小向量；`CompareBytes` 必须先编码两侧完整输入。若优化这些分配，必须证明自定义编码器的重置/状态语义以及错误替换规则完全不变，并同步比较、key 与并发测试。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/collate/gbk_bin.go`。Rust 保留了同名核心类型和 `Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`MaxKeyLen`、`Pattern`、`Clone` 的总体契约：逐字符 GBK 编码、失败替换为 `?`、PAD SPACE、单字符最大 2 字节，以及直接复用 derived binary pattern。

实现形态有三点明确差异：

- Go 的 `gbkBinCollator` 持有 `*encoding.Encoder`，Rust 类型无字段并在 `encode_char` 中为每个字符新建 `NewCustomGBKEncoder`。两边都逐字符转换并在错误时替换 `?`，但 Rust 避免共享有状态编码器。
- Go string 可直接携带非法 UTF-8，Go 的主方法按首字节估算 rune 长度处理。Rust `str` 保证合法 UTF-8，因此新增 `CompareBytes`/`KeyBytes` 显式保存原始字节兼容语义，并通过 `decodeRune` 对非法字节逐个替换。
- Go `ImmutableKey` 直接调用 `KeyWithoutTrimRightSpace(truncateTailingSpace(str))`；Rust 调用 `Key(str_)`，当前结果等价。Rust 另有 `as_any`，用于 trait object 向下转型；这是 Rust trait 工厂需要的接线，不是 Go API。

Go 当前文件没有单独的 `gbk_bin_test.go`；共享的 `pkg/util/collate/collate_test.go` 覆盖工厂返回类型和非法 UTF-8。Rust 将对应覆盖保留在 `collate_test.rs`，并在独立的 `gbk_bin_test.rs` 增加 U+20AC 定制编码器回归测试，在 `bin_1_aster_unit_test.rs` 覆盖中文 key、PAD SPACE 和 `MaxKeyLen`。

## 扩展指南

若改变 GBK 编码或替换策略，优先修改/审查 `encode_char` 与 `encode_bytes`，并确保 `Compare`、`CompareBytes`、`Key`、`KeyBytes`、`ImmutableKey` 仍生成一致的等价关系。需要同步更新独立测试 `pkg/util/collate/gbk_bin_test.rs`，并扩展 `collate_test.rs` 的表驱动比较/key/非法字节用例；测试逻辑必须继续放在独立测试文件，不能内嵌进生产文件。

若改变 PAD SPACE，接入点是 `Compare`、`Key`、`CompareBytes` 和 `KeyBytes` 的预处理调用，同时核对共享帮助函数及 Go `gbk_bin.go`。应覆盖空串、全空格、多个尾空格、制表符、内部空格，以及字符串入口和字节入口的一致性；此类变化可能改变索引 key、唯一性判断和 ORDER BY 结果，属于存储兼容风险。

若扩展 wildcard 行为，`gbkBinPattern` 当前只是委托层；真正算法在 `derivedBinPattern` 与 `stringutil`。只有 GBK 需要偏离 binary pattern 时才应在本类型增加状态或转换，并补充独立 pattern 测试，避免无意改变其他 binary collator。

若优化分配，可考虑复用输出缓冲或按字符串批量编码，但不能直接引入共享可变编码器：必须验证线程安全、编码器重置、逐字符短路次序、U+20AC 和其他不可编码字符、非法 UTF-8 每字节替换，以及 key 的稳定性。任何对 `MaxKeyLen` 的调整必须仍是所有可接受输入的安全上界。

工厂可见性变更应在 `collate.rs::new_collator`、字符集名称/ID 元数据和 `collate_test.rs` 的名称与 ID 断言中同步；`Cargo.toml` 的 `full_collate` 当前与 `gbk_bin` 无关，不应无依据地给本模块增加 feature 门控。

## 验证依据

- Rust 生产实现：`pkg/util/collate/gbk_bin.rs`，确认 22 个索引符号、全部 trait 方法、内部编码/字节比较帮助函数、pattern 包装和无条件编译事实。
- crate 边界：`pkg/util/collate/Cargo.toml` 与 `pkg/util/collate/lib.rs`，确认库路径、默认 feature、`parser_charset` 依赖、模块声明、公开再导出和独立测试接线。
- 工厂与共享语义：`pkg/util/collate/collate.rs::{Collator, WildcardPattern, new_collator, GetCollator, GetCollatorByID, truncateTailingSpace, truncateTailingSpaceBytes, decodeRune, sign}`；`pkg/util/collate/bin.rs::derivedBinPattern`。
- Go 对照：`pkg/util/collate/gbk_bin.go`；相关 Go 测试 `pkg/util/collate/collate_test.go` 的 `GetCollator("gbk_bin")`、ID 87 和非法 UTF-8 用例。
- Rust 测试：`pkg/util/collate/gbk_bin_test.rs::euro_uses_tidb_custom_gbk_replacement`；`pkg/util/collate/bin_1_aster_unit_test.rs::gbk_and_gb18030_keys_match_go_examples`、`focused_registry_returns_group_collators_and_binary_fallback`；`pkg/util/collate/collate_test.rs::{test_utf8_collator_compare, test_utf8_collator_key, test_campare_invalid_utf8_rune}` 及工厂类型断言。
- RustCodeGraph 检查：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/collate` 确认目标及对照文件已索引；`node --file pkg/util/collate/gbk_bin.rs` 读取完整实现；精确 `query/node/callers/callees` 确认 `gbkBinCollator`、`encode_char`、`encode_bytes`、`compare_encoded_bytes`、`gbkBinPattern`、`derivedBinPattern`，其中可靠的内部调用边为 `encode_bytes → decodeRune/encode_char` 和 `compare_encoded_bytes → encode_bytes`。图对 trait 动态分派调用者没有给出完整方法级 caller 边，因此上游工厂关系由已索引的 `collate.rs::new_collator` 源码和测试共同核验，未据此推测业务调用方。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的 `test`/`rg` 命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核只新增本说明和删除完成后的任务文件。
