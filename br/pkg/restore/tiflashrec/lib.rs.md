# `br/pkg/restore/tiflashrec/lib.rs`

## 文件定位

`lib.rs` 是 Cargo crate `astersql-br-pkg-restore-tiflashrec` 的根模块，不是 TiFlash 恢复录制逻辑的实现文件。`br/pkg/restore/tiflashrec/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它设为库入口，并用 `package.metadata.porting.go-package = "br/pkg/restore/tiflashrec"` 标明对应的 Go package。根 workspace `Cargo.toml` 把该 crate 列为 member。

该文件的实际作用是“门面”：它把 [`tiflash_recorder.rs`](tiflash_recorder.rs) 挂载为公开子模块，再把子模块的公开项扁平再导出到 crate 根。因此调用方理论上可以写 `astersql_br_pkg_restore_tiflashrec::TiFlashRecorder`，也可以通过 `...::tiflash_recorder::TiFlashRecorder` 访问同一定义。当前精确 Rust 文本搜索未找到该 crate 在其他生产 crate 中的依赖或导入；它已在 workspace 中可独立组织，但尚不应被描述为已接入完整 Rust restore 主链。

## 核心职责

- `#[path = "tiflash_recorder.rs"] pub mod tiflash_recorder;` 建立 crate 根到实现模块的公开路径。
- `pub use tiflash_recorder::*;` 将 `TiFlashRecorder`、`TiFlashReplicaInfo`、`CIStr`、`TableMeta` 和 `InfoSchema` 等公开符号暴露在 crate 根，统一外部 API 入口。
- 在单元测试构建中，用条件模块加载独立的 `parity_test.rs` 和 `tiflash_recorder_test.rs`；这使测试逻辑不与生产源文件混编。
- 在 crate 根层面允许迁移代码中常见的未使用项和 Go 风格命名，避免将迁移期命名差异升格为编译告警。

`lib.rs` 自身不保存表状态、不生成 DDL，也不执行恢复；这些行为都在 `tiflash_recorder.rs` 中。

## 主要符号

`lib.rs` 本身没有声明常量、结构体、枚举、trait、函数或 `impl`，也没有 feature 条件分支。它声明和导出的界面如下：

- `pub mod tiflash_recorder`：公开子模块，真实实现位于 `tiflash_recorder.rs`。
- `mod parity_test`：仅在 `cfg(test)` 下加载的 crate 内公开契约对照测试。
- `mod tiflash_recorder_test`：仅在 `cfg(test)` 下加载的 Go 语义移植测试。
- `pub use tiflash_recorder::*`：通配再导出。当前具体公开集合由实现文件决定，包括 `TiFlashReplicaInfo { Count, LocationLabels }`、`CIStr { O, L }`、`CIStr::new`、`TableMeta { Name }`、`InfoSchema::TableByID` 和 `TiFlashRecorder` 及其 Go 风格方法。`EncloseDBAndTable` 与 `alterTableSpecOf` 是子模块私有函数，不会被通配再导出。

crate 级 `#![allow(...)]` 覆盖 `dead_code`、Go 风格命名以及未使用 import/变量。这是编译器 lint 策略，不改变可见性或运行时行为。

## 执行流程

1. Cargo 按 `Cargo.toml` 的 `[lib]` 设置从 `lib.rs` 建立 crate。
2. 编译器根据 `#[path]` 解析 `tiflash_recorder.rs`，将其作为 `tiflash_recorder` 子模块。
3. `pub use tiflash_recorder::*` 在编译期将子模块的公开项纳入 crate 根命名空间；这一步没有运行时成本。
4. 普通库构建不编译两个测试模块。单元测试构建时，`cfg(test)` 成立，`parity_test.rs` 与 `tiflash_recorder_test.rs` 被编入同一 crate，因而可通过 `crate::{...}` 验证根门面的再导出。
5. 实际业务调用若接入该 crate，会通过再导出的 `TiFlashRecorder` 记录以 table ID 为键的副本配置，并在需要时生成恢复 TiFlash 副本的 `ALTER TABLE` 语句。此步是子模块行为，不是 `lib.rs` 中的运行时流程。

## 数据与状态

`lib.rs` 不创建全局状态、单例、缓存或持久化数据。它仅决定哪些类型和模块可从 crate 外看见。

被再导出的 `TiFlashRecorder` 在 `items: HashMap<i64, TiFlashReplicaInfo>` 中维护状态：键是表 ID，值包含副本数与 location labels。`Load` 整体替换 map，`AddTable` 插入或覆盖，`DelTable` 删除，`Rewrite` 在 ID 改变时搬迁条目，`GetItems` 返回借用的只读视图，`Iterate` 按 `HashMap` 的非稳定顺序访问条目。这些不变量来自 `tiflash_recorder.rs` 及两个独立 Rust 测试，在扩展门面时必须保持。

## 依赖与调用关系

- 编译边界：`Cargo.toml` 没有 `[dependencies]`，该 crate 当前仅使用 Rust 标准库的 `HashMap`。它不依赖 parser、infoschema 或日志 crate；实现用本地最小类型和 `InfoSchema` trait 表示边界。
- 模块下游：`lib.rs` 直接加载 `tiflash_recorder.rs`；单测构建额外加载 `parity_test.rs` 和 `tiflash_recorder_test.rs`。
- 实现内部调用：`GenerateAlterTableDDLs` 与 `GenerateResetAlterTableDDLs` 调用 `Iterate`、`InfoSchema::TableByID`、`alterTableSpecOf` 和 `EncloseDBAndTable`；`Rewrite` 依赖 `HashMap::remove/insert`。RustCodeGraph 对这些 Go 风格方法未返回可用的 Rust callers/callees 列表，上述关系由索引中的完整实现源码直接核对。
- 当前 Rust 上游：在生产 `.rs` 中精确搜索 crate 名、模块名和 `TiFlashRecorder` 未找到其他 crate 的导入。`br/pkg/task/restore.rs` 定义了独立的 `TiFlashReplicaRecorder` trait，`PreCheckTableTiFlashReplica` 通过该 trait 的 `AddTable` 记录副本，但搜索未发现 `TiFlashRecorder` 对此 trait 的 `impl`。因此两者是语义上相邻的接线点，不是当前已证实的调用边。
- Go 上游：RustCodeGraph 显示 Go `tiflash_recorder.go` 被 `br/pkg/restore/log_client/client.go` 和 `br/pkg/task/restore.go` 使用，这是 Rust 后续完成主链接线时的参考，不能当作 Rust 已接线的证据。

## 错误处理与边界

`lib.rs` 没有可失败的运行时操作，因此不定义 `Result`、错误类型或 panic 策略。它的主要边界是编译期 API 面：子模块公开项的增删会由通配再导出自动改变 crate 根 API，因而可能影响下游兼容性。

实现边界中，`InfoSchema::TableByID` 返回 `None` 时，两个 DDL 生成方法都静默跳过对应表。`alterTableSpecOf` 虽返回 `Result<String, String>`，当前分支只构造字符串；调用方对 `Err` 采用跳过策略。表名和库名中的反引号会加倍，label 中的反斜杠和单引号会转义。由于返回 DDL 的顺序受 `HashMap` 迭代影响，调用方不应依赖输出顺序。

Go 实现在缺表、缺 schema 或 DDL spec 恢复失败时会记录告警；Rust 最小实现没有日志依赖，对缺表和 spec 错误都静默跳过。这是可观测性差异，扩展时不应误写为完全等价。

## 并发与资源生命周期

`lib.rs` 不创建线程、异步任务、通道、锁、事务或外部资源，也没有初始化/关闭钩子。模块加载和再导出均在编译期完成。

`InfoSchema` 被约束为 `Send + Sync`，使 DDL 生成边界能接收可跨线程安全共享的实现；但 `TiFlashRecorder` 的修改方法要求 `&mut self`，内部只是普通 `HashMap`，不自带同步。若多线程共享录制器，同步和所有权必须由上层提供。`GetItems` 的返回借用受录制器生命周期限制，`Iterate` 中的值也仅在回调期间借用，不发生隐式 clone 或持久化。

## 与 Go 版本的对应关系

Go 中的 `br/pkg/restore/tiflashrec` 是包目录，没有与 Rust `lib.rs` 一对一的源文件；Rust 入口是 Cargo 模块系统额外需要的门面。业务对照文件是 Go [`tiflash_recorder.go`](tiflash_recorder.go) 和 Rust [`tiflash_recorder.rs`](tiflash_recorder.rs)。

已对齐的主要契约包括：以 table ID 记录 `TiFlashReplicaInfo`，`Load` 整体替换，增删、迭代和 ID rewrite，生成普通或“先归零后恢复”的 TiFlash replica DDL，缺表时跳过，以及库表名/label 转义。`tiflash_recorder_test.rs` 对齐 Go `tiflash_recorder_test.go` 的 `TestRecorder`、`TestGenSql` 和 `TestGenResetSql`；`parity_test.rs::go_rust_public_contract_matches` 额外从 crate 根再导出入口验证 `Load`、同 ID rewrite 和缺表跳过等契约。

已知差异有：Rust 用本地 `TiFlashReplicaInfo`/`TableMeta`/`CIStr` 和最小 `InfoSchema` trait，而 Go 使用 TiDB model/infoschema/AST；Rust 手写 DDL 片段，Go 通过 `ast.AlterTableSpec.Restore` 输出；Rust 没有 Go 的 info/warn 日志；Rust 当前未找到与 `br/pkg/task/restore.rs::TiFlashReplicaRecorder` 的接线实现。因此该 crate 是已有独立行为和对照测试的迁移模块，但并非已验证接入完整 Rust restore 运行链。

## 扩展指南

- 新增录制器行为应优先修改 `tiflash_recorder.rs` 中的具体类型或 `impl TiFlashRecorder`，不要把业务逻辑塞进门面 `lib.rs`。
- 新增公开符号前需评估 `pub use ...::*` 会自动扩大 crate 根 API 的影响；若必须控制兼容面，可考虑改为显式再导出，但这是 API 设计变更，应单独评审。
- 扩展 `InfoSchema`、DDL 生成或引入新依赖时，需同步 `Cargo.toml`，并对照 Go `tiflash_recorder.go` 中的 AST restore 标志、缺表/缺 schema 策略和日志行为。手写 SQL 的兼容性与注入风险要用特殊引号用例锁定。
- 要将录制器接入 Rust restore 主链，需在 `br/pkg/task/restore.rs::TiFlashReplicaRecorder` 与该 crate 之间建立明确实现/依赖，并继续跟踪 Go `br/pkg/task/restore.go` 和 `br/pkg/restore/log_client/client.go` 的生命周期钩子。不应只因为方法同名就假定接线已存在。
- 生产行为修改应同步独立测试 `tiflash_recorder_test.rs`；公开门面或 Go/Rust 契约变更还应同步 `parity_test.rs`。保持测试与生产文件分离，并使顺序无关断言适应 `HashMap` 的非稳定迭代顺序。
- 性能上，增删改主要是 `HashMap` 常数时间操作，DDL 生成与记录数线性相关；扩展时避免在每条记录内引入额外全表扫描。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件（其中 7,032 个 Rust 文件）；`files --filter br/pkg/restore/tiflashrec` 列出 `lib.rs`、实现、Go 对照及 Rust/Go 测试六个文件。
- RustCodeGraph 源码节点：`node --file br/pkg/restore/tiflashrec/lib.rs`、`tiflash_recorder.rs`、`parity_test.rs`、`tiflash_recorder_test.rs`、`tiflash_recorder.go` 和 `tiflash_recorder_test.go`。`query TiFlashRecorder` 同时定位 Go/Rust 结构体，`query GenerateAlterTableDDLs` 同时定位 Go/Rust 方法。
- RustCodeGraph 调用证据：`explore` 返回 Go `Iterate <- GenerateAlterTableDDLs/GenerateResetAlterTableDDLs` 和 Go 实现的两个使用文件；对 Rust Go 风格方法的精确 `callers/callees` 查询无输出，因此 Rust 内部调用关系由索引源码节点核对，没有伪造静态图边。
- Cargo 边界：`br/pkg/restore/tiflashrec/Cargo.toml` 确认 crate 名、`lib.rs` 入口、Go package 元数据、library/lane 1 属性以及当前无外部依赖；根 `Cargo.toml` 确认 workspace member。
- 精确文本核对：在 `br/pkg/restore` 和 `br/pkg/task` 的 Rust 源码中搜索 crate/模块/类型与关键方法，确认当前生产路径只有 `br/pkg/task/restore.rs` 的独立 `TiFlashReplicaRecorder` trait 和 `PreCheckTableTiFlashReplica` 调用点，未找到本 crate 对其的实现或外部导入。
- 测试边界：Rust `parity_test.rs` 和 `tiflash_recorder_test.rs` 为独立测试文件，Go 对照为 `tiflash_recorder_test.go`。本任务是纯文档分析，按计划不运行 Cargo；交付验证仅检查文档存在且固定的十一个二级标题恰好各出现一次。
