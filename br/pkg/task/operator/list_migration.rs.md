# `br/pkg/task/operator/list_migration.rs`

## 文件定位

[`list_migration.rs`](./list_migration.rs) 是 `astersql-br-pkg-task-operator` library crate 中 `list-migrations` 运维子命令的执行模块。crate 根在 [`lib.rs`](./lib.rs) 中以 `pub mod list_migration` 挂载它，并通过 `pub use list_migration::*` 将其公开入口平铺导出；[`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 的 `newListMigrationsCommand` 随后把 `RunListMigrations` 注册为 CLI 回调。

该文件对照 [`list_migration.go`](./list_migration.go) 移植，位于“解析命令参数之后、读取 migration 元数据并输出之前”。它本身不解析 flags，也不创建或修改 migration；配置解析由 [`config.rs`](./config.rs) 的 `ListMigrationConfig::ParseFromFlags` 完成，migration 的读取和表格行构造则委托给 [`stubs.rs`](./stubs.rs) 中的本地兼容实现。

## 核心职责

文件只承担两项职责：

1. `statusOK` 组装控制台成功提示的绿色圆点和粗体消息。
2. `RunListMigrations` 根据 `ListMigrationConfig` 打开外部存储、加载完整 migration 栈，并在 JSON 与人类可读表格两种输出格式之间分流。

当前实现是只读查询路径：传给 `CreateStorage` 的 `send_creds` 为 `false`，之后只调用 `MigrationExt::Load` 与 `AddMigrationToTable`，没有写文件、合并 migration 或改变集群状态。不过“外部存储”目前由 operator crate 的 `stubs.rs` 提供兼容层，并非直接复用 [`br/pkg/stream/stream_metas.rs`](../../stream/stream_metas.rs) 的 canonical Rust 实现；扩展功能时必须先确认应继续维护兼容层，还是改接真实 stream/objstore crate。

## 主要符号

- `pub fn statusOK(message: &str) -> String`：公开的展示辅助函数。它先用 `color_green("●")` 包装圆点，再用 `color_bold` 包装带前导空格的消息，最终直接拼接两段。该函数不返回 `Result`，也不执行 I/O。
- `pub fn RunListMigrations(cfg: ListMigrationConfig) -> Result<()>`：文件的业务入口。输入包含 `BackendOptions`、`StorageURI`、`JSONOutput` 三项；任何解析、存储、加载或 JSON 序列化错误均通过 `?` 立即返回，成功时返回 `Ok(())`。

文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项。两个函数都因 `lib.rs` 的 glob re-export 成为 crate 公开 API；命名保留 Go 风格，crate 根通过 lint allow 接受 `non_snake_case`。

## 执行流程

`RunListMigrations` 的顺序流程如下：

1. 调用 `ParseBackend(&cfg.StorageURI, &cfg.BackendOptions)`。当前兼容层拒绝空 URI；带 `://` 的 URI 被拆为 scheme/path，不带 scheme 的值按 `local` 处理。
2. 调用 `CreateStorage(&backend, false)` 打开存储。当前 `stubs.rs` 最终构造按 URI 标识的 `MemStorage`；`false` 表明不向存储端发送凭据。
3. 用 `MigrationExtension(st)` 包装存储，再调用 `ext.Load(MLNotFoundIsErr())`。`MLNotFoundIsErr()` 固定返回 `true`，因此缺少 `migrations.json` 不是“空列表”，而是 `migrations not found` 错误。
4. 若 `cfg.JSONOutput` 为真，用 `serde_json::to_string(&migs)` 序列化整个 `Migrations`，并以 `println!` 输出一行。序列化失败时把 serde 错误文本包装为 `crate::stubs::Error`。
5. 否则创建 `ConsoleOperations::StdIO()`，先输出 `Total {Layers.len() + 1} Migrations.`；`+1` 表示独立计算 BASE。
6. 打印 `>   BASE   <`，为 `migs.Base` 新建表格、填行并打印。
7. 按 `migs.Layers` 的现有向量顺序逐层遍历；每层标题以八位零填充的 `SeqNum` 输出，再为 `Content` 创建和打印独立表格。

这里没有额外排序、过滤或去重。层次顺序完全取决于 `MigrationExt::Load` 返回的 `Layers` 顺序；JSON 分支也原样序列化同一对象。

## 数据与状态

入口按值接收 `ListMigrationConfig`，只读取三个字段，不修改配置。加载结果的最小本地模型定义在 `stubs.rs`：`Migrations { Base, Layers }`，每个 `MigrationLayer` 含 `SeqNum: i32` 与 `Content: Migration`，当前 `Migration` 仅含 `Name: String`。这些类型派生 `Serialize`/`Deserialize`，且没有 serde rename 属性，因此当前 Rust JSON 字段名是 `Base`、`Layers`、`SeqNum`、`Content`、`Name`。

非 JSON 分支的 `ConsoleOperations` 内部有 `Arc<Mutex<Vec<String>>>`，`Println`/`Printf` 会先保存输出片段再写标准输出；每个 `ConsoleTable` 独立拥有 `Vec<Vec<String>>`，`AddMigrationToTable` 当前只追加 migration 名称这一列。JSON 分支绕过该缓冲，直接写标准输出。

`RunListMigrations` 不保存全局状态。存储句柄由 `Arc<dyn ExternalStorage>` 持有并转移进 `MigrationExt`，函数结束后按 Rust 所有权规则释放本地引用；接口没有显式 `Close` 调用。

## 依赖与调用关系

上游调用链为：

`br/cmd/br/operator.rs::newListMigrationsCommand` → `ListMigrationConfig::ParseFromFlags` → `RunListMigrations`。

命令构造器把该路径注册为无位置参数的 `list-migrations` 子命令，并定义 `--storage/-s` 与 `--json` 等 flags。`br/pkg/task/operator/lib.rs` 是二者之间的 crate 导出门面。

`RunListMigrations` 的直接下游均来自本 crate：

- [`config.rs`](./config.rs)：`ListMigrationConfig`。
- [`stubs.rs`](./stubs.rs)：`ParseBackend`、`CreateStorage`、`MigrationExtension`、`MigrationExt::Load`、`MLNotFoundIsErr`、`ConsoleOperations`、`MigrationExt::AddMigrationToTable`、颜色辅助与统一 `Result`/`Error`。
- `serde_json`：JSON 输出序列化；它由 [`Cargo.toml`](./Cargo.toml) 直接声明。

`Cargo.toml` 将此目录定义为 `kind = "library"`、Go 包映射为 `br/pkg/task/operator`，且注释明确当前 arm64 Darwin 路径只使用本地 traits/stubs。虽然 crate 声明了 `astersql-objstore`，本文件当前没有直接导入它；同样，本文件也没有直接依赖 `br/pkg/stream` crate。

## 错误处理与边界

- 空 `StorageURI` 在 `ParseBackend` 阶段失败；后续步骤和输出不会执行。
- 存储创建错误由 `CreateStorage` 原样传播。
- 缺少 `migrations.json` 必须失败，因为调用显式传入 `MLNotFoundIsErr() == true`。[`parity_test.rs`](./parity_test.rs) 的 `contract_resource_cleanup` 用空的 `mem://list` 存储断言该入口返回错误。
- 文件存在但无法读取或 JSON 非法时，`MigrationExt::Load` 返回读取/反序列化错误，函数不会输出部分 migration 表格。
- JSON 序列化错误被转换为只保留文本的本地 `Error`；标准输出的 `println!` 写入失败无法通过当前签名返回。
- `Layers` 为空仍会报告一层，因为 BASE 始终计入总数；代码不验证 BASE 内容是否为空。
- 负数或超过八位的 `SeqNum` 没有被拒绝；`{:08}` 只控制最小显示宽度，不构成业务校验。
- 当前函数不接受 cancellation context。Rust CLI 虽取得 `_ctx = GetDefaultContext()`，但没有把它传入该入口；这与 Go 版所有存储/加载/表格调用均携带 `context.Context` 不同。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、锁、事务或显式取消句柄，整个读取与输出流程同步串行执行。唯一可见的锁来自 `ConsoleOperations.out` 和 `MigrationExt` 兼容结构内部的 `Mutex`，本函数没有跨线程共享这些对象，也没有在持锁时调用外部回调。

存储的所有权路径是 `Arc<dyn ExternalStorage>` → `MigrationExt`；表格则在每个 BASE/layer 分支中创建、打印并立即离开作用域。没有显式 flush/close 协议，因而真实后端若需要确定性关闭，不能假设本入口已完成该动作。输出也不是原子的：表格分支可能在若干行已写出后发生进程级 I/O 问题，当前 API 无法回滚。

## 与 Go 版本的对应关系

Rust 的主控制流与 [`list_migration.go`](./list_migration.go) 对齐：均执行 ParseBackend → Create → MigrationExtension.Load；均将 migration 缺失视为错误；均提供整包 JSON 与 BASE/Layer 表格两种输出；总数均为 `Layers + 1`，层标题均为八位零填充序号。

已确认的差异包括：

- Go `RunListMigrations(ctx, cfg)` 全链路传递 `context.Context`；Rust 签名只有 `cfg`，CLI 获取的默认 context 被忽略。
- Go 直接使用 `pkg/objstore`、`br/pkg/stream` 和 `br/pkg/glue` 的实现；Rust 当前使用 operator crate 的 `stubs.rs`，实际读取固定的 `migrations.json`，表格目前只含 `Name`。
- Go `json.Encoder.Encode` 依据 Go struct tags 输出并追加换行；Rust 使用默认 serde 字段名构造字符串后 `println!`。未发现针对此入口 JSON schema 的独立对齐测试，因此不能宣称字段布局与 Go 完全一致。
- Go 存储创建与加载可因 context 取消返回；Rust 当前没有对应取消边界。
- 两端都没有在该函数内排序 layers，也都直接传播关键错误。

因此，该文件可视为已接线的 CLI 兼容实现，但不是 Go 生产依赖链的完整等价替代。

## 扩展指南

- 新增过滤、排序或选择层级时，应优先修改 `RunListMigrations` 的加载后分流点，同时明确 JSON 与表格分支是否必须观察同一结果；不要只改一种输出造成语义分叉。
- 扩充输出字段时，要同步检查 `stubs.rs` 的 `Migration`/`MigrationLayer`/`Migrations`、`AddMigrationToTable` 以及 Go `br/pkg/stream` 的真实模型和表格实现。JSON 字段兼容属于外部 CLI 契约，应新增独立 Rust 测试固定 schema。
- 接入真实后端时，最可能替换的是本文件导入的 `ParseBackend`、`CreateStorage`、`MigrationExtension` 及相关类型；同时需要决定 context/cancellation、凭据发送、关闭与 I/O 错误如何进入 Rust API。
- 回归测试应放在独立文件，沿用 [`parity_test.rs`](./parity_test.rs) 或新增同目录 `list_migration_test.rs` 并由 `lib.rs` 在 `#[cfg(test)]` 下挂载；不要把测试内嵌进生产源文件。
- 至少补齐：有效 migration 的 JSON 输出、BASE 加多层表格顺序与八位序号、非法 JSON、缺失文件、空 URI、Go/Rust JSON 字段名一致性。测试输出时应注入或捕获 writer，避免依赖并行测试共享的进程 stdout。
- 性能上当前会把完整 `migrations.json` 和完整 `Migrations` 一次性载入内存，JSON 分支还会额外分配完整字符串；大 migration 栈若需要流式输出，必须同时评估顺序稳定性与部分输出失败语义。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件；`files --filter br/pkg/task/operator/list_migration.rs` 确认目标文件已索引。
- RustCodeGraph `explore "br/pkg/task/operator/list_migration.rs ..."`：读取目标文件完整源码，并识别 `RunListMigrations`、`statusOK`、`newListMigrationsCommand` 与 `parity_test.rs` 的调用/测试关系。
- 源码：[`list_migration.rs`](./list_migration.rs)、[`lib.rs`](./lib.rs)、[`config.rs`](./config.rs)、[`stubs.rs`](./stubs.rs)、[`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs)。
- crate 声明：[`Cargo.toml`](./Cargo.toml)。
- Go 对照：[`list_migration.go`](./list_migration.go)、[`config.go`](./config.go)、[`br/cmd/br/operator.go`](../../../cmd/br/operator.go)。
- 相关独立 Rust 测试：[`parity_test.rs`](./parity_test.rs) 的 `contract_normal_config_and_helpers` 验证 `statusOK`，`contract_resource_cleanup` 验证空存储缺少 migration 时 `RunListMigrations` 返回错误。仓库搜索未发现直接覆盖该入口的同目录 Go 测试，也未发现 Rust 的成功输出路径测试。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构以任务指定的 11 个固定二级标题检查。
