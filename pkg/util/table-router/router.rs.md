# `pkg/util/table-router/router.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-table-router`，crate 入口 `pkg/util/table-router/lib.rs` 将本模块及其公开项全部再导出。它移植自同目录 `router.go`，提供两层职责：一是定义可由配置和其他路由实现复用的 `TableRule` 及三种提取器；二是实现基于 `astersql-util-table-rule-selector` trie 的旧式 `Table` 路由器。

当前仓库的运行时接线需要区分“规则类型”和“旧路由实现”。`pkg/util/regexpr-router/regexpr_router.rs` 通过 `pub use router_crate::router` 复用这里的 `TableRule`、校验和提取器定义，Lightning 的 `lightning/pkg/importer/import.rs::addExtendDataForCheckpoint` 实际构造的是 `NewRegExprRouter`。仓库搜索未发现测试之外直接构造本文件 `NewTableRouter` 的生产调用；因此 `Table` 目前主要是 Go 兼容实现与回归基准，不能表述为 Lightning 当前扩展列流程的实际路由器。

## 核心职责

- `TableRule::Valid` 校验必填字段并按 table、schema、source 的固定次序编译提取正则。
- `TableRule::ToLower` 为大小写不敏感模式归一化两个源 pattern；目标名称与提取正则不改写。
- `NewTableRouter`、`Table::{AddRule,UpdateRule,RemoveRule}` 管理 trie 中的规则，并保留 Go 风格错误上下文。
- `Table::Route` 区分 schema 级与 table 级命中，优先 table 级规则，拒绝同一级多规则路由，并对空目标字段回退原输入。
- `Table::FetchExtendColumn` 选择一条命中规则，按 table → schema → source 顺序返回扩展列名和值。
- `compile_extractor` 委托相邻 `go_regex.rs` 将 Go regexp 方言转换并交给 Rust `regex`，使校验和捕获行为尽量与 Go 对齐。

## 主要符号

- `RouterResult<T> = Result<T, String>`：本 crate 的字符串错误边界，没有独立错误枚举或错误链类型。
- `TableRule`：公开规则载体，包含 `SchemaPattern`、`TablePattern`、`TargetSchema`、`TargetTable`，以及三个可选 boxed extractor。`TablePattern == ""` 是 schema 级规则判据。
- `TableExtractor`、`SchemaExtractor`、`SourceExtractor`：分别保存公开的目标列名、正则原文，以及 crate 内可见的已编译 `Option<Regex>`。只有成功经过 `Valid` 的相关提取器才可安全执行。
- `Table`：持有公开的 `Box<dyn selector::Selector>` 和私有 `caseSensitive`。公开 `Selector` 是兼容 Go 嵌入接口的逃生口，也允许调用方绕过本文件校验。
- `TableRouter = Table`：仅为命名兼容提供的类型别名。
- `NewTableRouter(caseSensitive, rules)`：创建 `NewTrieSelector`，逐条以 `selector::Insert` 插入；任一规则失败即返回带 `initial rule ...` 上下文的错误。
- `Table::{AddRule,UpdateRule}`：共同进入私有 `insert_rule`，区别只是向 selector 传递 `Insert` 或 `Replace`。
- `Table::RemoveRule`：按归一化后的 pattern 删除，不重新校验目标字段或提取器。
- `Table::{Route,FetchExtendColumn}`：只读查询入口；前者返回目标二元组或冲突/类型错误，后者按 Go API 约定用空向量吞掉非法动态规则类型。
- `classify_rules`：把动态 `RuleSet` downcast 为 `TableRule` 引用，并按 `TablePattern` 是否为空分组。
- `ExtractorRef`、`extractVal`：统一三种提取器的正则访问；跳过完整匹配组 0，将所有存在的捕获组直接拼接。
- `go_lowercase`、`go_rule_value`、`Display for TableRule`：分别模拟 Go 单 rune 小写、动态值诊断与 `%+v` 风格规则输出。

## 执行流程

构造或新增/更新规则时，流程是：`Valid` 先拒绝空 `SchemaPattern`、空 `TargetSchema`，再逐个编译存在的 extractor 并检查 `TargetColumn`；大小写不敏感时调用 `ToLower`；最后把规则克隆进 `Arc<dyn Any + Send + Sync>`，交给 selector 的 `Insert` 或 `Replace`。构造器按输入顺序执行，遇到第一条错误立即结束。

`Route(schema, table)` 先按 `caseSensitive` 决定是否对查询键调用 `go_lowercase`，再执行 `Selector::Match`。`classify_rules` 要求每个动态值都是 `TableRule`：空 `TablePattern` 进入 schema 组，其余进入 table 组。有非空 table 且 table 组非空时选择 table 组，否则选择 schema 组；被选择的同级组超过一条即返回“不支持多目标”的错误。零条命中或规则目标字段为空时，对应结果回退到调用者传入的原始 schema/table，因此匹配规范化不会改变回退结果的大小写。

`FetchExtendColumn(schema, table, source)` 直接以传入的 schema/table 调用 `Match`，这一点与 `Route` 不同：它不会在大小写不敏感模式下自行小写化查询参数，这与 Go `FetchExtendColumn` 当前实现一致。分类失败或无规则时返回两个空向量；否则取第一条 table 级规则，若无则取第一条 schema 级规则，并依次对 table、schema、source extractor 执行捕获。正则未命中产生空字符串，但仍会输出对应列名；多个捕获组按序无分隔拼接。

## 数据与状态

路由器的持久状态只有 `caseSensitive` 和 selector。规则插入 selector 时以 `Arc` 包装克隆后的 `TableRule`，因此后续修改调用方原值不会修改已注册规则；`UpdateRule` 通过同 pattern 的替换产生新快照。提取器中的 `regexp` 是从正则原文派生的缓存状态，`Valid` 会原地逐项填写，且不是事务式校验：例如 table 正则成功、schema 正则失败时，table 的已编译状态会保留。由于公开 API 按值接收规则，这种部分状态通常只在显式直接调用 `TableRule::Valid` 时对调用者可见。

底层 selector（`pkg/util/table-rule-selector/trie_selector.rs`）把规则定义为 `Arc<dyn Any + Send + Sync>`，以 `RwLock` 保护 trie、缓存和操作边界，并将匹配缓存限制为最多 1024 项；插入或删除负责使缓存失效。本文件不复制该缓存，而是借用 `RuleSet` 中的 `Arc` 并 downcast。

## 依赖与调用关系

直接依赖由 `pkg/util/table-router/Cargo.toml` 声明：`astersql-util-table-rule-selector` 提供 `Selector`、`RuleSet`、`NewTrieSelector` 以及 `Insert/Replace` 模式；`regex` 提供编译结果和捕获；`regex-syntax` 被相邻 `go_regex.rs` 用于 Go 方言转换与重复次数校验。crate 没有 feature 条件，`router.rs` 也没有条件编译项。

RustCodeGraph 对 `router.rs::NewTableRouter` 的精确节点显示其调用 `Table` 构造和 `insert_rule`，并显示调用者全部来自 `router_test.rs`；仓库搜索另确认 `migration_aster_unit_test.rs` 与 `pkg/util/regexpr-router/*test.rs` 的兼容性调用。生产侧，`pkg/util/regexpr-router/Cargo.toml` 依赖本 crate 并复用 `TableRule`，根 `Cargo.toml`/`pkg/lib.rs` 还通过 facade 导出该 crate；Lightning importer 依赖它承载 `Config.Routes` 的规则类型，但实际执行路由的是 regexpr router。

下游调用链为：规则管理 → `TableRule::{Valid,ToLower}` → `Selector::{Insert,Remove}`；查询 → `Selector::Match` → `classify_rules` → 目标选择或 `extractVal`；正则校验 → `compile_extractor` → `go_regex::compile` → `regex`。公开 `Selector` 也允许外部直接插入任意动态值，这条旁路是错误边界测试的来源。

## 错误处理与边界

规则必须有非空源 schema pattern 和非空目标 schema。存在的 extractor 必须有 Go 方言下合法的正则和非空目标列；`TargetTable`、`TablePattern` 允许为空。`AddRule` 的重复 pattern、`UpdateRule` 的缺失/替换错误以及 `RemoveRule` 的不存在错误均由 selector 产生，再由本文件增加 add/update/remove 上下文。

`Route` 不支持同一级别多个命中：只报告数量和前两条规则。selector 中若被公开接口注入非 `TableRule` 值，`classify_rules` 返回 `table route rule ... not valid`；`Route` 传播该错误，而 `FetchExtendColumn` 返回空结果。若通过公开 `Selector` 注入含未编译 extractor 的 `TableRule`，`extractVal` 会以 `extractor regexp must be initialized by TableRule::Valid` panic；这是刻意对齐绕过 Go 校验后的编程错误，不是普通“不匹配”。

大小写不敏感采用 `go_lowercase` 的逐字符单一映射，而不是 Rust `str::to_lowercase` 的多字符展开；测试以 `İ` 覆盖差异。正则转换还限制 Go 不接受的 Rust 扩展、ASCII Perl 类、重复上限与捕获语义。`go_rule_value` 对未知动态类型无法复刻 Go reflection，只输出 `Arc` 指针身份，这是诊断文本的已知兼容边界。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务、文件句柄或网络资源。`Regex`、`String`、`Box` 和 `Arc` 均随规则/路由器所有权自动释放。selector trait 要求 `Send + Sync`，实际 trie 使用 `RwLock` 支持并发匹配及内部缓存更新；`Route` 和 `FetchExtendColumn` 只需 `&self`。规则管理方法在本文件 API 上要求 `&mut self`，因此正常调用者必须独占 `Table`，即使底层 selector 的方法本身接受 `&self`。

由于 `Selector` 字段公开，持有共享 `&Table` 的代码也能直接调用 selector 的内部可变方法；这样可以绕过 `Valid`、大小写归一化和 `Table` 的 `&mut` 管理边界。扩展代码应优先使用 `AddRule/UpdateRule/RemoveRule`，并把直接 selector 操作限制在针对错误边界的测试中。

## 与 Go 版本的对应关系

同目录 `router.go` 是直接语义基准。Rust 保留了 Go 的公开类型/字段/方法命名、schema/table 两级优先级、目标回退、重复冲突、三种 extractor 顺序和多捕获组拼接。`router_test.go` 的 `TestRoute`、`TestCaseSensitive`、`TestFetchExtendColumn` 在独立 `router_test.rs` 中有对应主流程测试。

所有权层面，Go 接受并原地修改 `*TableRule`，Rust 的构造和增删改按值接收并在存入 selector 前克隆，避免注册规则受调用者后续修改；行为测试以显式 clone 模拟 Go 生命周期。Go 的 `error`/PingCAP errors 包装在 Rust 中压平为精确格式的 `String`。Go `regexp` 由 `go_regex.rs` 兼容层映射到 Rust regex；`router_test.rs` 额外覆盖 Go 1.25.10 的 regexp oracle、错误文本、部分校验状态和未校验 panic。

需要特别保留的相同点包括：`FetchExtendColumn` 不执行大小写归一化；校验会留下先前 extractor 的部分编译状态；正则不匹配仍返回列名和空值；公开 selector 可承载任意动态规则。Rust 特有的 `TableRouter` 别名不是 Go 新行为，只是命名兼容。

## 扩展指南

新增规则字段时，应同步修改 `TableRule`、`Valid`、必要的 `ToLower`、`Display`，以及 `pkg/util/regexpr-router/regexpr_router.rs` 对共享规则的消费；若字段来自配置，还要检查 Lightning 配置编解码链。新增 extractor 时，还需扩展 `ExtractorRef`、`extractVal`、`FetchExtendColumn` 的固定输出顺序和 `go_regex.rs` 兼容要求。

修改匹配优先级或冲突策略时，入口是 `classify_rules`、`Route` 和 `FetchExtendColumn`，同时评估 regexpr router 是否必须保持同样语义。修改规则生命周期时应保持 selector 的缓存失效契约，避免绕过 `Insert/Replace/Remove`。不要把测试嵌回源文件；应更新同目录独立 `router_test.rs`，并视 Go 行为变更同步 `router_test.go` 或补充 `migration_aster_unit_test.rs`。跨实现兼容还应更新 `pkg/util/regexpr-router/regexpr_router_test.rs`。

主要兼容风险是 Go/Rust Unicode 小写与 regexp 方言差异、精确错误文本差异和公开动态 selector 的 downcast；正确性风险是 table/schema 级优先级或空目标回退改变；性能风险集中在规则变更导致缓存频繁失效、复杂正则编译，以及匹配后对目标字符串和扩展值的分配。当前文档任务未改变代码，以上是后续改动的检查清单。

## 验证依据

- 目标实现与模块装配：`pkg/util/table-router/router.rs`、`pkg/util/table-router/lib.rs`、`pkg/util/table-router/go_regex.rs`。
- crate 与依赖边界：`pkg/util/table-router/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- selector 数据结构和同步依据：`pkg/util/table-rule-selector/trie_selector.rs` 中的 `Rule`、`Selector`、`RuleSet`、`trieSelector`、`NewTrieSelector`、`Insert/Replace`。
- Go 对照：`pkg/util/table-router/router.go`、`pkg/util/table-router/router_test.go`。
- Rust 独立测试：`pkg/util/table-router/router_test.rs` 覆盖主路由、大小写、错误上下文、Go regexp oracle、部分校验状态与未初始化 panic；`pkg/util/table-router/migration_aster_unit_test.rs` 覆盖移植生命周期、冲突和捕获拼接。
- 当前调用接线：`pkg/util/regexpr-router/Cargo.toml`、`pkg/util/regexpr-router/regexpr_router.rs`、`pkg/util/regexpr-router/regexpr_router_test.rs`、`lightning/pkg/importer/Cargo.toml`、`lightning/pkg/importer/stubs.rs`、`lightning/pkg/importer/import.rs::addExtendDataForCheckpoint`。
- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter pkg/util/table-router` 命中目标 Rust/Go/测试文件；精确 `node router.rs::NewTableRouter` 确认其调用 `insert_rule`，并列出 `router_test.rs` 的构造调用者。宽泛 `explore` 因同名 `Route/table` 噪声未作为跨模块调用结论，跨 crate 接线改用限定 `rg` 核对。
- 人工复核结论：本文件存在的原因是共享 Go 兼容规则模型并保留 trie 路由实现；运行路径由规则校验/归一化、selector 变更或匹配、规则分级、路由/捕获组成；安全扩展必须同步共享规则消费者和独立测试，并保留上述兼容边界。
