# `br/pkg/config/lib.rs`

## 文件定位

本文件是 Cargo workspace 成员 `astersql-br-pkg-config` 的 crate 根；[`Cargo.toml`](Cargo.toml) 通过 `[lib] path = "lib.rs"` 指向它，并用 `package.metadata.porting.go-package = "br/pkg/config"` 标记对应的 Go 包。文件本身不是配置算法实现，而是把 [`ebs.rs`](ebs.rs) 与 [`kv.rs`](kv.rs) 组织成公开模块、挂载独立测试，并将两个子模块的公开项扁平再导出。

当前仓库的 Cargo manifest 搜索只在本 crate 清单中出现包名 `astersql-br-pkg-config`；Rust 源码也没有 `astersql_br_pkg_config` 的生产引用。因此它目前是 workspace 可单独构建和测试的迁移 crate 门面，尚未接入 Rust BR 的生产依赖链。Go 的 `br/pkg/config` 已被 `br/pkg/aws`、`br/pkg/conn` 和 `br/pkg/task` 使用，不能据此声称 Rust 门面也已接线。

## 核心职责

- 以 `pub mod ebs` 暴露 EBS 卷级备份/恢复元数据模型、校验及外部存储读取逻辑。
- 以 `pub mod kv` 暴露 TiKV 配置 JSON 的导入线程数、region 阈值与日志备份开关解析逻辑。
- 通过 `pub use ebs::*` 和 `pub use kv::*` 提供 crate 根级兼容 API，使调用方既可写 `crate::ebs::EBSBasedBRMeta`，也可直接写 `crate::EBSBasedBRMeta`。
- 仅在 `cfg(test)` 下挂载 [`parity_test.rs`](parity_test.rs) 与 [`ebs_test.rs`](ebs_test.rs)，保持生产构建不包含测试代码，并遵守测试与源文件分离的仓库约定。
- 在 crate 级放宽未使用项和 Go 风格命名 lint，使迁移代码能保留 `NewMetaFromStorage`、`ParseMergeRegionSizeFromConfig`、`ClusterInfo` 等 Go 对齐名称。

## 主要符号

本文件没有常量、结构体、枚举、trait、函数或 `impl`，也没有 feature 条件；它的全部语义来自模块项、再导出和 crate 属性。

- `#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_imports, unused_variables)]`：作用域覆盖整个 crate。它允许尚未接线的迁移 API 和 Go 风格公开符号存在，但也会降低编译器对死代码、命名漂移和未使用项的提示强度。
- `#[path = "ebs.rs"] pub mod ebs`：显式把同目录文件作为公开 `ebs` 模块。主要公开项包括 `EBSBasedBRMeta`、各组件/卷结构、`EBSVolumeType_Valid` 与 `NewMetaFromStorage`。
- `#[path = "kv.rs"] pub mod kv`：显式把同目录文件作为公开 `kv` 模块。主要公开项包括 `ConfigTerm<T>`、`KVConfig`、三个 `Parse*FromConfig` 函数及公开 `units` 模块。
- `#[cfg(test)] mod parity_test`、`#[cfg(test)] mod ebs_test`：测试构建中的私有子模块；显式 `#[path]` 指向独立文件，不形成生产 API。
- `pub use ebs::*`、`pub use kv::*`：通配再导出两个模块的全部公开项。新增 `pub` 符号会自动进入 crate 根 API，无需再改本文件。

## 执行流程

作为 crate 根，本文件没有运行时控制流。编译时先应用 crate 级 lint 设置，再解析两个公开模块；普通构建跳过带 `cfg(test)` 的测试模块，测试构建则额外编译 `parity_test.rs` 与 `ebs_test.rs`。最后两条 `pub use` 将子模块公开命名加入 crate 根命名空间。

调用方进入实际逻辑后有两条主要路径：EBS 路径由 `NewMetaFromStorage` 读取 `metautil::metafile::MetaFile`、反序列化为 `EBSBasedBRMeta` 并按固定顺序校验；TiKV 配置路径由三个 `Parse*FromConfig` 函数解析 HTTP 响应式 JSON，其中 region size 进一步进入 `units::RAMInBytes`。这些步骤位于子模块，不由 `lib.rs` 调度或包装。

## 数据与状态

本文件不持有运行时数据、全局可变状态或缓存。它只决定命名空间和编译条件。业务数据由 `ebs.rs` 的 `EBSBasedBRMeta` 对象和 `kv.rs` 的 `KVConfig`/临时反序列化结构持有。

通配再导出形成一个值得维护的 API 不变量：`ebs` 与 `kv` 的公开名称必须互不冲突。若两个模块以后增加同名公开项，crate 根的 glob re-export 可能产生歧义或编译错误；即使模块限定路径仍清楚，根级兼容 API 也会受影响。`cfg(test)` 则保证测试函数和测试辅助 import 不进入普通构建的符号面。

## 依赖与调用关系

`lib.rs` 的直接下游是 `ebs` 与 `kv` 两个模块。根据 [`Cargo.toml`](Cargo.toml)，前者依赖 `astersql-objstore-storeapi` 的 `Context`/`Storage`、`astersql-br-pkg-metautil` 的元数据对象名，以及 `serde_json`；后者依赖 `serde_json` 并内置 Go `docker/go-units` 容量解析语义。清单还声明 `semver = "1"`，但当前 EBS 代码使用私有校验器模拟 Go `Masterminds/semver` 的宽松规则，并未直接调用该依赖。

RustCodeGraph 的文件节点确认 `lib.rs` 只有一个自身符号，并显示两个实现文件均已索引；精确源码节点显示 `ebs.rs` 被 11 个文件按同名符号关联、`kv.rs` 被 2 个文件关联，但仓库级 Cargo/API 搜索没有找到生产 crate 对 `astersql-br-pkg-config` 的依赖或 `astersql_br_pkg_config` 引用。故这些图关联不能替代真实依赖接线证据。当前可确认的直接上游只有本 crate 的 [`ebs_test.rs`](ebs_test.rs) 和 [`parity_test.rs`](parity_test.rs)：前者经根级再导出使用 `crate::EBSBasedBRMeta`，后者同时验证模块限定入口 `crate::ebs::*`、`crate::kv::*`。

Go 对照的生产调用面更完整：`br/pkg/aws/ebs.go` 使用 `config.EBSBasedBRMeta`；`br/pkg/conn/conn.go` 调用 `ParseMergeRegionSizeFromConfig` 与 `ParseLogBackupEnableFromConfig`；`br/pkg/task/backup_ebs.go`、`restore_ebs_meta.go`、`restore.go` 和 `restore_raw.go` 消费模型或 KV 配置。Rust 相邻实现目前部分使用其他 crate 内的同名模型或本地解析逻辑，未来接线需先统一类型归属。

## 错误处理与边界

本文件不创建、转换或传播运行时错误。错误契约完全由子模块决定：EBS 路径使用字符串包装的 `ebs::Error`，并保留“缺集群信息、非法版本、零 resolved-ts、空 TiKV store”的校验优先级；KV JSON 函数传播 `serde_json::Error`，region size 解析还返回装箱错误。

门面层的主要边界是编译期可见性。`pub mod` 让子模块路径成为稳定公开面，glob re-export 又扩大根级公开面；删除或重命名任一公开项会同时影响两种访问路径。crate 级 `allow` 会掩盖未使用项和命名告警，因此维护者不能把“编译无警告”当作生产调用存在的证据。测试模块只在测试配置存在，生产代码不能依赖其中的辅助项。

## 并发与资源生命周期

`lib.rs` 不启动线程、异步任务，不创建锁、通道、文件或网络资源，也不拥有需要释放的对象。模块初始化没有 `static mut`、懒加载器或注册副作用，所以加载 crate 根本身不存在并发次序问题。

资源生命周期位于下游实现：`EBSBasedBRMeta` 的修改通过 `&mut self` 串行化；`NewMetaFromStorage` 借用调用者持有的 `Storage` 并同步读取一次完整对象，不取得存储所有权；KV 解析只持有调用期间的字节切片和临时反序列化值。若未来在门面加入全局配置、注册表或异步初始化，必须新增独立源文件与独立测试，而不应把状态逻辑堆入 crate 根。

## 与 Go 版本的对应关系

Go 没有对应的 `lib.rs` 文件；最接近的语义是同一目录形成的 `config` 包，包内 [`ebs.go`](ebs.go) 与 [`kv.go`](kv.go) 的大写标识符天然处于统一公开命名空间。Rust 用两个 `pub mod` 保留文件边界，再用两条 glob re-export 模拟 Go 包级扁平访问。

公开行为由独立测试核对：[`ebs_test.go`](ebs_test.go) 与 [`ebs_test.rs`](ebs_test.rs) 都从真实 fixture `ebs_backup.json` 加载配置；Rust `parity_test.rs` 进一步覆盖卷类型、setter、校验错误顺序、部分 JSON 合并、TiKV 配置解析和容量单位。差异包括 Rust 将 Go Kubernetes 强类型保存为不透明 JSON、用 `Option` 表达 Go 指针，并接受历史 JSON 键 `version`；这些差异属于 `ebs.rs`，门面只负责暴露它们。

Go 生产包已经广泛接线，而 Rust crate 尚无外部依赖者，这是当前迁移状态的核心差异。扩展文档或架构图应把本文件标为配置 API 聚合门面，而不是已经驱动 BR 运行时的入口。

## 扩展指南

新增 EBS 或 TiKV 配置行为时，应把实现放在对应子模块，并在独立的 `ebs_test.rs` 或 `parity_test.rs` 增加契约测试；不要把测试或业务函数写进 `lib.rs`。新增第三类配置域时，可建立独立 `<domain>.rs` 与 `<domain>_test.rs`，再在这里声明模块，并明确决定是否根级再导出。

增加公开项前应检查两个模块及 crate 根是否重名，并评估 glob re-export 是否仍合适。若要收紧 API，可改为显式 `pub use`，但这会改变 Go 风格扁平兼容面，需同步所有调用点和契约测试。调整 crate 级 lint 时，应先修复暴露出的真实告警；尤其不能通过更宽泛的 `allow` 隐藏新增死代码。

若要接入 Rust BR 生产链，应先在目标消费者的 `Cargo.toml` 增加该 crate 的 path 依赖，再替换或转换当前 `br/pkg/aws/ebs.rs` 等位置的同名数据模型，并验证 JSON、错误顺序和调用生命周期。不能只添加 import 后让两套 `EBSBasedBRMeta` 长期并存。性能风险主要来自子模块的完整 JSON/对象读取，门面扩展本身不应引入额外复制或全局同步。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/config` 列出 `lib.rs`、两个实现文件和独立测试；`node --file br/pkg/config/lib.rs --offset 1 --limit 240` 确认本文件完整 33 行及模块/再导出结构；对 `ebs.rs`、`kv.rs` 的文件节点确认主要公开符号与实现边界。宽泛 `explore` 给出 `NewMetaFromStorage -> checkEBSBRMeta` 等内部关系；精确 `callers NewMetaFromStorage` 查询在本地索引上长时间无输出后被中止，未把它当作无调用者证据。
- 清单与入口：直接阅读 [`Cargo.toml`](Cargo.toml) 和 workspace 根 `Cargo.toml`，确认 crate 名、根文件、Go 包映射、依赖及 workspace 成员关系；全仓检索 `astersql-br-pkg-config`/`astersql_br_pkg_config` 未发现外部 Rust 依赖者。
- Rust 源码与测试：阅读 [`lib.rs`](lib.rs)、[`ebs.rs`](ebs.rs)、[`kv.rs`](kv.rs)、[`ebs_test.rs`](ebs_test.rs) 和 [`parity_test.rs`](parity_test.rs)，并检索公开解析函数与 EBS 入口的 Rust 引用。
- Go 对照：阅读 [`ebs.go`](ebs.go)、[`kv.go`](kv.go)、[`ebs_test.go`](ebs_test.go)，并检索 `br/pkg/aws`、`br/pkg/conn`、`br/pkg/task` 中的生产调用点，以区分 Go 已接线行为与 Rust 当前状态。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并人工复核唯一新增产物、相对链接、未修改 `plan.md`、未把测试建议写入 Rust 源文件。
