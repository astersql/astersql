# `pkg/util/collate/unicode_0400_ci_impl.rs`

## 文件定位

本文件是 `astersql-util-collate` crate 中 Unicode 4.0.0、大小写不敏感排序规则的“策略内核”。crate 入口 `pkg/util/collate/lib.rs` 以 `unicode_0400_ci_impl` 模块加载并重导出它；`pkg/util/collate/Cargo.toml` 的默认 `full_collate` feature 使 `collate.rs::group2_collator` 能把 `utf8_unicode_ci` 和 `utf8mb4_unicode_ci` 映射为 `unicode_0400_ci_generated.rs::unicodeCICollator`。后者负责完整的 `Collator` 接口和权重流比较，本文件只提供尾空格预处理、单字符 UCA 4.0 权重查询以及 LIKE 通配符的等价关系。

文件不是独立入口，也不是数据生成物：权重数据来自 `ucadata::DUCET0400Table`，完整 Collator 是相邻的生成文件 `unicode_0400_ci_generated.rs`。两者分离保留了 Go 版本由 `ucaimpl` 生成展开代码、让 `GetWeight`/`Preprocess` 位于热路径上的组织方式。

## 核心职责

1. `unicode0400Impl::Preprocess` 实现 PAD SPACE 语义：比较或生成普通 Key 前移除字符串尾部空格，但不移除制表符等其他字符。
2. `unicode0400Impl::GetWeight` 把一个 Unicode scalar value 映射成最多两段 `u64`；每段又打包至多四个 16 位 collation element，供生成版 Collator 顺序消费。
3. `unicode0400Impl::Pattern` 创建独立的 `unicodePattern`，后者通过 `WildcardPattern` trait 编译和匹配 SQL LIKE 风格模式。
4. `unicodePattern::DoMatch` 以 UCA 4.0 主表权重定义 BMP 字符等价关系，从而实现大小写、重音等排序规则等价；非 BMP 和长权重字符采用严格码点相等的保守分支。

本文件不负责 Collator 注册、整串比较、排序 Key 字节编码或 Unicode 权重表生成；这些职责分别位于 `collate.rs`、`unicode_0400_ci_generated.rs` 和 `ucadata/unicode_ci_data_generated.rs`。

## 主要符号

- `longRune: u64 = 0xFFFD`：`DUCET0400Table.MapTable4` 的哨兵。主表项等于它时，真实权重不止一个 `u64`，必须再查 `LongRuneMap`。它与非 BMP 字符的回退权重数值相同，但语义由控制流区分：非 BMP 在索引主表前直接返回 `(0xFFFD, 0)`。
- `unicode0400Impl`：无字段、可 `Clone`/`Default` 的策略类型。`Clone` 返回新的空值；它没有运行期配置或缓存。
- `unicode0400Impl::Preprocess(&self, &str) -> String`：调用 crate 内的 `truncateTailingSpace`，返回拥有所有权的裁剪结果。
- `unicode0400Impl::GetWeight(&self, char) -> (u64, u64)`：BMP 普通字符直接返回主表权重和零；哨兵字符从长权重表读取两段；非 BMP 返回统一的替代权重。
- `unicode0400Impl::Pattern(&self) -> Box<dyn WildcardPattern>`：分配默认的 `unicodePattern` trait object。
- `unicodePattern { patChars, patTypes }`：保存编译后的模式字符及每个位置的模式类型。字段私有，状态只能经 `Compile` 重建、经 `DoMatch` 读取。
- `WildcardPattern for unicodePattern`：`Compile` 委托 `stringutil::CompilePatternInner`；`DoMatch` 委托 `stringutil::DoMatchCustomized` 并提供 Unicode 4.0 字符相等闭包。

本文件没有条件编译项；`#![allow(...)]` 仅允许保留与 Go API 对齐的命名。公开项是 `longRune`、`unicode0400Impl` 及其四个固有方法；`unicodePattern` 类型公开但字段私有，其行为通过公开 trait 暴露。

## 执行流程

Collator 主链从 `collate.rs::GetCollator`/`GetCollatorByID` 开始。在新 collation 开启且 `full_collate` 可用时，`group2_collator` 为 `utf8_unicode_ci` 或 `utf8mb4_unicode_ci` 构造 `unicodeCICollator`。随后有三条关键路径：

1. 比较路径：`unicodeCICollator::Compare` 进入 `CompareBytes`，先裁掉双方尾空格，再逐字符解码。每个字符交给 `unicode0400Impl::GetWeight`；生成代码先消费第一段 `u64`，必要时再消费第二段，并从低 16 位向高 16 位比较 collation element。零权重字符会被跳过，任一侧权重流耗尽时决定大小关系。
2. Key 路径：`unicodeCICollator::Key` 和 `ImmutableKey` 先调用 `Preprocess`，再进入 `KeyWithoutTrimRightSpace`。后者逐字符调用 `GetWeight`，依次将两段权重中的每个 16 位元素按高字节、低字节写入结果。显式调用 `KeyWithoutTrimRightSpace` 时不会执行本文件的裁尾空格步骤。
3. LIKE 路径：`unicodeCICollator::Pattern` 调用本文件的 `Pattern` 得到空匹配器；调用方先以 `Compile` 解析模式，再用 `DoMatch` 匹配字符串。通用匹配器处理普通字符、单字符通配符、任意长度通配符和转义，本文件提供普通字符是否按此 collation 等价的判定。

`GetWeight` 的内部顺序是重要边界：先用 `char as usize` 判断是否超过 `0xFFFF`，避免越界索引固定长度 65,536 的主表；BMP 字符再读取 `MapTable4[idx]`。若值为 `longRune`，调用 `DUCET0400Table.long_rune_weight` 二分查找有序长权重表并取两个元素；否则第二段恒为零。

`DoMatch` 的比较闭包同样先排除非 BMP。两个 BMP 字符的主表权重不同即不等；普通权重相同即相等；如果共同权重是 `longRune`，则要求原字符相同，而不进一步比较长权重数组。这一分支与 Go 实现一致，不能直接改成“长权重相同即等价”。

## 数据与状态

`unicode0400Impl` 是零大小、无状态对象。`unicodePattern` 是唯一可变状态：`patChars: Vec<char>` 和 `patTypes: Vec<u8>` 必须来自同一次 `CompilePatternInner` 调用，位置一一对应；再次 `Compile` 会整体替换两者，不保留上一次模式。

UCA 数据是只读静态值 `ucadata::DUCET0400Table: UcaDataTable<65536>`。`MapTable4` 允许按 BMP 码点 O(1) 查表；`LongRuneMap` 按码点严格递增，`long_rune_weight` 用二分查找取得 `[u64; 2]`。相关数据测试验证主表边界、`A`/`a` 同权、代表性长权重以及长权重键和权重唯一性。

权重的零值表示当前段没有更多 collation element；生成版比较和 Key 逻辑依赖这一约定。非 BMP 统一得到 `0xFFFD`，因此在排序比较/Key 中多个非 BMP 字符可能折叠为同一权重；在通配符匹配中则明确要求码点相同。测试 `collate_test.rs::test_utf8_collator_compare` 和 `test_utf8_collator_key` 固化了这一区别。

## 依赖与调用关系

上游直接关系：

- `unicode_0400_ci_generated.rs::unicodeCICollator` 内嵌 `unicode0400Impl`；其 `Clone`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace` 和 `Pattern` 直接调用本文件的方法，`CompareBytes`/`key_bytes_without_trim` 也直接调用 `GetWeight`。
- `collate.rs::group2_collator` 在 `full_collate` 下构造 `unicodeCICollator`；`GetCollator` 和 `GetCollatorByID` 是应用侧取得该实现的工厂入口。
- `collate.rs::Collator::Pattern` 向更上层暴露 `Box<dyn WildcardPattern>`，因此表达式 LIKE 等使用者不需要知道 `unicodePattern` 的具体类型。

下游直接关系：

- crate 根导入的 `stringutil::truncateTailingSpace` 提供 PAD SPACE 预处理。
- `stringutil::CompilePatternInner` 产生模式字符/类型数组，`stringutil::DoMatchCustomized` 执行通用通配符状态机。
- `ucadata::DUCET0400Table.MapTable4` 提供 BMP 主权重，`UcaDataTable::long_rune_weight` 提供长权重；RustCodeGraph 对 `GetWeight` 识别出的明确调用边即指向后者。

crate 清单的直接依赖为 `dbterror`、`encoding_rs` 和 `parser_charset`；本文件本身不直接引用它们。其实际编译边界来自同 crate 的模块重导出和 `ucadata`/`stringutil` 子模块，而不是额外外部依赖。

## 错误处理与边界

本文件没有 `Result` 返回值，也不产生业务错误。边界行为如下：

- `GetWeight` 对 `U+10000` 及以上字符返回 `(0xFFFD, 0)`，不会索引 BMP 表。
- BMP 主表出现 `longRune` 时，代码以 `expect("longRune sentinel must have generated weights")` 强制“每个哨兵都有生成的长权重”这一数据不变量；若生成表损坏会 panic。数据一致性测试是防止该 panic 的主要保护。
- `Preprocess` 只处理尾随空格；空串以及全空格串均可得到空结果，其他尾随字符不应被误删。
- `DoMatch` 对非 BMP 字符仅接受码点完全相同；对两个命中 `longRune` 的 BMP 字符也仅接受原字符相同。普通 BMP 字符按主表权重相等判断。
- `Compile` 没有独立报错通道，转义和通配符边界由 `CompilePatternInner`/`DoMatchCustomized` 的通用契约决定。
- Rust 的 `&str` 保证有效 UTF-8；Go `string` 的无效 UTF-8 行为由生成 Collator 的 Rust 字节入口 `CompareBytes`/`KeyBytes` 补齐，不属于本文件方法的输入域。

## 并发与资源生命周期

没有全局可变状态、锁、任务、通道、事务或 I/O。`DUCET0400Table` 是进程期只读静态数据，可被所有线程共享。`unicode0400Impl` 无状态且派生 `Clone`；`WildcardPattern` trait 要求 `Send + Sync`，编译完成后的 `unicodePattern` 可只读共享。

模式生命周期为“构造 → 可变借用执行 `Compile` → 只读借用多次 `DoMatch`”。同一个实例若要并发重新编译，调用方仍需外部同步，因为 `Compile` 需要 `&mut self`；通常每次 `Pattern` 都新分配独立对象，避免跨请求共享可变编译状态。对象销毁时两个 `Vec` 和 trait object 由 Rust 所有权自动释放，没有显式清理步骤。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/collate/unicode_0400_ci_impl.go`，结构和分支基本逐项对应：Go 的空 `unicode0400Impl`、`longRune`、`Clone`、`Preprocess`、`GetWeight`、`Pattern`、`unicodePattern.Compile/DoMatch` 在 Rust 中均保留。Go `rune` 对超 BMP 的判断对应 Rust `char as usize`；Go 对 `LongRuneMap[r]` 的 map 索引对应 Rust 有序切片的二分查找。

可见的语言适配有三点：

1. Go `Pattern` 返回接口并取 `&unicodePattern{}`，Rust 返回 `Box<dyn WildcardPattern>`。
2. Go 的 `[]rune`/`[]byte` 状态分别成为 `Vec<char>`/`Vec<u8>`。
3. Go 对长权重 map 的缺失会在后续索引时触发运行期失败；Rust 用带不变量说明的 `expect` 显式失败。

完整算法的 Go 对照还包括 `unicode_0400_ci_generated.go`：它同样内嵌实现、按两段 `u64` 消费权重、Key 前裁尾空格并将每个 16 位权重写为两个字节。Rust 生成文件额外提供 `CompareBytes`/`KeyBytes` 以表达 Go 字符串可包含任意字节的语义，但不改变本文件的权重规则。Go 的 `collate_test.go` 与 Rust 的 `collate_test.rs` 使用对应表驱动用例验证比较、Key、工厂映射和无效 UTF-8 边界。

## 扩展指南

- 调整 Unicode 4.0 权重或生成格式时，应优先修改 `ucadata` 生成器及数据，而不是在 `GetWeight` 中添加例外；同时同步 `ucadata/unicode_ci_data_generated_3_aster_unit_test.rs`、`unicode_ci_data_test.rs` 及其 Go 对照。必须保持 `MapTable4` 长度、`longRune` 哨兵与 `LongRuneMap` 完整性一致。
- 修改 PAD SPACE 规则时，入口是 `Preprocess`，但还要核对生成文件的 `CompareBytes`/`KeyBytes` 是否绕过或等价实现了预处理，并扩展 `collate_test.rs` 的尾空格、全空格和非空格尾字符用例。
- 修改通配符等价关系时，入口是 `unicodePattern::DoMatch` 的比较闭包；通用 `%`、`_`、escape 解析应留在 `stringutil`。应在独立测试文件中扩展 `general_ci_2_aster_unit_test.rs::unicode_0400_matches_go_weights_padding_and_pattern`，覆盖普通同权字符、非 BMP、不同长权重字符和相同长权重字符，不能把测试内嵌进生产源文件。
- 新增 Unicode 版本不应复用本类型后偷偷切换表；应参照 0900 实现建立独立策略、生成 Collator、工厂名称/ID 接线和独立测试，避免改变既有 `utf8*_unicode_ci` 的持久化排序语义。
- 热路径修改要保留生成代码所强调的内联/展开意图，并评估 `GetWeight` 每字符查表及长权重二分查找的性能。排序权重或 Key 字节的改变会影响索引顺序、等值判断和存量 Key 兼容性，必须与 Go 行为和 MySQL collation 契约一起验证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标文件被索引且报告 9 个符号；`node --file pkg/util/collate/unicode_0400_ci_impl.rs` 核对了全部 119 行；`query Unicode0400`/`query unicode_0400` 定位实现、生成 Collator 和 UCA 测试；`callees` 明确识别 `GetWeight -> ucadata/unicode_ci_data_generated.rs::long_rune_weight`。同名方法较多导致 `callers/callees` 消歧结果含噪声，因此上游边另以生成文件源码和引用搜索交叉确认。
- Rust 源与配置：`pkg/util/collate/Cargo.toml`、`lib.rs`、`collate.rs`、`unicode_0400_ci_generated.rs`、`ucadata/unicode_ci_data_generated.rs`。
- Go 对照：`pkg/util/collate/unicode_0400_ci_impl.go`、`unicode_0400_ci_generated.go`、`collate_test.go`、`ucadata/unicode_ci_data_test.go`。
- 独立 Rust 测试：`pkg/util/collate/collate_test.rs` 覆盖工厂接线、比较、Key、PAD SPACE、非 BMP 和无效 UTF-8；`general_ci_2_aster_unit_test.rs` 直接覆盖 Unicode 4.0 权重与通配符；`ucadata/unicode_ci_data_generated_3_aster_unit_test.rs` 与 `ucadata/unicode_ci_data_test.rs` 覆盖表边界、长权重查找和 Go 原始数据一致性。
- 本任务是只读逻辑分析加 Markdown 文档，不修改运行时代码，按计划不运行 Cargo。结构验收使用任务文件给定的 11 章节命令；人工复核重点是文件存在理由、三条运行路径、数据不变量、Go 差异和安全扩展位置均有直接证据。
