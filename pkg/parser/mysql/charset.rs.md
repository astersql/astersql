# `pkg/parser/mysql/charset.rs`

## 文件定位

本文件属于 `astersql-parser-mysql` crate；crate 入口 `pkg/parser/mysql/lib.rs` 通过 `pub mod charset` 将其作为公开模块暴露。它位于 parser 的 MySQL 兼容元数据层，不负责字符编码转换本身，而是集中保存 MySQL/TiDB 的字符集名、默认排序规则（collation）ID、collation 名称双向映射，以及少量字符分类辅助逻辑。编码转换由相邻的 `pkg/parser/charset/encoding.rs` 等模块承担。

`pkg/parser/mysql/Cargo.toml` 声明该 crate 使用 Rust 2024 edition，并直接依赖 `unicode-general-category = "1.1.0"`；本文件只有 `IsRangeGraph` 使用该外部依赖。文件没有 feature 条件、平台条件或条件编译项。

## 核心职责

1. `CharsetNameToID` 把字符集名称映射为其默认 collation ID；五个高频名称走直接匹配，其余名称扫描 `CharsetIDs`。
2. `Collations` 与 `CollationNames` 分别保存 `ID -> 名称` 和 `名称 -> ID` 的完整静态表，`GetCollationNameByID`、`GetCollationIDByName` 提供可表达“未找到”的查表 API。
3. `DefaultCharset`、`DefaultCollationID`、`DefaultCollationName` 等常量为 parser、server 和 charset 元数据层提供一致的 MySQL 默认值。
4. `IsUTF8Charset` 判断名称是否严格等于 `utf8` 或 `utf8mb4`，供 lexer 决定是否可跳过连接字符集转换。
5. `IsRangeGraph` 将 Unicode General Category 映射为 MySQL 标识符可接受的图形字符集合；它排除分隔符、控制字符以及其他未列出的类别。

该文件是只读元数据与纯函数集合：不注册运行时全局状态，不访问系统表，也不执行网络、磁盘或编码转换。

## 主要符号

- `pub fn CharsetNameToID(charset: &str) -> u8`：对 `utf8mb4`、`binary`、`utf8`、`ascii`、`latin1` 直接返回对应常量；其他名称在线性表 `CharsetIDs` 中查找，未知名称返回 `0`。名称比较区分大小写。
- `pub static CharsetIDs: &[(&str, u8)]`：字符集名称到默认 collation ID 的只读切片，包含 `gb18030 -> 248` 等非快速路径条目。
- `pub static Collations: &[(u16, &str)]`：按 ID 查名称的事实表。ID 不是连续区间，例如 17、76、100 不在表中，并包含 MySQL 8 相关的 255 与 309。
- `pub static CollationNames: &[(&str, u16)]`：与 `Collations` 内容反向对应的名称索引。两个表在源码中分别维护，并非运行时互相生成。
- `pub fn GetCollationNameByID(id: u16) -> Option<&'static str>`：线性扫描 `Collations`；命中时返回静态名称，未命中返回 `None`。
- `pub fn GetCollationIDByName(name: &str) -> Option<u16>`：线性扫描 `CollationNames`；比较区分大小写，未命中返回 `None`。
- 字符集与默认值常量：`UTF8Charset`、`UTF8MB4Charset`、`DefaultCharset`、`DefaultCollationID`、各字符集默认 collation ID、`DefaultCollationName` 和 `UTF8MB4GeneralCICollation`。其中 `DefaultCharset` 是 `utf8mb4`，`DefaultCollationID`/`DefaultCollationName` 是 `46`/`utf8mb4_bin`。
- `pub const MaxBytesOfCharacter: usize = 4`：依据 RFC 3629 表示一个 Unicode 字符的最大 UTF-8 字节数。
- `pub fn IsUTF8Charset(charset: &str) -> bool`：仅接受精确的 `utf8` 与 `utf8mb4`。
- `pub fn IsRangeGraph(ch: char) -> bool`：接受 Unicode 的字母、组合标记、数字、标点和符号类别；不接受空格、行分隔符、控制字符等类别。

本文件没有 struct、enum、trait、impl、宏或可变静态项。

## 执行流程

字符集默认 ID 查询从 `CharsetNameToID` 开始。调用方传入名称后，函数先匹配五个常用值以避免遍历；若未命中，则依次比较 `CharsetIDs` 中的名称；找到即返回 ID，否则以 `0` 表示未知。server 的列元数据转换路径在 `pkg/server/internal/column/convert.rs` 中使用该函数，parser 侧的默认值则直接消费常量。

collation 查询有两条对称路径：`GetCollationNameByID` 遍历 `Collations`，`GetCollationIDByName` 遍历 `CollationNames`。两者都在首次命中时结束并返回 `Some`，遍历完成后返回 `None`。`pkg/server/tests/tidb_serial_test.rs::test_default_character_and_collation` 用 ID 255 验证这组元数据能够支撑会话连接 collation/charset 的可见结果。

lexer 的字符集快路径位于 `pkg/parser/lexer.rs::convert2Connection`：它调用 `IsUTF8Charset(self.client.Name())`；若为 UTF-8 家族便原样返回 token 和文本，否则继续执行解码及连接编码转换。本文件只回答分类问题，不拥有转换缓冲区或错误处理。

`IsRangeGraph` 对单个 Rust `char` 调用 `get_general_category`，随后以穷举 `matches!` 判断其类别。当前仓库搜索只发现测试调用该 Rust 函数，未发现生产调用边；因此它是已实现、已测试但目前未观察到生产接线的公共辅助函数。

## 数据与状态

所有表和常量均为进程生命周期内的不可变静态数据。`&str` 内容具有静态生命周期，查询不分配字符串；三个映射查询均为线性扫描，时间复杂度分别为 `O(|CharsetIDs|)`、`O(|Collations|)` 和 `O(|CollationNames|)`，额外空间为 `O(1)`。

关键不变量是 `Collations` 与 `CollationNames` 应保持严格双向一致，且同一 ID、同一名称不应重复产生歧义。`pkg/parser/mysql/charset_1_aster_unit_test.rs::charset_1_maps_charsets_and_collations_like_go` 遍历每个 `Collations` 条目并反向查询，覆盖这一不变量；但因为两个切片仍由人工分别维护，新增或修改条目时必须同步更新两处。

`0` 只用于 `CharsetNameToID` 的未知名称哨兵；collation API 不复用数值哨兵，而使用 `Option`。collation ID 使用 `u16` 以容纳 309，字符集默认 ID 使用 `u8`，当前最大值 248 可表示。

## 依赖与调用关系

- crate 边界：`pkg/parser/mysql/lib.rs` 声明 `pub mod charset`；上层 `pkg/parser/lib.rs` 进一步再导出 `DefaultCharset`、`DefaultCollationName`、`IsUTF8Charset`。
- 外部依赖：`IsRangeGraph -> unicode_general_category::get_general_category`，类别枚举来自同一 crate；其余函数只依赖 Rust 标准库迭代器与本文件静态数据。
- parser 调用：`pkg/parser/lexer.rs::convert2Connection -> IsUTF8Charset`；`pkg/parser/yy_parser.rs`、`pkg/parser/parser_actions/expression.rs`、`pkg/parser/types/field_type.rs` 等读取 `DefaultCharset`。
- charset 元数据调用：`pkg/parser/charset/charset.rs` 读取 `DefaultCharset`、`DefaultCollationName`、`DefaultCollationID`，将本文件的协议默认值接入更丰富的字符集/排序规则元数据。
- server 调用：`pkg/server/internal/column/convert.rs` 使用 `CharsetNameToID` 填充协议列字符集；`pkg/server/internal/column/column.rs` 使用 `DefaultCollationID`；`pkg/server/tests/tidb_serial_test.rs` 调用双向 collation 查询核验连接变量。
- RustCodeGraph 对目标文件报告四个使用文件：`pkg/parser/lexer.rs`、`pkg/parser/mysql/charset_1_aster_unit_test.rs`、`pkg/parser/mysql/unit_test.rs`、`pkg/server/tests/tidb_serial_test.rs`。由于图的文件级关系没有覆盖所有常量引用，本次同时用 `rg` 补齐上述直接使用点。

## 错误处理与边界

本文件没有 `Result`、panic、日志或 warning。未知字符集在 `CharsetNameToID` 中静默返回 `0`，而未知 collation ID/名称返回 `None`；调用方必须区分这两种契约，不能把 `0` 当成有效默认 collation。

所有名称比较均为字节级、区分大小写的精确比较，不做 trim、别名归一化或大小写折叠。例如 `UTF8MB4` 不会命中 `utf8mb4`。ID 表保留 MySQL 的空洞，`GetCollationNameByID(17)` 应返回 `None`；`u16::MAX` 也安全地返回 `None`。

`IsRangeGraph` 的输入是合法 Unicode 标量值 `char`，因此不存在非法 UTF-8 输入。其边界由 General Category 决定：空格、换行、NUL、U+2028 行分隔符被拒绝；字母、组合音标、阿拉伯数字、破折号和货币符号等被接受。Unicode 类别数据来自依赖版本，升级 `unicode-general-category` 可能随 Unicode 数据版本改变少数字符的分类结果。

## 并发与资源生命周期

所有公开函数均为无副作用纯查询，读取不可变 `static`/`const` 数据，可被多个线程并发调用而无需锁、原子量、channel 或任务协调。函数不缓存结果、不持有借用跨越调用，也不产生需要显式释放的资源。

双向表在程序装载后存续至进程结束；返回的 collation 名称是 `&'static str`，调用方无需复制即可长期引用。查询成本随表长度线性增长，但没有初始化竞争或延迟初始化开销。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mysql/charset.go`。Rust 的 `CharsetNameToID` 保留 Go 的五个快速分支和未知 map key 返回零值的语义；Rust 用静态切片线性查找替代 Go map。`CharsetIDs`、`Collations`、`CollationNames` 以及默认值常量按 Go 表迁移，并保留不连续 ID 与 255、309 等条目。

Go 侧直接暴露两个 map，调用方用 map 访问；Rust 在静态切片之上新增 `GetCollationNameByID`、`GetCollationIDByName`，以 `Option` 明确表示缺失。仓库中未找到同名 Go 函数，因此这两个函数是 Rust 对 Go map 访问模式的封装，而不是逐函数翻译。

Go 的 `RangeGraph` 是一组 `unicode.RangeTable`，列出 `No/Mn/Me/Pc/Pd/Ps/Pe/Pi/Pf/Po/Sm/Sc/Sk/So/Lu/Lt/Nl/Ll/Lm/Lo/Mc/Nd` 等类别；Rust 的 `IsRangeGraph` 将这组集合去重后用 `GeneralCategory` 逐字符判断，语义对应。Go 文件将其描述为可用于列名的 MySQL 图形字符定义；Rust 测试抽样验证相同类别边界，但当前未观察到生产调用该 Rust helper。

## 扩展指南

新增字符集时，应同时更新 `CharsetIDs`，必要时增加明确的默认 ID 常量；只有有证据表明调用频率值得优化时才扩充 `CharsetNameToID` 的快速分支。新增 collation 必须成对更新 `Collations` 和 `CollationNames`，保持 ID、名称唯一，并同步核对 `pkg/parser/mysql/charset.go` 及 `pkg/parser/charset/charset.rs` 中更完整的字符集元数据。

测试应放在独立文件，不要嵌入本源文件。表与 Unicode 分类的首选回归位置是 `pkg/parser/mysql/charset_1_aster_unit_test.rs`；公开 API 冒烟边界位于 `pkg/parser/mysql/unit_test.rs`；若变更影响会话连接变量，再扩展 `pkg/server/tests/tidb_serial_test.rs::test_default_character_and_collation`。至少覆盖新增条目的双向 round-trip、未知值、ID 空洞和名称大小写边界。

兼容性风险主要是改变既有 ID/名称会破坏 MySQL 协议和持久元数据解释；让两个方向的表失配会导致同一 collation 无法 round-trip；改变默认常量会广泛影响 parser、字段元数据和会话行为。性能方面，当前小表的线性扫描简单且无初始化成本；若表显著增长，应在基准证据支持下考虑编译期或惰性索引，同时保持静态生命周期与并发只读属性。升级 Unicode 分类依赖前应重跑分类边界用例并核对 Go 使用的 Unicode 版本差异。

## 验证依据

- 源码与符号：`pkg/parser/mysql/charset.rs`；RustCodeGraph `node --file ... --offset 1/260` 确认 621 行源码、五个公开函数及三个静态映射表。
- crate 与依赖：`pkg/parser/mysql/Cargo.toml`、`pkg/parser/mysql/lib.rs`；确认 crate 名、Rust edition、模块导出和 `unicode-general-category` 依赖。
- 调用关系：RustCodeGraph 报告目标文件由 lexer、两个 mysql 测试文件和 server 串行测试使用；`rg` 进一步核对 `pkg/parser/lexer.rs::convert2Connection`、`pkg/server/internal/column/convert.rs`、`pkg/parser/charset/charset.rs` 等直接引用。RustCodeGraph 的函数级 callers/callees 查询未解析出有效调用边，因此未将空结果解释为“无调用”。
- Go 对照：`pkg/parser/mysql/charset.go`；确认快速路径、三张表、默认常量和 `RangeGraph` 类别集合。全仓库 Go 搜索未找到 `GetCollationNameByID`/`GetCollationIDByName` 同名函数。
- Rust 测试：`pkg/parser/mysql/charset_1_aster_unit_test.rs`、`pkg/parser/mysql/unit_test.rs`、`pkg/server/tests/tidb_serial_test.rs`；分别覆盖全表双向一致性、Unicode 图形类别、默认/未知值、309 扩展 ID 和会话 ID 255。
- Go 调用/测试证据：`pkg/server/internal/column/column_test.go`、`pkg/server/conn_stmt_params_test.go`、`pkg/server/driver_tidb_test.go` 和 `pkg/server/tests/commontest/tidb_test.go` 使用 `CharsetNameToID` 验证协议列字符集行为。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的正则检查恰好存在十一个固定二级标题，并人工复核所有行为陈述均可回溯到上述符号、调用点或测试。
