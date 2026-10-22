# `pkg/util/collate/unicode_0900_ai_ci_impl.rs`

## 文件定位

`unicode0900Impl` 为生成 collator 提供四个小而稳定的策略入口：克隆、字符串预处理、字符到 UCA 权重的映射和通配符匹配器构造。

## 核心职责

- `unicode0900Impl` 为生成 collator 提供四个小而稳定的策略入口：克隆、字符串预处理、字符到 UCA 权重的映射和通配符匹配器构造。
- `convertRuneUnicodeCI0900` 把一个 Rust `char` 映射成一至两段 `u64`。每个 `u64` 可容纳最多四个 16 位 collation element；生成层逐个 16 位单元比较或编码，因此两段合计覆盖最多八个权重单元。
- `unicode0900AICIPattern` 保存由通用字符串工具编译出的 pattern 字符和 token 类型，并用同一套 UCA 9.0.0 权重判断 pattern 字符与输入字符是否等价。
- 本文件只解释字符权重与 pattern 等价性；完整字符串比较、key 编码、非法 UTF-8 字节入口和 `MaxKeyLen` 位于 `unicode_0900_ai_ci_generated.rs`。

## 主要符号

- `pub struct unicode0900Impl {}`：无字段、可 `Clone`/`Default` 的策略对象。它没有可变配置，生成 collator 通过组合而非 trait 动态分派调用它。
- `unicode0900Impl::Clone(&self) -> unicode0900Impl`：返回新的空值，供 `unicode0900AICICollator::Clone` 复制策略层。
- `unicode0900Impl::Preprocess(&self, s: &str) -> String`：原样复制输入，不裁剪尾空格。这使 0900 ai_ci 在本实现中表现为 NO PAD；`general_ci`/Unicode 4.0 实现的预处理规则不能套用到这里。
- `unicode0900Impl::GetWeight(&self, r: char) -> (u64, u64)`：生成层的权重入口，直接委托 `convertRuneUnicodeCI0900`。
- `unicode0900Impl::Pattern(&self) -> Box<dyn WildcardPattern>`：返回独立的、尚未编译的 `unicode0900AICIPattern`。
- `pub fn convertRuneUnicodeCI0900(r: char) -> (u64, u64)`：核心查表函数。短权重直接来自 `ucadata::DUCET0900Table.map_table4`；长权重由 `LongRune8` 哨兵引导到 `long_rune_map`；超表字符使用 FBC0 隐式权重。
- `pub struct unicode0900AICIPattern`：私有字段 `patChars: Vec<char>` 与 `patTypes: Vec<u8>` 分别保存编译后的字符和 pattern token 类型。
- `impl WildcardPattern for unicode0900AICIPattern`：`Compile` 调用 `stringutil::CompilePatternInner`，`DoMatch` 调用 `stringutil::DoMatchCustomized`，比较闭包要求两个字符的第一段和第二段权重都相等。

## 执行流程

常规比较与 key 生成的主链为：`collate.rs::GetCollator` → `group2_collator` → `unicode0900AICICollator` → 生成层的 `Compare`/`Key` → `unicode0900Impl::Preprocess` 和 `GetWeight` → `convertRuneUnicodeCI0900`。预处理先原样复制字符串；生成层逐字符取得 `(first, second)`，按低 16 位到高 16 位消费非零权重。比较按首个不同权重返回顺序，key 则把每个 16 位权重以高字节在前的顺序写入结果。

`convertRuneUnicodeCI0900` 有三条分支：

1. 若码点大于或等于 `map_table4.len()`，按 `high = raw >> 15`、`low = ((raw & 0x7fff) | 0x8000) << 16` 构造 `(high + 0xfbc0 + low, 0)`，为表外字符生成确定性隐式权重。
2. 否则读取 `map_table4[raw]`；若值不等于 `ucadata::LongRune8`，返回 `(first, 0)`。
3. 若命中 `LongRune8`，按码点对有序 `long_rune_map` 做二分查找并返回保存的两段权重。

LIKE/通配符主链为：`unicode0900AICICollator::Pattern` → `unicode0900Impl::Pattern` → `unicode0900AICIPattern::Compile` → `DoMatch`。pattern 的 `%`、`_` 和 escape 解析由通用 `stringutil` 完成；实际字符比较会分别调用两次 `convertRuneUnicodeCI0900`，仅当两段权重均相等才认为匹配，因此大小写或重音不同但主权重相同的字符可相互匹配。

## 数据与状态

`unicode0900Impl` 是零状态值；克隆不会复制缓存或资源。权重数据来自只读静态量 `ucadata::DUCET0900Table`：`map_table4` 是按码点直接索引的定长数组，`long_rune_map` 是 `(u32, [u64; 2])` 的有序切片。`unicode_0900_ai_ci_data_generated_2_aster_unit_test.rs` 验证当前表长为 183,969、长权重表有 27 项，并抽查 TAB、`A`、替换字符及长权重代表值。

`unicode0900AICIPattern` 是本文件唯一含运行期可变状态的类型。每次 `Compile` 都以新生成的两个向量替换旧状态；`DoMatch` 只借用这些向量。调用方必须先以所需 pattern 编译实例，再进行匹配；同一实例再次编译会覆盖此前 pattern。

权重中的零值承担“没有更多权重”的哨兵语义。数据测试 `unicode_0900_ai_ci_data_test.rs::test_first_is_not_zero` 保证所有长权重项的第一段非零，避免生成层把有效长权重误判为耗尽。

## 依赖与调用关系

- 上游模块边界：`lib.rs` 声明并重导出本模块；`unicode_0900_ai_ci_generated.rs` 是 RustCodeGraph 显示的直接使用文件，其 `unicode0900AICICollator` 调用 `Clone`、`Preprocess`、`GetWeight` 和 `Pattern`。
- 应用入口：`collate.rs::group2_collator` 在 `full_collate` feature 下为 `utf8mb4_0900_ai_ci` 构造生成 collator；`GetCollator`/`GetCollatorByID` 将名称或 ID 255 的查找带到该入口。
- 下游数据：`crate::ucadata::{DUCET0900Table, LongRune8}` 提供直接映射、长权重映射及哨兵值。权重表由 `pkg/util/collate/ucadata` 子模块公开。
- 下游算法：`crate::stringutil::CompilePatternInner` 编译 wildcard token，`DoMatchCustomized` 执行回溯/通配符匹配；本文件只注入字符等价闭包。
- crate 依赖：本文件自身仅使用同 crate 模块，不直接使用 `Cargo.toml` 中的 `dbterror`、`encoding_rs` 或 `parser_charset`。`full_collate` 是工厂是否能选到本实现的编译期开关，而不是本文件内部条件编译项。

调用关系由冻结目标源码、生成层与模块入口核验，不依赖索引的使用者计数推断。

## 错误处理与边界

这些 API 不返回 `Result`。正常查表、预处理和 pattern 匹配均为确定性内存计算；显式失败路径是长权重哨兵不满足生成数据不变量时，`binary_search_by_key(...).expect("LongRune8 sentinel must have generated weights")` 会 panic。修改生成表时必须保持每个 `LongRune8` 码点在 `long_rune_map` 中恰有对应项，且该切片继续按 key 排序，否则二分查找会失败。

表外判断现为 `raw >= map_table4.len()`，与冻结 Go 一致。有效表下标止于 `len - 1`；`raw == len` 已走隐式权重，不再索引越界。此次增量只把旧 `>` 改为 `>=`，没有改变既有表内权重、长权重查找或表外公式。

输入类型 `char` 保证是 Unicode scalar value，因此本函数不处理代理项或非法 UTF-8。原始字节的非法 UTF-8 行为由生成 collator 的 `CompareBytes`/`KeyBytes` 处理，不属于本文件。`Preprocess` 会分配一个新 `String`，即使内容不变；这是当前生成 API 的所有权边界。

## 并发与资源生命周期

`unicode0900Impl` 无字段，权重表是只读静态数据，权重转换没有锁、任务、通道、事务或外部 I/O。由此多个 collator 可以并发读取同一权重表，不存在本文件内的共享可变状态。

`WildcardPattern` trait 要求 `Send + Sync`；pattern 编译需要 `&mut self`，匹配只需 `&self`。构造出的 `Box<dyn WildcardPattern>` 由调用方拥有并负责释放，两个 `Pattern()` 调用得到彼此独立的向量状态。若要让同一 pattern 实例在编译和匹配之间跨线程共享，调用方仍须在可变编译阶段自行同步；本文件不提供内部锁。

`Preprocess` 返回的 `String`、pattern 的两个 `Vec` 以及 trait object 都遵循 Rust 所有权自动释放，没有显式清理协议或长期缓存。

## 与 Go 版本的对应关系

类型与方法一一对应：Go 空结构体对应 Rust 零字段结构体；值接收者 `Clone`/`Preprocess`/`GetWeight`/`Pattern` 对应 Rust 的共享借用方法；Go `rune` 对应 Rust `char`；两个命名返回值对应 `(u64, u64)` 元组；Go `WildcardPattern` 接口对应 `Box<dyn WildcardPattern>`。

Go 的 `LongRuneMap` 可按 rune 做 map 索引，Rust 生成数据改为有序切片并使用二分查找，这是数据表示差异，返回的两段权重语义不变。该差异要求 Rust 生成器保持排序，并使缺项由显式 `expect` 暴露。

Rust 的 `Preprocess` 需要复制成拥有所有权的 `String`，而 Go 可直接返回不可变 string 值；可观察内容一致。生成关系方面，Go 文件含 `go:generate` 指令，本文件只在注释中记录关系，Rust 生成流程不由该属性触发。

现有对齐证据包括：`general_ci_2_aster_unit_test.rs::unicode_0900_matches_go_weights_without_space_preprocessing` 验证大小写折叠、尾空格不裁剪、key 长度和重音 wildcard；`collate_test.rs::test_get_collator` 验证名称与 ID 路由；Go 的 `collate_test.go` 包含相同 collator 的比较、key 与工厂断言表。

## 扩展指南

- 调整单字符权重规则时，优先确认修改应落在 `ucadata` 生成数据/生成器还是 `convertRuneUnicodeCI0900` 的表外规则；不要在 `GetWeight` 与 pattern 闭包各写一套逻辑。
- 增加或改变长权重时，必须同步 `map_table4` 的 `LongRune8` 哨兵和有序 `long_rune_map`，并扩展独立测试 `ucadata/unicode_0900_ai_ci_data_test.rs` 或 `unicode_0900_ai_ci_data_generated_2_aster_unit_test.rs`，覆盖存在性、排序、非零首段及代表值。
- 改变 PAD/预处理语义时修改 `unicode0900Impl::Preprocess`，同时验证生成层的 `Compare`、`Key` 和 `ImmutableKey`；现有 `general_ci_2_aster_unit_test.rs` 中 0900 测试明确要求尾空格保留。
- 改变 wildcard 语义时保持测试逻辑在独立 `*_test.rs` 文件中，扩展 `general_ci_2_aster_unit_test.rs` 的 0900 pattern 场景，并与 Go `unicode_0900_ai_ci_impl.go`/相关 Go 测试核对 escape、`%`、`_` 和双段权重行为。
- 若调整表边界或 panic 策略，应保留 `unicode_0900_boundary_test.rs` 的等号边界回归，并为 `raw == map_table4.len()`、首个表外码点、普通短权重和 `LongRune8` 路径补独立回归测试；冻结 Go 同样采用 `>=`，应继续保持边界行为一致。
- 性能上该路径位于逐字符比较、key 和 LIKE 热循环。避免引入每字符分配或动态查找；生成文件说明内联会影响 20%–50% 性能。任何结构变化都应评估 `GetWeight` 的内联与长权重二分查找成本。
