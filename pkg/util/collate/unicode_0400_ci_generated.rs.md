# `pkg/util/collate/unicode_0400_ci_generated.rs`

## 文件定位

本文件是 [`unicode_0400_ci_generated.rs`](unicode_0400_ci_generated.rs) 的配套说明；源文件是 `astersql-util-collate` crate 中 Unicode 4.0.0、大小写不敏感（`unicode_ci`）排序规则的生成实现。`pkg/util/collate/lib.rs` 以 `unicode_0400_ci_generated` 模块装入并重导出它；crate 的 `default = ["full_collate"]` 特性使 `pkg/util/collate/collate.rs::group2_collator` 能为 `utf8_unicode_ci` 和 `utf8mb4_unicode_ci` 构造 `unicodeCICollator`。按 ID 查找时，同一工厂路径覆盖 ID 192 和 224（见 `collate_test.rs::test_get_collator`）。关闭新 collation 开关或未启用 `full_collate` 时，工厂不会走到本实现。

文件头声明它由 `util/collate/ucaimpl` 生成；展开专用代码的目的，是让 `GetWeight`、`Preprocess` 等热路径可内联，避免在逐字符比较中引入额外多态开销。具体 Unicode 4.0 权重查询和通配符实现不在本文件，而在 `unicode_0400_ci_impl.rs` 与 `ucadata` 数据模块。

## 核心职责

- 用 `unicodeCICollator` 把 `unicode0400Impl` 封装为统一的 `Collator` trait，供 SQL 字符串比较、排序、索引 key、去重及 LIKE 模式匹配调用。
- 实现 PAD SPACE 语义：`CompareBytes`、`Key`、`KeyBytes`、`ImmutableKey` 会忽略输入尾部的 ASCII 空格；`KeyWithoutTrimRightSpace` 则明确保留尾空格。
- 将一个字符至多八个 `u16` collation element 编排为可逐段比较的两组 `u64`，并按相同顺序编码为大端两字节权重 key，使 key 的字典序与比较语义相容。
- 为 Rust 的 `&str` API 和可携带非法 UTF-8 的 Go string 语义同时提供入口：`CompareBytes`/`KeyBytes` 处理原始字节，避免 trait 默认的 lossy UTF-8 转换改变行为。

## 主要符号

- `unicodeCICollator { r#impl: unicode0400Impl }`：唯一公开类型。字段用 raw identifier 保留 Go 字段 `impl` 的含义；`Default` 构造无状态的 `unicode0400Impl`。
- 固有方法 `Clone`：调用 `unicode0400Impl::Clone`，返回新的 `Box<dyn Collator>`。
- `Compare` / `CompareBytes`：前者把合法 UTF-8 字符串转为字节切片，后者完成尾空格裁剪、UTF-8 解码、权重获取和比较。
- `Key` / `KeyBytes` / 私有 `key_bytes_without_trim`：生成 PAD SPACE 排序 key；字节入口会保留非法 UTF-8 前已经生成的 key 前缀。
- `ImmutableKey`：当前与 `Key` 的计算相同并返回独立 `Vec<u8>`，没有共享缓存或借用缓冲区。
- `KeyWithoutTrimRightSpace`：合法 `&str` 的不裁尾空格 key 生成核心。
- `Pattern`：委托 `unicode0400Impl::Pattern` 创建 Unicode 4.0 专用 `WildcardPattern`。
- `MaxKeyLen`：以 Unicode scalar 数量乘 16，给出每字符最多八个 `u16` 权重的上界。
- `impl Collator for unicodeCICollator`：逐项转发到上述固有方法，并通过 `as_any` 支持 trait object 的真实类型检查。

本文件没有模块级可变常量、枚举、条件编译项或独立后台任务；唯一私有逻辑是 `key_bytes_without_trim`。

## 执行流程

`CompareBytes(a, b)` 的流程如下：

1. 用 `truncateTailingSpaceBytes` 分别裁去两侧尾部 ASCII 空格，建立 PAD SPACE 输入。
2. 为两侧维护字节索引 `ai`/`bi`，以及当前字符的前四段权重 `an`/`bn` 和后四段权重 `a_s`/`b_s`。
3. 当前前段为零时，优先把尚未消费的后段移入前段；否则用 `decodeRune` 解出下一个字符，并调用 `unicode0400Impl::GetWeight`。零权重字符会继续被跳过。
4. 任一侧再无有效权重时，用 `sign` 比较剩余权重是否为零；两组打包值完全相等时直接消费整组。
5. 不等时从 `u64` 的低 16 位开始逐段比较；每段差值经 `sign` 规范化为 `-1/0/1`，相同则右移 16 位继续。

`Key` 和 `ImmutableKey` 先调用 `unicode0400Impl::Preprocess` 裁尾空格，再进入 `KeyWithoutTrimRightSpace`。后者逐字符取得 `(sn, ss)`，依次消费两个 `u64` 的低 16 位，并按高字节、低字节写入 `Vec<u8>`。`KeyBytes` 先做字节级尾空格裁剪，再走同样的权重编码顺序。`Pattern` 不在这里编译或匹配模式，只返回由实现层负责的状态机。

## 数据与状态

`unicodeCICollator` 只持有零字段状态的 `unicode0400Impl`，自身不保存输入、key、pattern 或全局开关。Unicode 权重来自 `ucadata::DUCET0400Table`：普通 BMP 字符的权重位于 `MapTable4`；值等于 `longRune` 哨兵时，`unicode0400Impl::GetWeight` 从 `LongRuneMap` 取得两组 `u64`；非 BMP 字符统一得到 `(0xFFFD, 0)`。

比较循环中的四个 `u64` 是瞬时状态。每个 `u64` 最多打包四个 `u16`，因此一个字符最多提供八个权重、生成 16 字节 key。零权重不输出 key 字节，也不会单独决定比较结果。返回的 `Vec<u8>` 完全归调用方所有；`ImmutableKey` 名称表达接口语义，不表示内部共享不可变存储。

## 依赖与调用关系

上游装配和选择关系为：

- `pkg/util/collate/lib.rs` 声明并重导出本模块；`Cargo.toml` 将该目录定义为 `astersql-util-collate` crate，默认开启 `full_collate`。
- `collate.rs::group2_collator` 在 `full_collate` 下把 `utf8_unicode_ci`、`utf8mb4_unicode_ci` 映射为 `unicodeCICollator::default()`；`GetCollator`、`GetCollatorWithCollate` 和 `GetCollatorByID` 是外部进入该工厂的入口。
- RustCodeGraph 的文件关系显示直接使用者包括 `collate.rs`、`collate_test.rs`、`collate_bench_test.rs` 和 `general_ci_2_aster_unit_test.rs`。更上层通常通过 `Box<dyn Collator>` 调用，不依赖具体类型。

下游调用包括：`truncateTailingSpaceBytes`、`decodeRune`、`sign`（均在 `collate.rs`），以及 `unicode0400Impl::{Clone, Preprocess, GetWeight, Pattern}`。实现层继续依赖 `ucadata::DUCET0400Table` 和 `stringutil`。本文件只使用标准库的 `Any` 与 `Vec`，不直接使用 `Cargo.toml` 中的 `dbterror`、`encoding_rs` 或 `parser_charset`。

## 错误处理与边界

这些 API 不返回 `Result`。合法输入上的比较结果始终规范化为 `-1`、`0` 或 `1`；key 生成以空 `Vec` 或已编码的权重序列表达结果。

- `CompareBytes` 遇到任一非法 UTF-8 字节立即返回 `0`。这保留生成 Go 代码对 `utf8.RuneError` 且解码长度为 1 的处理，但意味着无效字节输入不会形成全序。
- `KeyBytes` 遇到非法 UTF-8 时返回此前已生成的前缀；非法字节位于开头时结果为空。`collate_test.rs::test_campare_invalid_utf8_rune` 明确覆盖这些约束。
- 合法的 U+FFFD 由 `decodeRune` 区分为有效字符，不会误走非法字节分支。
- 只有 ASCII 空格 `0x20` 被当作 PAD SPACE 裁掉，其他空白字符不受影响。
- `unicode0400Impl::GetWeight` 在生成表出现 `longRune` 哨兵但缺少扩展权重时会 `expect` panic；这是生成数据一致性不变量，而非面向用户的可恢复错误。
- `MaxKeyLen` 基于字符数而不是 UTF-8 字节数；trait 转发将 `usize` 转为 `i32`，极端超大字符串理论上存在截断风险，当前代码没有显式饱和或报错处理。

## 并发与资源生命周期

`Collator` trait 要求 `Send + Sync`，而 `unicodeCICollator` 仅包含无状态实现，因此一个实例可以被多线程只读共享。每次比较只使用栈上索引和权重；每次生成 key 都新建并返回 `Vec<u8>`；`Clone` 返回独立装箱实例。这里没有锁、原子变量、通道、异步任务、文件句柄、事务或需要显式清理的资源。

`Pattern` 每次分配一个新的 boxed 状态机；其编译后的字符和类型数组由该 pattern 对象独占，生命周期与返回的 `Box<dyn WildcardPattern>` 一致。全局新 collation 开关位于 `collate.rs`，只影响工厂是否选择本类型，不改变已经创建的实例状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/collate/unicode_0400_ci_generated.go`。Rust 保留了 Go 的结构体、`Clone`、`Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`Pattern`、`MaxKeyLen` 以及两段 `u64` 权重循环；权重的低 `u16` 优先比较、key 的每个权重按高低字节输出也一致。

主要语言适配如下：

- Go `string` 可以包含非法 UTF-8；Rust 的 `&str` 不可以。因此 Rust 增加 trait 的 `CompareBytes`/`KeyBytes` 覆盖，并由 `decodeRune` 复刻 `utf8.DecodeRuneInString` 的非法序列判定。
- Go 的 `Compare` 直接调用 `Preprocess`；Rust `Compare` 委托 `CompareBytes`，由字节级裁尾函数达到相同 PAD SPACE 结果。合法 `&str` 上行为一致。
- Go 只有一组固有方法即满足接口；Rust 同时保留固有方法和显式 `impl Collator` 转发，以支持静态调用和动态分派。
- Rust `MaxKeyLen` 固有方法返回 `usize`，trait 入口返回 `i32`；Go 返回 `int`。
- Rust 文件增加 `as_any` 下转支持，这是 Rust trait object 工厂测试和类型判断所需的局部接线，不是 Go 生成文件中的行为。

相关 Go 测试 `collate_test.go` 验证名称/ID 工厂选择、开关关闭后的 binary 回退和非法 UTF-8 行为；Rust 的 `collate_test.rs` 对齐这些意图，`general_ci_2_aster_unit_test.rs::unicode_0400_matches_go_weights_padding_and_pattern` 额外聚焦 UCA 权重、PAD SPACE、最大 key 长度与 wildcard。

## 扩展指南

此文件标记为生成代码，正常变更应优先修改 `pkg/util/collate/ucaimpl` 的生成模板/逻辑并重新生成 Go 与 Rust 对应文件，不应只手工改一侧。扩展或修复时按影响点同步：

- 比较/权重顺序：核对 `CompareBytes` 与 `KeyWithoutTrimRightSpace` 使用完全相同的 element 顺序，否则索引 key 与运行时比较会分歧。
- 预处理或 PAD SPACE：同时检查 `Compare`、`CompareBytes`、`Key`、`KeyBytes`、`ImmutableKey`，并保持 `KeyWithoutTrimRightSpace` 的“不裁剪”契约。
- Unicode 表或 long-rune 支持：修改 `unicode_0400_ci_impl.rs`/`ucadata` 的生成数据与测试，而不是在本文件加入旁路映射。
- 新 trait 方法或返回类型：同步固有方法、`impl Collator` 转发、Go 对照和 `collate.rs` 的 trait/object 调用者。
- 通配符语义：真实实现点在 `unicode_0400_ci_impl.rs::unicodePattern`；本文件只负责构造委托。

测试逻辑必须继续放在独立测试文件。功能回归优先扩展 `collate_test.rs`（工厂、无效字节、trait 行为）和 `general_ci_2_aster_unit_test.rs`（Unicode 4.0 权重、key、PAD SPACE、pattern）；性能或内联变化参考 `collate_bench_test.rs` 的短、中、长字符串 compare/key 基准。兼容性风险集中在索引排序次序、唯一性判断、LIKE 等价类和 Go/Rust 双实现偏差；性能风险集中在逐字符虚调用、额外分配和破坏 `GetWeight` 内联。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 11,467 个文件；目标文件被识别为 22 个符号。
- RustCodeGraph `node --file pkg/util/collate/unicode_0400_ci_generated.rs`：核对了完整 241 行源码，并得到直接使用文件 `collate.rs`、`collate_bench_test.rs`、`collate_test.rs`、`general_ci_2_aster_unit_test.rs`。
- RustCodeGraph `query Unicode0400`、`query unicode_0400`：定位 `unicode0400Impl::{Clone, Preprocess, GetWeight, Pattern}`、Go/Rust 生成文件及 Unicode 数据测试。
- RustCodeGraph 文件节点：读取 `collate.rs` 的 `Collator` trait、工厂、尾空格/解码辅助函数，以及 `unicode_0400_ci_impl.rs` 的权重表和 pattern 委托实现。
- crate/模块证据：`pkg/util/collate/Cargo.toml`、`pkg/util/collate/lib.rs`。
- Go 对照：`pkg/util/collate/unicode_0400_ci_generated.go`、`pkg/util/collate/collate_test.go`。
- Rust 测试证据：`pkg/util/collate/collate_test.rs::test_get_collator`、`test_campare_invalid_utf8_rune`，`pkg/util/collate/general_ci_2_aster_unit_test.rs::unicode_0400_matches_go_weights_padding_and_pattern`，以及 `pkg/util/collate/collate_bench_test.rs` 的 Unicode CI compare/key 基准入口。
- 本任务是纯文档分析，按计划不运行 Cargo；验收采用固定章节结构检查，并人工核对上述符号、调用边、边界与扩展入口。
