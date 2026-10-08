# `pkg/util/collate/general_ci.rs`

## 文件定位

本文件是 `astersql-util-collate` crate 中 `utf8_general_ci` 与 `utf8mb4_general_ci` 的具体排序规则实现。crate 入口 `pkg/util/collate/lib.rs` 以 `general_ci` 模块装入并重新导出这里的公开符号；`pkg/util/collate/Cargo.toml` 的默认 feature `full_collate` 启用后，`pkg/util/collate/collate.rs::group2_collator` 才会把上述两个名称注册为 `generalCICollator`。调用方通常不直接构造该类型，而是经 `GetCollator`/`GetCollatorByID` 获得 `Box<dyn Collator>`。

它位于 SQL 字符串语义的公共底层：表达式比较和 LIKE、类型层的字符串/枚举/集合比较、range 构造、codec 排序 key、聚合与会话运行时都通过通用 Collator 工厂间接使用这里的行为。具体使用点可见 `pkg/expression/builtin.rs`、`pkg/expression/builtin_string_vec.rs`、`pkg/types/compare.rs`、`pkg/util/ranger/points.rs`、`pkg/util/codec/codec.rs` 和 `pkg/session/runtime/relational_value.rs`。

## 核心职责

1. `generalCICollator` 实现 `Collator`，为字符串比较、原始字节比较、排序 key、最大 key 长度、通配符 pattern 与克隆提供统一接口。
2. `convertRuneGeneralCI` 把一个 Unicode 标量映射为 general_ci 的单个 16 位主权重。映射会折叠 ASCII 大小写以及表中指定的重音/字符变体；例如 `a`、`A`、`À` 都落到权重 `0x0041`。
3. `plane00` 等静态表与 `planeTable` 实现按 Unicode 高 8 位分页的常量时间查表。没有专用表的 BMP 页保持原码点，非 BMP 字符统一映射为 `0xFFFD`。
4. `ciPattern` 复用公共 pattern 编译/匹配器，仅把“字符相等”替换为“general_ci 权重相等”，从而令 LIKE 字面字符遵守同一大小写和重音规则。
5. general_ci 是 PAD SPACE collation：普通 `Compare`、`Key` 与 `ImmutableKey` 忽略右侧 ASCII 空格；`KeyWithoutTrimRightSpace` 明确保留它们。

## 主要符号

- `pub struct generalCICollator {}`：无实例状态的 Collator。固有方法保留 Go API 命名，`impl Collator` 再把 trait 调用转发到这些方法；`CompareBytes` 和 `KeyBytes` 是 Rust 为承载 Go `string` 中非法 UTF-8 而提供的字节入口。
- `generalCICollator::Compare(&self, a: &str, b: &str) -> i32`：调用 `compareCommon(a, b, convertRuneGeneralCI)`，返回 `-1`、`0` 或 `1`。
- `Key` / `ImmutableKey`：先调用 `truncateTailingSpace`，再进入 `KeyWithoutTrimRightSpace`。两者目前都分配并返回新的 `Vec<u8>`，行为相同。
- `KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8>`：逐 `char` 取权重，并以高字节、低字节的网络序形式写入结果，所以每个 Unicode 标量固定贡献 2 字节。
- `MaxKeyLen(&self, s: &str) -> usize`：返回 `s.chars().count() * 2`；trait 层转换为 `i32`。
- `Pattern` / `Clone`：分别返回新的空 `ciPattern` 与新的空 `generalCICollator`。
- `fn key_bytes_without_trim(value: &[u8]) -> Vec<u8>`：`KeyBytes` 的内部实现；逐次调用 `decodeRune`，遇到第一个非法 UTF-8 字节立即返回已经生成的前缀 key。
- `pub struct ciPattern { patChars: Vec<char>, patTypes: Vec<u8> }`：已编译 pattern 的两个平行数组，分别保存字符与 token 类型。
- `ciPattern::Compile`：调用 `stringutil::CompilePatternInner`；`ciPattern::DoMatch` 调用 `stringutil::DoMatchCustomized`，比较闭包两次使用 `convertRuneGeneralCI`。
- `pub fn convertRuneGeneralCI(r: char) -> u32`：唯一的权重转换入口。代码点大于 `0xFFFF` 时返回 `0xFFFD`；否则以高字节索引 `planeTable`，页为 `None` 时返回原码点，页存在时再用低字节索引。
- `plane00`、`plane01`、`plane02`、`plane03`、`plane04`、`plane05`、`plane1E`、`plane1F`、`plane21`、`plane24`、`planeFF`：11 个各含 256 项的专用 BMP 页；`planeTable` 是覆盖 256 个高字节页的稀疏索引。

## 执行流程

比较流程如下：调用方按 collation 名称或 ID 经 `GetCollator`/`GetCollatorByID` 得到 trait object；`generalCICollator::Compare` 进入 `compareCommon`；公共函数先从两侧裁掉尾部 ASCII 空格，再借助 `decodeRune` 同步解码字符；任一侧出现非法 UTF-8 时立即返回相等，否则把字符交给 `convertRuneGeneralCI`，按权重字典序返回。所有对应权重相等时，再按剩余字节数判断哪一侧还有字符。

key 流程分两类。安全 `&str` 入口先按需裁尾空格，然后遍历 `chars()`，每个字符查得一个权重并输出两个字节；原始 `[u8]` 入口先在字节层裁尾空格，再逐 rune 解码，非法序列截断输出。由此，相同 collation 下 `Compare` 的相等类与 key 的权重编码保持一致，而 `KeyWithoutTrimRightSpace` 专供必须保留 PAD SPACE 差异的调用场景（例如 `pkg/util/ranger/points.rs` 会按边界语义选择 `Key` 或该方法）。

pattern 流程是先由 `CompilePatternInner` 把字面量、单字符通配符、任意串通配符和转义信息编译到 `patChars`/`patTypes`；之后 `DoMatchCustomized` 执行状态机，只有字面字符比较被替换成 general_ci 权重比较。因此 `_`、`%` 与 escape 的语法归公共 `stringutil` 管理，本文件只负责 collation 等价关系。

## 数据与状态

`generalCICollator` 本身是零字段类型，不持有缓存或连接；`Clone` 只创建另一个等价实例。`ciPattern` 是唯一的可变实例状态：`Compile` 整体替换两个向量，之后 `DoMatch` 只读它们。调用者必须先编译再匹配；再次编译会覆盖旧 pattern。

权重数据全部是进程期只读的 `static` 切片。`planeTable` 以 Unicode BMP 码点的高 8 位选页，仅为有特殊映射的 11 页保存表指针，其余页以 `None` 表示恒等映射。这既避免为整个 BMP 保存完整二维表，也保证转换无需锁、堆分配或延迟初始化。

排序 key 是新分配的 `Vec<u8>`。初始容量采用输入 UTF-8 字节长度，但最终长度由字符数决定：每个成功解码的字符恰好输出 2 字节。ASCII 通常会扩容到输入长度的两倍；多字节 BMP 字符可能比原 UTF-8 更短；非 BMP 字符虽占 4 个 UTF-8 字节，也只输出 `FF FD`。

## 依赖与调用关系

下游依赖都来自同一 crate：`Collator`/`WildcardPattern` 定义接口，`compareCommon`/`compareCommonBytes` 提供 PAD SPACE 比较骨架，`decodeRune` 区分非法字节与合法 U+FFFD，`truncateTailingSpace`/`truncateTailingSpaceBytes` 处理尾部 ASCII 空格，`stringutil` 负责编译与运行 wildcard 状态机。该文件没有直接使用 `Cargo.toml` 中的 `dbterror`、`encoding_rs` 或 `parser_charset`；这些依赖属于 crate 的其他模块与工厂边界。

上游注册边是 `collate.rs::group2_collator -> generalCICollator::default`，只在 `full_collate` feature 下存在；该 feature 是 `Cargo.toml` 默认 feature。新 collation 全局开关关闭时，`GetCollatorWithCollate` 会返回 `derivedBinCollator`，因此即使请求 general_ci 也不会执行本文件。开关开启时，名称 `utf8_general_ci`、`utf8mb4_general_ci`，以及经 charset 元数据解析到它们的 ID 33、45，会进入这里。

RustCodeGraph 的文件节点报告该文件被 17 个已索引文件引用，并点名 `pkg/util/collate/collate.rs`、若干测试以及表达式/规划器文件。仓库级 `rg` 进一步确认通用工厂在表达式、types、ranger、codec、planner、executor 和 session runtime 中被广泛调用；这些模块依赖的是 trait 契约，而不是这里的具体类型。

## 错误处理与边界

本文件的 API 不返回 `Result`，也不产生业务错误；边界通过确定性回退表达。`&str` 保证 UTF-8 合法，故字符串版 key 不存在半途失败。字节版比较若任一侧在当前位置遇到非法 UTF-8，`compareCommonBytes` 直接返回 `0`；字节版 key 则返回非法位置之前的已编码前缀。合法编码的 U+FFFD 不会被误判为非法，因为 `decodeRune` 同时返回独立的 `invalid` 标志。

`convertRuneGeneralCI` 只精确区分 BMP；所有非 BMP 字符共享 `0xFFFD` 权重，因此两个不同 emoji 或补充平面字符可能比较相等并生成相同 key。这是 Go 实现与现有测试明确要求的兼容行为，不应擅自改成完整 Unicode 排序。BMP 中未列专用页的字符按码点排序；专用页中的折叠关系完全由静态表决定。

PAD SPACE 只裁 U+0020/字节 `0x20`，不裁制表符、换行、全角空格或其他 Unicode whitespace。key 长度的 trait 返回类型为 `i32`，固有实现先计算 `usize` 再转换；对现实字符串安全，但理论上超大字符数可能发生截断，这一点目前没有显式错误通道。静态表索引依赖每个专用页恰有 256 项以及 `planeTable` 覆盖全部 256 个 BMP 高字节页。

## 并发与资源生命周期

两个核心类型都没有共享可变状态。只读静态权重表可被所有线程并发访问；`Collator: Send + Sync` 与 `WildcardPattern: Send + Sync` 的 trait 约束在编译期保证其 trait object 可跨线程使用。`generalCICollator` 可安全共享，`ciPattern::DoMatch` 可在编译完成后共享只读引用；并发调用同一个 pattern 的 `Compile` 不可能通过普通 Rust 借用规则发生，除非上层另行提供同步可变性。

每次 key 生成拥有自己的 `Vec<u8>`，每次 `Pattern`/`Clone` 返回独立堆对象，没有后台任务、锁、通道、文件句柄、事务或显式清理协议。对象离开作用域即由 Rust 自动释放。唯一相关的全局可变状态是 `collate.rs` 中的原子 new-collation 开关，不在本文件内；它决定工厂是否选中该实现，但不改变已创建实例的内部状态。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/collate/general_ci.go`。Rust 保留了 Go 的类型和方法名称、PAD SPACE 调用顺序、每 rune 两字节 big-endian key、11 个专用平面表、稀疏 `planeTable`、非 BMP 到 `0xFFFD` 的回退，以及 wildcard 自定义权重比较。`pkg/util/collate/collate_test.go` 的比较/key 表是更完整的上游语义基准，涵盖 ASCII 大小写、重音字符、尾空格、tab、`ß`、组合字符、CJK、韩文与补充平面字符。

关键语言差异是 Go `string` 可保存任意字节，而 Rust `&str` 必须是合法 UTF-8。Rust 因此在 `Collator` trait 增加 `CompareBytes`/`KeyBytes`，general_ci 覆盖它们并使用 `decodeRune` 恢复 Go 的“RuneError 且长度为 1 才算非法”判断。Rust 的 `KeyWithoutTrimRightSpace(&str)` 不需要 Go 循环中的非法序列分支；该分支只存在于 `key_bytes_without_trim`。

另一个表面差异是 Go 的 `ImmutableKey` 用于表达返回切片不可被调用者修改的契约，而 Rust 仍返回有所有权的 `Vec<u8>`；当前实现与 `Key` 一样重新分配，类型系统没有只读容器区别。Go 的私有字段/类型在 Rust 中为迁移和独立测试公开，但生产调用仍应优先通过 `Collator` 与 `WildcardPattern` trait。

## 扩展指南

- 修改字符等价关系时，应首先确认 Go `general_ci.go` 的对应表项或明确记录有意差异；同步修改相应 `planeXX`，并在独立文件 `pkg/util/collate/general_ci_2_aster_unit_test.rs` 添加最小回归。不要把测试嵌入本源文件。
- 新增专用 BMP 页时，必须提供完整 256 项表并在 `planeTable` 的准确高字节位置接线；同时验证该页两端索引，防止表长或页位偏移。更改非 BMP 策略会影响比较、索引 key、LIKE 以及持久化/分布式排序兼容性，不能只改一个入口。
- 调整 key 编码时必须保持 `Compare` 与 key 字典序一致，并同时检查 `Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`KeyBytes` 和 `MaxKeyLen`。既有 key 可能进入索引/range/codec 路径，格式变化具有数据兼容和性能风险。
- 调整非法 UTF-8 行为时应同时核对 `collate.rs::decodeRune`、`compareCommonBytes` 和本文件的 `key_bytes_without_trim`，并扩展 `collate_test.rs::test_campare_invalid_utf8_rune`；不能用 `from_utf8_lossy` 替代，因为那会把非法字节变成合法 U+FFFD 并改变提前终止语义。
- 调整 wildcard 行为时，语法解析与回溯算法属于 `pkg/util/stringutil/string_util.rs`，本文件只应维护权重相等闭包；测试放在独立 general_ci 测试或 `collate_test.rs` 中。
- 新增 collation 名称或 ID 不是本文件单点修改：还需更新 `collate.rs` 工厂和 charset 元数据，并确认 `full_collate` feature、全局开关以及关闭时的 binary 回退语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/collate` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/util/collate/general_ci.rs` 读取了完整 743 行并报告 17 个引用文件；`query generalCICollator`、`query convertRuneGeneralCI --kind function`、`query ciPattern` 核对了 Rust/Go 符号。精确 `callers`/`callees` 查询在 30 秒窗口内未返回，因此没有把其空输出当作“无调用”，而以文件节点的使用关系和下面的直接搜索补证。
- 源码与 crate 边界：`pkg/util/collate/general_ci.rs`、`pkg/util/collate/lib.rs`、`pkg/util/collate/collate.rs`、`pkg/util/collate/Cargo.toml`。其中工厂注册、feature 条件、trait 契约、公共比较/解码辅助和模块导出共同证明本文件的运行位置。
- Go 对照：`pkg/util/collate/general_ci.go` 与 `pkg/util/collate/collate_test.go`，用于核对权重表、非法 UTF-8、比较与 key 的原始语义。
- Rust 测试：`pkg/util/collate/general_ci_2_aster_unit_test.rs` 直接覆盖大小写/重音/PAD SPACE/key/最大长度/非 BMP/wildcard；`pkg/util/collate/collate_test.rs` 覆盖工厂名称和 ID、完整比较/key 表及非法 UTF-8 字节入口；`pkg/util/collate/collate_bench_test.rs` 证明 general_ci 的比较/key 路径具有独立基准入口。
- 上游调用搜索：对仓库 Rust 文件检索 `GetCollator(`、`GetCollatorByID(`，确认表达式、types、ranger、codec、planner、executor、statistics 与 session runtime 等真实消费点；对 collate 目录检索具体类型与名称，确认注册和测试边。
- 本任务只新增说明文档，不修改运行时代码，按计划不运行 Cargo。结构验证要求文档存在且恰有固定的 11 个二级章节；交付前另以 `git diff --check` 和限定路径差异复核无额外产物。
