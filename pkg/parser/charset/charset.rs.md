# `pkg/parser/charset/charset.rs`

## 文件定位

本文件是 `astersql-parser-charset` crate 的字符集与排序规则元数据注册表。crate 入口 [`lib.rs`](./lib.rs) 通过 `include!("charset.rs")` 将其置于 `charset` 模块，并只在 crate 根重导出少量常用字符集常量；完整 API 通过 `parser_charset::charset::*` 使用。它不负责字节编码转换，后者位于同 crate 的 `encoding*.rs`。`Cargo.toml` 表明该 crate 直接依赖 `astersql-errors`、`astersql-parser-mysql`、`astersql-parser-terror` 和 `log`，分别为错误值、MySQL 常量/错误码、类型化错误与告警日志提供边界。

## 核心职责

文件承担四类职责。第一，定义 `Charset`、`Collation`、字符集/排序规则名称和 `PAD SPACE` 属性等 MySQL 兼容元数据。第二，维护“可编码支持”的 7 个字符集与“MySQL 已知”的完整字符集目录，从而区分 unsupported 和 unknown。第三，从 273 条静态 `collations` 记录构建按 ID、按名称、按字符集归属的查询索引，并筛出 7 条公开支持排序规则。第四，提供大小写归一化、`utf8mb3` 别名、默认值查询以及自定义字符集/排序规则注册接口。依据是 `CharacterSetInfos`、`charsets`、`collations`、`ensure_initialized` 及所有公开查询/变更函数。

## 主要符号

- `Charset { Name, DefaultCollation, Collations, Desc, Maxlen }` 描述一个字符集；`Collations` 是按排序规则名索引的成员快照。`Collation { ID, CharsetName, Name, IsDefault, Sortlen, PadAttribute }` 描述 MySQL 排序规则编号、归属及比较属性。Rust 使用拥有所有权的 `String`/值对象，而 Go 对照使用指针。
- `CharacterSetInfos` 是当前真正支持编码处理的注册表：`utf8`、`utf8mb4`、`ascii`、`latin1`、`binary`、`gbk`、`gb18030`。`charsets` 是更宽的 MySQL 目录，主要用于区分“不支持”与“不认识”。`TiFlashSupportedCharsets` 另列 TiFlash 接受的 5 项白名单。
- `collationsIDMap`、`collationsNameMap`、`supportedCollations` 是派生索引；`supportedCollationNames` 决定静态排序规则是否进入受支持列表；`INITIALIZE_COLLATIONS` 与 `ensure_initialized` 保证只建表一次。
- 查询入口包括 `GetSupportedCharsets`、`GetSupportedCollations`、`ValidCharsetAndCollation`、`GetDefaultCollationLegacy`、`GetDefaultCollation`、`GetDefaultCharsetAndCollate`、`GetCharsetInfo`、`GetCharsetInfoForIntroducer`、`GetCharsetInfoByID`、`GetCollationByName`、`GetCollationByID`。
- 变更入口包括 `AddCharset`、`RemoveCharset`、`AddCollation`、`AddSupportedCollation`；内部 `add_collation` 完成多索引接线。`all_collations_for_test` 仅在 `cfg(test)` 下可见。
- `ErrUnknownCollation` 与 `ErrCollationCharsetMismatch` 对齐 DDL 标准错误；后者在本文件中只定义供外部使用。`PadSpace`/`PadNone` 以及 `Charset*`、`Collation*` 常量避免调用者散落硬编码。

## 执行流程

首次进入查询或公开变更函数时，`ensure_initialized` 通过 `Once::call_once` 顺序遍历 `collations`。每条记录进入 `add_collation` 后写入 ID/名称索引；若名称位于 `supportedCollationNames`，再追加到 `supportedCollations`；最后分别挂到 `CharacterSetInfos` 和完整 `charsets` 中对应字符集的 `Collations`。因此查询前无需调用者显式执行 `init`，文件末尾的 `init` 也只是同一惰性初始化入口。

名称查询先做规范化。`GetCharsetInfo` 将输入转小写并把 `utf8mb3` 映射为 `utf8`：先查支持表，再查完整目录，最后判为未知。`ValidCharsetAndCollation` 还把空字符集当作 `utf8`，空排序规则直接合法，否则经 `utf8Alias` 归一化后检查字符集成员表。`GetCollationByName` 同样小写化并转换三个 `utf8mb3_*` 旧名；ID 查询直接访问 ID 索引。`GetSupportedCharsets` 返回值快照并按 `Name` 排序，`GetSupportedCollations` 保留静态登记顺序。

应用主链中的代表性入口是：`pkg/parser/lexer.rs` 用 `GetCharsetInfoForIntroducer` 识别 `_charset` token；`pkg/parser/parser_actions/expression.rs` 用字符集、排序规则和 legacy 默认规则构造表达式；`pkg/ddl/create_table.rs` 校验表/列字符集与排序规则；`pkg/sessionctx/variable/varsutil.rs` 校验会话变量；`pkg/format/textrow/result_encoder.rs` 按协议排序规则 ID 选择结果字符集。

## 数据与状态

静态源数据分成两层：`CharacterSetInfos` 的 7 项决定当前支持能力，`charsets` 的 41 项提供已知目录；`collations` 有 273 项，记录 MySQL/TiDB 排序规则 ID、默认标记、sort length 和 pad 属性。公开 API 返回克隆值或新 `Vec`，调用者不能通过返回对象绕过锁直接修改注册表。

所有可变全局表都包在 `LazyLock<RwLock<_>>` 中。添加字符集会按原样名称写入支持表；添加排序规则会更新两个全局索引和两个字符集目录，但只有白名单名称自动进入受支持列表。`AddSupportedCollation` 可无条件追加，因而调用者必须自行避免重复。`RemoveCharset` 删除同名字符集，并按“排序规则 `Name` 等于传入字符集名”过滤受支持列表；这是对 Go 当前实现的逐句保留，不会清理 ID/名称索引或字符集已有排序规则，扩展者不应误解为完整级联删除。

## 依赖与调用关系

向下依赖中，`mysql::DefaultCharset`、`DefaultCollationName`、`DefaultCollationID` 和错误码提供协议默认值；`terror::ClassDDL.NewStd`/`GenWithStackByArgs` 生成兼容错误；`errors::Errorf` 构造普通共享错误；`log::warn!` 记录未知 ID 的回退。标准库的 `HashMap`/`HashSet` 保存目录和白名单，`LazyLock`/`RwLock`/`Once` 管理共享状态及一次性初始化。

向上调用分布在解析、DDL、表达式、协议和工具层。RustCodeGraph 的文件节点报告本文件被 51 个文件使用；直接检索确认 `pkg/types/datum.rs` 和 `pkg/util/collate/collate.rs` 使用排序规则查询，`pkg/server/internal/column/convert.rs` 使用字符集 `Maxlen`，`pkg/meta/metabuild`、`pkg/expression` 与 `pkg/util/generatedexpr` 获取服务端默认字符集/排序规则。该文件因此是元数据兼容层，不是实际字符串比较器或编码器。

## 错误处理与边界

`GetCharsetInfo` 明确区分三种结果：支持项成功；完整目录中存在但未支持时返回 `Unsupported charset ...`；目录中也不存在时返回 `Unknown charset ...`。`GetCharsetInfoForIntroducer` 是词法器专用例外：已知但不支持也返回元数据，使后续解析错误能保留 introducer 名称。`GetDefaultCollationLegacy` 只接受旧解析器支持的五类字符集及 `utf8mb3` 别名，即使新表支持 GBK/GB18030 也会拒绝。

未知排序规则名通过 `ErrUnknownCollation` 返回 DDL 1273 语义；未知 ID 使用普通格式化错误。`GetCharsetInfoByID` 是有意的容错接口：失败时记录 warning，返回服务端默认字符集/排序规则，同时把错误放在第三个返回值中，调用者不可只看前两个值。所有 `RwLock` 获取都调用 `unwrap()`，锁中毒会 panic；代码没有恢复分支。静态表和注册接口不验证重复 ID、重复名称、默认项唯一性或排序规则与字符集是否一致，这些是不变量要求而非运行时防线。

## 并发与资源生命周期

`Once` 保证静态排序规则表在并发首次访问时只装载一次，`LazyLock` 保证各表按需构造，`RwLock` 允许并发读并串行化写。查询在持有读锁期间克隆结果；写函数只在局部持有写锁，没有跨调用保存 guard，也没有线程、异步任务、通道、I/O、事务或需要显式释放的外部资源。

锁顺序值得维护：`add_collation` 依次写 ID 表、名称表、支持列表、支持字符集表、完整字符集表，当前每个 guard 在单条语句后释放。修改时不要同时长期持有多个表锁，否则可能与其他注册路径形成死锁。自定义注册是进程全局且无作用域回滚；`charset_test.rs` 因而用子进程隔离增删测试，避免并行测试污染其他用例。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/parser/charset/charset.go`，Rust 保留了结构字段、常量、两层字符集目录、静态排序规则内容、公开函数命名和主要分支。Go 在 `init()` 中立即填表；Rust 改为 `Once` 惰性填表，但任何公开查询/变更入口都会先初始化。Go 返回共享指针，Rust 返回克隆值，并用 `RwLock` 替代无锁 package map，以适应 Rust 全局可变状态约束。

Rust 额外提供 `GetCharsetInfoForIntroducer`，用于保持扫描器对已知但不支持 introducer 的报错路径；Go 扫描器可直接利用 `GetCharsetInfo` 返回的对象与错误。Rust 的 `RemoveCharset` 保留 Go 按 `supportedCollations[i].Name == c` 删除的现状，文档不把它描述成按 `CharsetName` 级联清理。独立 Rust 测试 `charset_test.rs` 对齐 `charset_test.go` 的合法组合、默认值、未知错误、全量排序规则、自定义注册和 `utf8mb3` 别名；`charset_1_aster_unit_test.rs` 另验证 7 项排序、7 个受支持排序规则及 unsupported/unknown 区分。

## 扩展指南

新增“当前支持”的字符集时，应同步 `CharacterSetInfos`、相关 `Charset*`/默认 `Collation*` 常量、`supportedCollationNames` 和必要的编码实现；若 TiFlash 也支持，再更新 `TiFlashSupportedCharsets`。仅增加兼容目录项时修改 `charsets`，不要错误宣称已有编码支持。新增排序规则时维护 `collations` 的唯一 ID/名称、正确 `CharsetName`、默认标记、`Sortlen` 和 `PadAttribute`，并决定是否加入公开支持白名单。

修改归一化或错误语义时，优先检查 `GetCharsetInfo`、`GetCharsetInfoForIntroducer`、`utf8Alias`、`GetCollationByName` 和 `GetCharsetInfoByID` 的调用者。同步测试应放在独立文件 `pkg/parser/charset/charset_test.rs` 或相应集成测试中，不要嵌入生产源文件；同时核对 Go 的 `charset.go`/`charset_test.go`，避免 Rust 逻辑被简化。兼容风险主要是协议 ID、默认排序规则与错误文本，性能风险主要是扩大静态表克隆或增加锁竞争；自定义删除若要改为级联语义，则属于行为变更，必须新增回归测试并审查既有 Go 兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/parser/charset` 找到目标源码、crate 入口及独立测试；`node --file pkg/parser/charset/charset.rs --offset 1 --limit 500` 与 `--offset 480 --limit 300` 覆盖全部 726 行，并报告 51 个使用文件。对重名函数的裸 `callers/callees` 无法在 Go/Rust 候选间消歧，因此调用点以限定 `.rs` 的 `rg` 结果补充。
- 源码与边界：`pkg/parser/charset/charset.rs`、`pkg/parser/charset/lib.rs`、`pkg/parser/charset/Cargo.toml`；表项计数由对 `charset_row`/`collation_row` 构造行的限定检索获得（7 + 41 个字符集构造、273 个排序规则构造）。
- Go 对照与测试：`pkg/parser/charset/charset.go`、`pkg/parser/charset/charset_test.go`、`pkg/parser/charset/charset_test.rs`、`pkg/parser/charset/charset_1_aster_unit_test.rs`。
- 调用证据：`pkg/parser/lexer.rs`、`pkg/parser/parser_actions/expression.rs`、`pkg/ddl/create_table.rs`、`pkg/sessionctx/variable/varsutil.rs`、`pkg/format/textrow/result_encoder.rs`、`pkg/server/internal/column/convert.rs`、`pkg/types/datum.rs`、`pkg/util/collate/collate.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收以目标文件存在且固定二级标题恰好 11 个为准。
