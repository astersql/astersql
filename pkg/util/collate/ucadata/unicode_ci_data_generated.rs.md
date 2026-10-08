# `pkg/util/collate/ucadata/unicode_ci_data_generated.rs`

## 文件定位

该文件是 Unicode Collation Algorithm（UCA）4.0.0 的 Default Unicode Collation Element Table（DUCET）Rust 生成产物，数据来源在文件头标为 Unicode 官方 `allkeys-4.0.0.txt`。它位于 `astersql-util-collate-ucadata` crate；该 crate 由 [`Cargo.toml`](Cargo.toml) 定义、以 [`lib.rs`](lib.rs) 为入口，并从 `unicode_ci_data_generated` 模块公开再导出本文件的符号。上层 `astersql-util-collate` 同时通过 `#[path = "ucadata/lib.rs"]` 将该模块装入自身，因此 [`unicode_0400_ci_impl.rs`](../unicode_0400_ci_impl.rs) 可以用 `crate::ucadata` 访问表。

文件声明为生成代码（`Code generated ... DO NOT EDIT`）。数据或布局变更应进入 [`generator/main.rs`](generator/main.rs) 的解析、计算或 Rust 模板渲染流程，然后重新生成本文件，不能手改 65,536 项数据。目标文件没有 feature gate 或其他条件编译项；其独立测试才由 `ucadata/lib.rs` 中的 `#[cfg(test)]` 装配。

## 核心职责

- 以 `DUCET0400Table` 保存 BMP 范围 `U+0000..=U+FFFF` 的 Unicode 4.0.0 一级排序权重。`MapTable4` 的数组索引就是码点，因而普通查询是常量时间直接寻址。
- 对无法装入一个 `u64` 的多段权重，用主表中的 `0xFFFD` 哨兵配合 `LongRuneMap` 保存两段 `[u64; 2]`；哨兵约定也见 [`data.rs`](data.rs) 的 `LONG_RUNE_8` 和 [`unicode_0400_ci_impl.rs`](../unicode_0400_ci_impl.rs) 的 `longRune`。
- 提供安全的 `map_table_weight` 和 `long_rune_weight` 查询方法：前者把越界变成 `None`，后者把 Go `map` 查询语义移植为对有序切片的二分查找。
- 只承载不可变权重数据及最小查询逻辑，不负责字符串预处理、完整排序键生成、比较或通配符匹配；这些消费逻辑属于 `unicode0400Impl` 等上层实现。

## 主要符号

- `pub struct UcaDataTable<const N: usize>`：以 const generic 把主表长度编码进类型。公开字段 `MapTable4: [u64; N]` 保存短权重，`LongRuneMap: &'static [(u32, [u64; 2])]` 保存码点到两段长权重的静态映射。字段保留 Go 风格命名，crate 根通过 lint allowance 接受这些名称。
- `UcaDataTable::map_table_weight(&self, rune: u32) -> Option<u64>`：将码点转为数组索引，调用切片 `get` 并复制 `u64`。对 `DUCET0400Table`，`0..=0xFFFF` 返回 `Some`，`0x10000` 及以上返回 `None`。
- `UcaDataTable::long_rune_weight(&self, rune: u32) -> Option<[u64; 2]>`：对 `LongRuneMap` 的键执行 `binary_search_by_key`；命中后复制权重数组，缺失返回 `None`。正确性依赖生成器保持键严格递增。
- `pub static DUCET0400Table: UcaDataTable<65536>`：唯一表实例。`MapTable4` 恰有 65,536 项；`LongRuneMap` 在文件末尾包含 22 个按码点递增的条目，范围从 `0x321D` 到 `0xFDFB`。

本文件没有 trait、枚举、类型别名、宏、独立自由函数或条件编译分支。

## 执行流程

生成阶段由 [`generator/main.rs`](generator/main.rs) 完成：

1. `selectOutputTarget` 根据输出文件名 `unicode_ci_data_generated.rs` 选择 `Rust0400`。
2. `buildTable(unicode0400)` 解析内嵌的 Unicode 4.0.0 allkeys 数据，表长固定为 `0x10000`，命名为 `DUCET0400Table`，再由 `calcImplicitWeight` 补齐隐式权重。
3. 生成器把最多四个 16 位权重打包进 `MapTable4` 的一个 `u64`。更长的权重把主表项写为 `LongRune8`/`0xFFFD`，完整内容放进两段 `u64`。
4. `render_rust_unicode_template` 逐项渲染主表，并在输出长表前按码点 `sort_unstable_by_key`；这一步建立 `long_rune_weight` 所需的有序不变量。生成源码随后经 `rustfmt` 格式化并写入目标路径。

运行阶段的普通权重查询由 [`unicode0400Impl::GetWeight`](../unicode_0400_ci_impl.rs) 发起：

1. 非 BMP 字符不访问本表，上层直接返回 `(0xFFFD, 0)`。
2. BMP 码点先直接读取 `DUCET0400Table.MapTable4[idx]`。
3. 若结果不是 `0xFFFD`，上层返回 `(主表权重, 0)`；若是哨兵，则调用 `long_rune_weight`，取得两段权重后返回。
4. 通配符比较同样直接读取 `MapTable4`；短权重相等即可匹配，而长权重哨兵要求原码点相等，不进一步比较 `LongRuneMap`。

`map_table_weight` 当前主要用于独立边界测试；生产热路径为了先判断哨兵而直接索引公开数组。

## 数据与状态

全部状态在编译期固化、运行期只读：`DUCET0400Table` 是 `static`，主表元素和长表元素都是 `Copy` 标量。主表用码点作隐式键，数值 `0` 表示该位置生成权重为零；`0xFFFD` 是“转查长表”的协议值，不能当作普通短权重解释。长表每项为 `(u32, [u64; 2])`，两段 `u64` 继续按每 16 位一个 collation element 的方式承载多元素权重。

关键不变量是：主表长度固定为 65,536；所有运行时直接索引都先限定到 BMP；每个长权重哨兵必须有对应长表项；`LongRuneMap` 的键必须严格递增且唯一。当前独立测试还要求不同码点的长权重向量全局唯一。主表本体约占 512 KiB（不计对齐和长表），以空间换取常量时间的 BMP 查表；长表仅 22 项，二分查询开销很小。

## 依赖与调用关系

crate 边界由 [`ucadata/Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-util-collate-ucadata`，库入口是 `lib.rs`，且没有普通依赖或 feature 声明；工作区根还以 `facade_util_collate_ucadata` 注册该包。`ucadata/lib.rs` 声明并公开再导出 `unicode_ci_data_generated`，同时也把相同源码树嵌入 `astersql-util-collate` 的模块结构。

RustCodeGraph 对目标文件给出的直接使用文件只有两个：

- [`unicode_0400_ci_impl.rs`](../unicode_0400_ci_impl.rs)：生产调用者。图中的明确调用边为 `unicode0400Impl::GetWeight -> UcaDataTable::long_rune_weight`；该文件还直接读取 `DUCET0400Table.MapTable4` 供 `GetWeight` 与 `unicodePattern::DoMatch` 使用。
- [`unicode_ci_data_generated_3_aster_unit_test.rs`](unicode_ci_data_generated_3_aster_unit_test.rs)：直接调用两个查询方法，并遍历公开长表验证生成数据不变量。

此外，[`unicode_ci_data_test.rs`](unicode_ci_data_test.rs) 逐项遍历本表，与 [`unicode_ci_data_original_test.rs`](unicode_ci_data_original_test.rs) 的历史原表比较。下游没有 I/O、分配、网络、存储或第三方库调用；两个查询方法只依赖标准库数组/切片 API。

## 错误处理与边界

本文件不返回 `Result`、不产生业务错误，也不执行显式 panic。`map_table_weight` 用 `Option` 安全表达主表越界；`long_rune_weight` 用 `Option` 表达长表未命中。传入任意 `u32` 都不会因这两个方法自身发生越界访问。

生产调用者的契约更严格：`GetWeight` 先拒绝非 BMP 码点；发现 `0xFFFD` 后对 `long_rune_weight` 使用 `expect("longRune sentinel must have generated weights")`。因此“主表含哨兵但长表缺键”属于生成数据损坏，会在消费端 panic，而不是静默退回零权重。相反，直接访问公开 `MapTable4` 没有方法级边界保护，调用者必须先验证索引；当前生产调用者确实在索引前检查 `r <= 0xFFFF`。

需要区分两种值：主表中的普通 `0` 是有效表内容，不等同于查询失败；只有方法返回的 `None` 才表示越界或缺键。该文件也不验证 `u32` 是否是 Unicode scalar value，因为表查询协议处理的是数值码点，生产入口的 `char` 已保证标量有效。

## 并发与资源生命周期

表在程序映像中具有整个进程生命周期，不进行惰性初始化、堆分配或析构。查询只共享不可变引用并复制固定宽度值，没有锁、原子变量、线程局部状态、任务、通道或事务；因此并发调用无需同步，且不会改变后续查询结果。

资源风险主要是静态二进制体积与缓存局部性，而不是生命周期泄漏。`MapTable4` 的连续数组适合按码点直接读取；`LongRuneMap` 是短小连续切片，避免了 Go `map` 的运行时哈希状态和初始化开销。任何把长表改为无序容器或在查询时构建索引的方案都会改变当前确定性、内存布局或启动成本，应先用独立性能与一致性测试证明收益。

## 与 Go 版本的对应关系

直接对照文件是 [`unicode_ci_data_generated.go`](unicode_ci_data_generated.go)。两边都公开 `DUCET0400Table`，都含 `[65536]uint64` 主表和相同的 22 组长权重数据；代表条目如 `0x321D` 与末项 `0xFDFB` 数值一致。Rust 使用 `UcaDataTable<65536>` 命名类型替代 Go 匿名结构体，用 `&'static [(u32, [u64; 2])]` 替代 Go `map[rune][2]uint64`。

这一容器差异带来两点可见语义：Go map 缺键会给出零值，Rust `long_rune_weight` 明确返回 `None`；Go map 无顺序要求，Rust 为支持二分查找要求切片按键排序。生成器同时对 Go 和 Rust 输出先排序，使生成文本稳定，而 Rust 独立测试额外验证严格递增。

生产消费逻辑与 [`unicode_0400_ci_impl.go`](../unicode_0400_ci_impl.go) 对齐：非 BMP 返回 `0xFFFD`，BMP 先读主表，哨兵时读取两段长权重，通配符遇长权重时要求原 rune 相同。Rust 版本把 Go 的 map 索引改成返回 `Option` 的查询并在应当存在的生产路径使用 `expect`，从而把生成数据不一致暴露为明确失败。

Go 的 [`unicode_ci_data_test.go`](unicode_ci_data_test.go) 对生成表和旧表逐项比较，并检查长权重唯一性；Rust 的 `unicode_ci_data_test.rs` 保留这些测试意图，`unicode_ci_data_generated_3_aster_unit_test.rs` 进一步覆盖方法边界、已知权重和排序不变量。

## 扩展指南

- 更新 Unicode 4.0.0 数据或修正规则时，修改 [`generator/main.rs`](generator/main.rs) 或其输入/模板，重新生成 Go 与 Rust 对应文件，并保持两侧数值一致；不要直接编辑本文件的数据段。
- 若改变表长、权重打包或哨兵协议，必须同步 `UcaDataTable`/`DUCET0400Table`、[`data.rs`](data.rs) 的 `LONG_RUNE_8`、[`unicode_0400_ci_impl.rs`](../unicode_0400_ci_impl.rs) 的 `longRune` 与 `GetWeight`，并评估非 BMP 回退和通配符相等语义。
- 若改变 `LongRuneMap` 的存储结构或查找算法，必须保持“确定性生成、唯一键、查不到返回 `None`”的外部契约；继续使用二分查找时，生成器排序和严格递增测试不可删除。
- 测试应继续放在独立文件，不能内嵌到这个生成源文件。表内容对照更新在 `unicode_ci_data_test.rs` 与 `unicode_ci_data_original_test.rs`；查询 API、边界和排序回归更新在 `unicode_ci_data_generated_3_aster_unit_test.rs`；生成器渲染/目标选择回归更新在 `generator/migration_aster_unit_test.rs`。
- 性能敏感修改应分别观察主表直接索引热路径、哨兵后二分查找路径和静态数据体积。兼容性审查需特别检查排序结果是否仍与 Go collation 权重和历史持久化/比较行为一致。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标目录的索引列出了本文件、模块入口、生成器、Go 对照和独立测试。
- `rustcodegraph node --file pkg/util/collate/ucadata/unicode_ci_data_generated.rs`：确认文件共 65,607 行，定义 `UcaDataTable`、两个查询方法、`DUCET0400Table<65536>`，以及文件末尾 22 项有序长表。
- `rustcodegraph node`/`query`：确认 `map_table_weight` 的直接测试调用者，以及 `long_rune_weight` 的生产调用者 `GetWeight` 和测试调用者；目标文件的文件级使用方为 `unicode_0400_ci_impl.rs` 与 `unicode_ci_data_generated_3_aster_unit_test.rs`。单独的 `callers` 命令在本地索引上长时间无输出，故调用边以 `node` 返回的 Trail 和精确源码上下文交叉核对。
- [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs) 与工作区根 `Cargo.toml`：确认 crate 名称、入口、公开再导出、无 crate 级运行时依赖，以及工作区/门面注册关系；目标包附近没有 `doc.go`。
- [`generator/main.rs`](generator/main.rs)：确认 Unicode 4.0.0 表长、名称、数据 URL、隐式权重计算、长表排序、Rust 模板渲染和格式化写入流程。
- [`unicode_ci_data_generated.go`](unicode_ci_data_generated.go) 与 [`unicode_0400_ci_impl.go`](../unicode_0400_ci_impl.go)：确认 Go 数据布局、代表长表值和生产查询/回退/通配符语义。
- [`unicode_ci_data_generated_3_aster_unit_test.rs`](unicode_ci_data_generated_3_aster_unit_test.rs)、[`unicode_ci_data_test.rs`](unicode_ci_data_test.rs)、[`unicode_ci_data_original_test.rs`](unicode_ci_data_original_test.rs) 及 Go `unicode_ci_data_test.go`：确认主表长度与边界、已知权重、长表查找、严格递增、权重唯一以及新旧表逐项一致的测试意图。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰好包含 11 个固定二级章节，并人工检查所有运行时结论均可回指上述符号或文件。
