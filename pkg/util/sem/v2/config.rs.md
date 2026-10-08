# `pkg/util/sem/v2/config.rs`

## 文件定位

本文件是 `astersql-util-sem-v2` crate 的配置边界：把磁盘上的 SEM（Security Enhanced Mode）v2 JSON 解码为 Rust 数据结构，并在配置进入运行时前完成最低 TiDB 版本、系统变量和命名 SQL 规则校验。crate 入口 `pkg/util/sem/v2/lib.rs` 以 `mod config; pub use config::*;` 装配本模块；其中结构体经 crate 根再导出，而 `parseSEMConfigFromFile` 与 `validateSEMConfig` 保持 `pub(crate)`，只供同 crate 的启用流程和测试辅助使用。

运行主链是 `sem.rs::Enable` → `parseSEMConfigFromFile` → `sem.rs::EnableBy` → `validateSEMConfig` → `sem.rs::buildSEMFromConfig`。因此本文件负责“配置表示、读取、准入”，不负责把限制编译成查询集合、不维护全局启用状态，也不执行权限或 SQL 拒绝判定；这些职责分别位于 `pkg/util/sem/v2/sem.rs`、`sql_rule.rs` 和 `restricted_hint.rs`。

`pkg/util/sem/v2/Cargo.toml` 将该目录声明为库 crate `astersql-util-sem-v2`，入口为 `lib.rs`、禁用 doctest；本文件直接使用标准库文件 I/O，以及 `serde`、`serde_json`、`semver`、`mysql`、`variable`、`vardef` 和同 crate 的 `sqlRuleNameMap`。

## 核心职责

1. 用 `Config` 描述完整 SEM v2 配置，并用 `TableRestriction`、`ColumnRestriction`、`VariableRestriction`、`SQLRestriction` 表达嵌套限制项。
2. 通过 serde 的 snake_case JSON 字段映射和 `deserialize_null_default`，同时兼容缺失字段与显式 `null`，使它们落到 Rust `Default` 零值，贴合 Go `encoding/json` 对非指针字段的行为。
3. `parseSEMConfigFromFile` 从路径打开文件并用流式 `serde_json::Deserializer` 解码一个 `Config`。它刻意不调用要求输入结束的 `serde_json::from_reader`，因此和 Go `json.Decoder.Decode` 一样，首个完整 JSON 值之后再出现第二个 JSON 值也不会在此处报错。
4. `validateSEMConfig` 在配置生效前执行三类短路校验：当前 TiDB 版本不得低于配置下限；每个受限系统变量必须已注册，且非空强制值只能用于 `ScopeNone` 变量；每个命名 SQL 规则必须存在于 `sqlRuleNameMap`。

配置的 `Version` 当前只被保存和序列化，本文件没有按配置格式版本分派或拒绝未知版本；这与 `config.go` 中“目前只有一种 SEM 配置版本，暂未使用该字段”的说明一致。

## 主要符号

- `deserialize_null_default<'de, D, T>(deserializer) -> Result<T, D::Error>`：私有通用反序列化器。先把输入解析成 `Option<T>`，`Some` 返回原值，`None` 使用 `T::default()`；所有配置字段的 `T` 都满足 `Deserialize + Default`。
- `pub struct Config`：顶层配置。`Version`、`TiDBVersion` 是元信息；`RestrictedDatabases`、`RestrictedTables`、`RestrictedVariables`、`RestrictedStatusVar`、`RestrictedPrivileges`、`RestrictedSQL`、`RestrictedHints` 是后续 `buildSEMFromConfig` 的输入。类型派生 `Debug`、`Clone`、`Default`、`Deserialize`、`Serialize`，并以 `#[serde(default)]` 允许字段缺失。
- `pub struct TableRestriction`：由 `Schema` 与 `Name` 定位表，`Hidden` 控制整表隐藏，`Columns` 携带列级配置。当前 Rust 运行时 `sem.rs::buildSEMFromConfig` 只消费表的 `Hidden`，列配置虽被忠实解析和保留，但未在该构建函数中消费。
- `pub struct ColumnRestriction`：保存列 `Name`、`Hidden` 和固定 `Value`。它是配置契约的一部分；不能仅因当前 `SemImpl` 未消费就从格式中删除。
- `pub struct VariableRestriction`：保存系统变量名、隐藏标记、只读标记和可选强制值。校验只检查变量存在性及 `Value` 与变量 scope 的相容性；`Hidden`/`Readonly` 的运行时含义由 `sem.rs` 构建和查询逻辑实现。
- `pub struct SQLRestriction`：`SQL` 保存命令名，`Rule` 保存 `sqlRuleNameMap` 的键。前者由运行时构建逻辑规范化和匹配，后者在本文件先校验存在性。
- `parseSEMConfigFromFile(filePath: &str) -> Result<Config, String>`：crate 内文件解析入口，分别为打开失败和 JSON 解码失败添加路径上下文。
- `validateSEMConfig(cfg: &Config) -> Result<(), String>`：crate 内准入校验入口；按版本、变量、规则的固定顺序检查，遇到首个问题立即返回。

字段保留 Go 风格的大写名称，并以 `#[allow(non_snake_case)]` 明确允许；JSON 名由每个 `#[serde(rename = "...")]` 固定，不依赖 Rust 标识符拼写。`RestrictedHints` 额外设置 `skip_serializing_if = "Vec::is_empty"`，对应 Go 的 `omitempty`。

## 执行流程

文件加载流程如下：

1. `sem.rs::Enable(configPath)` 断言 SEM 尚未启用，然后调用 `parseSEMConfigFromFile`。
2. 解析函数用 `File::open(Path::new(filePath))` 打开路径；失败时返回 `failed to open file <path>: <source>`。
3. 它把文件交给 `serde_json::Deserializer::from_reader`，只调用一次 `Config::deserialize`。serde 先应用结构级 `default`，再按 JSON 字段覆盖；字段值为 `null` 时，`deserialize_null_default` 将其还原为该字段类型的默认值。
4. JSON 语法或类型不合法时返回带路径的 decode 错误；成功时返回拥有全部字符串和向量的 `Config`。`File` 在函数离开作用域时自动关闭。
5. `Enable` 把配置借给 `EnableBy`；`EnableBy` 首先调用 `validateSEMConfig`，只有校验成功才构建 `SemImpl`、覆盖受限变量、发布全局 SEM 实例。

校验流程如下：

1. 读取 `mysql::r#const::TiDBReleaseVersion`，分别去除当前版本和最低版本最前面的一个小写 `v`，再用 `semver::Version::parse` 解析。
2. 当前版本小于最低版本时返回错误；等于或高于时继续。
3. 顺序遍历 `RestrictedVariables`。`variable::GetSysVar` 找不到名称则报错；配置了非空 `Value`、但注册变量的 `Scope` 不是 `vardef::ScopeNone` 时也报错。这里判断的是系统变量注册表的 scope，而不是配置内的 `Readonly` 布尔值。
4. 顺序遍历 `RestrictedSQL.Rule`，要求每个名称都是 `sql_rule.rs::sqlRuleNameMap` 的键。
5. 全部通过后返回 `Ok(())`。空列表自然通过；验证不修改 `Config`。

## 数据与状态

所有配置结构都拥有自己的 `String`/`Vec` 数据，没有借用外部缓冲区。`Default` 令字符串和列表为空、布尔值为 `false`、嵌套 `SQLRestriction` 为空；所以 `{}` 以及字段为 `null` 的对象都可成功反序列化，但这并不等于能通过 `validateSEMConfig`：例如空 `TiDBVersion` 随后会触发 semver 解析错误。

配置结构本身不维护缓存、单例或可变全局状态。校验会读取两个进程级外部状态：`mysql::r#const::TiDBReleaseVersion` 和 `variable` 的系统变量注册表；命名规则表 `sqlRuleNameMap` 是 `LazyLock<HashMap<...>>`，首次访问时初始化。`validateSEMConfig` 对输入是只读的，也不会注册、注销或写入系统变量。

大小写处理有意保持局部：版本只剥离小写 `v`；系统变量名原样交给 `GetSysVar`；SQL 规则名按 map 键精确匹配。数据库、表、权限和命令名的规范化发生在 `sem.rs::buildSEMFromConfig`，不是本文件的职责。

## 依赖与调用关系

上游直接调用关系由 RustCodeGraph 文件节点和源码交叉核对：

- `sem.rs::Enable` 调用 `parseSEMConfigFromFile`，构成生产启用入口。
- `sem.rs::EnableBy` 调用 `validateSEMConfig`，并在成功后调用 `buildSEMFromConfig`；这是已解析配置的生产入口。
- `testhelper.rs::EnableFromPathForTest` 直接调用 `parseSEMConfigFromFile`，先备份涉及的系统变量，再调用公开的 `Enable`。
- `config_test.rs` 的本地 helper 调用解析函数，且测试直接调用校验函数；`migration_aster_unit_test.rs` 通过 `EnableBy` 间接覆盖校验及配置到运行时的接线。

主要下游依赖为：

- `std::fs::File`、`std::path::Path`：文件打开与路径传递。
- `serde`/`serde_json`：结构派生、字段兼容和流式 JSON 解码。
- `semver::Version`：当前版本与最低版本的语义化版本解析、比较。
- `mysql::r#const::TiDBReleaseVersion`：当前进程发布版本。该读取位于 `unsafe` 块，因为迁移后的全局变量是可变静态值。
- `variable::GetSysVar` 与 `vardef::ScopeNone`：系统变量注册信息及只读 scope 判定。
- `sql_rule.rs::sqlRuleNameMap`：把配置规则名约束到已有规则实现。

crate manifest 中 `serde`、`serde_json`、`semver`、`mysql`、`variable`、`vardef` 均为普通依赖；`tempfile` 与 `serial_test` 只在独立测试中使用。仓库其他 crate 通过 `astersql-util-sem-v2` 的公开 `Enable`、`EnableBy` 和查询 API 使用最终行为，而不能直接调用两个 `pub(crate)` 函数。

## 错误处理与边界

两个入口统一返回 `Result<_, String>`，保留面向 Go 迁移的错误文本，不暴露具体错误类型。打开错误和解码错误包含原始路径与底层错误；版本解析错误区分当前版本和最低要求；变量及规则错误带出具体名称。校验采用首错返回，因而同一配置有多个问题时，调用方只能看到固定顺序中的第一个。

已由测试明确的边界包括：合法对象、截断 JSON、空对象、顶层与嵌套字段 `null`、尾随第二个 JSON 值、当前版本过低、未知系统变量、为非 `ScopeNone` 变量配置强制值、未知 SQL 规则。未知 JSON 字段未设置 `deny_unknown_fields`，会被 serde 忽略；缺失字段使用默认值。解析函数也没有文件大小限制、schema 版本判断、EOF 检查或显式路径 canonicalize。

值得注意的不变量是：配置中的 `Readonly: true` 不能让一个可写系统变量合法地拥有强制值；决定条件始终是注册表中的 `SysVar.Scope == ScopeNone`。反过来，值为空时不检查 scope。规则 map 只验证 `RestrictedSQL.Rule`，不验证 `RestrictedSQL.SQL` 命令字符串。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务。解析期间 `File` 由局部变量和 serde reader 借用，函数返回或报错时通过 RAII 关闭；返回的 `Config` 与文件生命周期完全解耦。

`validateSEMConfig` 本身只顺序读取数据，但其结果依赖进程级版本字符串和系统变量注册表在校验期间保持稳定。Rust 测试会修改 `mysql::r#const::TiDBReleaseVersion`、注册或注销系统变量，因此 `config_test.rs::test_validate_config` 使用 `serial_test::serial` 串行化这类全局状态测试。生产启用的全局锁与发布顺序在 `sem.rs` 中实现，不应在本文件重复建立锁。

反序列化得到的配置随后会被 `buildSEMFromConfig` 复制进由 `Arc` 和 `RwLock` 管理的运行时结构；本文件只提供这一生命周期的不可变输入。若未来让配置热更新，必须重新评估“校验外部注册表”与“发布运行时状态”之间的竞态窗口，而不能只修改解析函数。

## 与 Go 版本的对应关系

`pkg/util/sem/v2/config.go` 是直接语义基线。五个 Rust 结构与 Go 的 `Config`、`TableRestriction`、`ColumnRestriction`、`VariableRestriction`、`SQLRestriction` 字段逐项对应；serde rename 对应 Go JSON tag，`RestrictedHints` 的空列表省略对应 `omitempty`。

核心行为保持一致：

- Go `os.Open(filepath.Clean(filePath))` 与 Rust `File::open(Path::new(filePath))` 都打开给定配置文件；Rust 没有显式 `Clean`，由平台路径处理 `.`/`..`，但错误消息仍使用原始字符串。
- Go `json.Decoder.Decode` 只消费一个值；Rust特意直接调用流式 deserializer 的一次 `Config::deserialize`，避免 `serde_json::from_reader` 的 EOF 检查改变尾随第二值语义。
- Go 对 `null` 非指针字段保留零值；Rust用 `deserialize_null_default` 明确复现。Rust 测试比当前 Go `config_test.go` 多覆盖顶层/嵌套 `null` 和尾随 JSON 值，这是对 Go 库行为的迁移回归，不是新业务规则。
- 两边均去掉可选小写 `v` 后做 semver 比较，均用系统变量注册表检查名称和 scope，并用规则名 map 拒绝未知命名规则。
- 两边都没有校验 `Config.Version`，也都只在 `Value` 非空时要求 `ScopeNone`。

实现层差异主要是所有权与错误表示：Go 返回 `*Config, error` 并 `defer file.Close()`；Rust 返回拥有值的 `Config` 和 `String` 错误，文件自动析构。Go 可安全直接读取包级版本变量；Rust 迁移值是 `static mut`，所以读取需要 `unsafe`。这些差异不应改变成功/失败分支或错误主文案。

## 扩展指南

- 新增配置字段时，应同时修改 `Config` 或对应嵌套结构、补齐准确的 `serde(rename)` 与 `deserialize_null_default` 策略，并同步 Go `config.go`；若空值不应等于默认值，不要机械套用现有 helper。测试放在独立的 `config_test.rs`，并视跨模块行为补充 `migration_aster_unit_test.rs` 或 `sem_test.rs`，不要把测试内嵌回生产文件。
- 新增命名 SQL 规则时，规则实现和 `sqlRuleNameMap` 位于 `sql_rule.rs`；本文件通常无需增加分支，因为 `validateSEMConfig` 已统一检查 map membership。应同步 `sql_rule_test.rs`，证明规则 AST 分支和配置名称都有效。
- 改变系统变量强制值规则时，应优先修改 `validateSEMConfig` 并同步 `sem.rs::overrideRestrictedVariable`，同时覆盖未知变量、空值、`ScopeNone` 与非 `ScopeNone`。只信任配置中的 `Readonly` 会破坏现有 Go 契约。
- 若要求严格单文档、拒绝未知字段或拒绝尾随内容，应先确认这是有意偏离 Go 的兼容变更；改用 `serde_json::from_reader` 或调用 deserializer `end()` 会改变已由 Rust 回归测试固定的行为。
- 若要支持配置格式多版本，应在 `Config.Version` 上设计显式分派/迁移，并明确空版本的兼容策略；当前不能把字段存在误认为已有版本校验。
- 性能风险主要来自无上限地把 JSON 字符串和数组全部载入内存，以及逐项查系统变量和规则 map。常规 SEM 配置很小；若引入大规模配置限制，应在不破坏流式单值语义的前提下评估大小/数量上限。

## 验证依据

本说明依据以下本地事实完成，未运行 Cargo：

- RustCodeGraph `status`：索引可用，包含 11,467 个文件；`files --filter pkg/util/sem/v2` 显示目标目录 20 个已索引文件。
- RustCodeGraph `node --file pkg/util/sem/v2/config.rs`：确认 209 行源码、五个配置结构、私有 null helper、解析和校验函数，以及索引记录的直接使用文件 `config_test.rs`、`migration_aster_unit_test.rs`、`sql_rule.rs`。
- RustCodeGraph 文件节点：读取 `lib.rs`、`config_test.rs`、`config.go`、`config_test.go`；精确 callers/callees 查询因同名 Go/Rust 符号消歧结果不足，随后用 `rg` 与直接源码读取补齐 `sem.rs`、`testhelper.rs`、`sql_rule.rs` 和 `migration_aster_unit_test.rs` 的真实调用边。
- `pkg/util/sem/v2/Cargo.toml`：确认 crate 名称、入口、直接依赖、测试依赖和 Go 包迁移元数据。
- Rust 独立测试 `config_test.rs`：覆盖解析格式、null/尾随值、版本、变量和规则错误；`migration_aster_unit_test.rs`：覆盖 JSON tag、序列化名称、`EnableBy` 校验错误和配置进入运行时后的行为；Go 对照 `config_test.go`：确认原表驱动测试意图。
- 人工核对主链：`sem.rs::Enable` → `parseSEMConfigFromFile` → `EnableBy` → `validateSEMConfig` → `buildSEMFromConfig`，以及 `testhelper.rs::EnableFromPathForTest` 的测试接线。

文档结构另以任务指定命令验证，要求目标文件存在且恰好出现全部 11 个固定二级标题。
