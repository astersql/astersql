# `pkg/util/collate/unicode_0900_ai_ci_generated.rs`

## 文件定位

本文件是 `astersql-util-collate` crate 中 Unicode 9.0.0、accent-insensitive/case-insensitive（`utf8mb4_0900_ai_ci`）排序器的生成代码移植。crate 入口 `pkg/util/collate/lib.rs` 将它声明为 `unicode_0900_ai_ci_generated` 模块并公开重导出；`pkg/util/collate/Cargo.toml` 的默认特性包含 `full_collate`，因此 `pkg/util/collate/collate.rs::group2_collator` 在默认构建中可按名称构造 `unicode0900AICICollator`。该文件负责通用 `Collator` 接口与 Unicode 9.0.0 权重实现之间的高频、可内联适配；具体字符到权重的映射和通配符匹配器位于 `unicode_0900_ai_ci_impl.rs`，权重数据位于 `ucadata` 子 crate/模块。

源码声明其由 `util/collate/ucaimpl` 生成，原因是避免在 `GetWeight`、`Preprocess` 的热循环上引入额外多态分派。Rust 文件当前同时保留 PingCAP Apache License 和 `// Copyright 2026 AsterSQL.` 标记；维护时应把它视为生成产物，而不是独立手改的算法源。

## 核心职责

- `unicode0900AICICollator` 持有一个 `unicode0900Impl`，把 `Collator` trait 的比较、排序 key、通配符、克隆和最大 key 长度能力公开给调用方。
- `Compare`/`CompareBytes` 将输入逐 Unicode 标量解码，经 `unicode0900Impl::GetWeight` 映射为最多两段 `u64`。每段从低 16 位到高 16 位逐权重比较，返回 `-1`、`0` 或 `1`。
- `Key`/`ImmutableKey`/`KeyWithoutTrimRightSpace`/`KeyBytes` 将每个非零 16 位权重按高字节在前写入 `Vec<u8>`，使 key 的字节序与比较所用权重顺序一致。
- `Pattern` 不在生成层重复实现 LIKE 逻辑，而是委托 `unicode0900Impl::Pattern` 创建按相同两段权重判断字符等价的匹配器。
- `MaxKeyLen` 按 Unicode 标量数乘 16 给出上界：单字符最多两段 `u64`，即八个 `u16`、16 字节。

该排序器的 `Preprocess` 是 no-op，所以与会裁剪尾空格的 PAD SPACE 排序器不同：`"a"` 与 `"a "` 的比较和 key 均不相等。`general_ci_2_aster_unit_test.rs::unicode_0900_matches_go_weights_without_space_preprocessing` 明确覆盖了这个契约。

## 主要符号

- `pub struct unicode0900AICICollator { impl_: unicode0900Impl }`：唯一公开类型。`Default` 创建无状态的 `unicode0900Impl`；字段保持私有，外部通过 `Collator` 或固有方法使用。
- `impl Collator for unicode0900AICICollator`：trait 适配层。各方法转发到同名固有方法；`as_any` 支持工厂测试和运行时具体类型检查；trait 的 `MaxKeyLen` 将固有方法的 `usize` 转为 `i32`。
- `Clone`：调用 `unicode0900Impl::Clone` 构造独立 boxed 排序器。当前 impl 是空结构体，因此没有共享可变状态。
- `Compare(&str, &str)`：先对两侧调用 `Preprocess`，再进入 `CompareBytes`。当前预处理复制字符串但不改变内容。
- `CompareBytes(&[u8], &[u8])`：允许表达 Go `string` 可包含的原始无效 UTF-8，是实际比较循环。`an/bn` 保存第一段权重，`as_/bs` 保存第二段权重，`ai/bi` 保存字节游标。
- `Key` 与 `ImmutableKey`：当前具有相同实现，均为 `Preprocess` 后调用 `KeyWithoutTrimRightSpace`；名称差异保留 `Collator` 契约，而不是缓冲区共享差异，两者都返回新 `Vec<u8>`。
- `KeyBytes`：原始字节入口，遇到无效 UTF-8 时返回此前已生成的 key 前缀。
- `KeyWithoutTrimRightSpace`：从合法 `&str` 生成完整 key；名称明确保证本层不裁剪尾空格。
- `Pattern`：返回 `Box<dyn WildcardPattern>`，实际对象是 `unicode_0900_ai_ci_impl.rs::unicode0900AICIPattern`。
- `MaxKeyLen`：使用 `s.chars().count() * 16`，计算字符数而不是 UTF-8 字节数。
- 私有 `decode_next_rune`：按 UTF-8 字节下标推进合法 `&str`；为与 Go 生成结构对齐保留 `invalid` 返回位，但对安全 Rust `&str` 该位恒为 `false`。
- 私有 `sign`：把有符号差值规约为 `-1/0/1`。

文件没有 trait 定义、枚举、模块级数据常量、条件编译分支或内部锁；三个 crate 级 `allow` 仅容纳 Go 风格命名和生成代码形态。

## 执行流程

工厂入口首先由 `GetCollator`/`GetCollatorByID` 根据全局新 collation 开关选择真实排序器；新 collation 开启且默认 `full_collate` 可用时，名称 `utf8mb4_0900_ai_ci`（ID 255）落到 `group2_collator` 并构造本类型。关闭开关时工厂返回 `derivedBinCollator`，不会进入本文件。

比较路径如下：

1. `Compare` 分别调用 `unicode0900Impl::Preprocess`；0900 实现原样返回输入内容。
2. `CompareBytes` 在某侧当前权重为零时调用 `decodeRune` 读取下一个标量，并调用 `GetWeight`。零权重字符会继续读取，因而在主级权重比较中被忽略。
3. `GetWeight` 最终进入 `convertRuneUnicodeCI0900`：普通字符查 `DUCET0900Table.map_table4`；`LongRune8` 哨兵从 `long_rune_map` 取得两段权重；表外字符使用 FBC0 区间的合成权重。
4. 第一段耗尽后把第二段提升为当前段。若一侧已无权重，`sign(an - bn)` 决定顺序；两段相等则读取后续字符。
5. 当两侧 `u64` 不同，循环从最低 16 位开始比较；首个不同权重经 `sign` 返回。两侧所有有效权重均耗尽则返回 0。

key 路径先执行同样的预处理和权重查找，然后把第一段、第二段各自从低 16 位向高 16 位遍历；每个 `u16` 以大端两个字节追加。由此字节序 key 可复现比较中的权重先后。`Pattern` 路径则由 impl 层编译 `%`、`_` 和转义状态，并以 `convertRuneUnicodeCI0900(a) == convertRuneUnicodeCI0900(b)` 作为字符等价条件。

## 数据与状态

`unicode0900AICICollator` 只包含零字段状态的 `unicode0900Impl`，没有缓存、配置或引用生命周期。一次比较的全部状态都在栈上：两侧字节游标与四个 `u64` 权重槽；一次 key 生成只拥有局部 `Vec<u8>` 和当前权重。返回的 `Vec<u8>` 与 collator 生命周期无关。

重要不变量是：每个字符最多贡献两个 `u64`；每个 `u64` 按四个低位优先的 `u16` 消费；key 对每个 `u16` 以大端写出；值为零的尾部权重不写入。`MaxKeyLen` 的每字符 16 字节正好覆盖两段权重的最大展开量。`Vec` 初始容量使用输入字节数的两倍，这只是减少常见路径扩容的启发式值，不是长度上限，长权重仍可触发安全扩容。

大小写和重音折叠不是本文件中的条件分支，而是 `DUCET0900Table` 与 `convertRuneUnicodeCI0900` 的数据语义；本文件只消费映射结果。尾空格也没有特殊状态或裁剪步骤，因此空格权重被正常保留。

## 依赖与调用关系

上游直接接线位于 `pkg/util/collate/collate.rs`：`group2_collator` 为 `utf8mb4_0900_ai_ci` 构造该类型，`GetCollator` 和 `GetCollatorByID` 再向 SQL 类型、表达式、索引 key、排序或 LIKE 等 crate 用户提供 `Box<dyn Collator>`。`pkg/util/collate/lib.rs` 声明并重导出本模块。RustCodeGraph 将目标文件识别为 26 个符号，并确认结构体内部由 `Clone` 实例化；索引对本生成文件的固有方法未生成可精确寻址的独立调用节点，因此具体工厂边由源码搜索补证。

下游依赖只有标准库 `Any` 和 crate 内接口/实现：`Collator`、`WildcardPattern`、`decodeRune` 以及 `unicode0900Impl`。后者继续依赖 `ucadata::DUCET0900Table`、`LongRune8` 与 `stringutil` 的 pattern 编译/匹配函数。本文件自身不直接使用 `Cargo.toml` 中的 `dbterror`、`encoding_rs`、`parser_charset`，这些是同 crate 其他模块的依赖。

测试调用包括 `collate_test.rs` 的名称/ID 工厂选择和无效 UTF-8 原始字节行为、`general_ci_2_aster_unit_test.rs` 的大小写/重音/尾空格/key/pattern/上界行为，以及 `collate_bench_test.rs` 的短、中、长输入比较与 key 基准。Go 侧对应入口和测试分别在 `collate.go`、`collate_test.go` 与 `collate_bench_test.go`。

## 错误处理与边界

API 不返回 `Result`。合法 `&str` 路径不能产生 UTF-8 解码错误；`decode_next_rune` 的 `invalid` 分支是为保持 Go 生成算法形状而保留的不可达兼容分支。原始字节路径通过 crate 的 `decodeRune` 显式处理无效 UTF-8：`CompareBytes` 一旦任一侧遇到无效序列便返回 0，`KeyBytes` 返回无效字节之前已生成的前缀。这会形成非严格的比较结果，但刻意对齐 Go 现有契约；`collate_test.rs::test_campare_invalid_utf8_rune` 覆盖了这些边界。

空串或所有字符均映射为零权重时，比较在双方权重耗尽后返回 0，key 为空。单侧尚有非零权重时，耗尽分支按剩余当前权重的符号决定顺序。有效的 U+FFFD 与无效 UTF-8 必须区分：前者是合法标量并正常查权重，后者由 `decodeRune` 的无效标记提前终止。

`unicode_0900_ai_ci_impl.rs::convertRuneUnicodeCI0900` 对 `LongRune8` 哨兵要求生成表必有对应长权重；缺失时会以 `expect("LongRune8 sentinel must have generated weights")` panic。这是生成数据完整性不变量，而不是用户输入错误。`MaxKeyLen` 的 trait 返回值存在 `usize` 到 `i32` 的转换；极端大字符串理论上可能截断，但实际分配和平台限制通常先成为约束，修改该接口时仍应专门验证溢出语义。

## 并发与资源生命周期

`Collator` 要求 `Send + Sync`，本类型只包含可克隆的空 impl，因此可在线程间安全共享。比较和 key 生成不访问全局可变状态，不持锁、不启动任务、不使用通道，也没有事务或 I/O。全局“是否启用新 collation”的原子开关属于 `collate.rs` 工厂层，只决定是否构造本类型，不参与实例方法执行。

`Clone` 分配一个新的 `Box<unicode0900AICICollator>`；`Pattern` 分配独立的 boxed pattern 状态；key 方法为每次调用分配并独占 `Vec<u8>`。这些资源在所有者离开作用域时由 Rust 自动释放。权重表是只读静态数据，实例不会复制它。性能敏感点是逐字符查表、可能的长权重二分查找和 key 缓冲扩容；生成代码保留静态调用形态是为了让编译器内联热路径。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/collate/unicode_0900_ai_ci_generated.go`。结构体、`Clone`、`Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`Pattern`、`MaxKeyLen` 以及两段权重循环均逐项对应。Go 的 `unicode_0900_ai_ci_impl.go` 对应 Rust 的 `unicode_0900_ai_ci_impl.rs`，两者都把预处理定义为 no-op，并使用同一 DUCET 0900 权重策略。

需要明确的语言边界差异有三项：

1. Go `string` 可携带任意字节，生成代码直接调用 `utf8.DecodeRuneInString`；Rust 的常规 API 使用合法 `&str`，另增 `CompareBytes`/`KeyBytes` 承接 Go 的原始字节语义。
2. Go `ImmutableKey`/`Key` 返回 `[]byte`，Rust 两者都返回新拥有的 `Vec<u8>`；当前并不存在只读借用或共享缓冲优化。
3. Go 以 `int` 做差和 rune 计数，Rust 用 `i64` 中间差值、`usize` 固有上界并在 trait 边界转 `i32`；正常 Unicode 权重与可分配字符串范围内结果一致。

`ucaimpl/migration_aster_unit_test.rs::generates_unicode_0900_source_identical_to_go_generator` 验证 Rust 生成器产生的 Go 文件与仓库 Go 参考产物全文一致；它验证的是生成模板的 Go 输出，不表示 Rust 文件本身由测试逐字生成。行为对照还由 Rust `general_ci_2_aster_unit_test.rs`、`collate_test.rs` 与 Go `collate_test.go` 共同覆盖。

## 扩展指南

调整排序语义时先判断修改层级：字符权重或 pattern 字符等价规则应改 `unicode_0900_ai_ci_impl.rs` 及 `ucadata` 数据/生成器；比较和 key 展开算法应改 `ucaimpl` 生成模板，再同步生成产物，避免只手改本文件后被再生成覆盖；名称、ID、feature 可见性则改 `collate.rs`/`Cargo.toml` 接线。

任何权重布局变化都必须同时保持三个不变量：`Compare` 的首个差异顺序与 key 的字典序一致；最多两段 `u64` 时 `MaxKeyLen == rune_count * 16` 仍是安全上界；`LongRune8` 的表项与长权重映射完整一致。若增加第三段权重，当前四槽状态机、两个 key 展开循环和最大长度公式都必须一起修改，不能只扩数据表。

测试必须放在独立文件而非生产 `.rs` 内。优先扩展 `general_ci_2_aster_unit_test.rs` 验证具体 Unicode 等价类、尾空格、key 和 pattern；扩展 `collate_test.rs` 验证工厂/ID、trait object 与无效字节；权重表边界放入 `ucadata/unicode_0900_ai_ci_data_test.rs`；生成模板变化同步扩展 `ucaimpl/migration_aster_unit_test.rs`；性能形态变化用 `collate_bench_test.rs` 与 Go 基准对照。兼容风险集中在索引 key 字节变化（可能要求重建索引）、排序/唯一性语义变化和 LIKE 等价关系变化；性能风险集中在破坏内联、增加每字符分配或查表次数。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件且目标目录已索引；`files --filter pkg/util/collate` 确认目标及其 impl、测试、生成器文件；`node --file ... --offset 1 --limit 500` 读取目标完整 275 行；`query unicode0900AICICollator --json` 定位 Rust/Go 同名结构体；`node unicode_0900_ai_ci_generated.rs::unicode0900AICICollator` 确认结构体定义与 `Clone` 实例化边。通用名称的 `explore` 结果噪声较大，固有方法也未形成精确节点，因此调用边另由下列源码搜索核验。
- 生产源码：`pkg/util/collate/unicode_0900_ai_ci_generated.rs`（全部符号与算法）、`unicode_0900_ai_ci_impl.rs`（预处理、查权重、pattern）、`collate.rs`（trait、名称工厂与 feature 门控）、`lib.rs`（模块边界和重导出）、`ucadata/unicode_0900_ai_ci_data_generated.rs`（实际表来源，通过 impl 间接引用）。
- crate 配置：`pkg/util/collate/Cargo.toml`，确认 crate 名、`lib.rs` 入口、默认 `full_collate` 特性、依赖与 Go package 元数据。
- Go 对照：`pkg/util/collate/unicode_0900_ai_ci_generated.go`、`unicode_0900_ai_ci_impl.go`、`collate.go`，确认生成算法、权重委托和工厂注册。
- 独立测试：`general_ci_2_aster_unit_test.rs::unicode_0900_matches_go_weights_without_space_preprocessing`，`collate_test.rs::test_get_collator` 与 `test_campare_invalid_utf8_rune`，`ucaimpl/migration_aster_unit_test.rs::generates_unicode_0900_source_identical_to_go_generator`；Go 侧对照为 `collate_test.go::TestGetCollator`、`TestCampareInvalidUTF8Rune`。`collate_bench_test.rs`/`.go` 提供热路径性能入口。
- 本任务是纯文档分析，按总计划不运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题；交付前还人工检查仅新增本说明、未修改 Rust/Go/Cargo/`plan.md`，且文档没有建议把 Rust 测试内嵌进生产文件。
