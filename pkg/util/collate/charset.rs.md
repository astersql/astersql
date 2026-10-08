# `pkg/util/collate/charset.rs`

## 文件定位

本文件属于 `astersql-util-collate` crate。crate 入口 `pkg/util/collate/lib.rs` 通过 `#[path = "charset.rs"] pub mod charset_switch` 将它注册为 `charset_switch` 模块，以避免与同一入口重新导出的 `parser_charset::charset` 模块重名。其唯一生产函数 `switchDefaultCollation` 是“新排序规则开关”与字符集元数据之间的适配层：它不实现字符串比较，而是同步 GBK、GB18030 的默认排序规则元数据。

`pkg/util/collate/Cargo.toml` 表明该 crate 直接依赖 `astersql-parser-charset`，并在 `lib.rs` 中将其以 `pub use parser_charset::{charset, mysql}` 暴露。本文件因此通过 `crate::charset` 操作真正的字符集注册表。默认 feature `full_collate` 控制其他 Unicode collator 的编译范围，不改变本文件的逻辑，也没有条件编译分支。

## 核心职责

- 根据布尔开关同时切换 GBK 与 GB18030 的 `Charset::DefaultCollation`：开启时选择各自的 `chinese_ci`，关闭时选择各自的 `bin`。
- 同步维护每个字符集 `Collations` 映射中成对规则的 `Collation::IsDefault` 标志，保证名称字段与默认标志一致。
- 在写元数据前调用 `charset::init()`，确保 `pkg/parser/charset/charset.rs` 中的静态 collation 表已通过 `Once` 注册进 `CharacterSetInfos`；这是 Rust 惰性初始化相对 Go 包初始化机制所需的局部接线。

它只修改目录元数据，不修改具体 `gbkBinCollator`、`gbkChineseCICollator`、`gb18030BinCollator` 或 `gb18030ChineseCICollator` 的比较、排序键生成行为。

## 主要符号

- `pub fn switchDefaultCollation(flag: bool)`：文件唯一的函数和公开 API。`flag == true` 表示新排序规则开启，GBK/GB18030 默认项切到 `*_chinese_ci`；`false` 表示切回 `*_bin`。函数无返回值，失败路径通过 `expect` panic 暴露不变量破坏。
- `use crate::charset`：指向 `lib.rs` 重新导出的 `astersql-parser-charset` 模块，而非本文件自身。实际使用的共享符号包括 `init`、`CharacterSetInfos`、`CharsetGBK`、`CharsetGB18030` 以及四个 collation 名称常量。
- 函数内的 `(charset_name, binary, chinese)` 二元组表：把两个字符集的相同切换算法数据化，确保两组字段按同一顺序、同一规则更新；它是内部局部值，不形成长期状态。

文件没有自定义类型、trait、impl、模块级常量或条件编译项。

## 执行流程

1. `switchDefaultCollation` 首先调用 `charset::init()`。该函数最终进入 `ensure_initialized()`，由 `INITIALIZE_COLLATIONS: Once` 至多一次地把静态 `collations` 表登记到按 ID/名称索引以及各字符集的 `Collations` 映射。
2. 函数获取 `charset::CharacterSetInfos.write()` 的独占写锁，并在整个双字符集循环期间持有该锁。
3. 循环先处理 GBK，再处理 GB18030；每项携带字符集名、二进制规则名和中文不区分大小写规则名。
4. 从注册表取得对应 `Charset`。根据 `flag` 把 `DefaultCollation` 设为 `chinese` 或 `binary`。
5. 将二进制规则的 `IsDefault` 设为 `!flag`，将中文规则的 `IsDefault` 设为 `flag`。因此每组恰有一个默认标志，且与 `DefaultCollation` 指向同一规则。
6. 循环结束后写锁随局部守卫离开作用域自动释放。函数不返回旧值，也没有显式回滚步骤。

生产调用链为 `collate::SetNewCollationEnabledForTest(flag) -> charset_switch::switchDefaultCollation(flag)`；随后前者才以 `SeqCst` 更新 `newCollationEnabled: AtomicI32`。直接测试 `charset_test::switch_default_collation_updates_both_charset_catalogs` 则直接调用本函数并读取注册表验证结果。

## 数据与状态

被修改的核心状态是 `pkg/parser/charset/charset.rs` 中的 `CharacterSetInfos: LazyLock<RwLock<HashMap<String, Charset>>>`。本文件只触及键 `gbk` 与 `gb18030` 下的：

- `Charset::DefaultCollation: String`；
- `Charset::Collations` 中 `gbk_bin`、`gbk_chinese_ci`、`gb18030_bin`、`gb18030_chinese_ci` 对应值的 `Collation::IsDefault`。

开启后的不变量是 GBK 默认 `gbk_chinese_ci`、GB18030 默认 `gb18030_chinese_ci`，两条中文规则的 `IsDefault == true`，两条二进制规则为 `false`。关闭后的不变量完全反向。该操作可重复：相同 `flag` 多次执行会写入相同终态，不累积额外条目。

`charset::init()` 还会初始化其他索引和字符集，但本函数不会添加、删除或替换任何注册表项，也不会保存调用前状态。

## 依赖与调用关系

RustCodeGraph 将 `pkg/util/collate/charset.rs` 标记为被两个文件使用：

- 生产上游 `pkg/util/collate/collate.rs`：导入 `crate::charset_switch::switchDefaultCollation`，并由 `SetNewCollationEnabledForTest` 调用；该入口先切元数据、再更新全局原子开关。
- 测试上游 `pkg/util/collate/charset_test.rs`：直接调用函数，覆盖开启和关闭两个方向。

下游依赖全部来自 `crate::charset`，其实现位于 `pkg/parser/charset/charset.rs`：`init`/`ensure_initialized` 提供一次性初始化，`CharacterSetInfos` 提供共享可变目录，常量提供稳定的注册表键。RustCodeGraph 的 `callees switchDefaultCollation` 未解析出函数内调用边，因此上述下游关系同时由目标源码中的 `charset::init`、`write`、`get_mut` 调用和 Cargo 路径依赖核实。

本文件不执行 I/O、SQL、编码转换或 collator 构造；它对完整应用的影响通过共享字符集目录传播，例如后续查询字符集默认 collation 的代码会观察到这里写入的值。

## 错误处理与边界

函数没有 `Result` 返回值，并把以下情况视为程序内部不变量破坏：

- `CharacterSetInfos` 的 `RwLock` 已中毒：`write().expect("charset metadata lock poisoned")` panic。
- 内建 GBK 或 GB18030 元数据缺失：第一次 `get_mut(...).expect(...)` panic。
- 任一内建 `bin` 或 `chinese_ci` 规则未在 `Collations` 中注册：对应 `get_mut(...).expect(...)` panic。

前置 `charset::init()` 是避免正常惰性初始化路径误触“规则缺失”panic 的关键。由于函数逐字段就地写入且没有回滚，如果有人在运行期破坏内建注册表，panic 可能发生在部分字段已经修改之后；正常内建表满足这些不变量，不走该边界。

该 API 只接受布尔值，没有未知字符集、任意 collation 名或用户输入解析分支。它也不会校验具体 collator 实现是否由 feature 提供；四个目标规则是 crate 的基础 GBK/GB18030 路径，不受 `full_collate` 开关控制。

## 并发与资源生命周期

`charset::init()` 由 `std::sync::Once` 保护，多个线程首次进入时不会重复构建静态索引。随后本函数在一次 `RwLock` 写锁持有期内更新两个字符集，因此其他通过同一 `CharacterSetInfos` 锁读取或写入的线程不会看到循环中间状态；守卫在函数返回或 panic 展开时按 RAII 释放，没有线程、任务、通道、事务或外部资源生命周期。

但更高层的 `SetNewCollationEnabledForTest` 分两步完成“目录切换”和 `newCollationEnabled` 原子值更新，两者不是一个共同的原子事务。并发调用者可能短暂观察到开关值与目录默认项不同步；Go 入口的注释也明确要求使用该测试辅助函数的测试串行执行。Rust 的锁保证内存安全和目录内部一致性，并不消除这一全局语义竞态。因此扩展测试必须恢复状态，且不应并行切换该全局配置。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/collate/charset.go`，两版的业务语义一致：`true` 将 GBK/GB18030 默认值切到 `chinese_ci`，`false` 切到 `bin`，并同步四个 `IsDefault` 标志。

实现形态有三点差异：

- Go 直接写包级 map 中的指针对象；Rust 的 `CharacterSetInfos` 是 `LazyLock<RwLock<HashMap<...>>>`，因此必须取得写锁。
- Go 包初始化已经准备好 collation 表；Rust 显式调用 `charset::init()` 触发 `Once` 驱动的惰性登记。
- Go 展开写出两组赋值；Rust 用二元组数组循环复用完全相同的算法，但没有删减任一字段更新。

Go 的 `pkg/util/collate/collate.go::SetNewCollationEnabledForTest` 与 Rust 同名入口都先调用本切换函数，再更新原子开关；Go 注释要求相关测试串行。Go 的 `pkg/util/collate/collate_test.go::TestSetNewCollateEnabled` 验证高层开关，Rust 的 `pkg/util/collate/charset_test.rs::switch_default_collation_updates_both_charset_catalogs` 进一步逐项验证本文件负责的两个默认名称与四个标志。

## 扩展指南

若要让新的字符集参与此开关，应在 `switchDefaultCollation` 的局部元组表增加完整的 `(charset_name, binary, chinese)` 配置，而不是只改 `DefaultCollation`；同时必须确认 `pkg/parser/charset/charset.rs` 的静态表会在 `init()` 后把两条规则登记进该字符集。随后扩展独立测试 `pkg/util/collate/charset_test.rs`，在开启、关闭两种状态下同时断言默认名称及成对 `IsDefault`，不要把测试嵌入生产源文件。

若增加可失败的动态注册或允许规则缺失，应重新设计当前 `expect` 契约和部分写入风险，优先在拿锁后先完成所有目标查找/校验，再统一修改，或返回可诊断的错误。若把接口用于测试之外的运行期并发切换，还需将目录状态与 `newCollationEnabled` 的一致性协议一并设计，不能只依赖当前单表写锁。

兼容性重点是默认 collation 会影响依赖字符集元数据的 SQL/DDL 行为；性能重点是切换持有全局目录写锁并执行固定数量查找，目前复杂度为常数。不要为此函数引入 I/O 或昂贵的 collator 构建。修改后应同步比较 `pkg/util/collate/charset.go`，并保留 GBK、GB18030 两组行为一致。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/util/collate/charset.rs` 读取全部 57 行并报告使用者为 `pkg/util/collate/collate.rs`、`pkg/util/collate/charset_test.rs`；`query switchDefaultCollation` 定位 Go/Rust 两个实现；`callers` 未返回静态边，`callees` 对两个同名实现均未解析下游边，因此调用关系又以直接源码引用交叉核验。
- 目标与模块边界：`pkg/util/collate/charset.rs`、`pkg/util/collate/lib.rs`、`pkg/util/collate/Cargo.toml`。
- Rust 下游状态：`pkg/parser/charset/charset.rs` 中的 `CharacterSetInfos`、四个 collation 常量、`CharsetGBK`/`CharsetGB18030`、`add_collation`、`init`/`ensure_initialized`。
- Rust 调用与测试：`pkg/util/collate/collate.rs::SetNewCollationEnabledForTest`；`pkg/util/collate/charset_test.rs::switch_default_collation_updates_both_charset_catalogs`。
- Go 对照：`pkg/util/collate/charset.go::switchDefaultCollation`、`pkg/util/collate/collate.go::SetNewCollationEnabledForTest`、`pkg/util/collate/collate_test.go::TestSetNewCollateEnabled`。
- 人工复核结论：本文件存在是为了让新 collation 测试开关同步修改 GBK/GB18030 的共享默认元数据；运行时先初始化目录、再在独占锁内切换名称与标志；安全扩展必须同时维护注册、成对标志、独立测试及全局并发约束。
