# `pkg/util/collate/gb18030_chinese_ci.rs`

## 文件定位

本文件属于 `astersql-util-collate` crate，crate 入口 `pkg/util/collate/lib.rs` 以 `gb18030_chinese_ci` 模块装入并公开重导出全部公开符号。它实现名为 `gb18030_chinese_ci` 的 GB18030 中文、不区分大小写（CI）、PAD SPACE 排序规则；`pkg/util/collate/collate.rs::new_collator` 在新 collation 开启时按该名称构造 `gb18030ChineseCICollator`。该实现不受 `full_collate` feature 控制；`Cargo.toml` 中的 `full_collate` 只控制另一组 Unicode/general/pinyin collator。

文件本身不做 GB18030 字节编解码。输入接口是 Rust UTF-8 `str` 或模拟 Go string 的原始字节切片，排序语义来自随 crate 编译进二进制的 `gb18030_weight.data`。因此它处于“collation 名称/ID 分发”与上层比较、索引 key、LIKE 匹配调用之间，而非字符集转码层。

## 核心职责

- `gb18030ChineseCICollator` 实现 `Collator` 的比较、排序 key、最大 key 长度、通配符模式和克隆接口（`gb18030_chinese_ci.rs:37-79`）。
- `gb18030ChineseCISortKey` 将一个 Unicode scalar value 映射为固定表中的 32 位排序权重；大小写或其他等价关系完全由该权重表表达（`gb18030_chinese_ci.rs:119-130`）。
- `Key`/`KeyBytes` 实现 PAD SPACE：生成 key 前移除尾部 ASCII 空格；`KeyWithoutTrimRightSpace` 保留尾空格（`gb18030_chinese_ci.rs:44-65`、`collate.rs:297-308`）。
- `gb18030ChineseCIPattern` 编译 LIKE 模式，并以“两个字符的排序权重相同”作为字符相等判据（`gb18030_chinese_ci.rs:100-116`）。

本文件不负责启停新 collation、名称/ID 解析、未知 collation 回退或 SQL 层错误构造；这些职责在 `pkg/util/collate/collate.rs`。

## 主要符号

- `static gb18030WeightData: &[u8]`：通过 `include_bytes!("gb18030_weight.data")` 嵌入的只读权重表。当前数据文件为 4,456,448 字节，恰为 `0x110000 * 4`，每个 Unicode 码点占连续 4 字节（`gb18030_chinese_ci.rs:28-29`）。
- `pub const gb18030MaxCodePoint: u32 = 0x10ffff`：权重表覆盖上界（`gb18030_chinese_ci.rs:30-31`）。Rust `char` 本身不会超过此值，所以当前 Rust 函数签名下的越界回退分支不可由安全 `char` 输入触发；该分支保留了 Go 版本对任意 `rune` 的防御语义。
- `pub struct gb18030ChineseCICollator`：无字段、`Default` 的零状态 collator。公开方法由 `Collator` trait 提供；其 `Clone` 创建另一个同样的零状态实例（`gb18030_chinese_ci.rs:33-79`）。
- `fn key_bytes_without_trim(&[u8]) -> Vec<u8>`：内部原始字节 key 生成器。它逐个模拟 Go 的 UTF-8 rune 解码，遇到首个非法序列即返回此前已生成的前缀（`gb18030_chinese_ci.rs:81-98`）。
- `pub struct gb18030ChineseCIPattern`：保存 `patChars` 与 `patTypes` 两个已编译模式数组；新实例为空，调用 `Compile` 后才能表达给定模式（`gb18030_chinese_ci.rs:100-116`）。
- `pub fn gb18030ChineseCISortKey(char) -> u32`：以 `code_point * 4` 定位数据，并按 little-endian 读取权重（`gb18030_chinese_ci.rs:118-130`）。

## 执行流程

比较路径如下：

1. `GetCollator("gb18030_chinese_ci")` 经 `new_collator` 得到 trait object；若全局新 collation 被关闭，则工厂会在进入本实现前改用 `derivedBinCollator`（`collate.rs:175-224`）。
2. `Compare`/`CompareBytes` 把 `gb18030ChineseCISortKey` 交给 `compareCommon`/`compareCommonBytes`（`gb18030_chinese_ci.rs:38-43`）。
3. 通用比较器先裁掉两侧尾部 ASCII 空格，再同步解码字符；任一侧遇到非法 UTF-8 就返回相等 `0`。否则逐字符比较 32 位权重，首次不等立即返回 `-1` 或 `1`，全部公共前缀相等时按剩余输入长度决定结果（`collate.rs:420-444`）。

key 路径如下：

1. `Key(&str)` 先调用 `truncateTailingSpace`，随后进入 `KeyWithoutTrimRightSpace`；`KeyBytes` 对原始字节使用对应的裁尾函数后进入 `key_bytes_without_trim`。`ImmutableKey` 直接复用 `Key`，返回独立拥有的 `Vec<u8>`（`gb18030_chinese_ci.rs:44-53`）。
2. 每个字符经 `gb18030ChineseCISortKey` 查得 `u32` 权重。
3. 权重按高位到低位写出，但省略所有不需要的高位字节：大于 `0xFFFFFF` 写 4 字节，大于 `0xFFFF` 写至少 3 字节，大于 `0xFF` 写至少 2 字节，最后总会写最低字节（`gb18030_chinese_ci.rs:53-65`、`81-98`）。
4. `MaxKeyLen` 按字符数乘 4 给出上界，而不是实际 key 长度（`gb18030_chinese_ci.rs:67-69`）。

LIKE 路径中，`Pattern` 创建空状态，`Compile` 委托 `stringutil::CompilePatternInner` 切分普通字符、单字符通配符、任意长度通配符及转义；`DoMatch` 委托 `DoMatchCustomized` 运行匹配，并用权重相等闭包取代码点直接相等（`gb18030_chinese_ci.rs:70-72`、`107-115`）。

## 数据与状态

排序权重是进程内静态只读数据。索引公式是 `start = (r as u32) * 4`，表项使用 little-endian 存储；生成外部排序 key 时则按有效高字节到低字节写出，这两个字节序用途不可互换（`gb18030_chinese_ci.rs:124-128`、`56-64`）。

`gb18030ChineseCICollator` 没有可变状态，所有实例行为相同。`gb18030ChineseCIPattern` 是唯一有实例状态的类型：`Compile` 整体替换 `patChars` 和 `patTypes`，`DoMatch` 只读取它们。返回的 key 均为新分配的 `Vec<u8>`；容量初值为输入 UTF-8 字节长度的两倍，但权重最坏可占每字符 4 字节，因此该容量只是减少常见场景扩容的启发式值，不是容量上界。

PAD SPACE 只移除 ASCII `0x20`，不移除制表符或其他 Unicode 空白（`collate.rs:297-308`）。原始字节路径允许输入不是合法 UTF-8，并以“首个非法字节之前的 key 前缀”作为结果；字符串路径接受 `&str`，编译期保证 UTF-8 合法。

## 依赖与调用关系

直接上游是 `pkg/util/collate/collate.rs`：它导入 `gb18030ChineseCICollator`，在 `new_collator` 的名称分支注册该实现；`GetCollator`、`GetCollatorWithCollate`、按 ID 查找以及大量 SQL 执行/编码调用方再通过 `dyn Collator` 间接使用它。RustCodeGraph 将目标文件标为由 `collate.rs` 和 `bin_1_aster_unit_test.rs` 使用；trait object 的动态方法调用不会全部表现为指向具体实现的静态边。

直接下游包括：

- `crate::collate::{compareCommon, compareCommonBytes}`：比较框架；
- `truncateTailingSpace`、`truncateTailingSpaceBytes`、`decodeRune`：PAD SPACE 与 Go 兼容的字节解码；
- `crate::stringutil::{CompilePatternInner, DoMatchCustomized}`：LIKE 模式编译和匹配；
- `gb18030_weight.data`：权重事实来源。

RustCodeGraph 的精确 callee 查询确认 `key_bytes_without_trim -> gb18030ChineseCISortKey`；对 `gb18030ChineseCISortKey` 本身没有函数 callee，因为它只做边界判断、切片和标准库字节转换。`Cargo.toml` 声明本 crate 直接依赖 `astersql-util-dbterror`、`encoding_rs` 与 `astersql-parser-charset`，但本文件只通过 crate 内模块使用标准库与 `stringutil/collate` 辅助代码，没有直接引用这三个外部依赖。

## 错误处理与边界

本 API 不返回 `Result`。合法 `char` 的查表是确定性的；理论越界时返回问号权重 `0x3f`，但 Rust `char` 最大值就是 U+10FFFF，所以当前公开签名下该分支主要是与 Go 逻辑对齐（`gb18030_chinese_ci.rs:119-123`）。

权重文件是编译期包含的受信资产。实现假定其长度至少覆盖全部 `0x110000` 个表项；若文件被截断或索引布局改变，切片会 panic，若恰有 4 字节但转换失败的内部不变量被破坏则 `expect("complete GB18030 weight")` 会 panic。当前文件尺寸验证满足该不变量，但没有运行时校验或校验和。

原始字节的 key 生成在非法 UTF-8 处静默截断；通用比较在任一侧遇到非法 UTF-8 时立即返回 `0`。这不是“替换字符后继续”的策略，调用者若依赖原始无效字节的全序，应选择 binary collator。`KeyWithoutTrimRightSpace(&str)` 不存在非法 UTF-8 分支；合法的 U+FFFD 与非法字节也由 `decodeRune` 的 `invalid` 标记区分（`collate.rs:310-337`）。

key 长度最终转为 `i32`：`MaxKeyLen` 对极端超大字符串存在从 `usize` 截断的理论风险；当前代码没有显式溢出检查。权重表内容与可变长序列化共同定义持久 key 兼容性，任意修改都可能影响索引顺序与已有编码数据。

## 并发与资源生命周期

`gb18030ChineseCICollator` 无状态且 `Collator` 要求 `Send + Sync`，可被多个线程并发调用；静态权重切片只读，无锁、无延迟初始化，也没有文件句柄或堆外资源。每次 key 调用只拥有自己的 `Vec<u8>`，实例之间没有共享可变缓冲。

`gb18030ChineseCIPattern` 的 `Compile` 需要 `&mut self`，因此编译阶段不能与同一实例上的读取并发；编译完成后的 `DoMatch(&self)` 只读模式数组，并因 `WildcardPattern: Send + Sync` 可在线程间共享。重新 `Compile` 会替换旧数组，旧内存随赋值正常释放。文件不创建任务、通道、锁、事务或显式清理动作。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/collate/gb18030_chinese_ci.go`。Rust 保留了 Go 的空 collator、嵌入式权重文件、U+10FFFF 上界、逐权重比较、PAD SPACE、1–4 字节 key 编码、最大长度、pattern 状态以及小端表读取。

关键接口差异如下：

- Go string 可含任意字节，所以 Go 的 `KeyWithoutTrimRightSpace` 必须在循环内检测 `utf8.RuneError` 且长度为 1；Rust 的 `&str` 路径天然合法，另增 `CompareBytes`/`KeyBytes` 来承接 Go 原始字节语义。
- Go `ImmutableKey` 直接调用 `KeyWithoutTrimRightSpace(truncateTailingSpace(str))`；Rust 调用 `self.Key(str_)`。因为 Rust `Key` 正是同一组合且总返回新 `Vec`，当前可观察结果一致。
- Go 的排序函数接收可能超出 Unicode 范围的有符号 `rune`，而 Rust 接收受范围约束的 `char`；因此 Go 的 `r > max` 防御分支可触达范围与 Rust 不完全相同。Go 代码也未显式防御负 rune，但正常 UTF-8 解码不会产生负值。
- Go 只暴露字符串接口；Rust 的字节接口和 `as_any` 是 Rust trait/Go 兼容层的额外接线。

现有 Go `collate_test.go` 的主表没有把 `gb18030_chinese_ci` 放入 compare/key collator 列表，非法 UTF-8 表也没有该 CI collator。因此不能把这些通用 Go 表测试当作本实现的直接覆盖证据。

## 扩展指南

修改权重或排序行为时，应首先确认变更属于 `gb18030_weight.data`、查表函数还是通用比较器；不要在本文件复制 `compareCommonBytes` 或模式引擎。权重数据或 `gb18030ChineseCISortKey` 的变化必须同时核对 Go 同路径实现和数据生成来源，并评估排序、唯一索引、分区/哈希 key、协议兼容及已有持久数据的影响。

若改变 key 编码，必须保持“数值权重比较”和“字节 key 比较”所需顺序一致，并更新 `MaxKeyLen` 上界。尤其不要把表内 little-endian 格式直接当成输出 key 格式，也不要无意补齐固定 4 字节或删除前导零规则，因为这会改变 key 的二进制兼容性和空间占用。

测试应放在独立 Rust 测试文件中，不要内嵌到本生产文件。最接近的现有测试是 `pkg/util/collate/bin_1_aster_unit_test.rs::chinese_ci_weights_and_keys_match_go_tables`，目前仅断言 `a/A` 等价、尾空格、最大长度；扩展时应在独立测试中补充中文权重 key、LIKE 大小写/中文等价、不同权重顺序、最高码点、合法 U+FFFD 与非法字节中止，以及 `Key` 和 `Compare` 顺序一致性。Go 语义变化还应同步 `pkg/util/collate/gb18030_chinese_ci.go` 的相关独立测试，而不是只让 Rust 用例通过。

若新增 collation 名或 ID，接线位置是 `collate.rs::new_collator`、parser charset 元数据及名称/ID 映射；这超出本文件单纯权重行为的职责。性能修改应以长字符串、ASCII/中文混合和多字节权重为基准，关注重复查表、`Vec` 扩容及 pattern 回调成本。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被索引为 17 个符号，并显示直接使用文件 `pkg/util/collate/collate.rs`、`pkg/util/collate/bin_1_aster_unit_test.rs`。
- RustCodeGraph 源码/符号查询：`node --file pkg/util/collate/gb18030_chinese_ci.rs`；`query gb18030ChineseCICollator`、`query gb18030ChineseCIPattern`、`query gb18030ChineseCISortKey`、`query key_bytes_without_trim`；`callers`/`callees` 查询确认图中可见的直接边，其中 `key_bytes_without_trim` 调用 `gb18030ChineseCISortKey`，部分 trait 动态调用无具体静态边。
- crate 与入口：`pkg/util/collate/Cargo.toml`、`pkg/util/collate/lib.rs`、`pkg/util/collate/collate.rs`；后者提供 trait、名称工厂、PAD SPACE、UTF-8 字节解码和通用比较实现。
- Go 对照：`pkg/util/collate/gb18030_chinese_ci.go`、`pkg/util/collate/collate.go`、`pkg/util/collate/collate_test.go`。
- Rust 测试：`pkg/util/collate/bin_1_aster_unit_test.rs` 的 `chinese_ci_weights_and_keys_match_go_tables`；同时检查 `pkg/util/collate/collate_test.rs` 和 `main_test.rs`，未发现对 GB18030 Chinese CI 中文权重表的完整专门覆盖。
- 数据资产：`pkg/util/collate/gb18030_weight.data` 的本地尺寸为 4,456,448 字节，与全 Unicode 范围每码点 4 字节的索引布局一致。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 章节结构命令校验，并人工复核公开/内部符号、调用链、错误边界、生命周期、Go 差异与测试缺口。
