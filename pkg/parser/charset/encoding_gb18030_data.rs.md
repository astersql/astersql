# `pkg/parser/charset/encoding_gb18030_data.rs`

## 文件定位

该文件属于 `astersql-parser-charset` crate；crate 入口 `pkg/parser/charset/lib.rs` 以 `pub mod encoding_gb18030_data` 声明模块，并通过 `pub use encoding_gb18030_data::*` 再导出其公开数据和访问函数。它不是通用 GB18030 编解码器本体，而是为 GB18030-2022 兼容覆盖与 MySQL 大小写语义提供静态数据及只读查询接口。实际字符边界探测、转换和 `Encoding` trait 实现在 `pkg/parser/charset/encoding_gb18030.rs`，二进制排序键的一个直接消费者位于 `pkg/util/collate/gb18030_bin.rs`。

`pkg/parser/charset/Cargo.toml` 将本目录定义为独立 crate `astersql-parser-charset`，入口为 `lib.rs`。本文件自身只依赖 Rust 标准库的 `HashMap` 与 `LazyLock`，没有条件编译项、外部 I/O 或环境配置。

## 核心职责

文件承担三类职责。

1. `GB18030_ENCODING_LIST` 保存 Unicode 标量值与 GB18030 大端编码整数的补充映射。当前源码中有 2,094 项，既包含两字节值（如 `€ -> 0xA2E3`），也包含四字节值（如 `U+20087 -> 0x95329031`）。这些条目是标准编码器之外需要保持与 Go/MySQL 行为一致的覆盖数据。
2. `UNICODE_TO_GB18030` 和 `GB18030_TO_UNICODE` 从同一列表分别构造正向、反向哈希表，并由 `unicode_to_gb18030()`、`gb18030_to_unicode()` 暴露共享只读引用。
3. `GB18030_CASE_RANGES` 保存 58 个 MySQL 特殊大小写区间；`Gb18030Case` 在命中区间时应用显式偏移，未命中时采用与 Go 单码点大小写转换相容的回退，而不是 Rust 可能产生多字符结果的完整大小写展开。

因此，该文件既是数据源，也是一个很薄的查询/大小写转换层；它不负责解析 SQL、检测 GB18030 字节边界或决定非法字节替换策略。

## 主要符号

- `pub const GB18030_ENCODING_LIST: &[(char, u32)]`：补充映射的唯一源数据。`u32` 按大端字节序表达二字节或四字节 GB18030 编码，真正转成字节由消费者（例如 `convert_u32_to_bytes`）完成。
- `pub static UNICODE_TO_GB18030: LazyLock<HashMap<char, u32>>`：首次访问时遍历列表生成正向表；重复 Unicode 键若出现会由后项覆盖前项。
- `pub static GB18030_TO_UNICODE: LazyLock<HashMap<u32, char>>`：同一列表生成的反向表；重复编码值同样遵循 `HashMap::insert` 的后项覆盖语义。
- `pub fn unicode_to_gb18030() -> &'static HashMap<char, u32>` 与 `pub fn gb18030_to_unicode() -> &'static HashMap<u32, char>`：触发相应 `LazyLock` 初始化并返回进程生命周期内有效的只读引用，不复制表。
- `pub struct CaseRange { lo, hi, delta }`：对应 Go `unicode.CaseRange`。`lo`/`hi` 是含端点区间，`delta` 依次表示大写、小写、标题格式的码点偏移；当前转换器只使用索引 0 和 1。
- `pub const GB18030_CASE_RANGES: &[CaseRange]`：MySQL 特殊规则的 58 个区间。零偏移区间具有“阻止默认 Unicode 转换”的意义，不能当作冗余项删除。
- `pub struct Gb18030Case`、`pub static GB18030_CASE`、`pub fn gb18030_case()`：无内部状态的转换器类型、全局单例与访问入口。
- `Gb18030Case::{to_upper,to_lower}`：公开字符串转换入口，分别调用私有 `convert(input, 0)` 与 `convert(input, 1)`。
- `go_simple_upper`、`go_simple_lower`：私有兼容回退。它们在 Rust 完整大小写映射会扩展为多个字符时，恢复 Go `unicode.ToUpper`/`ToLower` 的单码点结果；源码显式处理希腊组合形式和 `U+0130` 等差异。

## 执行流程

正向编码覆盖的路径如下：`pkg/util/collate/gb18030_bin.rs::encode_char` 调用 `unicode_to_gb18030()`；第一次调用会执行 `UNICODE_TO_GB18030` 的闭包，按 `GB18030_ENCODING_LIST` 预分配并填充哈希表；若字符命中，消费者将 `u32` 转为大端字节并直接用于 `gb18030_bin` 比较或排序键，否则回退到 `encoding_rs::GB18030`。这一层只负责“是否存在覆盖值”，不处理回退失败时的 `?` 替换。

大小写路径为：`pkg/parser/charset/encoding_gb18030.rs` 的 `EncodingGb18030::ToUpper`/`ToLower` 获取 `gb18030_case()`，再逐个 Rust `char` 调用 `convert`。每个字符线性查找首个满足 `lo <= code <= hi` 的 `CaseRange`；命中后将对应大小写偏移加到码点上，并用 `char::from_u32` 生成结果。未命中时进入 `go_simple_upper` 或 `go_simple_lower`，只输出一个 Unicode 标量。整个输入按字符顺序处理并写入预分配容量为原 UTF-8 字节长度的 `String`。

反向访问 `gb18030_to_unicode()` 的初始化过程与正向表对称。当前 Rust 生产代码没有直接调用该函数；它由测试校验双向一致性并作为 crate 公共兼容数据保留。与之不同，Go 的自定义解码器会直接查询 `gb18030ToUnicode`。

## 数据与状态

映射列表和大小写区间都是编译期只读切片。两张 `HashMap` 是进程级 `LazyLock`：各自首次被解引用时独立构造，之后不再变化。公开函数只返回共享引用，没有提供可变入口。

正反向表都由同一个 `GB18030_ENCODING_LIST` 派生，因此正常数据下键值应一一对应。代码没有在初始化时检测重复 Unicode 或重复 GB18030 值；列表的唯一性是数据维护不变量。`pkg/parser/charset/charset_1_aster_unit_test.rs::gb18030_supplemental_maps_are_bidirectional` 当前验证欧元映射以及正向表长度等于列表长度，能发现 Unicode 重复键，但没有穷举验证每个反向条目或显式断言反向表长度。

`Gb18030Case` 本身是零大小、无状态类型。`convert` 的容量参数只是初始容量提示；若 UTF-8 结果变长，`String` 会正常扩容。当前接口只支持大写和小写，虽然 `CaseRange::delta[2]` 保留了与 Go 结构一致的标题格式偏移。

## 依赖与调用关系

上游与下游关系可以分为三组：

- 模块装配：`pkg/parser/charset/lib.rs` 声明并再导出本模块，因此其他 crate 可通过 `parser_charset::unicode_to_gb18030()` 等路径调用。
- 编码大小写：`pkg/parser/charset/encoding_gb18030.rs::{ToUpper,ToLower}` 调用 `gb18030_case()`，再调用 `Gb18030Case::{to_upper,to_lower}`。RustCodeGraph 的调用结果也列出了这两个生产调用者。
- 排序规则：`pkg/util/collate/gb18030_bin.rs::encode_char` 调用 `unicode_to_gb18030()`，使 `gb18030_bin` 的字符串比较和排序键优先采用仓库的 GB18030-2022 覆盖值。

下游仅使用 `std::collections::HashMap`、`std::sync::LazyLock` 与 `char` 的标准大小写 API。`Cargo.toml` 中的 `encoding_rs` 依赖由实际编码实现和排序规则回退使用，不由本数据文件直接调用。RustCodeGraph 未找到 `gb18030_to_unicode()` 的生产调用者；已确认的调用者是映射一致性测试。

## 错误处理与边界

本文件没有 `Result`/`Option` 型公开错误通道，也不产生编码错误。映射未命中由调用方决定如何回退；例如 `gb18030_bin.rs::encode_char` 先尝试 `encoding_rs`，仍失败时输出 `?`。

大小写转换的防御边界在 `convert`：特殊区间的偏移先以 `i64` 计算，再尝试 `char::from_u32`；若表数据意外产生无效 Unicode 标量，则保留原字符而不 panic。未命中的默认映射也保证一进一出：`go_simple_upper` 对 Rust 多字符大写展开使用显式简单映射或保留原码点，`go_simple_lower` 对 `U+0130` 返回 `i`，其余多字符展开保留原字符。这是兼容 Go 单 rune 语义的刻意限制，不应改成直接收集 Rust 的完整展开。

空字符串会返回空字符串；表查询未命中返回 `HashMap::get` 的 `None`。区间查找采用首个命中项，因此区间必须保持无歧义；代码没有运行时排序或重叠检查。映射表也不自行判断 `u32` 是两字节还是四字节，调用者必须使用与大端表示匹配的转换函数。

## 并发与资源生命周期

两张哈希表由标准库 `LazyLock` 提供线程安全的一次性初始化：并发首次访问时只有一个初始化过程对外发布完成结果，避免 Go `init` 式逐项填充在 Rust 中被误写成可见的全局可变状态。正向表和反向表是两个独立的锁；只访问其中一张不会分配另一张。

初始化完成后访问是共享只读借用，生命周期为 `'static`，没有锁保护下的后续写入、后台任务、通道、文件句柄或显式清理。内存随进程存活。`GB18030_CASE` 不需要惰性初始化或同步，因为它没有字段；每次大小写转换只分配返回的 `String`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/charset/encoding_gb18030_data.go`。Go 在 `init()` 中遍历局部 `gb18030EncodingList`，填充包级 `unicodeToGB18030` 与 `gb18030ToUnicode`；Rust 把同一数据提升为公开常量切片，并以两个 `LazyLock<HashMap<...>>` 惰性构造，避免可变全局状态。`CaseRange` 与 `GB18030_CASE_RANGES` 对应 Go 的 `unicode.CaseRange` 和 `GB18030Case`，保留大写、小写、标题三项 delta。

调用接线并非完全逐行等同。Go 的 `pkg/parser/charset/encoding_gb18030.go::customGB18030Encoder.Transform` 查询正向表，`customGB18030Decoder.Transform` 查询反向表，然后才回退 `simplifiedchinese.GB18030`。当前 Rust 的 `pkg/parser/charset/encoding_gb18030.rs` 主编解码路径直接使用 `encoding_rs::GB18030`；仓库内确认的正向表生产消费者是 `pkg/util/collate/gb18030_bin.rs::encode_char`，反向表当前没有生产消费者。因此，文档不能把 Rust 双向表描述为编码器与解码器都已接线。

Go `strings.ToUpperSpecial`/`ToLowerSpecial` 在特殊表之外使用 Unicode 简单映射，每个 rune 仍映射为单个 rune；Rust `char::to_uppercase`/`to_lowercase` 可能产生多个字符。私有的 `go_simple_upper`/`go_simple_lower` 补上这一语义差异，相关独立测试覆盖 `ß`、希腊组合形式和 `U+0130`。

## 扩展指南

修改 GB18030 补充映射时，应只把 `GB18030_ENCODING_LIST` 作为源数据维护，并同时核对 Go `gb18030EncodingList`。新增条目前检查 Unicode 键和编码值都不重复，否则两张哈希表会静默覆盖且破坏双向性。至少扩展 `pkg/parser/charset/charset_1_aster_unit_test.rs::gb18030_supplemental_maps_are_bidirectional`，验证新增值的正反向查询；若映射影响真实编码/解码，还应同步 `pkg/parser/charset/encoding_gb18030_2_aster_unit_test.rs` 和 Go `pkg/parser/charset/encoding_test.go::TestEncodingGB18030` 的往返/边界用例。测试逻辑应继续放在独立测试文件，不嵌入本生产文件。

修改大小写规则时，应同步 `GB18030_CASE_RANGES` 与 Go `GB18030Case`，保留零 delta 项的屏蔽作用，并检查区间不重叠。特殊表行为可在 `charset_1_aster_unit_test.rs::gb18030_special_case_overrides_unicode_defaults` 覆盖；Go/Rust 默认映射差异则应放在 `encoding_gb18030_data_test.rs::gb18030_case_fallback_uses_go_simple_case_mapping`。若要新增标题格式 API，需要明确使用 `delta[2]`，同时实现与 Go 简单标题映射一致的回退，而不是复用完整 Unicode 展开。

性能风险主要有两处：增加映射会提高首次哈希表初始化成本和常驻内存；增加或重排大小写区间会影响每个字符的线性扫描成本。若区间规模显著增长，可以在保持“首个匹配/无重叠”语义并补充等价测试后考虑二分查找。兼容性风险包括排序键改变、已有数据比较顺序改变、Go/Rust 大小写结果分叉，以及错误地把大端编码整数当作宿主端序字节。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`query` 将 `unicode_to_gb18030`、`gb18030_to_unicode`、`gb18030_case` 定位到本文件；`explore`/调用关系确认 `encoding_gb18030.rs::{ToUpper,ToLower}`、`gb18030_bin.rs::encode_char` 及两处 Rust 测试调用。
- 源码与模块：`pkg/parser/charset/encoding_gb18030_data.rs`（2,534 行）、`pkg/parser/charset/lib.rs`、`pkg/parser/charset/Cargo.toml`、`pkg/parser/charset/encoding_gb18030.rs`、`pkg/util/collate/gb18030_bin.rs`。
- Go 对照：`pkg/parser/charset/encoding_gb18030_data.go` 与 `pkg/parser/charset/encoding_gb18030.go`；后者直接展示 Go 编码器/解码器对正反向表的查询位置。
- 测试证据：`pkg/parser/charset/encoding_gb18030_data_test.rs` 验证 Go 简单大小写回退；`pkg/parser/charset/charset_1_aster_unit_test.rs` 验证欧元双向映射、列表长度和 µ/μ 特例；`pkg/parser/charset/encoding_gb18030_2_aster_unit_test.rs` 与 Go `pkg/parser/charset/encoding_test.go::TestEncodingGB18030` 覆盖更完整的 GB18030-2022 往返及非法输入行为。
- 静态计数：`rg -c "^    \\('" pkg/parser/charset/encoding_gb18030_data.rs` 得到 2,094；`rg -c '^    CaseRange \\{' ...` 得到 58。结构验收命令见任务验证记录；按任务约束未运行 Cargo。
