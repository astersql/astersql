# `pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs`

## 文件定位

本文件是 `astersql-util-collate-ucadata` crate 中的 Unicode Collation Algorithm（UCA）9.0.0 默认排序元素表（DUCET）生成产物。crate 入口 `pkg/util/collate/ucadata/lib.rs` 将它声明为 `unicode_0900_ai_ci_data_generated` 模块，并通过 glob re-export 暴露其公共项；所属 crate 由 `pkg/util/collate/ucadata/Cargo.toml` 定义，未声明额外 feature 或第三方依赖。

它不实现排序、比较或字符转换算法，只把生成器根据 Unicode 9.0.0 `allkeys.txt` 计算出的结果固化为只读 Rust 数据。直接运行时消费者是 `pkg/util/collate/unicode_0900_ai_ci_impl.rs` 的 `convertRuneUnicodeCI0900`，后者为 `unicode0900Impl::GetWeight` 和 `unicode0900AICIPattern::DoMatch` 提供字符权重，因此该表间接参与 `utf8mb4_0900_ai_ci` 风格的权重查询和通配符等值判断。

源文件头明确标记 `Code generated ... DO NOT EDIT`；权重或布局变化应从 `pkg/util/collate/ucadata/generator/main.rs`、`generator/data_0900.rs.tpl` 和输入 `generator/allkeys-9.0.0.txt` 发起，而不是手改这 184031 行生成数据。

## 核心职责

文件只承担三项数据契约：

1. 用 `UcaDataTable<const N: usize>` 固定主表与超长权重表的 Rust 表示。
2. 用 `DUCET0900Table` 保存码点 `0..=0x2CEA0` 的 183969 个主表槽位；数组下标就是 Unicode 码点。
3. 对无法装入一个 `u64` 的排序元素序列，在主表写入 `data.rs` 定义的 `LongRune8`（`0xFFFD`）哨兵，并在 `long_rune_map` 保存两段 `u64` 权重。

该文件是数据而非执行逻辑。解析 `allkeys`、补齐隐式权重、Hangul 分解、特殊码点处理、格式化和写文件均在生成器中完成；运行时文件只被读取。

## 主要符号

- `pub struct UcaDataTable<const N: usize>`：生成表容器。const generic `N` 把主表长度编码进类型，避免运行时携带可变长度元数据。
- `UcaDataTable::map_table4: [u64; N]`：按码点直接寻址的主表。一个 `u64` 可按 16 位单元打包最多四个主要排序权重；值为 `LongRune8` 时必须改查长表。
- `UcaDataTable::long_rune_map: &'static [(u32, [u64; 2])]`：27 项静态切片；每项是码点及最多八个 16 位权重所占的两段 `u64`。源码按码点严格递增排列，满足消费者 `binary_search_by_key` 的前置条件。
- `pub static DUCET0900Table: UcaDataTable<183969>`：本文件唯一表实例。主表末端对应生成器使用的独占上界 `0x2CEA1`；长表包含 U+321D、U+FDFA、U+FFFD、U+1F1A9 等代表项。

文件没有函数、trait、`impl`、可变静态量或条件编译项。两个符号均为公开 API；其字段也公开，以便 collator 和独立测试直接读取。

## 执行流程

生成阶段与运行阶段彼此分离：

1. `generator/main.rs::selectOutputTarget` 根据输出 basename `unicode_0900_ai_ci_data_generated.rs` 选择 `OutputTarget::Rust0900`。
2. `buildTable(unicodeVersion::unicode0900)` 以长度 `0x2CEA1` 解析内嵌的 Unicode 9.0.0 allkeys 数据，命名为 `DUCET0900Table`，再由 `calcImplicitWeight` 补齐未显式列出的码点。
3. 生成器将最多四个非零 16 位权重打包进 `MapTable4`；更长序列把主表项设为 `LongRune8`，并把两段权重放入有序映射。`generateRustFile` 使用 `data_0900.rs.tpl` 产生本文件。
4. 运行时 `convertRuneUnicodeCI0900(r)` 先将 `char` 转为 `u32`，以其数值索引 `DUCET0900Table.map_table4`。
5. 普通表项直接作为第一段权重返回，第二段为零；若主表值等于 `LongRune8`，则在 `long_rune_map` 中按码点二分查找并返回完整的两段权重。消费者以 `expect` 表达“哨兵必有长表项”的生成数据不变量。
6. 消费者意图对超出生成表覆盖范围的码点现场计算 FBC0 区间的兜底隐式权重，不由本文件保存；但当前边界判断使用 `raw as usize > len()`，码点恰等于 `len()` 时仍会进入数组索引并越界，属于既有实现边界，不能把它描述为已安全覆盖。

## 数据与状态

`DUCET0900Table` 全部数据在编译期静态初始化，生命周期为 `'static`，运行时没有初始化步骤、缓存填充或状态转换。`map_table4` 是定长内联数组，空间开销固定为 `183969 * 8` 字节（不计类型对齐外的其他内容）；长表是指向静态 27 项切片的引用。

主表的零值表示生成数据中没有普通权重的槽位；非零值可能含一至四个按 16 位打包的权重，也可能是 `LongRune8` 哨兵。不能只凭数值 `0xFFFD` 区分“替换字符本身的权重”和“长序列标记”：生成器为 U+FFFD 显式加入长表项 `[0xFFFD, 0]`，因此消费者仍通过同一哨兵路径得到正确结果。

长表排序是运行时二分查找的语义要求，而不仅是便于阅读的格式。其权重首段必须非零，不同码点的两段组合必须唯一；相关 Rust/Go 测试分别验证这些不变量。表中数值是排序协议数据，任何变化都可能改变索引、比较、LIKE 匹配或持久化排序键的兼容表现。

## 依赖与调用关系

上游装配链为 `ucadata/lib.rs` → `unicode_0900_ai_ci_data_generated`，并由 `pub use` 把 `DUCET0900Table` 重导出到 `ucadata` crate 根。RustCodeGraph 能索引本文件的 `UcaDataTable`，但未为静态量 `DUCET0900Table` 建立可查询节点，并把文件级使用数报告为 0；因此具体静态量调用边通过精确源码引用补充核验。

主要运行时调用链是：

`unicode0900Impl::GetWeight` / `unicode0900AICIPattern::DoMatch` → `convertRuneUnicodeCI0900` → `ucadata::DUCET0900Table.map_table4` →（哨兵分支）`long_rune_map.binary_search_by_key`。

本文件自身不导入模块，也不调用函数。它与 `data.rs::LongRune8` 通过“数值为 `0xFFFD` 的主表项必须存在长表记录”这一约定耦合；与生成器通过 `UcaDataTable` 字段名、数组长度、条目排序和模板格式耦合；与消费者通过公开字段布局耦合。

## 错误处理与边界

本文件没有返回错误或 panic 路径，因为它只声明静态数据。错误行为存在于生成与消费边界：生成器遇到无法表达的权重数会触发不可达分支；消费者若见到 `LongRune8` 却找不到对应长表项，会在 `expect("LongRune8 sentinel must have generated weights")` 处 panic。这使损坏或不同步的生成数据尽早暴露，而不是静默产生错误排序。

覆盖边界由主表长度 183969 决定，即合法索引为 `0..183969`、最后索引为 `0x2CEA0`。当前 Rust 与 Go 消费者都以严格大于表长判断表外字符，因此 U+2CEA1（数值恰等于长度）不会进入兜底分支，随后数组索引会越界；真正大于该值的字符才走兜底计算。这个既有 off-by-one 不由生成数据文件修正，扩展或修复消费者时应加入独立回归测试。Rust `char` 已排除 UTF-16 surrogate 值；生成器仍按 Unicode 9.0.0 规则为相关数值和 U+FFFD 生成特殊权重，以保持生成数据与 Go 版本一致。

必须保持的边界不变量包括：Hangul Jamo `0x1100..0x11FF` 的权重只占低 16 位，以供 Hangul 音节组合；长表按码点排序；每个哨兵主表项都有长表项；长表首段非零；数组长度与生成上界一致。

## 并发与资源生命周期

`DUCET0900Table` 是不可变 `static`，字段只暴露共享读取，没有锁、原子量、任务、通道、事务、堆上惰性初始化或外部 I/O。所有线程都可安全共享同一份表数据；单码点普通查询是一次数组读取，长权重查询再增加一次对 27 项静态切片的 `O(log n)` 二分查找。

资源在程序映像装载时即存在，并持续到进程退出；调用者不拥有也不释放表或切片。生成时读取 allkeys、启动格式化器和写文件的资源生命周期属于 `generator/main.rs`，不进入生产运行时路径。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.go`。两端具有相同的 183969 项主表、相同的 27 项长权重数据和相同的 `DUCET0900Table` 名称。对应关系为：

- Go 匿名结构体的 `[183969]uint64 MapTable4` 对应 Rust `UcaDataTable<183969>::map_table4`。
- Go `map[rune][2]uint64 LongRuneMap` 对应 Rust `&'static [(u32, [u64; 2])]`。Rust 选择有序切片以避免运行时哈希表，并支持消费者的二分查找；这要求生成顺序稳定。
- Go 通过 map 下标读取长权重；Rust 显式二分搜索并在缺项时 `expect`。正常生成数据的返回语义相同，损坏数据时 Rust 的失败更明确。
- Go 与 Rust 的运行时转换函数都先查主表，再在 `LongRune8` 分支取两段权重，并对表外码点计算隐式权重。

`unicode_0900_ai_ci_data_test.go` 与 Rust `unicode_0900_ai_ci_data_test.rs` 一一覆盖 Hangul Jamo 单权重和长表首段非零。Rust 额外的 `unicode_0900_ai_ci_data_generated_2_aster_unit_test.rs` 校验表长、TAB、`A`、U+FFFD 和两项长权重代表值；`unicode_ci_data_test.rs` 与 Go 同名测试还检查 9.0.0 长权重组合互不重复。

## 扩展指南

需要更新 Unicode 数据、覆盖上界或权重编码时，应先修改/替换生成输入和 `generator/main.rs` 的构表规则，必要时同步 `generator/data_0900.rs.tpl`，再按 `data.rs` 记录的命令重新生成本文件与 Go 对照产物。不要直接编辑 `DUCET0900Table`。

修改表布局时须同步检查 `pkg/util/collate/unicode_0900_ai_ci_impl.rs::convertRuneUnicodeCI0900`：若改变哨兵、长表容器或排序规则，二分查找和失败条件也必须一起调整。新增运行时代码不应放进本生成文件；应放在实现文件，并在同目录独立测试文件中覆盖，遵守 Rust 源码与测试逻辑分离要求。

验证更新至少应覆盖：生成器迁移测试 `generator/migration_aster_unit_test.rs`、表形状测试 `unicode_0900_ai_ci_data_test.rs`、代表值测试 `unicode_0900_ai_ci_data_generated_2_aster_unit_test.rs`、唯一性测试 `unicode_ci_data_test.rs`，以及对应 Go 测试。兼容性风险集中在排序结果变化和 Go/Rust 数据漂移；性能风险集中在增大主表静态体积或破坏长表有序性后退化/失效的查找；正确性风险集中在哨兵与长表不同步、权重打包顺序变化和覆盖上界的 off-by-one。

## 验证依据

- 目标数据与符号：`pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs` 第 16–30 行及第 184002–184031 行；RustCodeGraph `node --file` 核验了文件头、类型、静态实例和完整长表尾部。
- crate 与模块边界：`pkg/util/collate/ucadata/Cargo.toml`、`pkg/util/collate/ucadata/lib.rs`；该目录没有 `doc.go`，模块说明以 Rust crate 入口为准。
- 运行时调用边：`pkg/util/collate/unicode_0900_ai_ci_impl.rs::convertRuneUnicodeCI0900`、`unicode0900Impl::GetWeight`、`unicode0900AICIPattern::DoMatch`。RustCodeGraph 精确查询同时定位了 Go/Rust 两个同名转换函数；静态量节点缺失后以 `rg` 的直接引用结果补齐调用证据。
- 生成依据：`pkg/util/collate/ucadata/data.rs::LongRune8`、`generator/main.rs::{calcImplicitWeight,getImplicitWeight0900,selectOutputTarget,buildTable,generateOutputTarget}`、`generator/data_0900.rs.tpl` 和 `generator/allkeys-9.0.0.txt`。
- Go 对照：`pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.go`、`pkg/util/collate/unicode_0900_ai_ci_impl.go`。
- 测试依据：`unicode_0900_ai_ci_data_test.rs`、`unicode_0900_ai_ci_data_generated_2_aster_unit_test.rs`、`unicode_ci_data_test.rs`，以及对应的 `unicode_0900_ai_ci_data_test.go`、`unicode_ci_data_test.go`。本任务按计划不运行 Cargo；这些文件用于核对既有不变量与迁移语义。
