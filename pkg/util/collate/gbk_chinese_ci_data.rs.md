# `pkg/util/collate/gbk_chinese_ci_data.rs`

## 文件定位

本文件属于 `astersql-util-collate` crate，是 `gbk_chinese_ci` 排序规则的静态权重数据层。crate 根 `pkg/util/collate/lib.rs` 以 `#[path = "gbk_chinese_ci_data.rs"]` 声明并公开重导出该模块；实际算法位于相邻的 `pkg/util/collate/gbk_chinese_ci.rs`，本文件本身不实现比较、编码、通配匹配或注册逻辑。

`pkg/util/collate/Cargo.toml` 指定 `lib.rs` 为库入口，并声明默认 feature `full_collate`；当前模块在 `lib.rs` 中是无条件接入的，没有 `cfg` 或 feature 分支。它也不直接使用 Cargo 依赖，依赖它的是同 crate 的 GBK 中文排序实现。

## 核心职责

唯一职责是提供 `gbkChineseCISortKeyTable`：以 Unicode BMP code point 为下标、以 GBK `chinese_ci` 排序权重为值的定长查找表。表长为 `0xFFFF + 1`，所以从 U+0000 到 U+FFFF 每个可能下标都有效；调用方可在完成非 BMP 检查后直接做 O(1) 索引。

表值同时表达排序次序和等价关系。例如 U+0041（`A`）与 U+0061（`a`）都映射到 `0x41`，使该 collation 大小写不敏感；U+4E2D（`中`）和 U+6587（`文`）分别映射到 `0xD321`、`0xC1AD`。大量未定义或被折叠的项使用 `0x3F`；该值也是 `gbkChineseCISortKey` 对非 BMP 字符采用的默认权重。

## 主要符号

- `pub static gbkChineseCISortKeyTable: [u16; 0xFFFF + 1]`（`pkg/util/collate/gbk_chinese_ci_data.rs`）：文件内唯一的模块级符号，也是唯一公开 API。它没有懒初始化、可变性或运行时生成步骤。
- 文件内没有类型、trait、函数、`impl`、宏定义或条件编译项。数组之外的注释明确其索引域、权重含义和默认值。
- `gbkChineseCISortKey`（`pkg/util/collate/gbk_chinese_ci.rs`）是表的直接消费者：先把 `char` 转成整数；大于 `0xFFFF` 时返回 `0x3F`，否则读取该表并扩展为 `u32`。

由于 `lib.rs` 同时 `pub mod gbk_chinese_ci_data` 和 `pub use gbk_chinese_ci_data::*`，表既能通过模块路径访问，也被重导出到 crate 根。尽管公开可见，仓库内生产 Rust 代码的直接读取点仅见 `gbk_chinese_ci.rs`。

## 执行流程

本文件没有主动执行流程；它在编译期形成静态数据，运行时只参与读取。完整调用链如下：

1. `GetCollator`/排序规则工厂在 `pkg/util/collate/collate.rs` 中把名称 `gbk_chinese_ci` 构造为 `gbkChineseCICollator`。
2. `gbkChineseCICollator::Compare`、`CompareBytes`、`KeyWithoutTrimRightSpace`、字节键生成辅助函数和 `gbkChineseCIPattern::DoMatch` 都把字符交给 `gbkChineseCISortKey`。
3. `gbkChineseCISortKey` 对非 BMP 字符直接返回 `0x3F`；BMP 字符以其 code point 索引 `gbkChineseCISortKeyTable`。
4. 比较路径比较权重；通配匹配路径以权重相等判断字符等价；键生成路径将大于 `0xFF` 的权重按高字节、低字节输出，小权重只输出低字节。

因此，修改任意表项会同时影响比较结果、排序键、索引键以及 `LIKE` 模式下的字符等价判断，而不只是显示顺序。

## 数据与状态

数组含 65,536 个 `u16`，静态有效载荷为 131,072 字节（不计目标文件/二进制节的对齐与元数据）。数据按 code point 稠密存储，空间换取常数时间查询；不存在哈希、范围压缩、缓存或运行时解压。

状态完全不可变：符号是 `static` 而非 `static mut`，元素类型也是纯值 `u16`。表内有三类值得特别注意：ASCII 范围包含大小写折叠；GBK 可排序字符通常具有两字节形式的权重；未映射项多为 `0x3F`。`0x3F` 不是错误标志，消费者会把它作为正常权重继续比较或编码，因此多个未映射字符可能被视为等价。

## 依赖与调用关系

向下依赖为零：本文件只使用 Rust 内建数组和整数类型，不引用 `dbterror`、`encoding_rs` 或 `parser_charset`。这些依赖属于整个 `astersql-util-collate` crate 的其他实现。

直接上游是 `pkg/util/collate/gbk_chinese_ci.rs` 中的 `use crate::gbk_chinese_ci_data::gbkChineseCISortKeyTable` 和 `gbkChineseCISortKey`。再向上是 `gbkChineseCICollator` 的比较/键生成方法及 `gbkChineseCIPattern`；`pkg/util/collate/collate.rs` 负责按名称注册和构造该 collator。应用侧凡通过 collator 接口执行 GBK 中文 CI 比较、排序键或通配匹配，都会间接依赖本表。

RustCodeGraph 的文件查询确认该文件已索引且只识别到一个符号，但当前索引不能按 `gbkChineseCISortKeyTable` 执行 `query/node/callers/callees`，并错误显示文件 `used by 0 files`。因此直接调用边由 `rg` 和相邻源码补证；不能把文件级 `used by 0 files` 解读为“尚未接线”。

## 错误处理与边界

本文件不返回 `Result`、不 panic、也不处理输入错误。数组边界安全由调用方 `gbkChineseCISortKey` 的分支保证：Rust `char` 最大可达 U+10FFFF，只有 `code <= 0xFFFF` 才索引 65,536 项数组，非 BMP 统一返回 `0x3F`。

重要语义边界包括：U+FFFF 是最后一个合法表下标；U+10000 及以上不会访问本表；表内 `0x3F` 与非 BMP 回退值相同。非法 UTF-8 不会进入本文件：`CompareBytes`/`KeyBytes` 的解码层在 `pkg/util/collate/gbk_chinese_ci.rs` 中处理，键生成遇到非法序列时返回截至错误前已生成的前缀。`pkg/util/collate/collate_test.rs` 的 `test_campare_invalid_utf8_rune` 覆盖了这一字节输入边界。

表长度写入类型签名，条目缺失或超出会在编译期造成数组长度不匹配；但错误权重仍是合法 `u16`，不会被类型系统发现，需要 Go 对照和行为测试防止静默语义漂移。

## 并发与资源生命周期

该表在程序映像中静态存在，生命周期覆盖整个进程；没有分配、析构、文件句柄、锁、任务、通道或事务。不可变 `static [u16; 65536]` 可被多个线程并发读取，不需要同步，也不存在初始化竞态。

每次查表不创建资源。真正可能分配的是上层 `Key`/`KeyBytes` 返回的 `Vec<u8>`，该资源属于调用方及 `gbk_chinese_ci.rs`，不是本文件管理的状态。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/collate/gbk_chinese_ci_data.go`，其唯一数据为 `[0xFFFF + 1]uint16` 类型的 `gbkChineseCISortKeyTable`。Rust 使用 `[u16; 0xFFFF + 1]`，索引域、元素宽度和名称均直接对应；Rust 用不可变 `pub static`，Go 则是包内变量，但现有 Go 代码只读取它。

对两个声明体抽取十六进制元素后，双方各有 65,536 项且逐项相等。消费逻辑也对应：Go `pkg/util/collate/gbk_chinese_ci.go` 的 `gbkChineseCISortKey(r rune) uint32` 与 Rust 同名函数都在非 BMP 时返回 `0x3F`，否则读取表并转换为 `uint32`/`u32`。

Rust 侧增加了面向字节的 trait 方法和明确的模块重导出，但没有改变本表语义。测试对照位于 `pkg/util/collate/collate_test.go` 与 `pkg/util/collate/collate_test.rs`：两者都把 `gbk_chinese_ci` 放入同一组表驱动比较和键用例，包括大小写、中文、非 BMP、未映射字符和尾空格。

## 扩展指南

若要修订 GBK 中文 CI 权重，最可能修改的就是 `gbkChineseCISortKeyTable` 对应下标。应先确认变更是与 Go/MySQL 兼容性修正，而非仅让单个测试通过；同一语义应同步到 `pkg/util/collate/gbk_chinese_ci_data.go`，并重新逐项验证 65,536 项一致。不要把非 BMP 数据强塞入本表；扩展索引域需要同时改变表类型和 `gbkChineseCISortKey` 的边界策略，属于更大的兼容性设计。

测试应优先扩展独立文件 `pkg/util/collate/collate_test.rs`，并同步 Go 的 `pkg/util/collate/collate_test.go`；若是简洁的 GBK/GB18030 键回归，也可扩展 `pkg/util/collate/bin_1_aster_unit_test.rs` 中的 `chinese_ci_weights_and_keys_match_go_tables`。至少覆盖：具体字符权重产生的键、相邻字符排序方向、预期等价字符、U+FFFF/U+10000 边界，以及该表项是否影响通配匹配。

主要风险是兼容性和持久数据一致性：权重改变会改变比较、`ORDER BY`、唯一性判断和索引键，既有索引可能与新规则不一致。性能风险主要来自增大稠密表或把 O(1) 查表替换为更复杂逻辑；公开 API 风险来自该符号已在 crate 根重导出。修改数据时还应避免复制整表造成无关 diff，并保持源文件中的 PingCAP 与 AsterSQL 版权注释。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/util/collate/gbk_chinese_ci_data.rs` 显示该文件和一个符号；`node --file ... --offset 1 --limit 240` 展示声明与表头。精确的 `query/node/callers/callees gbkChineseCISortKeyTable` 均返回未找到，故调用边使用文本搜索补证。
- 源码与装配：`pkg/util/collate/gbk_chinese_ci_data.rs`、`pkg/util/collate/gbk_chinese_ci.rs`、`pkg/util/collate/lib.rs`、`pkg/util/collate/collate.rs`。
- crate 边界：`pkg/util/collate/Cargo.toml`。
- Go 对照：`pkg/util/collate/gbk_chinese_ci_data.go`、`pkg/util/collate/gbk_chinese_ci.go`；脚本抽取声明体的十六进制值，确认 Rust/Go 均为 65,536 项且逐项相等。
- Rust 测试：`pkg/util/collate/collate_test.rs` 的 `test_utf8_collator_compare`、`test_utf8_collator_key`、`test_campare_invalid_utf8_rune`，以及 `pkg/util/collate/bin_1_aster_unit_test.rs` 的 `chinese_ci_weights_and_keys_match_go_tables`。
- Go 测试：`pkg/util/collate/collate_test.go` 的 `TestUTF8CollatorCompare`、`TestUTF8CollatorKey` 和非法 UTF-8 比较用例。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付只执行固定 11 章节的结构验证和文档范围自检。
