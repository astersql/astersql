# `lightning/pkg/checkpoints/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-lightning-pkg-checkpoints` 的 crate 根；`lightning/pkg/checkpoints/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确指定它。它对应 Go 包 `lightning/pkg/checkpoints` 的 Rust 包级入口，但不是检查点算法或持久化后端的实现文件。业务实现位于 [`checkpoints.rs`](checkpoints.rs)，迁移期外部边界适配位于 [`stubs.rs`](stubs.rs)，本文件只负责把两者装配成一个公共 API 面，并在测试构建中注册独立测试模块（`lib.rs:26-50`）。

## 核心职责

1. 通过 `#[path = "stubs.rs"] mod stubs` 引入迁移边界桩，并以 `pub use stubs::*` 将其中的错误、配置、SQL、存储、日志、数据模型等兼容接口暴露到 crate 根（`lib.rs:26-29`）。这些接口是受控替身；`stubs.rs:5-18` 明确说明其中部分能力仅满足当前检查点测试，不能等同于完整生产依赖。
2. 通过 `#[path = "checkpoints.rs"] mod checkpoints` 引入真实检查点模型、diff/merger、`DB` 契约以及 Null、MySQL、文件三类后端，并以 `pub use checkpoints::*` 提供与 Go 包级导出面相近的访问路径（`lib.rs:31-34`）。
3. 使用 `#[cfg(test)]` 将 parity、核心模型、文件后端和 SQL 后端测试保留在独立文件，遵守测试逻辑不与生产源文件混放的仓库约定（`lib.rs:36-50`）。
4. 在 crate 级集中允许 Go 到 Rust 迁移期间产生的命名、未使用项和 Clippy 告警（`lib.rs:14-24`）。这降低了机械移植的编译噪声，但也意味着“无警告”不能用来证明桩已完成或 API 已被真实调用。

## 主要符号

本文件自身不定义常量、类型、trait、函数或 `impl`，也没有条件 feature；它的主要符号是模块声明和 glob 重导出：

- `mod stubs` / `pub use stubs::*`：私有模块、公开其全部公开项。典型导出包括 `Error`、`Result<T>`、`context`、`config`、`sql`、`storeapi`、`mydump` 和 `verify`（`stubs.rs`）。这些名字也供同一 crate 的 `checkpoints.rs` 通过 `crate::...` 使用。
- `mod checkpoints` / `pub use checkpoints::*`：私有实现模块、公开其全部公开项。核心导出包括状态常量、`ChunkCheckpointKey`、`ChunkCheckpoint`、`EngineCheckpoint`、`TableCheckpoint`、`TableCheckpointDiff`、`TableCheckpointMerger`、`DB`、`OpenCheckpointsDB`、`NullCheckpointsDB`、`MySQLCheckpointsDB` 和 `FileCheckpointsDB`（`checkpoints.rs:105-1742`）。
- `parity_test`：验证代表性的公共契约和 Go 对齐点（`parity_test.rs`）。
- `checkpoints_test`：验证状态、chunk、rebase merger，diff 应用、序列化和路径拆分（`checkpoints_test.rs`）。
- `checkpoints_file_test`：用临时文件验证 `FileCheckpointsDB` 的读写、删除、忽略和销毁错误检查点（`checkpoints_file_test.rs`）。
- `checkpoints_sql_test`：以本地内存 `sql::DB` 桩验证 MySQL 后端的公共行为，不连接网络数据库（`checkpoints_sql_test.rs:16-23`）。

由于两个 glob 重导出共享同一命名空间，新增公开符号时必须检查重名；冲突会在 crate 编译阶段暴露，而不是由本文件进行运行时仲裁。

## 执行流程

本文件没有运行时控制流。其“执行”发生在编译和名称解析阶段：

1. 编译器先应用 crate 级 `allow` 属性（`lib.rs:14-24`）。
2. `stubs.rs` 被装入私有模块，随后其公开项被提升到 crate 根（`lib.rs:26-29`）。源码顺序使后续 `checkpoints.rs` 可以通过 `crate::build`、`crate::config`、`crate::errors`、`crate::sql` 等路径引用这些边界（`checkpoints.rs:68-85`）。
3. `checkpoints.rs` 被装入并重导出，外部调用方因此可直接写 `astersql_lightning_pkg_checkpoints::OpenCheckpointsDB`，无须经过 `checkpoints` 子模块（`lib.rs:31-34`）。
4. 普通库构建到此结束；测试构建额外编译四个 `#[cfg(test)]` 模块（`lib.rs:36-50`）。测试以 `use crate::*` 访问同一公共面，从而同时验证门面装配和实现语义。
5. 运行时真正的驱动选择、初始化、读取、增量更新和持久化发生在重导出的 `OpenCheckpointsDB`、`DB` 实现及 merger 中，不发生在 `lib.rs`。例如 `server/lightning.rs` 通过 crate 根调用 `IsCheckpointTable` 与 `OpenCheckpointsDB`，`server/checkpoint_control.rs` 以 `dyn DB` 操作检查点。

## 数据与状态

`lib.rs` 不拥有全局变量、缓存、锁或持久化状态。它暴露的状态模型来自 `checkpoints.rs`：

- `CheckpointStatus` 及从 `Missing` 到 `Analyzed` 的数值常量构成与 Go 对齐的阶段协议（`checkpoints.rs:105-133`）。
- `TableCheckpoint` 聚合表、engine、chunk 的恢复快照；`TableCheckpointDiff` 和各类 merger 表示增量更新（`checkpoints.rs:334-742`）。
- `DB: Send` 是上层依赖的持久化边界，具体状态由 Null、MySQL 或文件后端持有（`checkpoints.rs:781-1742`）。
- 文件后端使用本地 `checkpointspb` crate；Cargo 清单仅声明该路径依赖以及 `serde`、`serde_json`。其余 MySQL、对象存储、配置等依赖当前由 `stubs.rs` 在本 crate 内提供，而非外部真实 crate。

所以修改本门面不会直接变更数据格式，但改变重导出集合或模块可见性会改变所有消费者可见的类型身份和访问路径。

## 依赖与调用关系

下游装配关系为：`lib.rs -> stubs.rs`、`lib.rs -> checkpoints.rs -> checkpointspb`。`checkpoints.rs` 还通过 crate 根依赖 `stubs.rs` 提供的 `config`、`errors`、`sql`、`storeapi`、`mydump`、`verify` 等边界（`checkpoints.rs:66-85`）。

Cargo 与源码引用搜索确认主要上游为：

- `lightning/pkg/server`：Cargo 依赖本 crate；`lightning.rs` 调用 `IsCheckpointTable`、`OpenCheckpointsDB`，`checkpoint_control.rs` 使用 `DB` 并执行任务、表及错误检查点管理。
- `lightning/pkg/importer`：Cargo 依赖本 crate；`import.rs`、`table_import.rs`、`chunk_process.rs`、`dup_detect.rs`、`precheck.rs` 等以 `checkpoints` 别名使用快照、状态和 merger。
- `lightning/pkg/progress`：Cargo 依赖本 crate；`progress.rs` 导入检查点状态与结构，用于进度汇总。

RustCodeGraph 将 `lib.rs` 识别为单一文件节点，没有可供 `callers`/`callees` 查询的函数；对 `OpenCheckpointsDB` 的图查询也未返回 Rust 调用边。因此上述上游关系由 Cargo 清单和限定名源码引用补证，不把图中“空调用者”解释成“没有消费者”。

## 错误处理与边界

本文件不创建、转换或传播运行时错误。错误契约来自 `stubs.rs` 的 `Error`/`Result<T>` 与 `errors` 兼容模块，并由 `checkpoints.rs` 的公开入口返回。当前桩至少区分一般错误、`not_found` 与 `no_rows` 标志（`stubs.rs:46-112`），文件及 SQL 测试会验证不存在表等错误的分类和文本。

门面的关键边界如下：

- `#![allow(..., clippy::all)]` 会掩盖整个 crate 的静态告警；新增真实实现不能据此省略输入校验或错误分支。
- `pub use stubs::*` 使迁移桩成为外部可见 API。调用方能编译不代表底层拥有完整的取消、网络 SQL、对象存储或日志能力；例如当前 `context::Context` 不承载取消、deadline 或键值（`stubs.rs:115-133`）。
- `#[cfg(test)]` 模块不会进入普通库构建，测试辅助代码不能成为生产实现的隐式依赖。
- `#[path]` 把模块绑定到固定相对文件名；移动或拆分实现时必须同步这里，否则 crate 根无法装配。

## 并发与资源生命周期

`lib.rs` 不启动线程、任务或通道，也不获取锁和文件句柄。并发和资源生命周期完全由重导出的实现承担：`DB` 的 `Send` 上界允许数据库对象跨线程所有权边界传递（`checkpoints.rs:781`）；MySQL/内存 SQL 状态和文件模型中的互斥保护定义在 `checkpoints.rs`、`stubs.rs` 内。本文件不会自动关闭数据库或删除文件，调用方必须遵守具体后端的生命周期契约。

测试模块的资源也在独立文件管理：文件后端测试为每个夹具创建带进程号和纳秒时间戳的临时目录，并由测试持有路径直到数据库使用结束（`checkpoints_file_test.rs:66-96`）；SQL 测试使用 `sql::DB::new_memory()`，不建立网络连接（`checkpoints_sql_test.rs:19-23,72-79`）。这些事实不能外推为生产 MySQL 后端没有外部资源。

## 与 Go 版本的对应关系

Go 版本以目录和 `package checkpoints` 自然形成包级命名空间；Rust 没有对应的单一 Go `lib.go`，因此本文件承担的是 Cargo crate 根和包级导出聚合角色。`checkpoints.rs` 明确移植自 `lightning/pkg/checkpoints/checkpoints.go`，状态值、表名版本、主要模型、`DB` 接口及后端入口保持 Go 风格命名和语义映射。

测试也按 Go 文件逐组对应：

- `checkpoints_test.rs` 对应 `checkpoints_test.go`，双方覆盖 8 组核心模型/辅助函数测试。
- `checkpoints_file_test.rs` 对应 `checkpoints_file_test.go`，覆盖文件后端读取、删除、忽略与销毁错误状态。
- `checkpoints_sql_test.rs` 对应 `checkpoints_sql_test.go`，但 Go 使用 `go-sqlmock`，Rust 当前使用 crate 内存 SQL 桩；这验证公共行为，并不证明真实数据库协议完全等价。
- `parity_test.rs` 是 Rust 侧额外的代表性契约校验，不对应单个 Go 源文件。
- Go 的 `main_test.go` 负责测试套件初始化；当前 `lib.rs` 文档注释提到 `main_test`，但实际没有声明 `main_test.rs`，所以 Rust 测试入口以这里列出的四个模块为准。

## 扩展指南

- 新增检查点业务类型、算法或后端行为，应修改 `checkpoints.rs`，不要把实现写入 `lib.rs`；同步扩展同目录独立 `*_test.rs`，并对照相应 Go 实现/测试保持状态值、键、持久化格式和错误语义。
- 新增外部依赖边界时，先判断是否应接入真实 crate。若仍需迁移桩，应放在 `stubs.rs` 并明确能力限制；不要在 `lib.rs` 中添加业务桩。
- 新模块只有确需成为 crate API 时才在这里声明/重导出。优先显式重导出以降低 glob 冲突风险；若继续使用 glob，至少检查与 `stubs`、`checkpoints` 现有公开项重名。
- 修改 `CheckpointStatus` 数值、检查点表名版本、protobuf 模型、chunk 键或 `DB` trait 属于兼容性高风险变更，应同步 Go 对照、文件 round-trip、SQL 行为和 importer/server 调用测试。
- 将桩替换为真实实现时，重点复核取消传播、真实 MySQL/对象存储错误、锁粒度、文件原子性和资源关闭；当前本地测试替身不能覆盖这些生产风险。
- 保持测试在 `parity_test.rs`、`checkpoints_test.rs`、`checkpoints_file_test.rs`、`checkpoints_sql_test.rs` 等独立文件，不要把测试逻辑嵌回 `lib.rs` 或 `checkpoints.rs`。

## 验证依据

- RustCodeGraph 状态：索引包含 7,032 个 Rust 文件；`files --filter lightning/pkg/checkpoints` 找到目标、实现、桩、protobuf 和对应测试文件。
- RustCodeGraph `node --file lightning/pkg/checkpoints/lib.rs`：确认目标共 50 行、只有模块装配与重导出，没有运行时符号。
- RustCodeGraph `query`：确认 Rust `OpenCheckpointsDB` 位于 `checkpoints.rs:816`、`IsCheckpointTable` 位于 `checkpoints.rs:279`、`NewFileCheckpointsDB` 位于 `checkpoints.rs:1701`；对应 Go 符号分别位于 `checkpoints.go:631`、`:212`、`:1199`。对 Rust `OpenCheckpointsDB` 的 `callers`/`callees` 查询为空，已用 Cargo 和源码引用搜索补证调用关系。
- 已读生产与清单：`lightning/pkg/checkpoints/lib.rs`、`Cargo.toml`、`checkpoints.rs`、`stubs.rs`、`checkpointspb/Cargo.toml`（依赖关系由根 Cargo 清单与源码引用确认）。
- 已读 Go 对照：`lightning/pkg/checkpoints/checkpoints.go`，并检索 `checkpoints_test.go`、`checkpoints_file_test.go`、`checkpoints_sql_test.go`、`main_test.go` 的测试入口。
- 已读 Rust 测试：`parity_test.rs`、`checkpoints_test.rs`、`checkpoints_file_test.rs`、`checkpoints_sql_test.rs`；测试均由 `lib.rs` 的 `#[cfg(test)]` 独立模块装配。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文存在且恰好包含本计划规定的 11 个二级标题。
