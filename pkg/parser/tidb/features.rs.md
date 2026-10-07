# `pkg/parser/tidb/features.rs`

## 文件定位

该文件定义 TiDB 专有语法特性的字符串标识，并提供“一组特性 ID 是否全部可解析”的白名单判定。它属于 `astersql-parser-tidb` crate：`pkg/parser/tidb/Cargo.toml` 将库入口指向 `lib.rs`，`pkg/parser/tidb/lib.rs` 以 `pub mod features; pub use features::*;` 对外公开本文件的 API。

同一源文件还被主解析器通过 `pkg/parser/lib.rs` 中的 `#[path = "tidb/features.rs"] pub mod tidbfeature;` 直接纳入。因此，实际 SQL 词法路径使用的是 `parser::tidbfeature`，而根 workspace 和 `pkg/lib.rs` 又将独立 crate 作为 `facade_parser_tidb` / `parser::tidb` 门面导出。本文件不解析 SQL，也不实现特性本身；它只维护标识和解析准入策略。

## 核心职责

1. 用 `FEATURE_ID_*` 常量固定 TiDB 特性注释中使用的稳定协议字符串，例如 `"auto_rand"`、`"ttl"` 和 `"pre_split"`。
2. 用私有切片 `FEATURE_IDS` 明确列出当前解析器允许展开的特性注释。“定义了常量”不等于“已加入可解析白名单”：`FEATURE_ID_TIDB` 和 `FEATURE_ID_RESOURCE_GROUP` 就有公开常量，但不在 `FEATURE_IDS` 中。
3. 用 `can_parse_feature` 执行全称量检查，保证只有所有 ID 均被允许时才返回 `true`。
4. 提供 `FeatureID*` 和 `CanParseFeature` 这组 Go 风格兼容名，使移植代码可继续沿用 `pkg/parser/tidb/features.go` 的公开命名。

## 主要符号

- `FEATURE_ID_TIDB: &str = ""`：通用 TiDB 占位 ID。它不在白名单中，单独校验会失败。
- `FEATURE_ID_AUTO_RANDOM`、`FEATURE_ID_AUTO_ID_CACHE`、`FEATURE_ID_AUTO_RANDOM_BASE`、`FEATURE_ID_CLUSTERED_INDEX`、`FEATURE_ID_FORCE_AUTO_INC`、`FEATURE_ID_PLACEMENT`、`FEATURE_ID_TTL`、`FEATURE_ID_GLOBAL_INDEX`、`FEATURE_ID_PRE_SPLIT`、`FEATURE_ID_AUTO_PRE_SPLIT`、`FEATURE_ID_AFFINITY` 和 `FEATURE_ID_SPLIT_REGION`：同时是公开协议常量和 `FEATURE_IDS` 成员。
- `FEATURE_ID_RESOURCE_GROUP`：保留了 `"resource_group"` 的公开标识，但按当前 Go 实现未注册到白名单。
- `FEATURE_ID_PRESPLIT`：`FEATURE_ID_PRE_SPLIT` 的兼容别名，两者都是 `"pre_split"`。
- `FEATURE_IDS: &[&str]`：私有、编译期固定的允许集合；当前含 12 个条目，无运行期注册接口。
- `can_parse_feature(features: &[&str]) -> bool`：Rust 风格的核心判定函数。
- `FeatureIDTiDB` 等 `FeatureID*` 常量：上述 `FEATURE_ID_*` 的 Go 风格公开别名，没有独立状态或不同值。
- `CanParseFeature(features: &[&str]) -> bool`：Go 风格公开包装，唯一行为调用 `can_parse_feature(features)`。

文件级 `#![allow(non_upper_case_globals, non_snake_case)]` 是为了保留这些 Go 风格兼容符号，不改变运行行为。

## 执行流程

1. 主解析器的 `pkg/parser/lexer.rs::startWithSlash` 识别到 `/*T!` 开头后，调用 `Scanner::scanFeatureIDs` 解出方括号内的 ID 字符串。
2. 扫描成功时，调用方将 `Vec<String>` 借用为 `Vec<&str>`，传入 `tidbfeature::CanParseFeature`。
3. `CanParseFeature` 直接委托给 `can_parse_feature`。
4. `can_parse_feature` 对输入执行 `iter().all(...)`；每个 ID 都用 `FEATURE_IDS.contains(feature)` 与静态白名单比较。
5. 若所有 ID 都命中，`startWithSlash` 设置 `inBangComment = true` 并继续扫描注释体，使特性注释内容像 SQL 令牌一样被解析；否则该注释不按可执行特性注释展开。

算法是短路的：首个未知 ID 会使 `all` 立即返回 `false`。空切片没有反例，因此返回 `true`；重复的合法 ID 也不会改变结果。

## 数据与状态

所有 ID 均是 `&'static str` 编译期常量。`FEATURE_IDS` 是指向这些静态字符串的私有切片，不会在启动时构造 map，也不存在动态注册、缓存、配置或全局可变状态。

`can_parse_feature` 只借用调用方的 `&[&str]`，不获取所有权，不修改输入，也不分配内存。对长度为 `n` 的输入，当前线性切片查找的最坏比较量约为 `n * FEATURE_IDS.len()`；由于白名单很小且固定，实现选择了简单静态数据而非集合容器。

## 依赖与调用关系

- 上游生产调用者：`pkg/parser/lexer.rs::startWithSlash` 在处理 TiDB 特性注释时调用 `tidbfeature::CanParseFeature`。RustCodeGraph 也给出 `startWithSlash -> CanParseFeature -> can_parse_feature -> FEATURE_IDS` 的路径。
- 上游测试调用者：`pkg/parser/tidb/migration_aster_unit_test.rs` 直接检查常量、白名单、空输入、重复项、未知项和别名；`pkg/parser/go_merge_35_test.rs::go_merge_35_auto_presplit_feature_comment` 检查 `pre_split` / `auto_presplit`；`pkg/parser/lexer_test.rs::test_feature_ids_comment` 通过 digester 公开入口检查注释展开。
- 下游依赖：本文件只使用 Rust 标准库的 slice / iterator 能力，`pkg/parser/tidb/Cargo.toml` 未声明 crate 依赖。
- crate 边界：workspace 根 `Cargo.toml` 以 `facade_parser_tidb = { package = "astersql-parser-tidb", path = "pkg/parser/tidb" }` 引入它，`pkg/lib.rs` 再从 `parser::tidb` 导出；`pkg/executor/Cargo.toml` 也直接依赖该 crate。这些依赖说明公开常量是可复用协议面，但本次搜索到的 Rust SQL 解析主链调用仍是 `pkg/parser/lib.rs` 的直接模块挂载。

## 错误处理与边界

函数不返回 `Result` 也不会 panic；“不支持”通过 `false` 表达。判定是大小写敏感的精确字符串匹配，不会去除空白、归一化大小写或识别别的拼写。任一未知项都使整组失败；不会只接受其中已知的子集。

值得特别注意的边界是：

- `&[]` 返回 `true`，与 Go 的空可变参数循环一致。
- `FEATURE_ID_TIDB` 的空串和 `FEATURE_ID_RESOURCE_GROUP` 均返回 `false`，因为它们未出现在 `FEATURE_IDS`。
- `FEATURE_ID_PRESPLIT` 与 `FEATURE_ID_PRE_SPLIT` 值相同，所以兼容旧名不会产生额外白名单条目。
- 输入语法错误不由本文件处理。Rust 调用方 `scanFeatureIDs` 用 `Option` 区分“扫描失败”与“有一组 ID”，且只在 `Some` 时调用本函数；因此本函数的空切片规则不会把扫描失败误当成成功。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、文件句柄或事务。所有共享数据都是不可变的编译期静态值，因此多线程可同时调用校验函数，无需同步。

输入字符串的生命周期由调用方所有；函数只在调用期间借用它们。`pkg/parser/lexer.rs::startWithSlash` 中临时的 `Vec<&str>` 借用自局部 `Vec<String>`，校验完成后立即释放；本文件不保存任何引用。

## 与 Go 版本的对应关系

`pkg/parser/tidb/features.go` 是直接对照实现。Rust 与 Go 对外暴露的 `FeatureID*` 字符串值一致，包括 `FeatureIDPresplit = FeatureIDPreSplit` 的兼容关系；Go 的 `featureIDs map[string]struct{}` 和 Rust 的 `FEATURE_IDS` 含有相同的 12 个允许值，两者都故意排除空串 `FeatureIDTiDB` 和 `FeatureIDResourceGroup`。

Go `CanParseFeature(fs ...string)` 遍历可变参数并查 map；Rust `can_parse_feature(&[&str])` 遍历切片并查静态数组。两者在空输入、重复合法项、任一未知项及短路返回方面语义一致；数据结构不同不改变当前小规模白名单的可观测结果。

调用点有一个重要的 Rust 类型化差异：Go `scanFeatureIDs` 以 `nil` 表示格式失败，而 Rust 版返回 `Option<Vec<String>>`；Rust `startWithSlash` 只在 `Some(ids)` 时校验。这个差异属于 lexer 边界表达，不是本文件白名单算法的差异。

## 扩展指南

- 新增特性 ID 时，先确认它是否只需要作为输出/恢复标记，还是应允许 lexer 展开对应 `/*T![...] ... */` 注释。前者只需公开常量，后者还必须加入 `FEATURE_IDS`。`RESOURCE_GROUP` 证明两者不能自动等同。
- 同步更新 snake_case `FEATURE_ID_*` 和 Go 风格 `FeatureID*` 导出，并与 `pkg/parser/tidb/features.go` 核对协议字符串。若是更名，参照 `PRE_SPLIT` / `PRESPLIT` 保留兼容别名，避免破坏下游源码兼容性。
- 更新独立测试 `pkg/parser/tidb/migration_aster_unit_test.rs`：至少覆盖字面量、是否注册以及两套公开名的一致性。若特性注释的展开行为改变，还应同步独立的 `pkg/parser/lexer_test.rs`、`pkg/parser/lexer_5_aster_unit_test.rs` 及对应 Go 测试意图，不要把测试嵌入本生产文件。
- 修改白名单会直接改变解析器是否把特性注释体当作 SQL 令牌处理，属于兼容性行为，不应仅因存在同名常量就加入。
- 若白名单大幅增长或该函数出现在高频非解析路径，再用基准数据评估是否需要替换线性 `contains`；当前没有为了理论优化而引入运行期集合的证据。

## 验证依据

- 源码与模块边界：`pkg/parser/tidb/features.rs`、`pkg/parser/tidb/lib.rs`、`pkg/parser/tidb/Cargo.toml`、`pkg/parser/lib.rs`、workspace 根 `Cargo.toml` 和 `pkg/lib.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/parser/tidb` 列出 `features.rs`、`features.go`、`lib.rs` 和独立测试；`explore 'pkg/parser/tidb/features.rs can_parse_feature CanParseFeature FEATURE_IDS'` 给出 `CanParseFeature -> can_parse_feature` 调用边及测试引用；`node can_parse_feature` 确认其只引用 `FEATURE_IDS` 且被 Rust `CanParseFeature` 调用。
- 生产调用链：`pkg/parser/lexer.rs::Scanner::scanFeatureIDs`、`pkg/parser/lexer.rs::startWithSlash`；Go 对照为 `pkg/parser/lexer.go` 中同名符号。
- Go 语义对照：`pkg/parser/tidb/features.go`，包括常量值、`featureIDs` map 和 `CanParseFeature`。
- Rust 独立测试：`pkg/parser/tidb/migration_aster_unit_test.rs`、`pkg/parser/go_merge_35_test.rs::go_merge_35_auto_presplit_feature_comment`、`pkg/parser/lexer_test.rs::test_feature_ids_comment`、`pkg/parser/lexer_5_aster_unit_test.rs::lexer_matches_go_version_and_feature_scanners`。Go 对照测试为 `pkg/parser/lexer_test.go::TestFeatureIDsComment` 和 `TestFeatureIDs`。
- 人工复核结论：文件之所以存在，是为 TiDB 特性注释提供稳定 ID 和可解析准入集；它以静态、无状态、全称量短路校验运行；安全扩展需区分“公开标识”与“允许解析”，并同步 Go 协议值和独立测试。
