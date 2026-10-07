# `br/pkg/errors/lib.rs`

源码：[lib.rs](./lib.rs)；实际实现：[errors.rs](./errors.rs)。

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-errors` 的 crate 根，而不是错误实现本身。`br/pkg/errors/Cargo.toml` 的 `[lib] path = "lib.rs"` 将它指定为编译入口，`[package.metadata.porting]` 则把该 crate 对应到 Go 包 `br/pkg/errors`，类型为 `library`。入口通过 `#[path = "errors.rs"] mod errors;` 装入实际实现，再用 `pub use errors::*;` 形成供其他 BR Rust crate 使用的公共门面。

该文件位于 BR 公共基础设施层：连接、PD、恢复、元数据、GC、日志、版本和通用工具等 crate 都通过 Cargo 路径依赖 `astersql-br-pkg-errors`，再从这个根模块导入统一的 RFC 错误常量与判定函数。它不属于某一条备份或恢复业务流程，而是这些流程共享的错误协议入口。

## 核心职责

1. 固定 crate 的公共 API 边界：实现模块 `errors` 保持私有，`errors.rs` 中的公开项由 `pub use errors::*` 平铺到 crate 根，调用方因此使用 `astersql_br_pkg_errors::ErrInvalidArgument`、`Is` 或 `IsContextCanceled`，无需知道内部文件布局。
2. 固定测试装配方式：仅在 `cfg(test)` 下以显式路径加载 `parity_test.rs` 和 `errors_test.rs`，使测试逻辑与生产源文件分离。
3. 为 Go 风格移植保留命名兼容：crate 级 `allow` 放宽 `dead_code`、`non_snake_case`、`non_camel_case_types` 和 `non_upper_case_globals`，从而允许 `Is`、`ErrPDUpdateFailed` 等名称忠实对应 Go 公共契约。
4. 提供 crate 级文档，明确真正实现位于 `errors.rs`，并说明测试模块与 Go 的 `package errors` / `package errors_test` 分工对应。

## 主要符号

- `mod errors`（`lib.rs:14-15`）：私有生产模块声明，路径被显式绑定到同目录 `errors.rs`。模块内包含 `Canceled`、`DeadlineExceeded`、`Is`、`IsContextCanceled`、私有取消链辅助函数、`br_err!` 宏和全部 BR RFC 错误静态量。
- `pub use errors::*`（`lib.rs:16`）：唯一的生产公共出口。它把 `errors.rs` 中所有 `pub` 项提升到 crate 根；私有的 `chain_is_canceled`、`shared_is_canceled` 和 `br_err!` 不会被导出。
- `mod parity_test`（`lib.rs:18-20`）：测试配置下加载公共契约快照测试。`all_normalized_errors_match_go` 穷举核对消息模板与 RFC ID，`go_rust_public_contract_matches` 抽查 `Is`、`Equal`、取消链及包装语义。
- `mod errors_test`（`lib.rs:22-24`）：测试配置下加载从 Go 测试直接移植的行为测试，覆盖 `IsContextCanceled` 与按 RFC ID 判断相等的行为。
- crate 属性 `#![allow(...)]`（`lib.rs:7-12`）：作用于整个 crate，主要服务 Go API 名称兼容；新增 Rust API 不应无理由沿用这些非惯用命名。

`lib.rs` 没有自定义类型、函数、trait、常量、`impl`、线程或异步入口。RustCodeGraph 将其识别为只有一个模块级符号的文件，实际业务符号均来自被挂载的 `errors.rs`。

## 执行流程

编译生产 crate 时，Rust 先读取 `lib.rs`，应用四项 crate 级 lint 放宽，然后解析 `errors.rs` 为私有子模块。`pub use errors::*` 随后把其中的公共哨兵、函数和惰性错误静态量加入 crate 根命名空间。下游 crate 编译时，通过各自 `Cargo.toml` 的路径依赖解析本 crate，并直接引用这些再导出项。

运行期没有“调用 `lib.rs`”这一步。以 `br/pkg/conn/conn.rs` 为例，调用方从 crate 根导入 `Canceled`、`ErrKVNotTiKV`、`ErrPDInvalidResponse` 和 `IsContextCanceled`；真正执行的是 `errors.rs` 中的判定函数，或首次解引用相应 `LazyLock<astersql_errors::Error>` 时执行规范化错误构造。恢复导入路径 `br/pkg/restore/snap_client/import.rs` 还直接使用多个 KV/PD 错误静态量并构造 `Canceled`，说明该门面同时服务错误生成、分类和取消传播。

运行 `cargo test` 对该 crate 进行测试编译时，两个 `#[cfg(test)]` 分支才生效；测试模块以 `super::*` 或明确的 `super::{...}` 访问相同的 crate 根契约。普通依赖构建不会编译这两个测试文件。

## 数据与状态

`lib.rs` 自身不保存可变数据。它暴露的数据来自 `errors.rs`：

- `Canceled` 与 `DeadlineExceeded` 是无字段、`Copy` 的哨兵类型，显示文本分别对齐 Go `context.Canceled` 与 `context.DeadlineExceeded`。
- BR 错误定义由私有 `br_err!` 宏生成公开的 `LazyLock<astersql_errors::Error>`。每项由稳定的消息模板和 RFC CodeText 构成，覆盖 Common、PD、Backup、Restore、Stream、PiTR、ExternalStorage、EBS 和 KV 分类。
- `LazyLock` 只在第一次访问时构造规范化错误，之后共享只读值；该初始化状态由标准库管理，不由 `lib.rs` 手工维护。

公共面的重要不变量是名称、消息模板和 RFC ID 的稳定性。部分 ID 保留历史拼写或与 Rust 标识不同，例如 `ErrPDUnknownScatterResult`/`ErrPDSplitFailed` 使用 `BR:PD:ErrPDUknownScatterResult`，`ErrKVNotTiKV` 使用 `BR:KV:ErrNotTiKVStorage`；入口的全量再导出意味着这些细节就是跨 crate 兼容契约。

## 依赖与调用关系

下游依赖只有 `astersql-errors`，由 `br/pkg/errors/Cargo.toml` 以工作区路径 `../../../pkg/errors` 引入。`errors.rs` 从它使用 `Cause`、`Find`、`SharedError`、`Normalize` 和 `RFCCodeText`；`lib.rs` 本身不直接调用第三方 API，也没有 feature 条件。

RustCodeGraph 显示 `errors.rs` 被 `br/pkg/conn/conn.rs`、`br/pkg/utils/backoff.rs`、`br/pkg/utils/store_manager.rs`、`br/pkg/restore/split/split.rs` 及相关测试等直接使用。Cargo 与源码反向搜索进一步确认直接依赖者包括：

- `br/pkg/conn` 与 `br/pkg/conn/util`：连接失败、PD 响应、取消判定；
- `br/pkg/pdutil`：PD 更新和响应错误，并用 `Is` 做 ID 匹配；
- `br/pkg/restore/utils`、`br/pkg/restore/split`、`br/pkg/restore/snap_client`：范围、重写、备份有效性、region/KV 下载与 ingest 分类；
- `br/pkg/metautil`、`br/pkg/gc`、`br/pkg/version`：元文件、safepoint、版本兼容错误；
- `br/pkg/logutil`、`br/pkg/utils`：参数、未知错误、连接、等待取消和重试分类。

典型下游边包括 `conn.rs -> IsContextCanceled`、`split.rs -> Is as BrIs`、`import.rs -> ErrKVEpochNotMatch/ErrKVDownloadFailed/ErrKVIngestFailed`。典型内部边是 `Is -> astersql_errors::Find`，以及 `IsContextCanceled -> Cause -> shared_is_canceled -> chain_is_canceled -> StdError::source`。门面只负责让这些符号可达，不改变调用语义。

## 错误处理与边界

入口文件不捕获、转换或记录错误；所有错误语义由再导出的实现决定。`Is(None, ...)` 因找不到因果链元素而返回 `false`；`Is` 对 `astersql_errors::Error` 按 RFC ID 而非实例地址或消息匹配，因此注解后的错误仍可分类。`IsContextCanceled(None)` 返回 `false`，并同时检查 `Cause` 结果和完整 `StdError::source` 链，以覆盖直接哨兵、`Trace` 包装及 Go `fmt.Errorf("%w")`/`url.Error` 形态的间接包装。

边界风险主要来自公共面过宽：`pub use errors::*` 会自动公开 `errors.rs` 新增的任何 `pub` 项，也会在重命名或删除现有项时立即破坏所有下游。错误名称、消息模板和 RFC ID 可能被日志、重试策略、兼容检查或外部诊断工具消费，不能把拼写“修正”视为无害重构。当前门面也刻意允许未使用与非 Rust 风格命名，不能用收紧 lint 的方式意外阻断尚未接线但属于 Go 对齐面的错误项。

## 并发与资源生命周期

`lib.rs` 不创建线程、任务、通道、锁、事务、网络连接或文件句柄，也没有显式清理阶段。两个测试模块仅受编译期 `cfg(test)` 控制，不参与生产资源生命周期。

唯一与并发初始化有关的间接状态是再导出的 `LazyLock<errors::Error>`：首次并发访问由 `std::sync::LazyLock` 保证一次初始化，初始化完成后只读共享。`Canceled` 和 `DeadlineExceeded` 无资源所有权；`Is`/`IsContextCanceled` 只借用或克隆共享错误句柄并遍历有限的因果链，不持有跨调用锁。若未来为入口增加全局注册表或可变配置，必须单独说明同步、初始化失败和关闭语义，不能假设现有门面已提供这些能力。

## 与 Go 版本的对应关系

Go 的 `br/pkg/errors/errors.go` 将函数与所有 `errors.Normalize` 变量放在同一个包文件中；Rust 将同等实现放在 `errors.rs`，由本 `lib.rs` 组装成独立 Cargo crate。`pub use errors::*` 使 Rust 使用方获得接近 Go 包级导出的调用形态，是两种模块系统之间的适配层。

语义对应如下：Go `Is(err error, is *errors.Error)` 使用 `errors.Find` 并比较 ID，Rust `Is(Option<&SharedError>, &errors::Error)` 使用相同的查找与 ID 比较；Go `IsContextCanceled` 先取 `errors.Cause`，再比较 context 哨兵并调用标准库 `errors.Is`，Rust用 `Cause`、`Canceled`/`DeadlineExceeded` 和 `StdError::source` 链实现相同行为。Go 的包级 `var` 在 Rust 中对应 `LazyLock` 静态量。

测试分工也被显式保留：`errors_test.rs` 对应 Go `errors_test.go` 的外部包视角，覆盖 nil/普通错误、取消、超时、Trace、类 `url.Error` 包装与 Annotate 后 Equal；`parity_test.rs` 是 Rust 额外的契约保护，穷举全部 Go `errors.Normalize` 声明，防止 RFC ID 或消息模板漂移。当前 Go Bazel `go_library`/`go_test` 与 Rust Cargo crate 并存；`lib.rs` 不替代或桥接 Go 构建目标。

## 扩展指南

新增 BR 错误码时，应在 `errors.rs` 使用现有 `br_err!` 形式增加公开静态量，保持 Go `errors.go` 的名称、消息模板、RFC CodeText 和分类完全一致；随后把对应条目加入 `parity_test.rs::all_normalized_errors_match_go`。若 Go 侧同时新增边界行为，应扩展独立的 `errors_test.rs`，不要把测试写入 `lib.rs` 或 `errors.rs`。

新增判定辅助函数时，先判断它是否应成为整个 crate 的公共契约。若标为 `pub`，现有通配再导出会自动公开它；若仅供实现内部使用，应保持私有。修改 `Is` 或取消识别链时，至少同步覆盖 `None`、无关错误、直接哨兵、`Trace`、自定义 `source` 包装和不同 RFC ID；还应检查 `conn.rs`、`pdutil`、`restore/split` 等现有消费者的分类及重试语义。

不要为单个业务模块把依赖反向塞入本 crate，以免基础错误层形成环。性能方面应继续保持错误定义惰性、只读且低成本；兼容方面禁止未经验证改动历史 ID 拼写。若未来不再使用通配再导出，可改为显式导出列表，但这是影响所有下游名称解析的 API 变更，需要先枚举 Cargo 反向依赖并进行全工作区验证。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，其中 Rust 文件 7,032 个；`files --filter br/pkg/errors` 确认 `lib.rs`、`errors.rs`、两份 Rust 测试及 Go 对照均在索引中。
- `rustcodegraph node --file br/pkg/errors/lib.rs --offset 1 --limit 240`：确认 24 行入口、私有实现模块、全量再导出、两项 `cfg(test)` 模块与 crate lint。
- `rustcodegraph node --file br/pkg/errors/errors.rs`：确认公开哨兵、`Is`、`IsContextCanceled`、内部 source 链遍历、`br_err!` 与各错误分类；文件共 516 行。
- `rustcodegraph explore "br/pkg/errors/lib.rs error constants register_error errors"` 及符号查询：确认入口本身只有模块级装配职责，并看到 BR 连接、PD、恢复和工具链上的实际消费者；精确查询定位 Rust `IsContextCanceled` 于 `errors.rs:87`、`Is` 于 `errors.rs:46`。
- `br/pkg/errors/Cargo.toml`：确认 crate 名、`lib.rs` 入口、Go 包映射、library/lane 元数据、唯一直接依赖 `astersql-errors`，且未声明 feature。
- `br/pkg/errors/errors.go` 与 `br/pkg/errors/errors_test.go`：确认 Go 的 ID 匹配、context 取消语义、全部规范化错误定义及原始测试边界。
- `br/pkg/errors/errors_test.rs` 与 `br/pkg/errors/parity_test.rs`：确认测试保持独立文件，并分别覆盖 Go 测试语义和全量消息/RFC ID 快照。
- Cargo/源码反向搜索 `rg -n 'astersql-br-pkg-errors|astersql_br_pkg_errors'`：确认 `conn`、`pdutil`、`restore`、`metautil`、`gc`、`logutil`、`version`、`utils` 等直接依赖和引用。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务给定命令检查目标文件存在且恰有十一个固定二级标题，并人工复核文档未把门面误写为业务实现。
