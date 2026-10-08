# `pkg/util/collate/pinyin_tidb_as_cs.rs`

## 文件定位

本文件属于 `astersql-util-collate` crate，是 `utf8mb4_zh_pinyin_tidb_as_cs` 排序规则的 Rust 占位实现。crate 入口 `pkg/util/collate/lib.rs` 通过 `#[path = "pinyin_tidb_as_cs.rs"]` 声明公开模块并重新导出其符号；`pkg/util/collate/Cargo.toml` 的 `[lib]` 指向该入口，且默认 feature 包含 `full_collate`。

它位于字符串 collation 分派层，而不是 SQL 解析或执行入口。`pkg/util/collate/collate.rs` 中的 `group2_collator` 在启用 `full_collate` 时，把名称 `utf8mb4_zh_pinyin_tidb_as_cs` 映射为 `zhPinyinTiDBASCSCollator::default()`；`GetCollator`、`GetCollatorWithCollate` 和 `GetCollatorByID` 再把这一实现提供给上层表达式、编码、排序和索引逻辑。当前文件只保留接口形状，不提供拼音权重或实际排序能力。

## 核心职责

1. 定义无字段类型 `zhPinyinTiDBASCSCollator`，使 Rust 的具体类型和 Go 的同名空结构体对应。
2. 暴露 `Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`MaxKeyLen`、`Pattern`、`Clone` 等固有方法，并明确让它们全部以 `panic!("implement me")` 表示尚未实现。
3. 为该类型实现 `Collator` trait，把动态分派入口逐项转发到同名固有方法；`as_any` 是唯一能够正常返回的方法，用于运行时具体类型检查。
4. 保持“可以由注册表识别和构造，但不得作为可工作的排序规则调用”的迁移期契约。`pkg/util/collate/collate_test.rs::test_get_collator` 验证名称和 ID 2048 能得到该具体类型，同时验证支持列表不向用户暴露它；`pkg/util/collate/general_ci_2_aster_unit_test.rs::pinyin_collator_preserves_go_unimplemented_contract` 验证调用比较会 panic。

## 主要符号

- `pub struct zhPinyinTiDBASCSCollator {}`：零大小、无内部状态的占位类型，派生 `Default` 以供工厂构造。
- `zhPinyinTiDBASCSCollator::Compare(&self, &str, &str) -> i32`：预留三向比较入口；当前不返回 `-1/0/1`，而是 panic。
- `Key(&self, &str) -> Vec<u8>` 与 `ImmutableKey(&self, &str) -> Vec<u8>`：预留可变/不可变语义的排序键入口；当前均 panic，也没有缓冲复用或权重编码。
- `KeyWithoutTrimRightSpace(&self, &str) -> Vec<u8>`：预留不裁剪尾随空格的键生成入口；当前 panic，尚未定义 PAD SPACE 细节。
- `MaxKeyLen(&self, &str) -> usize`：预留排序键最大长度估算；当前 panic。
- `Pattern(&self) -> Box<dyn WildcardPattern>`：预留与该 collation 权重一致的 LIKE 通配符状态机；当前 panic。
- `Clone(&self) -> Box<dyn Collator>`：预留 trait object 克隆；当前 panic。
- `impl Collator for zhPinyinTiDBASCSCollator`：将上述固有方法接到 crate 的公共 trait。trait 的 `MaxKeyLen` 返回 `i32`，转发时把固有方法的 `usize` 结果转换为 `i32`；目前 panic 发生在转换之前。`as_any(&self) -> &dyn Any` 直接返回 `self`，可供 `downcast`/`is::<T>()` 检查。

## 执行流程

按名称取得该对象时，主流程为：上层调用 `GetCollator("utf8mb4_zh_pinyin_tidb_as_cs")` → `GetCollatorWithCollate(NewCollationEnabled(), ...)` → `new_collator` → 在 `full_collate` 构建中进入 `group2_collator` → `Box::new(zhPinyinTiDBASCSCollator::default())`。按 ID 取得时，`GetCollatorByID(2048)` 先从 charset 元数据解析规范名称，再经过同一个 `new_collator` 路径。

若全局新 collation 开关关闭，`GetCollatorWithCollate` 和 `GetCollatorByID` 不进入本文件，而是返回 `derivedBinCollator`。若构建时关闭 `full_collate`，`group2_collator` 恒返回 `None`，名称查找最终回退到 `binPaddingCollator`，所以本类型虽仍由模块导出，却不会由正常工厂路径选中。

对象一旦被选中，动态调用例如 `collator.Compare(a, b)` 会进入 `impl Collator::Compare`，再转发到固有 `zhPinyinTiDBASCSCollator::Compare`，随后立即 panic。其余六个业务方法遵循同样的“trait 方法 → 固有方法 → panic”流程；没有字符扫描、拼音转换、权重比较或 key 编码步骤。

## 数据与状态

该结构体没有字段，也不保存缓存、语言表、权重表、编译后的 pattern 或可变状态；`Default` 只构造零大小值。所有输入都以借用的 `&str` 传入，但在当前实现中参数以下划线命名且不会被读取。

文件自身没有模块级常量、静态变量或条件编译项。影响它是否能从工厂取得的状态位于外部：`collate.rs` 的 `newCollationEnabled: AtomicI32` 控制新 collation 分派，`full_collate` feature 控制 `group2_collator` 是否注册该名称。collation 名称与 ID 的映射来自 `parser_charset` 提供的 charset 元数据，不由本文件维护。

## 依赖与调用关系

直接依赖只有标准库 `std::any::Any`，以及 crate 根重新导出的 `Collator`、`WildcardPattern` trait。文件不直接使用 `Cargo.toml` 中的 `dbterror`、`encoding_rs` 或 `parser_charset`；这些依赖服务于同 crate 的注册、字符集元数据和其他实现。

RustCodeGraph 将本文件列为由 `pkg/util/collate/collate.rs`、`pkg/util/collate/collate_test.rs`、`pkg/util/collate/general_ci_2_aster_unit_test.rs` 使用。生产侧的直接调用关系集中在 `collate.rs::group2_collator` 对类型的构造；业务消费者并不直接依赖具体类型，而是通过 `Box<dyn Collator>` 调用 trait。下游目前没有算法调用边：每个业务方法都在本文件内终止于 panic，只有 `as_any` 返回 trait object 的类型视图。

## 错误处理与边界

本实现不用 `Result`，也不会返回可恢复错误。七个业务入口无条件 panic，消息固定为 `implement me`；因此任何实际比较、索引键生成、LIKE pattern 创建、克隆或长度估算都会终止当前线程，调用方不能把“能够从工厂取得对象”解释为“功能已支持”。

输入为空串、ASCII、中文、多音字、大小写差异、带音调字符、非法 UTF-8、尾随空格等边界目前都没有独立语义，因为方法会在读取输入前 panic。名称/ID 分派边界由 `collate.rs` 处理：新 collation 关闭时回退到 `derivedBinCollator`；未启用 `full_collate` 或名称没有实现时，名称查找会回退到 `binPaddingCollator`。对外展示边界由支持列表测试锁定：开发中的拼音 collation 不应出现在 `GetSupportedCollations()` 结果中。

## 并发与资源生命周期

`Collator` trait 要求实现 `Send + Sync`。本类型无字段，仅包含自动派生的零大小状态，因此没有锁、原子变量、通道、任务、事务、文件句柄或堆缓存需要协调；通过 `Box<dyn Collator>` 持有时，其生命周期只受该 Box 所有权控制。

当前 `Clone` 并不会创建新 Box，而是 panic；调用方不能依赖克隆生命周期。`Pattern` 同样不会分配或返回状态机。全局开关的并发语义属于 `collate.rs` 的 `AtomicI32`，相关测试用 `COLLATION_TEST_LOCK` 串行化状态切换，不是本文件内部机制。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/util/collate/pinyin_tidb_as_cs.go`。两端都定义无字段的 `zhPinyinTiDBASCSCollator`，并让 `Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`MaxKeyLen`、`Pattern`、`Clone` 直接 `panic("implement me")`；Rust 没有补写 Go 尚不存在的拼音算法。

接口形状存在语言差异：Go 通过方法集隐式满足 `Collator`，Rust 额外写出显式 `impl Collator`；Go 的 `MaxKeyLen` 返回 `int`，Rust 固有方法返回 `usize`，trait 适配层再转为 `i32`；Go 的接口返回值分别是 `WildcardPattern`/`Collator`，Rust 使用 `Box<dyn WildcardPattern>`/`Box<dyn Collator>` 表达动态对象；Rust 还提供 `as_any` 支持具体类型检查。

注册语义也保持迁移目标一致。Go `collate.go` 同时把名称和 ID 注册到 map，并在 `GetSupportedCollations` 中显式排除开发中的拼音规则；Rust `collate.rs::group2_collator` 在 `full_collate` 下按名称构造，按 ID 的入口先经 charset 元数据回到名称。Rust 测试对应 Go `collate_test.go::TestGetCollator`，另有独立 `should_panic` 测试补充锁定当前未实现契约。

## 扩展指南

真正实现该排序规则时，应首先以 Go 上游的同一提交或明确规范为依据，不能仅凭名称推断拼音、音调、大小写、多音字或尾随空格顺序。主要修改点是七个固有方法；trait 转发层通常无需改动。应把共享权重数据、pattern 状态机和辅助算法放在生产模块中，而把 Rust 测试继续放在独立测试文件，遵守“源文件与单元测试不放在同一文件”的仓库约束。

需要同步扩展的最小测试面包括：`general_ci_2_aster_unit_test.rs` 中当前的 panic 契约应替换为 Compare/Key/Pattern 等行为测试；`collate_test.rs` 继续验证名称、ID、feature/开关回退和支持列表策略；Go 行为已有实现时还应与 `pinyin_tidb_as_cs.go` 及 `collate_test.go` 的用例逐项对齐。至少覆盖 ASCII 大小写、重音/声调、常用与多音汉字、非汉字、组合字符、空串、尾随空格、最大 key 长度、通配符转义和 Clone 独立性。

兼容性风险在于排序权重会影响比较、ORDER BY、范围边界、哈希/排序键与索引编码；一旦已有数据按某版权重建索引，修改权重可能改变持久化顺序。性能风险包括逐字符拼音查表、key 膨胀和 pattern 匹配分配。实现时还应明确 panic 是否改为完整功能，并同步调整“开发中且不对用户展示”的支持列表契约，避免出现可选择但会崩溃的公开能力。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录与文件均已索引。
- RustCodeGraph `node --file pkg/util/collate/pinyin_tidb_as_cs.rs`：核对完整 111 行源码、零字段结构体、七个 panic 固有方法和 `Collator` 转发实现；图报告该文件由 `collate.rs`、`collate_test.rs`、`general_ci_2_aster_unit_test.rs` 使用。
- RustCodeGraph `query PinyinTiDBASCS`、`query pinyin_tidb_as_cs`：核对 Rust/Go 同名类型和方法集合。
- RustCodeGraph `node`/`explore`：核对 `collate.rs::new_collator`、两种 `group2_collator`、`GetCollatorWithCollate`、`GetCollatorByID` 的构造和 feature/开关回退路径。
- `pkg/util/collate/Cargo.toml` 与 `pkg/util/collate/lib.rs`：核对 crate 名称、默认 `full_collate` feature、依赖边界、模块公开与重新导出关系。
- `pkg/util/collate/pinyin_tidb_as_cs.go`、`pkg/util/collate/collate.go`、`pkg/util/collate/collate_test.go`：核对 Go 占位方法、名称/ID 注册、隐藏开发中规则及工厂类型断言。
- `pkg/util/collate/collate_test.rs` 与 `pkg/util/collate/general_ci_2_aster_unit_test.rs`：核对 Rust 名称/ID/开关/支持列表行为和 `should_panic(expected = "implement me")` 契约。本任务按计划只做文档分析，未运行 Cargo。
