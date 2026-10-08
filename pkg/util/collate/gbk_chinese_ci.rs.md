# `pkg/util/collate/gbk_chinese_ci.rs`

## 文件定位

本文件实现 `astersql-util-collate` crate 中的 `gbk_chinese_ci` 排序规则。crate 入口 `pkg/util/collate/lib.rs` 通过 `gbk_chinese_ci` 模块加载并公开重导出本文件的符号；`pkg/util/collate/Cargo.toml` 将该 crate 定义为默认启用 `full_collate` 的库，但本实现本身没有条件编译项，也不依赖 `full_collate`。

应用侧不会通常直接构造这里的类型，而是经 `pkg/util/collate/collate.rs::GetCollator` 或 `GetCollatorByID` 进入 `new_collator`；名称为 `"gbk_chinese_ci"` 时，工厂返回 `gbkChineseCICollator`。因此，该文件位于 SQL 字符串比较、排序键/索引键生成和 `LIKE` 通配匹配所共用的 collation 层，而非字符集编码转换层。

## 核心职责

- `gbkChineseCICollator` 实现 `Collator` trait，为字符串比较、原始字节比较、排序 key、最大 key 长度、通配 pattern 和 trait 对象克隆提供一组一致的 GBK 中文 CI 语义。
- `gbkChineseCISortKey` 把 Unicode 字符映射为 `pkg/util/collate/gbk_chinese_ci_data.rs::gbkChineseCISortKeyTable` 中的排序权重；超出 BMP（U+FFFF）的字符统一映射为 `0x3F`。
- `gbkChineseCIPattern` 编译并保存通配模式，匹配字符时以排序权重相等而不是 Unicode 标量值相等作为等价条件，所以 `a` 与 `A` 可相互匹配。
- `Key`、`Compare` 与 `Pattern` 共享同一个权重函数，维持“比较相等、key 相同、通配字符等价”三者的一致性。`pkg/util/collate/bin_1_aster_unit_test.rs::chinese_ci_weights_and_keys_match_go_tables` 直接验证了大小写等价和中英文 key。

## 主要符号

- `pub struct gbkChineseCICollator;`：无字段、可 `Default` 构造的零大小类型。其 `Collator` 实现是本文件的主要公开能力。
- `Collator::Compare(&self, a, b)`：调用 `compareCommon(a, b, gbkChineseCISortKey)`；公共实现会裁掉两端尾部 ASCII 空格、逐字符比较权重并返回 `-1/0/1`。
- `Collator::CompareBytes(&self, a, b)`：调用 `compareCommonBytes`，为可含非法 UTF-8 的 Go-string 兼容字节入口保留精确行为。
- `Collator::Key` / `KeyBytes`：先分别调用 `truncateTailingSpace` / `truncateTailingSpaceBytes`，再生成排序 key，体现该规则的 PAD SPACE 语义。
- `Collator::KeyWithoutTrimRightSpace`：逐个 Rust `char` 查权重；权重高于 `0xFF` 时按大端顺序写高、低两个字节，否则只写低字节。
- `fn key_bytes_without_trim(value: &[u8])`：`KeyBytes` 的内部字节版本。它使用 `decodeRune` 前进游标，遇到首个非法 UTF-8 字节立即返回此前已生成的 key 前缀。
- `Collator::ImmutableKey`：委托 `Key` 并返回新 `Vec<u8>`；接口上的“不可变”是调用约定，本实现没有共享或借用内部缓冲区。
- `Collator::MaxKeyLen`：按 Unicode 字符数乘二计算上界，因为单个表权重最多编码为两个字节；该值是上界，ASCII 等单字节权重的实际 key 可以更短。
- `Collator::Pattern` / `Clone` / `as_any`：分别构造独立 pattern、构造新的无状态 collator，以及支持 `Collator` trait 对象向下转型。
- `pub struct gbkChineseCIPattern`：保存 `patChars: Vec<char>` 和 `patTypes: Vec<u8>`；两者由 `CompilePatternInner` 同步生成并由 `DoMatchCustomized` 消费。
- `pub fn gbkChineseCISortKey(r: char) -> u32`：唯一公开权重查询函数。BMP 字符直接按 code point 索引静态表，非 BMP 字符返回问号权重 `0x3F`。

## 执行流程

1. 调用方通过 `GetCollator("gbk_chinese_ci")` 进入 `collate.rs::new_collator`，获得装箱的 `gbkChineseCICollator`；新 collation 开关关闭时，`GetCollatorWithCollate` 会改为返回 `derivedBinCollator`，因此本文件不会参与该分支。
2. 比较字符串时，`Compare` 把输入交给 `compareCommon`。后者转为字节切片后由 `compareCommonBytes` 去除尾部 ASCII 空格，再通过 `decodeRune` 并行读取左右字符。
3. 每对字符经 `gbkChineseCISortKey` 映射为权重；首个不等权重立即决定 `-1` 或 `1`。共同前缀结束后，以两侧未消费的字节长度差确定结果。任一侧遇到非法 UTF-8 时，兼容逻辑立即返回 `0`。
4. 生成 `Key` 时先裁尾部空格，再由 `KeyWithoutTrimRightSpace` 将每个字符的权重按一或两个字节写入新缓冲区。`KeyBytes` 的写入规则相同，但非法 UTF-8 会终止并保留已完成的前缀。
5. `Pattern` 返回空状态的 `gbkChineseCIPattern`。`Compile` 调用 `stringutil::CompilePatternInner` 将模式分解为字符和类型；`DoMatch` 调用 `DoMatchCustomized`，并用“两个字符的 GBK CI 权重相等”作为普通字符匹配谓词。

## 数据与状态

`gbkChineseCICollator` 没有实例字段，也没有缓存；所有实例行为由只读静态权重表决定。`gbkChineseCISortKeyTable` 是 `[u16; 0xFFFF + 1]`，恰好覆盖整个 BMP，函数先做 `code > 0xffff` 检查后才索引，因此合法 `char` 不会越界。

排序 key 是调用时新分配的 `Vec<u8>`。容量按输入 UTF-8 字节数的两倍预留，而 `MaxKeyLen` 按字符数的两倍报告语义上界；两者用途不同。权重使用变长的一或两字节编码，并保持高字节在前，使 key 的字节序比较与权重数值顺序一致。

`gbkChineseCIPattern` 是文件内唯一持有可变状态的类型：`Compile` 会整体替换 `patChars` 与 `patTypes`，`DoMatch` 仅只读访问。调用方必须先编译再匹配；默认实例等同于空模式状态。

## 依赖与调用关系

- 上游注册：`pkg/util/collate/collate.rs::new_collator` 在名称 `gbk_chinese_ci` 分支构造本 collator；`GetCollator`、`GetCollatorWithCollate` 和 `GetCollatorByID` 是其间接入口。RustCodeGraph 将本文件列为被 `collate.rs` 使用。
- trait 契约：`pkg/util/collate/collate.rs::{Collator, WildcardPattern}` 定义对外调用面；`as_any` 供工厂/测试等 trait 对象用户做真实类型判断。
- 比较与解码：`compareCommon`、`compareCommonBytes`、`decodeRune`、`truncateTailingSpace` 和 `truncateTailingSpaceBytes` 均来自 `collate.rs`。其中 `compareCommonBytes` 决定非法 UTF-8 和共同前缀行为，本文件只提供权重函数。
- 权重数据：`pkg/util/collate/gbk_chinese_ci_data.rs::gbkChineseCISortKeyTable` 是直接下游数据依赖；调整表内容会同时改变比较、key 和 pattern 等价关系。
- 通配引擎：`pkg/util/stringutil/string_util.rs` 通过 crate 的 `stringutil` 模块提供 `CompilePatternInner` 与 `DoMatchCustomized`。
- crate 依赖：本文件仅使用标准库 `Any` 和 crate 内部符号；`Cargo.toml` 中的 `dbterror`、`encoding_rs`、`parser_charset` 是整个 collate crate 的依赖，并非本文件的直接调用依赖。

## 错误处理与边界

本文件的公开方法不返回 `Result`，也不主动抛出业务错误。主要边界通过确定性回退处理：非 BMP 字符统一获得 `0x3F` 权重，因此不同非 BMP 字符会在此规则下比较相等并生成相同 key；这由 Go 对照实现和表驱动比较用例共同确认。

Rust `&str` 保证 UTF-8 合法，所以 `KeyWithoutTrimRightSpace` 的字符迭代不存在非法序列。为保持 Go `string` 可携带任意字节的行为，`CompareBytes` 和 `KeyBytes` 是额外的原始字节入口：前者在比较任一侧遇到首个非法字节时返回相等，后者在首个非法字节处停止并返回 key 前缀。`pkg/util/collate/collate_test.rs::test_campare_invalid_utf8_rune` 验证 `gbk_chinese_ci` 的非法字节比较和空 key 行为。

`Key` 与 `Compare` 只裁剪 ASCII 空格 `0x20`，不会裁制表符或其他 Unicode 空白。因为权重编码允许一字节或两字节，不能把 key 长度误当字符数；扩展时也不能改成固定两字节而不评估已持久化索引键兼容性。

## 并发与资源生命周期

`Collator` 和 `WildcardPattern` trait 都要求 `Send + Sync`。`gbkChineseCICollator` 无状态且只读访问静态表，可安全跨线程共享；每次 key 生成都拥有自己的 `Vec<u8>`，没有全局锁、任务、通道、文件句柄或事务资源。

pattern 对象由 `Pattern` 每次独立分配。`Compile` 需要 `&mut self`，编译完成后的 `DoMatch(&self, ...)` 可只读共享；重新编译会释放并替换原来的两个向量。`Clone` 返回新的零状态 collator，并不复制 pattern 编译状态，因为两者是不同对象和生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/collate/gbk_chinese_ci.go`。Rust 保留了 Go 的三个核心算法：`compareCommon` 权重比较、裁尾空格后按权重生成变长 key、以及 pattern 中按权重判断字符等价。`gbkChineseCISortKey` 的 BMP 查表与非 BMP 返回 `0x3F` 也逐分支一致。

语言层差异主要在字符串表示。Go 的 `KeyWithoutTrimRightSpace` 显式循环 `utf8.DecodeRuneInString` 并在非法序列处返回前缀；Rust 的同名 `&str` 方法可直接遍历合法 `char`，而兼容非法字节的逻辑被放入 Rust trait 新增的 `KeyBytes`/`CompareBytes` 路径。Rust `ImmutableKey` 委托 `Key`，Go 版本委托 `KeyWithoutTrimRightSpace(truncateTailingSpace(str))`，两者结果相同。

Rust 的 `gbkChineseCICollator` 是零大小 struct，Go 是空 struct；Rust `Box<dyn Collator>` 对应 Go interface 返回值。Rust 还实现 `as_any` 以支持 trait 对象向下转型，这是 Rust trait 工厂所需的局部接线，不改变 Go 排序语义。

相关验证位于独立文件而非生产源码内：`pkg/util/collate/bin_1_aster_unit_test.rs::chinese_ci_weights_and_keys_match_go_tables` 验证关键权重和 key；`pkg/util/collate/collate_test.rs::{test_utf8_collator_compare,test_utf8_collator_key,test_campare_invalid_utf8_rune}` 对齐 `pkg/util/collate/collate_test.go` 的表驱动比较、key、PAD SPACE、非 BMP 和非法 UTF-8 场景。

## 扩展指南

- 修改排序等价或顺序时，应优先确认是调整 `gbkChineseCISortKeyTable` 还是 `gbkChineseCISortKey` 的回退规则；同一变更必须同时满足 `Compare`、`Key` 和 `Pattern` 的一致性，不能只改某一入口。
- 修改 key 编码时，应在 `KeyWithoutTrimRightSpace` 与 `key_bytes_without_trim` 两处保持相同的高低字节规则，并同步评估索引键、排序和哈希/去重调用方的兼容性。既有 key 是外部可观察行为，格式变化可能导致新旧数据不兼容。
- 修改尾空格或非法 UTF-8 语义时，应从 `collate.rs::{compareCommonBytes,decodeRune,truncateTailingSpaceBytes}` 的共享契约入手，避免只在本文件制造比较与 key 不一致；同时检查其他复用这些辅助函数的 collator。
- 扩展 pattern 行为时，应保留 `CompilePatternInner` 生成的 `patChars`/`patTypes` 对齐不变量，并仅通过 `DoMatchCustomized` 的等价谓词注入 collation 语义。
- 测试必须继续放在独立测试文件。至少同步 `bin_1_aster_unit_test.rs::chinese_ci_weights_and_keys_match_go_tables` 和 `collate_test.rs` 中对应表项，并与 `collate_test.go` 的预期比较；新增非法字节用例应走 `CompareBytes`/`KeyBytes`，而不是尝试构造非法 `&str`。
- 性能风险集中在每次调用的分配、逐字符查表和 pattern 匹配回调。引入缓存或共享缓冲区会改变当前无状态并发模型，需要单独证明线程安全和生命周期正确性。

## 验证依据

- RustCodeGraph：`status` 显示当前仓库索引包含本文件；`files --filter pkg/util/collate` 确认模块集合；`node --file pkg/util/collate/gbk_chinese_ci.rs` 展示全部 118 行并报告直接使用者为 `pkg/util/collate/collate.rs` 与 `pkg/util/collate/bin_1_aster_unit_test.rs`；`query` 确认 Rust/Go 两侧的 `gbkChineseCICollator`、`gbkChineseCIPattern`、`gbkChineseCISortKey` 和内部 `key_bytes_without_trim` 符号。
- 生产源码：`pkg/util/collate/gbk_chinese_ci.rs`；trait、工厂及共享比较/解码逻辑见 `pkg/util/collate/collate.rs`；模块公开边界见 `pkg/util/collate/lib.rs`；权重表见 `pkg/util/collate/gbk_chinese_ci_data.rs`。
- crate 配置：`pkg/util/collate/Cargo.toml`，确认库名、入口、默认 feature 和依赖边界。
- Go 对照：`pkg/util/collate/gbk_chinese_ci.go`、`pkg/util/collate/collate.go`、`pkg/util/collate/collate_test.go`。
- Rust 独立测试：`pkg/util/collate/bin_1_aster_unit_test.rs` 与 `pkg/util/collate/collate_test.rs`。本任务是纯文档分析，按计划不运行 Cargo；上述测试仅作为现有行为证据读取。
- 交付结构检查要求：文档存在，且固定的十一个二级标题各出现一次；检查命令记录在本任务最终交付信息中。
