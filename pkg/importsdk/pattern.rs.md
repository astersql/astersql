# [`pkg/importsdk/pattern.rs`](./pattern.rs)

## 文件定位

本文件属于 `astersql-importsdk` crate 的内部 `pattern` 模块。模块由 `pkg/importsdk/lib.rs` 的 `mod pattern;` 装配，但没有被 `pub use` 直接导出；入口 `generateWildcardPath` 也只有 `pub(crate)` 可见性。因此它不是 SDK 的外部 API，而是文件扫描流程内部用于把“一张表的一组数据文件”收敛成一条可供 `IMPORT INTO` 使用的通配路径的辅助层。

直接生产调用点是 `pkg/importsdk/file_scanner.rs` 的 `fileScanner::buildTableMeta`：该函数先构造 `TableMeta` 和数据文件清单，再以 `table.data_files`、扫描所得的全局 `all_files`、路由后的数据库名与表名调用 `generateWildcardPath`，随后通过 `buildWildcardPath` 把结果与存储 URI 组合并写入 `TableMeta.WildcardPath`。空表在调用前由 `buildTableMeta` 提前返回，所以本文件的空输入错误同时也是供其他 crate 内调用者复用的防御性契约。

crate 边界由 `pkg/importsdk/Cargo.toml` 定义：本文件直接使用 `astersql-lightning-mydump` 的 `FileInfo`/`Compression`，使用 `astersql-errors` 的共享错误与注解能力，并引用 crate 内 `ErrNoTableDataFiles`、`ErrWildcardNotSpecific` 两个错误哨兵。

## 核心职责

核心不变量是：返回的路径必须匹配本表在 `files` 中列出的全部文件，同时不能匹配 `all_files` 中任何其他表或其他用途的文件。`generateWildcardPath` 采用两级候选策略：优先生成稳定、可读的 Mydumper 命名模式；若该候选不够精确，再从实际路径计算公共前缀/后缀模式；每个候选都必须经过 `isValidPattern` 对全局文件集合验证。

本文件还承担三项配套职责：按文件扩展名保留数据格式和压缩后缀；让回退模式中的 `*` 尽量局限在单个 `/` 分隔的路径组件内；在 Rust 中实现与 Go `filepath.Match` 对齐的 `*`、`?`、字符类和反斜杠转义语义。它不扫描存储、不读取文件内容，也不决定 URI scheme；这些均由 `file_scanner.rs` 及对象存储层负责。

## 主要符号

- `generateWildcardPath(files, all_files, database, table) -> Result<String, errors::SharedError>`：crate 内主入口。它建立本表路径集合，处理空/单文件特例，按顺序尝试 Mydumper 模式和公共前后缀模式，并在都无法满足特异性时返回错误。
- `isValidPattern(pattern, table_files, all_files) -> bool`：对 `all_files` 的每个 key 比较“glob 是否匹配”和“是否属于本表”两个布尔值；两者不一致即拒绝。它隐含要求调用方传入的 `all_files` 是包含本表文件的完整扫描集合。
- `generateMydumperPattern(file, database, table) -> String`：保留首个文件的目录前缀，使用路由后的数据库名、表名以及文件的数据/压缩扩展名生成 `dir/db.table.*ext[.compression]`。数据库名或表名为空时返回空候选。
- `longestCommonPrefix(values)`、`longestCommonSuffix(values, prefix_len)`：以字节为单位计算公共前缀和不与前缀重叠的公共后缀，保持 Go 字符串索引语义；输出通过 `from_utf8_lossy` 恢复为 Rust `String`。
- `generateFlatPrefixSuffixPattern(paths)`：为空集返回空串，单值或全同值返回原值，其余返回 `prefix*suffix`。
- `generatePrefixSuffixPattern(paths)`：路径组件数一致且多于一段时逐组件调用扁平算法再用 `/` 拼接；否则退回整条字符串的扁平算法。
- `pathExtension(path)`：取得最后一个含点扩展名；它只服务于 Mydumper 候选生成。
- `pathPatternMatches`、`componentPatternMatches`：先要求模式与路径拥有相同的 `/` 组件数，再逐组件匹配，保证通配符不会跨越路径分隔符。
- `matchComponent`：带 `(pattern_index, value_index)` 记忆表的递归 glob 匹配器，处理连续 `*`、单字符 `?`、字符类、转义和普通字面量。
- `matchCharacterClass`、`escapedClassCharacter`：解析 `[...]`、`[^...]`、范围及类内转义；非法/未闭合结构通过 `Option::None` 传播为“不匹配”。

文件没有模块级常量、类型、trait、`impl` 或条件编译分支。公开程度最高的符号仍为 `pub(crate)`；glob 解析细节均是模块私有函数。

## 执行流程

1. `fileScanner::buildTableMeta` 从 `MDTableMeta` 取得本表数据文件，并从 `MDDatabaseMeta`/`MDTableMeta` 取得路由后的数据库名和表名，调用 `generateWildcardPath`。
2. `generateWildcardPath` 克隆各 `file_meta.path` 建立 `HashSet<String>`。空列表返回带 `ErrNoTableDataFiles` 的注解错误；恰有一个文件时直接返回原路径，不引入 glob。
3. 多文件首先由 `generateMydumperPattern` 查看首个文件的目录、数据扩展名和压缩状态，形成 `db.table.*` 风格候选。候选非空且 `isValidPattern` 证明其对全局集合分类完全正确时立即返回。
4. 首选候选失败后，入口收集全部实际路径，交给 `generatePrefixSuffixPattern`。路径层级一致时，该函数逐段计算公共前后缀，例如 Aurora 分区目录可得到 `export-1/db/db.users/*/part-*.parquet`；层级不一致或只有一个组件时按整条路径计算。
5. 第二候选同样必须通过 `isValidPattern`。校验经 `pathPatternMatches` 分段，再由递归匹配器解释 glob；任一本表文件漏匹配或任一外部文件误匹配都会使候选失败。
6. 两种候选都失败时，返回带 `ErrWildcardNotSpecific` 的注解错误。成功结果回到 `buildTableMeta`，在那里与存储 URI 合成最终 `WildcardPath`。

## 数据与状态

输入 `files` 是当前表的 `mydump::FileInfo` 切片；算法读取其中 `file_meta.path` 和首个文件的 `file_meta.compression`。`all_files` 是以路径为 key 的全局文件映射，值本身在校验时不读取。`database` 与 `table` 是上游元数据路由后的逻辑名称，而不要求从物理文件名重新解析。

函数不保存跨调用状态。临时状态包括本表路径 `HashSet`、回退阶段的路径 `Vec<String>`、按组件拆分的借用切片、候选字符串，以及每次单组件匹配独立创建的记忆化 `HashMap<(usize, usize), bool>`。这些对象都随同步调用结束而释放。

主要数据约束是 `table_files` 应当包含于 `all_files.keys()`。`isValidPattern` 只遍历 `all_files`，因此若调用者违反该约束、漏放某个本表路径，该遗漏路径不会被单独检查；当前生产调用点使用同一次扫描得到的全局集合来满足这一前提。`longestCommonSuffix` 的 `prefix_len` 也要求不超过每个字符串的字节长度；内部只传入刚由同一集合计算出的公共前缀长度，满足该前提。

## 依赖与调用关系

上游生产调用边由 RustCodeGraph 确认为 `pkg/importsdk/file_scanner.rs::fileScanner::buildTableMeta -> pkg/importsdk/pattern.rs::generateWildcardPath`。再向上，`buildTableMeta` 由同文件的 `GetTableMetas` 和 `GetTableMetaByName` 路径使用；因此本文件位于“扫描外部数据源并构建可导入表元数据”的链路中，而不是实际数据导入执行链中。

内部主要调用链为：

`generateWildcardPath -> generateMydumperPattern -> pathExtension`

`generateWildcardPath -> isValidPattern -> pathPatternMatches -> componentPatternMatches -> matchComponent -> matchCharacterClass -> escapedClassCharacter`

`generateWildcardPath -> generatePrefixSuffixPattern -> generateFlatPrefixSuffixPattern -> longestCommonPrefix / longestCommonSuffix`

下游 crate 依赖只有 `astersql-lightning-mydump` 与 `astersql-errors`；标准库依赖为 `HashMap`、`HashSet`。模块不调用文件系统 API，名称中的 “path” 表示逻辑路径字符串。Go 对照实现依赖 `path/filepath.Match`，Rust 为避免额外外部 glob 依赖而在本文件内实现对应语义。

## 错误处理与边界

空 `files` 返回 `ErrNoTableDataFiles` 的克隆并附加“无法为无数据文件表生成模式”的上下文。多文件没有任何安全候选时返回 `ErrWildcardNotSpecific` 并附加失败原因。`errors::Annotate(Some(...))` 随后使用 `expect`，其依据是传入值明确为 `Some`；这里的 panic 分支不是业务错误路径。

单文件总是原样成功返回，不检查该路径是否存在于 `all_files`。多文件候选为空会被拒绝。非法字符类、空字符类、类内非法 `-`/`]`、悬空反斜杠以及无法解析的匹配状态不会变成公开错误，而是被匹配层折叠为 `false`，最终可能导致 `ErrWildcardNotSpecific`；这与 Go 端把 `filepath.Match` 的错误当作无效模式处理的可观察结果一致。

`*` 可以匹配组件内零到多个字符，连续 `*` 会先合并；`?` 恰好消费一个字符；字符类支持 `^` 取反、范围和转义。`pathPatternMatches` 先比较 `/` 分段数量，所以这些操作符都不会越过 `/`。生成算法不会转义实际文件名中的 glob 元字符；例如文件名含 `[1]` 时，生成出的字符类会按 glob 语义解释，相关 Rust 测试明确要求在无法保持特异性时失败，而不是把方括号误当字面量。

公共前后缀按 UTF-8 字节求边界以贴近 Go 字符串算法；如果边界落在多字节字符中，`from_utf8_lossy` 会插入替代字符。这是当前实现明确选择的兼容折衷，扩展非 ASCII 路径行为时应重点验证。匹配器本身转为 `char` 后按 Unicode 标量匹配。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件句柄或网络连接。所有函数都只借用调用方输入或创建调用内局部集合，因此同一输入可被多个调用者并发使用，不存在共享可变状态。

资源成本主要来自路径克隆、全局集合扫描和 glob 匹配。入口为本表路径建立集合并在回退时再次克隆路径；`isValidPattern` 对 `all_files` 做线性遍历。单组件递归匹配使用索引对记忆化，避免 `*` 回溯的重复子问题，状态规模至多与模式字符数和路径字符数的乘积同阶；整体验证成本还乘以全局文件数和路径组件数。临时记忆表在每个组件匹配完成后释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/importsdk/pattern.go`，Rust 保留了相同的决策顺序、空/单文件行为、两类候选、全局特异性校验、字节式公共前后缀算法和逐路径组件回退策略。错误哨兵与主要错误文本也保持对应。

最明显的签名差异是 Go 的 `generateMydumperPattern` 从 `FileInfo.TableName` 读取路由结果，而 Rust 的 `FileInfo` 形状不在此处承载同样信息，因此 `generateWildcardPath` 和 `generateMydumperPattern` 显式接收 `database`、`table`。生产调用点从 `MDDatabaseMeta.name`、`MDTableMeta.name` 传入；`pattern_test.rs::generate_mydumper_pattern_uses_routed_table_name` 锁定了这一适配，防止错误地从 `incoming/part-0001.csv` 等物理文件名推断表名。

第二个实现差异是 Go 直接调用 `filepath.Match`，Rust 由 `pathPatternMatches` 及其私有解析函数复刻所需语义。`pattern_test.rs::generated_pattern_uses_go_filepath_character_class_semantics` 验证方括号仍是字符类而非普通文件名字节。Rust 独立测试还覆盖 Go 测试中的公共前后缀、空输入、Mydumper 压缩后缀、通用回退、冲突文件和子目录组件边界，并额外锁定 Aurora 分区目录场景。

## 扩展指南

若新增命名约定，最安全的接入点是在 `generateWildcardPath` 中把新候选放在通用公共前后缀回退之前，并且仍强制经过 `isValidPattern`；不要以“名称看起来正确”替代对 `all_files` 的排他验证。若改变压缩格式识别，应同步检查 `generateMydumperPattern`、`pathExtension` 与 `mydump::Compression` 的枚举行为。

若扩展 glob 语法，应修改 `componentPatternMatches`、`matchComponent` 或字符类辅助函数，同时以 Go `filepath.Match` 为兼容基准。特别需要增加非法模式、转义、Unicode、取反字符类、范围边界和多 `/` 组件测试。若想处理文件名中的 glob 元字符，应先明确 Go 端是否也转义，并确保生成与验证采用完全相同的语法，否则可能产生看似精确但实际误匹配的路径。

测试必须继续放在独立的 `pkg/importsdk/pattern_test.rs`，不要内嵌到生产源文件；Go 语义变化时同步参考 `pkg/importsdk/pattern_test.go`。涉及完整扫描到 `TableMeta.WildcardPath` 的行为还应扩展 `pkg/importsdk/file_scanner_test.rs`。兼容性风险集中在 Go glob 语义、路由后表名和错误哨兵；性能风险集中在超长模式、超长路径或非常大的 `all_files` 集合上的匹配成本。

## 验证依据

- 源码与符号：`pkg/importsdk/pattern.rs` 全部 341 行；关键入口为 `generateWildcardPath`，关键校验/生成函数为 `isValidPattern`、`generateMydumperPattern`、`generatePrefixSuffixPattern`，私有匹配链终止于 `matchComponent`、`matchCharacterClass`、`escapedClassCharacter`。
- 模块与依赖：`pkg/importsdk/lib.rs` 的 `mod pattern;` 和独立 `#[cfg(test)] mod pattern_test;`；`pkg/importsdk/Cargo.toml` 的 `astersql-importsdk` crate 声明以及 `astersql-errors`、`astersql-lightning-mydump` 依赖。
- 上下游调用：RustCodeGraph `status` 显示索引包含 `pkg/importsdk/pattern.rs`；文件级查询显示该文件被 `file_scanner.rs` 与 `pattern_test.rs` 使用；流程查询确认 `fileScanner::buildTableMeta -> generateWildcardPath` 以及入口到三项核心辅助函数的调用边。`pkg/importsdk/file_scanner.rs` 第 602 行起的实现确认结果写入 `TableMeta.WildcardPath`。
- Go 对照：`pkg/importsdk/pattern.go` 的八个对应函数及 `filepath.Match` 校验；`pkg/importsdk/pattern_test.go` 的公共前后缀、Mydumper、排他校验、回退和子目录用例。
- Rust 测试：`pkg/importsdk/pattern_test.rs` 的 11 个独立测试覆盖公共前后缀、候选校验、空表、单文件、压缩 Mydumper、Aurora 分区目录、回退/冲突、组件边界、路由表名和字符类语义。
- 本任务为纯文档分析，按任务约束未运行 Cargo 或代码测试；交付结构通过任务文件规定的 11 个固定二级标题检查，并人工复核未把测试放入生产文件、未把未验证设想写成现状。
