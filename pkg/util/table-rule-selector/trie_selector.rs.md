# `pkg/util/table-rule-selector/trie_selector.rs`

## 文件定位

本文件是 `astersql-util-table-rule-selector` crate 的核心实现。crate 入口 `pkg/util/table-rule-selector/lib.rs` 公开 `trie_selector` 模块并再导出其公共 API；`pkg/util/table-rule-selector/Cargo.toml` 将库入口指定为 `lib.rs`，且以 `package.metadata.porting.go-package = "pkg/util/table-rule-selector"` 标记其 Go 来源。该 crate 没有外部 Cargo 依赖，数据结构完全由标准库的 `HashMap`、`Arc` 和 `RwLock` 组成。

它位于通用工具层，负责按 schema pattern 与可选 table pattern 保存、匹配、替换、追加、删除和枚举任意规则载荷。已核实的 Rust 上游包括：`pkg/util/filter/filter.rs::New`、`pkg/util/table-router/router.rs::NewTableRouter` 和 `pkg/util/column-mapping/column.rs::NewMapping`，三者都通过 `NewTrieSelector` 创建选择器。前两者分别将它用作复制风格过滤和表路由的底层 pattern 索引，后者将它用作列映射规则索引。

## 核心职责

- `Selector` trait 定义 `Insert`、`Match`、`Remove`、`AllRules` 四项操作；`NewTrieSelector` 返回隐藏具体实现的 `Box<dyn Selector>`。
- `trieSelector` 用两级 trie 表达规则：第一层匹配 schema；schema 叶子 item 的 `nextLevel` 指向该 schema pattern 对应的 table trie。schema 叶子自身也可以同时携带 schema 级规则。
- 每一级 trie 都能表示普通字节、末尾 `*`、单字符 `?` 和 `[range]`/`[!range]`。`*` 只能出现在 pattern 末尾，匹配零个或多个剩余字符；`?` 匹配一个字节位置；range 由若干单字节或闭区间组成。
- `Insert`、`Replace`、`Append` 控制叶子规则集合的写法；`Remove` 只清空叶子规则，不回收路径；`AllRules` 从 trie 反向还原 pattern。
- `Match` 缓存 schema/table 输入对应的 `RuleSet`，任意有效规则修改都会清空缓存，缓存条目超过 `maxCacheNum`（1024）时淘汰 `HashMap` 当前迭代到的一个键。

## 主要符号

- `Rule = Arc<dyn Any + Send + Sync>`：对齐 Go 的 `any` 规则载荷。克隆规则只增加 `Arc` 引用计数，不复制载荷对象。
- `RuleSet(Vec<Rule>)`：命中规则序列。`clone_rule_set` 新建向量并浅克隆每个 `Arc`；调用方可修改返回向量而不影响缓存或 trie 中的向量。
- `Selector: Send + Sync`：公共抽象。`Insert(schema, table, rule, insertType)` 中空 table 表示 schema 级规则，非空 table 表示二级规则。
- `trieSelector { cache, root, guard }`：`root` 是 schema trie 根；`cache` 保存 `quoteSchemaTable` 生成的查询键；`guard` 保护一次完整公共操作的 trie/cache 一致性。
- `node`：一个 trie 字符位置，分别保存 `characters: HashMap<u8, ItemRef>`、`asterisk`、`question` 和 `rItems`。
- `itemKind::{base, range}` 与 `baseItem`：item 统一持有 `child`、可选 `rule` 和可选 `nextLevel`；`rangeItem` 在这些公共字段外增加 `hasNot` 与 `ranges`。
- `ran`、`rangeItem::{equal, match_range, matchChar, str}`：描述并比较字符类。`equal` 通过双向包含判断语义等价，而非比较原 pattern 文本；`matchChar` 在否定 range 时反转结果；`str` 为 `AllRules` 重建规范化文本。
- `matchedResult { nodes, rules }`：一次层级匹配的中间结果；`rules` 收集本层规则，`nodes` 收集命中 schema item 的 table trie 根。
- `Insert = 0`、`Replace = 1`、`Append = 2`：操作模式。核心 `insert` 仅对 `Insert` 执行重复检查，仅对 `Replace` 覆盖集合；其他值与 `Append` 一样追加，因此调用方应只传公开常量。
- `quoteSchemaTable`：生成缓存键；schema/table 非空时形如 `` `schema`.`table` ``，仅 schema 时形如 `` `schema` ``，空 schema 返回空串。

## 执行流程

1. 构造：`NewTrieSelector` 调用 `trieSelector::new_empty`，创建空 cache、空根节点和操作级锁。
2. 插入 schema 规则：`Selector::Insert` 校验 schema 非空且 `rule` 为 `Some`，取得 `guard` 写锁，再由 `insertSchema` 调用核心 `insert(root, schema, Some(rule), mode)`。
3. 插入 table 规则：`insertTable` 先以 `rule=None` 将 schema pattern 路径创建或取出；若叶子没有 `nextLevel` 则创建 table trie；随后在该 trie 中插入 table pattern。这个 `None` 只表示“提取/创建路径”，不会给 schema 叶子附加规则。
4. pattern 建路：`insert` 按字节扫描，普通字节写入 `characters`，`?` 和 `*` 使用专用槽，`[` 交给 `getRangeItem`。存在闭 `]` 时按 range 语义复用等价 item，否则把 `[` 当普通字符。每个 item 都确保存在 `child`。一旦处理过 `*` 后仍有字符，返回非法 pattern 错误。
5. 写叶子：`Insert` 在已有规则时拒绝重复；`Replace` 将集合替换为单条规则；`Append` 追加。成功写规则后 `clearCache`。
6. 匹配：`Match` 取得操作级写锁，用 `quoteSchemaTable` 查 cache；命中即返回浅克隆的独立 `RuleSet`。未命中时先以 `matchNode` 匹配 schema trie，收集 schema 规则及所有命中的 `nextLevel`，再逐个匹配 table trie并合并规则，最后缓存并返回克隆。
7. 递归匹配：`matchNodeBytes` 在每个输入位置先收集 `*`；对 `?` 和所有命中的 range 分支递归处理后缀；再沿普通字节分支继续。输入耗尽后收集最终普通 item 和可匹配空后缀的 `*`。
8. 删除：`Remove` 用 `track` 按原 pattern 精确回放路径。table 非空时先取得 schema 叶子的 `nextLevel`，再跟踪 table 路径；目标叶子必须确有规则。成功后仅 `resetRule` 并清 cache，不删除 item 或 node。
9. 枚举：`AllRules` 在 `guard` 读锁下调用 `travel` 深度遍历 schema trie，同时收集 schema 规则和 `nextLevel`；再分别遍历每个 table trie，返回两张 map。

## 数据与状态

结构所有权由 `Arc<RwLock<_>>` 构成：`NodeRef` 指向 `node`，`ItemRef` 指向 `itemKind`。从 root 到 child 的边形成 trie；schema 叶子的 `nextLevel` 形成第二棵 table trie。当前实现没有父指针，`Remove` 又不剪枝，所以被删规则的路径继续占用内存并可被后续插入复用。

一个 item 的 `rule: Option<RuleSet>` 区分“无规则”和“至少曾附加规则”；删除把它恢复为 `None`。Rust 的空匹配结果是 `RuleSet(Vec::new())`，而 Go 可以返回 nil slice；公开可观察的长度/迭代语义一致，但如果跨语言测试区分 nil 与空容器，需要意识到该表示差异。

range 是字节范围而不是 Unicode 字符范围。`[!]` 被特判成字面量 `!`；`[]` 产生空 ranges，因而不匹配任何字节；缺少闭 `]` 的 `[` 按普通字节处理。等价 range 会共享 item，因此 `AllRules` 输出由 `rangeItem::str` 规范化后的 pattern，不保证保留调用方原始但语义等价的字符类拼写。

cache 键只编码原始 schema/table 字符串，不做大小写归一化或转义；大小写策略由 filter/router/mapping 等上游在插入和查询前处理。缓存值与 trie 规则一样持有 `Arc`，但向量容器独立。

## 依赖与调用关系

crate 内调用主链为：`NewTrieSelector` → `trieSelector::new_empty`；`Insert` → `insertSchema`/`insertTable` → `insert` → `getRangeItem`/`newNode`/`clearCache`；`Match` → `quoteSchemaTable` → `matchNode` → `matchNodeBytes` → `appendMatchedItem` → `addToCache`；`Remove` → `track` → `getRangeItem`/`resetRule` → `clearCache`；`AllRules` → `travel` → `insertMatchedItemIntoMap`。

已核实的直接上游关系：

- `pkg/util/filter/filter.rs::New` 把 `selector::NewTrieSelector()` 放入 `Filter::Selector`，随后 `initRules` 以规则结尾载荷填充选择器。
- `pkg/util/table-router/router.rs::NewTableRouter` 构造选择器，并让 `AddRule` 使用 `Insert`、`UpdateRule` 使用 `Replace`。
- `pkg/util/column-mapping/column.rs::NewMapping` 构造选择器；`addOrUpdateRule` 在上游校验、大小写归一化和缓存失效后，把映射规则封装为 `SelectorRule` 插入或替换。
- `pkg/util/table-router/Cargo.toml`、`pkg/util/filter/Cargo.toml`、`pkg/util/column-mapping/Cargo.toml` 均通过路径依赖引用本 crate；根 `Cargo.toml` 还将其纳入 workspace 并以 `facade_util_table_rule_selector` 暴露。

RustCodeGraph 能索引目标文件及上述文件，但对精确 `NewTrieSelector`、`quoteSchemaTable` 的 `callers/callees` 命令未输出调用边；上述上游关系因此由索引源码与 `rg` 的精确引用结果交叉确认，而不是根据同名符号推断。

## 错误处理与边界

公共错误类型是 `Result<T, String>`。`Insert` 拒绝空 schema、空规则、重复 `Insert` 和 `*` 后仍有字符的 pattern，并在 schema/table 层包装上下文。`Remove` 拒绝空 schema、找不到的精确路径、缺少 table 层或叶子已无规则；重复删除因此报错。`Match` 与 `AllRules` 不返回错误，未命中得到空 `RuleSet`。

锁中毒通过 `expect(...)` 触发 panic，而非转换成 `String` 错误。若内部结构不变量被破坏（例如已存在 item 却没有预期 child），多处 `expect` 也会 panic；正常公共 API 会在创建 item 时同步创建 child，维持该不变量。

pattern 解析没有转义语法：`*`、`?` 和存在闭括号的 `[...]` 始终具有特殊含义。`*` 只允许在末尾；多个 `*` 会因首个星号后的字符而报错。range 解析按第一个 `]` 截止，`-` 仅在具备左右字节时形成区间，未验证或重排反向区间。

Unicode 行为刻意贴近 Go 原实现的混合语义：pattern 建树按 UTF-8 字节；普通输入分支读取当前首字节，却用 `goUtf8RuneWidth` 按 rune 宽度推进；`?`/range 的递归后缀按一个字节切分。独立测试明确证明单个 `?` 不匹配两字节的 `é`。因此不能把本选择器描述为 Unicode code-point glob；新增非 ASCII 需求必须先定义跨语言兼容目标。

## 并发与资源生命周期

`Selector`、`Rule` 均要求 `Send + Sync`。公共修改操作 `Insert`/`Remove` 取得 `guard` 写锁，`AllRules` 取得读锁。当前 Rust `Match` 即使只是 cache 命中也取得 `guard` 写锁，因此多个匹配会串行；这比 Go 版本先用 `RLock` 查 cache、cache miss 后再取得写锁更保守，保证安全但可能降低高并发命中场景吞吐。

节点和 item 另有细粒度 `RwLock`，cache 也有独立 `RwLock`。它们通常嵌套在操作级 `guard` 内，保证一次匹配、修改或遍历看到一致结构。当前调用顺序统一由外层 guard 进入内部锁，未发现后台任务、通道、异步 future、事务或显式文件/网络资源。

`Arc` 负责节点、item 和规则载荷生命周期；选择器释放后且无外部 `Rule` 克隆时资源自动释放。结构边只有父到子和 schema item 到 table root，没有反向强引用，正常结构不形成 `Arc` 环。缓存上限只限制查询条目数，不限制 trie；懒删除路径会保留至整个选择器释放。

## 与 Go 版本的对应关系

Rust 文件逐段对应同目录 `trie_selector.go`：`Selector`、`RuleSet`、`matchedResult`、`trieSelector`、`node`、`baseItem`、`rangeItem` 以及插入/匹配/删除/遍历 helper 均保留 Go 命名与控制流。`item` 接口在 Rust 中改成 `itemKind` enum；Go 指针与 `sync.RWMutex` 改成 `Arc<RwLock<_>>`；Go `any` 改成线程安全的 `Arc<dyn Any + Send + Sync>`；`errors` 包的分类错误和 trace/annotate 被扁平化成带上下文的 `String`。

重要差异包括：Go `RuleSet` 可为 nil，Rust 总以向量表示；Go 的 cache hit 使用读锁，Rust `Match` 使用操作级写锁；Go `NewTrieSelector` 返回接口，Rust返回 `Box<dyn Selector>`；Go item 可依赖 nil 指针，Rust用 `Option` 和 enum 明确状态。匹配算法保留 Go 按字符串 range 偏移和 `s[i]` 字节读取的行为，Rust 通过 `goUtf8RuneWidth` 与字节切片避免在非字符边界切 `&str` 时 panic。

`pkg/util/table-rule-selector/selector_test.rs` 基本复刻 `selector_test.go` 的 fixtures 和 Insert → Match → Append → Replace → Remove 流程；`migration_aster_unit_test.rs` 额外固定了缓存失效、返回向量独立性和 `Send + Sync`；`trie_selector_test.rs` 固定了 Go 式 UTF-8 边界。由这些证据可确认当前实现目标是行为移植，而不是重新设计 glob 语义。

## 扩展指南

- 新增通配语法时，应成组修改 `node`/`itemKind` 的存储、`insert` 建路、`track` 精确回放、`matchNodeBytes` 匹配和 `travel` 文本重建；只改其中一个入口会导致无法删除、无法枚举或缓存错误。测试应放在独立的 `selector_test.rs`、`trie_selector_test.rs` 或新的同目录 `*_test.rs`，不要嵌入生产文件。
- 修改 range 语义时，应同步审查 `getRangeItem`、`rangeItem::{equal, match_range, matchChar, str}`，并用 Go 同路径实现和测试确认 `[!]`、`[]`、否定集合、重叠/等价区间与无闭括号行为。
- 新增写操作或改变规则载荷时，必须在成功改变可观察规则后调用 `clearCache`；返回集合仍应保留“容器独立、载荷共享”的约定。
- 优化并发时，最直接的入口是 `Selector for trieSelector::Match` 的 `guard` 粒度。若恢复 Go 式读锁快路径，必须证明 cache 查询与 cache miss 后 trie 匹配之间的升级窗口不会返回过期结果，并扩充并发修改/匹配回归测试。
- 若实现删除剪枝，需要同时处理共享前缀、schema item 的 `rule` 与 `nextLevel` 可独立存在、range 语义等价 item，以及 cache 失效；当前 `track` 返回完整 item 路径但不返回父 node，剪枝设计需要补充父级定位。
- 若要正式支持 Unicode glob，不应局部替换某个循环；必须先决定与 Go 的兼容策略，再整体调整 pattern 建树、普通字符键类型、`?`/range 单位、递归切片和测试，否则容易产生字节/字符混合状态。
- 扩展公共模式常量时，应考虑把当前 `i32` 改为受控 enum 或至少显式拒绝未知值；当前未知值会静默走追加分支，是兼容与误用风险。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列出 `lib.rs`、目标实现、Go 对照及三份 Rust 测试相关文件。
- RustCodeGraph 分段读取：`pkg/util/table-rule-selector/trie_selector.rs` 全部 1,151 行；模块入口 `lib.rs`；Go 对照 `trie_selector.go`；Rust 测试 `selector_test.rs`、`migration_aster_unit_test.rs`、`trie_selector_test.rs`；Go 测试 `selector_test.go`。
- RustCodeGraph/源码调用证据：`pkg/util/filter/filter.rs::New`、`pkg/util/table-router/router.rs::NewTableRouter`、`pkg/util/column-mapping/column.rs::NewMapping` 均直接调用 `NewTrieSelector`。精确 `callers/callees` 查询无输出，未将宽泛同名查询结果当作调用证据。
- 配置证据：`pkg/util/table-rule-selector/Cargo.toml`、根 `Cargo.toml` 及 filter/table-router/column-mapping 的 Cargo manifests；目标包不存在 `doc.go`。
- 行为测试证据：`selector_test.rs::TestSelector` 覆盖插入、匹配、追加、替换、删除、AllRules 和 cache；`migration_aster_unit_test.rs` 覆盖非法输入、缓存失效、返回向量独立性与多线程；`trie_selector_test.rs::question_mark_follows_go_byte_index_semantics_for_utf8` 覆盖 UTF-8 差异。
- 本任务是只读行为分析加文档产出，按任务约束未运行 Cargo；交付前以指定 shell 命令验证目标文件存在且固定二级标题恰好为 11 个，并人工复核所有重要结论均可回指上述符号或文件。
