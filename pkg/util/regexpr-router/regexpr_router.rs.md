# `pkg/util/regexpr-router/regexpr_router.rs`

## 文件定位

本文件是 `astersql-util-regexpr-router` crate 的核心实现，提供一张由正则、glob 或字面量库表规则组成的路由表。crate 入口 [`lib.rs`](lib.rs) 通过 `include!("regexpr_router.rs")` 装入本文件并公开再导出其 API；workspace 根 [`Cargo.toml`](../../../Cargo.toml) 以 `facade_util_regexpr_router` 引入该 crate，[`pkg/lib.rs`](../../lib.rs) 又将它暴露为 `pkg::util::regexpr_router` 门面。

它对应 Go 文件 [`regexpr_router.go`](regexpr_router.go)，位于数据导入配置到实际文件元数据之间：规则把输入 schema/table 映射到目标 schema/table，并可从 table、schema、source 三类字符串提取扩展列。当前检索到的 Rust 生产接线是 `lightning/pkg/importer/import.rs::addExtendDataForCheckpoint`：仅当至少一条路由规则配置提取器时构造本路由器，从数据文件名取得 schema/table，调用 `FetchExtendColumn`，再把列名和值写入 checkpoint。Go 侧除 importer 外还在 `pkg/lightning/mydump/loader.go` 使用路由；当前 Rust 同路径 loader 未检索到等价直接调用，因此不能把 Go 的全部接线视为 Rust 现状。

## 核心职责

- `NewRegExprRouter` 与 `RouteTable::AddRule` 校验并编译 `router::TableRule`，按加入顺序保存规则。
- `AddRule` 以 `TablePattern` 是否为空区分库级规则和表级规则，并借助 `astersql-util-filter` 统一支持字面量、glob 与 `~` 前缀正则匹配。
- `RouteTable::Route` 收集所有命中规则：有表级命中时表级优先，否则采用库级规则；同一层级多规则命中时拒绝歧义。
- `RouteTable::AllRules` 将已保存规则按库级/表级拆分，并分别维持原添加顺序。
- `RouteTable::FetchExtendColumn` 从第一条适用规则的提取器生成扩展列，优先表级规则，提取顺序固定为 table、schema、source。
- 私有 `extractVal` 编译提取正则，跳过完整匹配（捕获组 0），把其余捕获组无分隔拼接为一个扩展列值。

## 主要符号

- `pub type FilterType = i32`：对应 Go `int32` 别名。
- `pub const TblFilter: FilterType = 1` 与 `SchmFilter = 2`：分别标记表级和库级包装器；数值与 Go `iota + 1` 一致。
- `struct filterWrapper`：每条规则的内部编译结果。`filter` 保存匹配器，`rawRule` 保存归一化后的规则，`target` 缓存目标库表，`typ` 保存层级。两个 `Option<Box<_>>` 只用于表达构造过程中的暂未初始化状态，成功加入 `RouteTable` 的包装器两者都应为 `Some`。
- `pub struct RouteTable`：拥有按加入顺序排列的 `filters: Vec<filterWrapper>` 和只读配置 `caseSensitive: bool`；字段不公开，调用者只能通过方法维护不变量。
- `pub fn NewRegExprRouter(caseSensitive, rules) -> Result<RouteTable, String>`：创建空表并逐条调用 `AddRule`；首个错误立即终止，不返回部分构造对象。
- `RouteTable::AddRule(&mut self, rule) -> Result<(), String>`：先调用 `TableRule::Valid`，必要时调用 `ToLower`，再以 `filter::New` 编译匹配器，成功后才追加到 `filters`。
- `RouteTable::Route(&self, schema, table) -> Result<(String, String), String>`：路由查询入口。无匹配时原样返回输入；目标 schema/table 为空时分别回退到对应输入字段。
- `RouteTable::AllRules(&self) -> (Vec<TableRule>, Vec<TableRule>)`：返回克隆快照，第一项为库级规则，第二项为表级规则。
- `RouteTable::FetchExtendColumn(&self, schema, table, source) -> (Vec<String>, Vec<String>)`：返回等长的列名和值向量；没有匹配规则或提取器时返回空向量。
- `enum ExtractorRef<'a>` 与 `fn extractVal`：用强类型枚举替代 Go `any` 加 type switch；三个变体分别借用 table/schema/source 提取器。

本文件没有 trait、宏或条件编译项。`pub use router_crate::router` 公开再导出规则类型所在模块，便于调用方使用与本路由器一致的 `TableRule` 类型。

## 执行流程

构造路径从 `NewRegExprRouter` 开始。它保留 `caseSensitive` 并按输入顺序逐条把规则交给 `AddRule`。`AddRule` 首先执行 `TableRule::Valid`：至少要求源 schema pattern 和目标 schema 非空，并校验各提取器的正则及目标列。大小写不敏感时，规则的 schema/table pattern 被转为小写；随后目标库表被复制到 `target`。若 `TablePattern` 为空，构造只含 `DoDBs` 的 `filter::Rules` 并标为 `SchmFilter`；否则同时构造 `DoTables` 与 `DoDBs` 并标为 `TblFilter`。只有 `filter::New` 成功，包装器才进入 `filters`。

`Route` 将输入组成 `filter::Table`，按添加顺序调用每个内部 `Filter::Match`，分别收集表级与库级命中。若输入 table 为空或没有表级命中，只检查库级集合；否则忽略库级集合并采用表级集合。被采用的集合若超过一项则返回冲突错误，恰好一项时读取其 `target`，零项时保持目标为空。最后，空目标 schema 回退为输入 schema，空目标 table 回退为输入 table。因此规则可以只改库名、只改表名或两者都改，未命中对象保持原名。

`AllRules` 单次遍历 `filters`，依据 `typ` 把 `rawRule` 克隆进两个结果向量。同类规则内部顺序与添加顺序一致，但两个类别被拆开，不提供跨类别的全局交错顺序。

`FetchExtendColumn` 先以与 `Route` 相同的 `Filter::Match` 收集命中规则，再按 `TablePattern` 是否为空分组。若存在任意表级命中，选择第一条表级规则；否则选择第一条库级规则。它不执行 `Route` 的重复匹配冲突检查。选定规则后按 table、schema、source 顺序处理存在的提取器：列向量加入 `TargetColumn`，值向量加入 `extractVal` 结果。`extractVal` 现场用 `regex::Regex::new` 编译对应正则；匹配成功时拼接捕获组 1..N，未匹配或编译失败时返回空串。

## 数据与状态

`RouteTable` 的长期状态是 `caseSensitive` 和拥有所有权的 `filters`。每个 `filterWrapper` 同时保留三种表示：可执行匹配器、用于枚举及提取的规则副本、目标库表副本。这避免查询时重新构建路由规则，但意味着规则数量增加时内存按规则线性增长。

`AddRule` 在大小写不敏感模式下修改自己拥有的 `TableRule`，保存的是小写后的 schema/table pattern；目标库表和提取器文本不由 `TableRule::ToLower` 修改。因此 `AllRules` 返回的是路由器内部的归一化快照，而不是调用者最初传入值的逐字副本。Rust 构造函数按值接收 `Vec<TableRule>`，不同于 Go 保存并原地修改调用方 `*TableRule`；Rust 调用者原有规则只有在显式移动后才不可再用，传入克隆时外部副本不会被修改。

底层 `filter::Filter` 自身含有受 `RwLock<HashMap<...>>` 保护的 schema/table 匹配缓存；`Match` 首次计算后写入缓存，后续相同名字复用结果。因此表面为 `&self` 的 `Route` 和 `FetchExtendColumn` 仍可能通过内部可变性更新缓存。`FetchExtendColumn` 另外会为每次提取重新编译正则；`TableRule::Valid` 已校验并缓存过提取器正则，但本文件的 `extractVal` 没有复用该私有缓存。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：库入口为 `lib.rs`；直接依赖 `astersql-util-filter`（本文件名为 `filter`）、`regex = 1.11` 和 `astersql-util-table-router`（本文件名为 `router_crate`）；porting 元数据指向 Go 包 `pkg/util/regexpr-router`，未声明 feature。

关键下游调用边为：

- `NewRegExprRouter -> RouteTable::AddRule`。
- `AddRule -> TableRule::{Valid, ToLower} -> filter::New`；`filter::New` 将规则编入 selector/正则结构。
- `Route -> filter::Filter::Match`，以及 `FetchExtendColumn -> filter::Filter::Match -> extractVal -> regex::Regex::new/captures`。
- `AllRules` 只遍历并克隆内部数据，不调用外部服务。

RustCodeGraph 将目标文件标为被 `lightning/pkg/importer/import.rs` 与迁移测试使用；精确检索确认生产边为 `addExtendDataForCheckpoint -> NewRegExprRouter -> FetchExtendColumn`。该调用把构造错误转为 importer 错误，把结果写入 `ChunkCheckpoint.FileMeta.ExtendData`。workspace facade 还公开了全部 API，但公开可见不等于当前所有方法都有生产调用：`Route`、`AllRules` 和 `AddRule` 的直接 Rust 使用点目前主要位于两个独立测试文件。

Go 对照的生产边还包括 `lightning/pkg/importer/import.go` 和 `pkg/lightning/mydump/loader.go`。这些是理解设计位置的直接证据，但后者不能作为 Rust 已接线的证据。

## 错误处理与边界

构造与追加规则返回 `Result<_, String>`。`TableRule::Valid` 会拒绝空 schema pattern、空目标 schema、非法提取器正则或空提取目标列；`filter::New` 还可能拒绝无法编译的路由 pattern。`AddRule` 在所有校验和编译完成前不修改 `filters`，所以单次失败不会加入半成品；但对一个已存在的 `RouteTable` 连续手工调用 `AddRule` 时，早先成功加入的规则不会因后续失败而回滚。

`Route` 只在当前实际采用的层级发生多个命中时返回错误。存在表级命中时，多条库级命中不会导致错误，因为表级集合优先；table 为空时即使表级过滤器意外命中，也走库级分支。错误文本包含输入 `schema.table`，形如 `table test2a.tbl2 matches more than one rule`。未命中不是错误，而是返回原输入；目标字段为空也分别回退原字段。

`FetchExtendColumn` 没有错误返回：无规则、无提取器或正则不匹配都产生空结果或空值。虽然 `AddRule` 已通过 `Valid` 拒绝非法提取正则，`extractVal` 仍显式吞掉现场编译错误并返回空串，与 Go 当前实现一致。多个同级规则命中时它静默选择第一条，而 `Route` 会报冲突；调用方若同时依赖路由与扩展列，应先避免歧义规则，不能把 `FetchExtendColumn` 当作冲突验证器。

捕获语义是拼接所有显式捕获组，不包含完整匹配；可选且未参与匹配的捕获组按空串处理，没有捕获组、没有匹配或仅完整匹配时结果为空。列和值按同一分支同步追加，正常返回时长度相等。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务、文件或网络句柄。`RouteTable` 独占规则及过滤器；新增规则要求 `&mut self`，因此不能与同一实例的只读查询同时发生，除非调用方另行同步。

查询方法使用 `&self`，但底层 `Filter::Match` 的缓存通过 `RwLock` 进行内部同步。并发只读查询可能竞争缓存锁，首次出现的新 schema/table 会分配并写入缓存；缓存随 `RouteTable` 及其 `Filter` 一起释放，没有显式清理或容量上限。本文没有为 `RouteTable` 手写 `Send`/`Sync`；能否跨线程共享由组成字段的自动 trait 决定，调用方仍应以编译器约束为准。

每次 `Route`/`FetchExtendColumn` 都分配临时命中向量；`Route` 还克隆返回字符串，`AllRules` 克隆完整规则。`FetchExtendColumn` 为各启用提取器重新编译 `Regex`，编译对象在单次 `extractVal` 返回时释放。这些都是请求内资源，没有后台生命周期，但大量规则、名称基数或高频提取会带来线性扫描、缓存增长和正则重复编译成本。

## 与 Go 版本的对应关系

Rust [`regexpr_router.rs`](regexpr_router.rs) 与 Go [`regexpr_router.go`](regexpr_router.go) 保持相同的常量值、规则分类、构造顺序、表级优先级、重复命中错误、未命中回退、`AllRules` 分类，以及 table/schema/source 提取顺序。独立 Rust 测试 [`regexpr_router_test.rs`](regexpr_router_test.rs) 复刻 Go [`regexpr_router_test.go`](regexpr_router_test.go) 的创建、追加、库级/表级/正则路由、扩展列、枚举及冲突案例；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 额外明确覆盖表级覆盖库级、大小写不敏感、非法空规则和 Go 风格错误文本。

语言与当前实现差异如下：

- Go 构造器接收 `[]*TableRule` 并保存指针；大小写不敏感时会原地修改调用者规则。Rust 接收拥有所有权的 `Vec<TableRule>`，内部用 `Box` 保存，通常不会修改调用者保留的克隆。
- Go 返回 `*RouteTable` 和 `error`；Rust 返回按值的 `RouteTable` 与 `String` 错误。Go 用 `errors.Trace/Annotatef` 保留错误包装，Rust 保留文本语义但没有等价错误链类型。
- Go 的 `filter`/`rawRule` 指针按成功构造不变量直接解引用；Rust 用 `Option<Box<_>>` 表达构造中状态，查询时若异常为 `None` 会按“不匹配/跳过”处理，而不是 panic。由于字段私有且只经 `AddRule` 构造，正常公共 API 不应产生该状态。
- Go `extractVal(any)` 运行时 type switch；Rust `ExtractorRef` 在编译期限制为三类提取器。两者都在本函数内重新编译正则，忽略编译错误并拼接捕获组 1..N。
- Go importer 从解析后的 checkpoint 文件路由结果中提取扩展列；当前 Rust importer 直接从文件名的前两个点分段取得 schema/table。这个上游差异属于 importer 接线，不由本文件定义，但扩展本文件时不能假定两侧输入来源完全相同。

## 扩展指南

新增规则分类或调整匹配优先级时，入口是 `filterWrapper.typ`、`AddRule`、`Route` 和 `FetchExtendColumn`；必须同时决定 `AllRules` 的返回分类，并核对 Go [`regexpr_router.go`](regexpr_router.go) 是否需要同等变更。新增提取器种类时应扩展 `ExtractorRef`、`FetchExtendColumn` 的列值顺序和 `extractVal` 的正则选择，且明确无匹配、无捕获组、非法正则和多个规则命中时的契约。

测试不得内嵌在生产文件。Go 对齐用例应更新独立的 [`regexpr_router_test.rs`](regexpr_router_test.rs)，迁移特有不变量可更新 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并以 [`regexpr_router_test.go`](regexpr_router_test.go) 复核原始意图。至少覆盖：库级与表级同时命中、同级重复命中、空 table、未命中回退、大小写模式、正则/glob 混用、多个捕获组和可选空捕获、`AllRules` 顺序，以及非法规则在追加前失败。

若优化性能，可考虑复用 `TableRule::Valid` 已编译的提取器正则或改变匹配索引，但要先解决其字段可见性和与 Go “提取时编译”语义的兼容问题。不要仅依据 facade 公开或测试使用就删除 API；应重新查询生产调用。主要兼容风险是优先级、错误文本、规则归一化和捕获拼接发生漂移；主要性能风险是每次查询扫描全部规则、匹配缓存无界增长、每次提取重复编译正则和 `AllRules` 深克隆。

## 验证依据

- RustCodeGraph `status` 确认索引有效：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/regexpr-router` 列出本目录 6 个已索引 Go/Rust 文件，目标文件含 18 个符号。
- RustCodeGraph `node --file` 完整读取了 [`regexpr_router.rs`](regexpr_router.rs)（363 行）、Go [`regexpr_router.go`](regexpr_router.go)（235 行）、Rust [`regexpr_router_test.rs`](regexpr_router_test.rs)（268 行）、Rust [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)（213 行）和 Go [`regexpr_router_test.go`](regexpr_router_test.go)（379 行）。
- RustCodeGraph `query` 核对了 `NewRegExprRouter`、`AddRule`、`Route`、`AllRules`、`FetchExtendColumn`、`extractVal`、`TableRule::{Valid, ToLower}` 和 `Filter::Match`。图的泛名 callers/callees 查询未返回稳定结果，因此又用限定 Rust 文件的精确文本检索核对直接调用，未把缺失图边推断成不存在调用。
- 生产调用证据来自 RustCodeGraph 对 `lightning/pkg/importer/import.rs::addExtendDataForCheckpoint` 的源码节点：`NewRegExprRouter -> FetchExtendColumn -> ChunkCheckpoint.FileMeta.ExtendData`；Go 侧使用点由 `lightning/pkg/importer/import.go` 与 `pkg/lightning/mydump/loader.go` 复核。
- crate 边界由 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、workspace 根 `Cargo.toml` 和 [`pkg/lib.rs`](../../lib.rs) 核对；下游校验、归一化及匹配事实由 `pkg/util/table-router/router.rs::TableRule::{Valid, ToLower}` 和 `pkg/util/filter/filter.rs::{New, Filter::Match}` 核对。
- 本任务只新增文档，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文能够回答文件为何存在、如何运行、如何安全扩展；仓库说明提及的 `.agents/skills/tidb-verify-profile` 在当前检出中不存在，无法加载额外 Ready playbook。
