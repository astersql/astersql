# `pkg/executor/importer/production_storage.rs`

## 文件定位

本文件属于 `astersql-executor-importer` crate；crate 根由 `pkg/executor/importer/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/importer/lib.rs` 通过 `mod production_storage` 挂载并以 `pub use production_storage::*` 导出这里的两个工厂。它是 IMPORT INTO 存储抽象与生产运行时之间的适配层：`ImportStorageFactory`、`SharedStorage` 定义在 `pkg/executor/importer/import.rs`，本文件只决定一个 URI 应由服务器本地文件后端处理，还是交回宿主提供的云存储工厂。

实际生产接线位于 `pkg/dxf/importinto/scheduler.rs` 的 `ImportSchedulerServices::FromEncodeRuntime`：它保留运行时原有的 `services.StorageFactory` 作为 `CloudFactory`，再以 `HostImportStorageFactory` 包装。因此这里不是通用 URI 解析器，也不负责发现文件、解析 CSV/Parquet 或执行导入；这些职责分别留在 `import.rs` 的 `parse_data_source_path`、`LoadDataController::InitDataStore`/`CheckDataSourceAccess` 及下游解析器中。

## 核心职责

1. `HostImportStorageFactory::Open` 做最窄的路由判断：只有 URI 被当前平台的 `std::path::Path` 判定为绝对路径，且 `target` 精确等于 `"IMPORT INTO data source"` 时，才走服务器磁盘；其他请求原样委托 `CloudFactory.Open`。这保证全局排序的 `"cloud storage"` 请求不会被误当成本地数据源。
2. `ServerDiskImportStorageFactory::Open` 在创建本地存储前检查取消状态、调用契约和父目录存在性，然后以请求文件的父目录为本地存储根。这与 `import.rs::storage_path` 对绝对路径只保留文件名相配合，使后续 `Storage::Open` 使用相对 key 读取目标文件。
3. 将具体的 `LocalStorage` 擦除为导入器统一使用的 `SharedStorage = Arc<Mutex<Box<dyn Storage + Send>>>`，供控制器、文件发现与解析流程共享。

本文件不验证文件扩展名或 glob，不确认目标文件本身存在，也不负责云 URI 的合法性；它只建立本地根目录并把其他后端交给宿主工厂。

## 主要符号

- `pub struct ServerDiskImportStorageFactory`：无字段的本地后端工厂。它实现 `ImportStorageFactory::Open(&Context, &str, &str) -> Result<SharedStorage, String>`，只接受服务器绝对路径形式的 IMPORT INTO 数据源。
- `pub struct HostImportStorageFactory { pub CloudFactory: Arc<dyn ImportStorageFactory> }`：生产路由门面。公开字段允许宿主注入任何满足 `Send + Sync` 的云存储工厂；自身也因 trait 约束可在线程间共享。
- `impl ImportStorageFactory for HostImportStorageFactory`：路由入口。绝对路径与目标标签必须同时匹配才调用 `ServerDiskImportStorageFactory.Open`，否则调用 `self.CloudFactory.Open`。
- `impl ImportStorageFactory for ServerDiskImportStorageFactory`：本地构造入口。顺序执行 `Context::check`、参数约束检查、`Path::parent`、`Path::is_dir`、`NewLocalStorage(parent)`，最后装箱并包入 `Mutex`、`Arc`。

文件内没有模块级常量、枚举、独立函数或条件编译项；唯一行为由上述两个公开类型的 trait 实现承载。

## 执行流程

生产主链可从 `pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime` 开始追踪：调度器构造控制器服务时安装 `HostImportStorageFactory`；`pkg/executor/importer/import.rs::NewLoadDataController` 保存该 `StorageFactory`；文件数据源初始化时，`LoadDataController::InitDataStore` 调用 `initExternalStore(..., "IMPORT INTO data source", factory)`，后者再动态分派到 `ImportStorageFactory::Open`。

对于服务器本地绝对路径，流程如下：

1. `HostImportStorageFactory::Open` 检查 `Path::is_absolute()` 和精确的 target 标签，命中后转交 `ServerDiskImportStorageFactory`。
2. 本地工厂先调用 `context.check()`；已经取消的请求立即以字符串错误返回，测试 `server_disk_factory_opens_real_csv_from_parent_directory` 覆盖了此分支。
3. 再次校验绝对路径和 target。该重复校验使本地工厂即使被直接调用，也不会绕过适用范围。
4. 取 `path.parent()`，要求父路径存在且是目录；否则分别返回“无父目录”或“目录不存在”错误。
5. `NewLocalStorage(parent)` 创建以父目录为根的本地存储，随后返回共享 trait object。
6. `import.rs::parse_data_source_path` 为本地源保留原始绝对路径作为 `storage_uri`，并抽取 basename 为 `file_name_key`；后续也可由 `storage_path` 得到同一相对文件名。`production_storage_test.rs` 以真实 `data.csv` 验证存储根加 `data.csv` 能读到 4 字节。

对于 `s3://` 等云 URI、相对路径，或者 target 为 `"cloud storage"` 的请求，门面不解释 URI，直接调用 `CloudFactory.Open(context, uri, target)`。云后端支持范围、鉴权和错误语义因此由注入的宿主实现决定。

## 数据与状态

两个工厂自身都不维护可变业务状态。`ServerDiskImportStorageFactory` 是零大小类型；`HostImportStorageFactory` 唯一持有的状态是引用计数指针 `Arc<dyn ImportStorageFactory>`。路由过程不缓存 URI、不保存 `Context`，每次 `Open` 都独立判断。

返回值 `SharedStorage` 的真实形状由 `pkg/executor/importer/import.rs` 定义为 `Arc<Mutex<Box<dyn Storage + Send>>>`：`Arc` 允许控制器和解析流程共享所有权，`Mutex` 串行化对 `Storage` trait object 的可变/同步访问，`Box` 隐藏具体 `LocalStorage` 类型。本文件把父目录作为本地后端的 base；文件 basename 或 glob 是存储内的相对 key，而不是 base 的组成部分。

`target` 是行为选择所依赖的字符串协议，不是类型安全枚举。当前本地分支只承认精确文本 `"IMPORT INTO data source"`；大小写、空白或新 target 都不会匹配。

## 依赖与调用关系

上游关系：

- `pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime` 构造 `HostImportStorageFactory`，这是源码搜索确认的生产构造点。
- `pkg/executor/importer/import.rs::LoadDataController::InitDataStore` 和 `CheckDataSourceAccess` 经 `initExternalStore` 使用 `ImportStorageFactory`；`GetSortStore` 也经同一入口请求 `"cloud storage"`，因此会走 `CloudFactory`。
- `pkg/executor/importer/lib.rs` 将两个工厂公开再导出，调度器以 `importer::HostImportStorageFactory` 使用它。

下游关系：

- `std::path::Path` 提供平台相关的绝对路径、父目录与目录存在性判断。
- `astersql_objstore_storeapi::Context`（由 `pkg/objstore/storeapi/storage.rs` 再导出 object I/O context）提供取消检查。
- `astersql_objstore::local::NewLocalStorage` 创建本地文件系统后端；`pkg/objstore/local.rs` 显示该构造器通常可创建缺失 base，但本文件预先要求 parent 已是目录，因此不会借此隐式创建数据源目录。
- `CloudFactory` 是动态依赖，本文件无法也不应假设其具体后端。

Cargo 边界由 `pkg/executor/importer/Cargo.toml` 证实：本 crate 直接依赖路径 crate `astersql-objstore` 与 `astersql-objstore-storeapi`，没有为本文件设置 feature gate。

RustCodeGraph 的文件查询确认 `production_storage.rs` 被索引且含 10 个符号，`query` 唯一定位了两个 struct；由于 `Open` 是 trait 动态分派且同名方法很多，图查询没有产出可归属本实现的 caller/callee 边，因此上述调用边由精确符号搜索和相邻源码核实，不把缺失的图边写成静态调用事实。

## 错误处理与边界

错误统一降为 `String`，并按执行顺序短路：

- 已取消的 `Context`：`context.check()` 的错误经 `to_string()` 传播，且优先于 URI/目录检查。
- 直接调用本地工厂但 URI 非绝对路径，或 target 不匹配：返回 `unsupported {target} storage URI: {uri}`。
- 路径无法取得父目录：返回 `server-disk path has no parent: {uri}`。在常见 Unix 绝对路径上根路径的 `parent()` 仍可能是根自身，因此是否触发取决于平台 `Path` 语义；文档不假定所有绝对路径都会有可用文件名。
- 父路径不存在或不是目录：返回 `server-disk directory does not exist: ...`。错误文本同时覆盖“存在但非目录”的情况。
- `NewLocalStorage` 失败：保留底层错误的显示文本，但不增加额外上下文。
- 委托分支：不改写 `CloudFactory` 的成功值或错误。本层之上的 `import.rs::initExternalStore` 会统一加上 `cannot access {target}:` 上下文。

边界上，本实现只验证目录而非文件：缺失文件、权限不足、basename/glob 不匹配会在后续 `Open`/`WalkDir` 阶段暴露。符号链接解析、路径规范化和目录逃逸也未在本文件执行；调用者应继续让后续对象 key 来自 `parse_data_source_path`/`storage_path`，不要把任意相对路径拼入共享本地根。

## 并发与资源生命周期

`ImportStorageFactory: Send + Sync` 要求两个实现可在线程间使用；`HostImportStorageFactory` 通过 `Arc` 共享云工厂，没有内部锁或后台任务。每次本地 `Open` 新建一个 `LocalStorage`，再以 `Arc<Mutex<...>>` 返回。锁中毒由使用方处理，例如 `CheckDataSourceAccess` 将其映射为 `data storage lock is poisoned`；本文件创建时不会持锁。

取消生命周期只在创建入口显式检查一次。创建成功后，实际 `Storage::Open`、reader 和解析器继续接收同一个或调用方传入的 `Context`，其后续取消行为属于对象存储实现。`production_storage_test.rs` 既验证创建前取消会失败，也验证 Parquet parser 在取消后读取路径报告 `cancelled`。

资源释放依靠 Rust 所有权：当最后一个 `SharedStorage` 的 `Arc` 和从中取得的 reader/parser 被释放时，相应对象析构。测试显式先 `drop(reader)`、再 `drop(storage)`，随后删除临时目录，说明活动 reader 可能占用本地文件资源；生产扩展不应在 reader 仍存活时清理其 base。文件中没有显式线程、异步任务、通道或事务。

## 与 Go 版本的对应关系

Go 同目录没有独立的 `production_storage.go`；对应语义分布在 `pkg/executor/importer/import.go`：

- Go `LoadDataController.InitDataStore` 先由 `parseDataSourcePath` 把本地文件路径拆成“父目录 URL + basename”，然后 `initExternalStore` 通过 `objstore.ParseBackendFromURL` 与 `objstore.NewWithDefaultOpt` 打开存储。
- Rust `parse_data_source_path` 为兼容生产本地工厂而保留原始绝对文件路径；本文件随后取 parent 建立 `LocalStorage`。两条实现最终都得到“父目录为存储根、basename 为对象 key”的不变量。
- Go 的 `initExternalStore` 是直接的通用后端构造；Rust 为适应宿主运行时采用可注入 `ImportStorageFactory`，并用 `HostImportStorageFactory` 仅截获本地 IMPORT INTO 数据源，云存储继续交回宿主。这是 Rust 接线差异，不代表 Go 有同名类型。
- Go 用结构化 TiDB 错误包装无效 URI与不可访问存储；本文件返回字符串，并由 Rust `initExternalStore` 添加目标上下文，错误类型丰富度并不完全等价。

独立 Rust 测试 `pkg/executor/importer/production_storage_test.rs` 覆盖真实 CSV、本地 basename、云委托、创建前取消，以及 Parquet 已知/未知 size 和取消传播。Go 的直接对照依据是 `pkg/executor/importer/import.go` 的 `InitDataStore`、`parseDataSourcePath`、`GetSortStore`、`initExternalStore`；`pkg/executor/importer/import_test.go` 提供同包对象存储行为测试，但没有与这两个 Rust 工厂一一同名的 Go 测试。

## 扩展指南

- 新增本地适用场景时，优先修改 `HostImportStorageFactory::Open` 的路由条件，并同步审查 `ServerDiskImportStorageFactory::Open` 的防绕过校验；不要只改其中一层。字符串 target 若扩展，必须同时核对 `import.rs::initExternalStore`、`GetSortStore` 的调用标签。
- 改变本地路径拆分时，要一起维护 `import.rs::parse_data_source_path`、`storage_path` 和本文件的 parent 选择，保持“base + 相对 key 指向原文件”的不变量；尤其要补充根路径、相对路径、缺失目录、目录伪装成文件和平台路径差异用例。
- 新增云 scheme 或鉴权参数不应硬编码到本文件；应由 `CloudFactory` 的宿主实现处理，除非需求明确改变路由边界。
- 若改变共享模型或去掉 `Mutex`，先审计 `SharedStorage` 的所有使用者及锁中毒错误处理，评估并发安全与锁竞争；本文件自身不足以证明具体 `Storage` 实现可无锁共享。
- 行为测试应继续放在独立的 `pkg/executor/importer/production_storage_test.rs`，遵守生产源码与 Rust 测试分离要求。生产接线变更还应覆盖 `pkg/dxf/importinto/scheduler_test.rs` 中的服务构造路径，而不是把测试嵌入本文件。
- 兼容风险主要是 Go/Rust 路径拆分漂移和错误文本变化；性能风险主要来自每次打开都做文件系统检查，以及共享 `Mutex` 在高并发读取时的串行化。任何优化都应保留取消优先级和云委托语义。

## 验证依据

- 目标源码：`pkg/executor/importer/production_storage.rs`，核对两个 struct、两个 trait impl、所有分支与错误文本。
- crate 与模块边界：`pkg/executor/importer/Cargo.toml`、`pkg/executor/importer/lib.rs`。
- Rust 调用链与相关类型：`pkg/executor/importer/import.rs` 中的 `ImportStorageFactory`、`SharedStorage`、`LoadDataController::InitDataStore`、`CheckDataSourceAccess`、`GetSortStore`、`initExternalStore`、`parse_data_source_path`、`storage_path`。
- 生产构造点：`pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime`。
- 下游实现：`pkg/objstore/local.rs::NewLocalStorage`、`pkg/objstore/storeapi/storage.rs` 对 `Context` 和 `Storage` API 的定义/再导出。
- 独立 Rust 测试：`pkg/executor/importer/production_storage_test.rs`；补充接线证据来自 `pkg/dxf/importinto/scheduler_test.rs` 和 `pkg/executor/importer/precheck_test.rs` 的工厂使用点。
- Go 对照：`pkg/executor/importer/import.go` 中 `InitDataStore`、`parseDataSourcePath`、`GetSortStore`、`initExternalStore`，以及同目录 `import_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/executor/importer/production_storage.rs` 确认目标已索引且有 10 个符号；`explore` 返回目标完整源码；`query ServerDiskImportStorageFactory` 与 `query HostImportStorageFactory` 各得到唯一目标定义。trait `Open` 的精确调用边未由图解析，已用 `rg` 对生产构造点、trait 定义和调用点作直接源码核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令确认本文档存在且恰好含 11 个固定二级章节，并人工复核未把测试建议放入生产 Rust 文件、未把未解析的动态调用边表述为 RustCodeGraph 已确认事实。
