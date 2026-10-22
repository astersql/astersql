# `pkg/util/collate/ucadata/generator/magic.rs`

## 文件定位

两个定义逐值对应 `magic.go`，使 Rust 生成器保持 Go 生成器的数据格式和解析语义。

## 核心职责

- `LongRune8` 定义值 `0xFFFD`。当一个码点有 5～8 个非零一级权重，或隐式权重需要两个 `u64` 保存时，`main.rs` 把该值写入 `cet::MapTable4`，真实的两段权重写入 `cet::LongRuneMap`。因此它是短表与长表之间的编码协议，不是普通排序权重。
- `reverseHexTable` 用 256 个字节覆盖完整单字节索引空间：ASCII `0`～`9` 映射到 0～9，`A`～`F` 和 `a`～`f` 映射到 10～15，其余字节映射为 `0xff`。`parseCETHex` 借助 `<= 0x0f` 判断输入字节是否为十六进制字符，并直接取得数值，避免分支式字符转换。
- 两个定义逐值对应 `magic.go`，使 Rust 生成器保持 Go 生成器的数据格式和解析语义。

## 主要符号

- `pub const LongRune8: u64 = 0xFFFD`：公开的长权重哨兵。类型为 `u64`，与 `cet::MapTable4: Vec<u64>` 的元素类型一致。名称保留 Go 风格；crate 根通过 `#![allow(non_upper_case_globals)]` 接受这一迁移期命名。
- `pub const reverseHexTable: &[u8; 256]`：指向编译期 256 字节数组的公开共享引用。有效十六进制字符的表项不超过 `0x0f`，非法字符统一为 `0xff`。名称同样保留 Go 风格，并由 crate 根的 lint allow 覆盖。
- 文件没有函数、类型、trait、`impl`、可变静态量或条件编译项；所有行为都发生在消费者中。

## 执行流程

十六进制解析路径如下：

1. `main.rs::parseCETHex` 用 `input.bytes()` 顺序取得输入字节，并以该字节的 `usize` 值索引 `reverseHexTable`。
2. 表项为 0～15 时，解析器把累计值左移 4 位并加上该表项；表项为 `0xff` 时停止，返回是否至少读到一位、累计值和未消费后缀。
3. `parseCETWeights` 用该函数读取一级及被忽略的高层级权重，`parseCETEntry` 用它读取码点；`parseAllKeys` 再把有效条目交给权重表构造流程。

长权重编码路径如下：

1. `cet::insertWeights` 去掉零权重；不超过 4 个的权重直接打包进一个 `u64`。
2. 5～8 个权重时，它把 `LongRune8` 写入 `MapTable4[char]`，并把前四个、后四个权重分别打包到 `LongRuneMap[char]` 的两个 `u64` 中；超过 8 个则触发不可达分支的 panic。
3. `cet::calcImplicitWeight` 在隐式权重返回第二段非零时采用相同的哨兵加长表布局。生成模板据此输出运行时使用的短表和长表。

## 数据与状态

本文件只有不可变的编译期数据，不产生运行时可变状态。

`LongRune8` 的关键不变量是必须与 Go `generator/magic.go`、运行时数据协议以及生成器消费者保持同值。`0xFFFD` 也可能对应 Unicode replacement character；`main.rs::insertWeights` 对 Unicode 9.0.0 的码点 `U+FFFD` 另行在 `LongRuneMap` 写入 `[0xFFFD, 0]`，以区分哨兵解释所需的实际数据。

`reverseHexTable` 的关键不变量是长度恰为 256，并且只有 22 个 ASCII 十六进制字符位置产生 0～15；所有其他位置必须为 `0xff`。Rust 消费者按字节遍历字符串，所以索引范围天然是 0～255，不会因 UTF-8 多字节内容越界；非 ASCII 字节只会命中非法项并终止解析。

## 依赖与调用关系

本文件自身不导入标准库或第三方 crate；`Cargo.toml` 也没有声明外部依赖或 feature。它只依赖 Rust 的常量、字节字符串和固定长度数组能力。

上游装配关系是 `lib.rs -> magic.rs`，且 `lib.rs` 将两个常量重导出到 crate 根。直接使用者只有同 crate 的 `main.rs`：

- `reverseHexTable` 被 `parseCETHex` 索引；后者的直接调用者是 `parseCETWeights` 和 `parseCETEntry`，并由 CET 输入解析链继续使用。
- `LongRune8` 被 `cet::insertWeights` 和 `cet::calcImplicitWeight` 写入 `MapTable4`。独立测试还通过 crate 根重导出的 `super::LongRune8` 校验长权重结果。

RustCodeGraph 能确认 `magic.rs`、`main.rs`、`lib.rs` 位于同一生成器区域，并给出 `parseCETHex` 到 `parseCETWeights`/`parseCETEntry` 的调用影响；当前索引没有把这两个 `const` 建成独立定义节点，因此常量的精确引用由 `rg` 结果补证。

## 错误处理与边界

本文件没有返回错误或 panic 的代码。边界行为由常量协议及消费者共同决定：

- `reverseHexTable` 以 `0xff` 表示非法输入，不区分空白、标点、非 ASCII 字节等非法类别；`parseCETHex` 在第一个非法字节停止，并以布尔值报告此前是否读到过合法十六进制位。
- `parseCETHex` 没有溢出检查，连续十六进制位通过 `u32` 左移和加法累计；输入来自固定格式的 allkeys 数据，安全扩展时仍应保持码点/权重长度约束。
- 与 Go 实现一致，若输入直到末尾全是合法十六进制字符，局部变量 `end` 保持 0，返回后缀会是原输入而非空串。现有调用依赖 allkeys token 后存在分隔符；若要支持无终止分隔符的通用输入，应同时修改 Go/Rust 行为并增加回归测试，不能只改表。
- `LongRune8` 只表达“去长表取值”。消费者必须同步写入 `LongRuneMap`；否则读取方会把缺失项解释成错误或零值。超过 8 个非零权重的处理属于 `insertWeights` 的 panic 边界，不由本文件兜底。

## 并发与资源生命周期

两个常量在程序映像中静态存在，只有共享只读访问，没有锁、原子变量、线程、异步任务、通道、文件句柄或堆资源。`reverseHexTable` 的引用具有静态生命周期；解析期间仅重复读取，不需要初始化或清理。`LongRune8` 是按值复制的 `u64`。

因此本文件本身没有并发竞态和资源释放顺序。并行调用解析器时，这两个常量可安全共享；生成过程中的 `MapTable4`/`LongRuneMap` 可变性归 `main.rs::cet` 实例所有，不属于本文件。

## 与 Go 版本的对应关系

Go 的常量块定义无类型的 `LongRune8 = 0xFFFD` 和由 16 段连接而成的 256 字节字符串 `reverseHexTable`；Rust 分别使用显式 `u64` 和 `&[u8; 256]`，数值与逐字节内容保持一致。

Rust 的差异主要是类型约束更显式：固定数组在编译期保证表长，按字节迭代保证索引可落入 256 项，哨兵类型直接匹配 `Vec<u64>`。这些差异没有改变现有 Go 数据格式。

## 扩展指南

- 新增可接受的数字字符或改变非法哨兵时，应修改 `reverseHexTable`，同步核对 Go `magic.go`，并在 `migration_aster_unit_test.rs::parses_hex_weights_and_entries_like_go` 增加大小写、边界字节、非法字节和终止位置用例。不要在本文件内嵌测试；仓库规则要求测试保留在独立测试文件。
- 修改长权重容量或布局时，不能只改 `LongRune8`：必须同步审查 `main.rs::cet::insertWeights`、`calcImplicitWeight`、生成模板、生成后的运行时表读取逻辑及 Go 对照。重点风险是哨兵与真实权重碰撞、短长表不一致和既有生成数据不兼容。
- 若希望常量不再公开，应先检查 `lib.rs` 的 `pub use magic::*` 和独立测试通过 crate 根访问 `LongRune8` 的方式；缩小可见性会改变 crate API。
- 性能相关修改应保留 O(1) 查表和只读静态存储特征。用分支或运行时初始化替代反查表前，应以生成器实际输入衡量收益，并确保 Go/Rust 解析结果逐项一致。
