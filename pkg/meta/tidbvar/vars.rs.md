# `pkg/meta/tidbvar/vars.rs`

## 文件定位

本文件是 `astersql-meta-tidbvar` crate 的键名定义文件，集中声明 `mysql.tidb` 内部表所使用的变量名，避免各调用点重复拼写字符串。crate 入口 `pkg/meta/tidbvar/lib.rs` 通过 `pub mod vars` 装入本模块，并以 `pub use vars::*` 将其公开项提升到 crate 根；仓库总门面 `pkg/lib.rs` 又将整个 crate 暴露为 `meta::tidbvar`。`pkg/meta/tidbvar/Cargo.toml` 的 `package.metadata.porting.go-package` 明确把它对应到 Go 包 `pkg/meta/tidbvar`。

该文件只定义协议键名，不负责访问 `mysql.tidb`、序列化变量值、判断 TTL 或执行资源伸缩。当前仓库中这些运行时行为仍可在 Go DXF 路径 `pkg/dxf/framework/handle/status.go::getPauseScaleInFlag` 和 `pkg/dxf/framework/handle/handle.go::UpdatePauseScaleInFlag` 中看到。

## 核心职责

1. 用规范的 Rust 名称 `DXF_SCHEDULE_PAUSE_SCALE_IN` 固定 DXF 暂停缩容标志的持久化键值 `"dxf_schedule_pause_scale_in"`。
2. 用公开再导出 `DXFSchedulePauseScaleIn` 保留 Go 的 PascalCase 名称，使逐步迁移的调用点无需同时改名。
3. 维持 Rust、Go 与 `mysql.tidb.variable_name` 三者的字符串兼容性。这里的兼容对象是键名，而不是键值中承载的 `TTLFlag` 数据结构。

## 主要符号

- `pub const DXF_SCHEDULE_PAUSE_SCALE_IN: &str`（`vars.rs:27`）：规范 Rust API。类型是具有静态生命周期的字符串切片；其值在编译期确定，不产生运行时初始化。
- `pub use DXF_SCHEDULE_PAUSE_SCALE_IN as DXFSchedulePauseScaleIn`（`vars.rs:32`）：同一常量项的公开别名，不创建第二份状态或第二个可独立修改的值。该 PascalCase 命名由 `lib.rs` 的 crate 级 lint 允许项兼容。

本文件没有结构体、枚举、trait、函数、`impl`、宏或条件编译项，也没有私有符号。

## 执行流程

本文件本身没有可执行控制流。其静态使用链如下：

1. `lib.rs` 编译并公开 `vars` 模块，然后把本文件的公开符号再导出到 `astersql_meta_tidbvar` crate 根。
2. 使用方在构造 `mysql.tidb` 查询或写入时引用常量，而不是内联字符串。Go 侧的现实读路径在 `getPauseScaleInFlag` 中把键作为 `WHERE VARIABLE_NAME = %?` 的参数；写路径在 `UpdatePauseScaleInFlag` 中把同一键作为 `REPLACE INTO mysql.tidb(variable_name, variable_value)` 的第一个值。
3. 数据库只按字符串匹配键，因此 Rust 规范名与 Go 兼容别名最终都必须解析为完全相同的字节序列。
4. Rust 独立测试 `migration_aster_unit_test.rs` 分别验证规范常量的字面量，以及兼容别名与规范常量相等。

需要注意：仓库搜索未发现生产 Rust 源码直接引用这两个符号。`pkg/dxf/framework/handle/Cargo.toml` 已声明可选依赖 `astersql-meta-tidbvar`，说明 crate 边界已预留，但不能据此声称 Rust DXF 读写流程已经接线。

## 数据与状态

唯一数据是静态字符串 `"dxf_schedule_pause_scale_in"`。它是 `mysql.tidb` 表中 `VARIABLE_NAME` 的协议标识符；对应 `VARIABLE_VALUE` 的 JSON 内容、`Enabled` 与 `ExpireTime` 等状态不在本文件定义。

该常量不可变、无堆分配、无延迟初始化，也不持有数据库连接或进程内缓存。别名是对同一常量项的重命名再导出，因此不存在两份可能漂移的 Rust 值。真正可变的持久化状态位于数据库行中；Go 读路径在记录不存在时返回默认的空 `TTLFlag`，在标志已过期时也重置为默认值，这些语义属于消费者而非本文件。

## 依赖与调用关系

**上游暴露关系：**

- `pkg/meta/tidbvar/lib.rs` 声明并通配再导出本模块。
- 根 `Cargo.toml` 将 `pkg/meta/tidbvar` 纳入 workspace，并以 `facade_meta_tidbvar` 引入根门面。
- `pkg/lib.rs::meta::tidbvar` 再导出 `facade_meta_tidbvar::*`。
- `pkg/dxf/framework/handle/Cargo.toml` 对本 crate 声明了可选路径依赖；当前生产 Rust 源码未检索到符号级使用。

**下游依赖：**

- 本文件不导入任何 Rust crate 或模块，常量声明只依赖语言内建的 `&str`。
- 语义消费者的直接证据来自 Go：`status.go::getPauseScaleInFlag` 使用键读取行，`handle.go::UpdatePauseScaleInFlag` 使用键替换行。
- Rust 测试消费者为 `pkg/meta/tidbvar/migration_aster_unit_test.rs` 中的 `dxf_schedule_pause_scale_in_matches_go_key` 与 `go_compatible_name_uses_the_canonical_rust_constant`。

RustCodeGraph 已索引 `vars.rs`，文件节点显示 32 行和一个文件级符号，但对两个公开常量名的 `query` 未返回独立符号节点，`callers`/`callees` 也未形成可用的符号调用边；因此这里的公开链与消费者关系以模块声明、Cargo 清单和精确文本引用为补充证据，而没有把图索引的缺失解释为“没有调用”。

## 错误处理与边界

常量定义不会返回错误，也没有运行时失败分支。它的主要边界是跨语言、跨版本的持久化协议：修改字符串会使新代码读写不同的 `mysql.tidb` 行，从而与既有集群控制器、旧版本节点或 Go 实现失配。仅修改 Rust 标识符也可能破坏调用方源码兼容性，因此现有 Go 风格别名不应在迁移完成前移除。

本文件不验证数据库表是否存在、不解析 `VARIABLE_VALUE`、不处理 SQL 或 JSON 错误，也不判定 TTL。Go 侧证据显示 SQL 错误、JSON 反序列化错误由消费者传播；这些错误不能归因于常量模块。记录缺失和记录过期也由消费者赋予默认语义，本模块没有隐含默认值。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或资源句柄。`&'static str` 常量可被任意线程只读共享，不需要同步或清理。

并发协调发生在该键所标识的外部状态上，而非 Rust 常量对象上：Go 注释说明集群控制器用它暂停 DXF worker 缩容，以避免缩容与任务调度冲突；Go 写路径在 `TaskManager.WithNewTxn` 中执行 `REPLACE`，读路径在 `WithNewSession` 中查询。事务边界、并发覆盖和过期清理策略均由这些消费者管理。

## 与 Go 版本的对应关系

`pkg/meta/tidbvar/vars.go` 定义 `DXFSchedulePauseScaleIn = "dxf_schedule_pause_scale_in"`，与 Rust 的字面量完全一致。Rust 在此基础上采用双名称策略：全大写的 `DXF_SCHEDULE_PAUSE_SCALE_IN` 符合 Rust 常量惯例，`DXFSchedulePauseScaleIn` 则保留 Go API 名称，并通过 `pub use` 保证二者不会各自维护字符串。

Go 包注释把 `mysql.tidb` 描述为 bootstrap、GC 与 DXF 等组件共用的内部变量位置；当前这个具体文件只包含 DXF 的一个键，不能从包级说明推导出 Rust 已经移植了其他 bootstrap 或 GC 键。Go DXF 已有真实 SQL 读写调用；Rust 侧目前只有常量、门面/可选依赖接线与迁移单元测试，尚未检索到生产 Rust 调用点。

## 扩展指南

- 新增 `mysql.tidb` 键时，应在本文件增加一个规范的 `SCREAMING_SNAKE_CASE` 常量；若需要承接 Go 调用名，再用 `pub use` 建立别名，避免复制字面量。
- 必须先核对同路径 Go 定义和真实 SQL 消费者，确认键名、值格式、缺失记录与过期行为；不要把值结构或业务默认逻辑塞进纯键名模块。
- 在独立测试文件 `pkg/meta/tidbvar/migration_aster_unit_test.rs` 中同步增加字面量兼容测试与别名一致性测试。按照仓库约定，不要把测试内嵌到 `vars.rs`。
- 如果为 Rust DXF 增加生产调用，应在对应 crate 中启用/使用 `astersql-meta-tidbvar` 依赖，并引用常量而不是重复字符串；同时为 SQL 读写、JSON 错误、记录缺失、TTL 过期和事务行为在消费者目录增加独立测试。
- 变更既有键值属于持久化协议变更，需评估滚动升级兼容、旧数据库行迁移和混合版本集群行为。读取常量本身无性能风险，性能与事务风险位于数据库消费者。

## 验证依据

- 源码与模块边界：`pkg/meta/tidbvar/vars.rs:16-32`、`pkg/meta/tidbvar/lib.rs:15-21`、`pkg/meta/tidbvar/Cargo.toml:1-12`。
- workspace 与门面：根 `Cargo.toml` 的 workspace 成员 `pkg/meta/tidbvar` 和 `facade_meta_tidbvar` 依赖；`pkg/lib.rs::meta::tidbvar`。
- Go 对照与运行态调用：`pkg/meta/tidbvar/vars.go:15-25`、`pkg/dxf/framework/handle/status.go::getPauseScaleInFlag`、`pkg/dxf/framework/handle/handle.go::UpdatePauseScaleInFlag`。
- Rust 独立测试：`pkg/meta/tidbvar/migration_aster_unit_test.rs::dxf_schedule_pause_scale_in_matches_go_key` 与 `go_compatible_name_uses_the_canonical_rust_constant`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/meta/tidbvar` 确认目标、入口、Go 对照和独立测试均被索引；`node --file pkg/meta/tidbvar/vars.rs --offset 1 --limit 400` 返回目标文件全貌。精确 `query` 未给常量生成独立节点，故调用关系由 `rg`、Cargo 和模块入口交叉核验。
- 文档结构采用任务要求的十一个固定二级章节；本任务是纯文档分析，按计划不运行 Cargo 或代码测试。
