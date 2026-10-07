# `pkg/planner/core/access_object.rs`

## 文件定位

`pkg/planner/core/access_object.rs` 是 `astersql-planner-core` crate 根模块中的私有占位模块。`pkg/planner/core/lib.rs` 通过 `mod access_object;` 将它编译进 crate，但没有使用 `pub mod` 或 `pub use` 向 crate 外暴露它。目标文件只有许可证和边界说明注释，没有 Rust 项，因此它不是 EXPLAIN 访问对象的实现位置。

相邻职责被刻意拆分：`pkg/planner/core/base/misc_base.rs::AccessObject` 定义接口语义，`pkg/planner/core/access/access_obj.rs` 定义扫描、其他文本和动态分区等对象。后者属于独立的 `astersql-planner-core-access` crate，其工作区声明见根 `Cargo.toml` 中的 `facade_planner_core_access`。

## 核心职责

本文件的唯一现有职责是保留 `core::access_object` 的空生产边界：

- 与同路径 Go 文件 `pkg/planner/core/access_object.go` 保持“只有包边界，没有实现”的语义对应。
- 不在 `astersql-planner-core` 根 crate 里重复声明 `AccessObject`、`ScanAccessObject` 或 protobuf 转换逻辑。
- 把真实实现留在 `base` 和 `access` 边界，避免形成第二套类型或新的 crate 依赖。

因此，该文件“如何运行”的精确答案是：它被模块系统解析，但不产生可调用逻辑、类型或运行时副作用。

## 主要符号

本文件没有模块级常量、`static`、类型、trait、函数、`impl`、宏调用或条件编译项，也没有公开 API。RustCodeGraph 将文件本身识别为唯一节点，但对该文件执行 `callers` 和 `callees` 均找不到可调用定义，与空模块事实一致。

不应归属于本文件的关键相邻符号包括：

- `pkg/planner/core/base/misc_base.rs::AccessObject`：声明 `string`、`normalized_string` 和 `set_into_pb` 三个接口方法。
- `pkg/planner/core/access/access_obj.rs::{ScanAccessObject, IndexAccess, OtherAccessObject, DynamicPartitionAccessObject, DynamicPartitionAccessObjects}`：承载真实数据和格式化/protobuf 转换行为。

## 执行流程

1. Rust 编译器处理 `pkg/planner/core/lib.rs:137` 的 `mod access_object;`。
2. 模块系统加载本文件。
3. 文件中没有任何项需要展开、类型检查或生成可执行代码；模块也没有可供上游调用的入口。
4. 当物理算子需要产生 access object 时，它们使用各自实现或 `base`/`access` 边界，不经过本模块。例如 `PhysicalTableScan::AccessObject` 和 `PhysicalIndexScan::AccessObject` 在 `pkg/planner/core/operator/physicalop/` 中生成 EXPLAIN 文本；这些是边界证据，不是本文件的调用链。

## 数据与状态

本文件不定义字段、容器、全局变量、缓存、配置或序列化格式，也不持有任何状态。

为避免边界混淆：数据库名、表名、索引列、分区集合及动态裁剪错误存储在 `pkg/planner/core/access/access_obj.rs` 的相关 struct 中；本文件不拥有、复制或转换这些数据。

## 依赖与调用关系

- **直接上游**：`pkg/planner/core/lib.rs:137` 的私有模块声明。这是文本包含关系，不是函数调用。
- **直接下游**：无。本文件没有 `use`、路径引用、函数调用或类型实例化。
- **crate 边界**：`pkg/planner/core/Cargo.toml` 把根 crate 命名为 `astersql-planner-core`，`[lib]` 入口是 `lib.rs`；目标文件本身不使用该 manifest 中的任何依赖或 feature。
- **实现边界**：`pkg/planner/core/access/Cargo.toml` 定义独立 `astersql-planner-core-access` crate，直接依赖 `protobuf` 和 `tipb`；根工作区通过 `facade_planner_core_access` 声明它。不应为让本占位模块“有内容”而在根 crate 中复制这些依赖。

RustCodeGraph 证据表明 `access_object.rs` “used by 0 files”，且没有可供 `callers`/`callees` 解析的定义；同时它将真实实现文件 `access/access_obj.rs` 识别为被多个模块和测试使用。

## 错误处理与边界

本文件无函数、无返回值、无 `Result`/`Option`、无 panic 路径，因而没有运行时错误处理。其最重要的边界不变量是“保持为空”：不把 `access` 子 crate 的数据类型或 protobuf 逻辑重新实现在这里。

真实实现的边界条件，如空表名、空分区集、空 `OtherAccessObject`、无 protobuf 目标、动态分区错误和旧值覆盖，由 `pkg/planner/core/access/access_obj.rs` 及 `migration_aster_unit_test.rs` 处理和验证；它们不是本文件的自身分支。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、文件句柄、网络连接或其他需要释放的资源。它的生命周期仅是编译期模块加载；运行时没有实例，也没有 `Send`/`Sync` 或锁顺序要求。

## 与 Go 版本的对应关系

`pkg/planner/core/access_object.go` 除 PingCAP Apache License 头外只有 `package core`，没有类型或函数。Rust 文件保留同样的空边界，额外的只是说明性注释；因此两者在可观测行为上一致：都不提供 Access Object 实现。

Go 的真实实现在 `pkg/planner/core/access/access_obj.go`，Rust 对照实现在 `pkg/planner/core/access/access_obj.rs`。二者的扫描对象文本、归一化文本、索引 protobuf、其他对象和动态分区行为由独立 Rust 测试 `pkg/planner/core/access/migration_aster_unit_test.rs` 覆盖。这个对照证明实现已移到何处，不改变目标文件本身为空的结论。

## 扩展指南

- 若新增访问对象数据类型、字符串格式或 tipb 序列化，修改 `pkg/planner/core/access/access_obj.rs`，并在独立测试 `pkg/planner/core/access/migration_aster_unit_test.rs` 中增加对照用例；同时核对 Go 文件 `access/access_obj.go`。
- 若扩展所有访问对象共有的计划接口，修改 `pkg/planner/core/base/misc_base.rs::AccessObject` 及其独立测试，再更新所有实现者。
- 若某个物理算子需要新的 EXPLAIN access object，应在该算子的 `AccessObject`/`access_object` 方法和同目录独立 `*_test.rs` 中实现与验证。
- 只有当 Go 同路径文件或 crate 分层发生明确变化时，才应考虑给本文件增加符号。这会改变当前空边界契约，需先检查 crate 依赖方向、重复类型风险和循环依赖，并把 Rust 测试放在独立文件中，不要内嵌到生产源文件。

兼容风险主要来自 EXPLAIN 文本和 tipb 格式的变化；性能风险主要来自在高频计划展示路径中增加不必要的分配。但就本空文件而言，当前两类运行时风险均不存在。

## 验证依据

- 目标源文件：`pkg/planner/core/access_object.rs` 全部 26 行，仅含许可证与空边界注释。
- 模块入口：`pkg/planner/core/lib.rs:137` 以 `mod access_object;` 私有声明本模块。目标包根目录下没有 `doc.go`；最近的 `pkg/planner/core/base/doc.go` 属于不同的 `base` 子包，不是本包契约。
- crate 声明：`pkg/planner/core/Cargo.toml` 的 package 名为 `astersql-planner-core`，入口为 `lib.rs`；`pkg/planner/core/access/Cargo.toml` 另行定义 `astersql-planner-core-access`。
- Go 对照：`pkg/planner/core/access_object.go` 只含 `package core`；真实 Go/Rust 实现分别在 `pkg/planner/core/access/access_obj.go` 和 `access_obj.rs`。
- 测试搜索：未找到直接引用 `access_object.rs` 或 `access_object.go` 的独立测试，这与其无行为一致。真实实现的对照测试为 `pkg/planner/core/access/migration_aster_unit_test.rs`；算子层格式测试包括 `operator/physicalop/physical_table_scan_test.rs`、`physical_index_scan_test.rs` 和 `physical_mem_table_test.rs`。
- RustCodeGraph：`status` 报告 11,467 个已索引文件；`files --filter pkg/planner/core/access_object.rs` 找到目标；`node --file ...` 显示完整 26 行且 `used by 0 files`；对文件执行 `callers`/`callees` 无定义可解析；`query AccessObject` 将 trait 定位到 `base/misc_base.rs`，`node --file access/access_obj.rs` 将实现定位到独立 access crate。
- 结构验收要求：本文档必须存在，且上述固定二级标题必须恰好出现 11 个。本任务是纯文档分析，不运行 Cargo。
