# `pkg/dxf/importinto/taskkey/task_key.rs`

源文件：[task_key.rs](task_key.rs)；Go 对照：[task_key.go](task_key.go)。

## 文件定位

本文件属于 `astersql-dxf-importinto-taskkey` 子 crate，是 IMPORT INTO 在 DXF（分布式执行框架）中构造任务键的实现。`lib.rs` 通过 `#[path = "task_key.rs"] pub mod taskkey` 暴露本模块；`Cargo.toml` 将 crate 入口设为 `lib.rs`，只直接依赖 `astersql-config-kerneltype`，并把本 crate 的 `nextgen` feature 转发给该依赖。

任务键最终用于把一个 IMPORT INTO job ID 映射到 DXF 任务表中的 `task_key` 字符串。Classic 构建使用 `ImportInto/{jobID}`；NextGen 构建使用 `{keyspace}/ImportInto/{jobID}`，从而在共享的系统侧任务表中保留用户 keyspace 维度。这里仅负责字符串构造，不负责创建任务、访问存储或检查任务是否存在。

## 核心职责

- `ForJob` 为当前运行上下文中的 job 生成键：NextGen 从全局 settings 读取 keyspace，Classic 保持旧格式。
- `ForJobInKeyspace` 为调用者明确给出的 keyspace 生成键，主要适合需要跨 keyspace 定位任务的路径；Classic 下有意忽略该参数。
- `forJobInKeyspace` 集中维护 NextGen 三段式格式，避免两个公开入口分别拼接而发生漂移。
- 保持与 Go `pkg/dxf/importinto/taskkey/task_key.go` 相同的分支与格式化语义，不额外校验 keyspace 或 job ID。

本文件没有定义常量、结构体、枚举、trait、`impl` 或条件编译项；条件差异来自依赖的 `kerneltype::IsNextGen()`。文件级 `#![allow(dead_code, non_snake_case)]` 用于保留 Go 风格公开 API 名称。

## 主要符号

| 符号 | 可见性与签名 | 语义 |
| --- | --- | --- |
| `ForJob` | `pub fn ForJob(jobID: i64) -> String` | 按当前内核类型生成任务键。NextGen 调用 `keyspace::GetKeyspaceNameBySettings()` 后进入私有辅助函数；Classic 直接拼接 `proto::ImportInto` 与十进制 job ID。 |
| `ForJobInKeyspace` | `pub fn ForJobInKeyspace(keyspaceName: String, jobID: i64) -> String` | NextGen 使用显式 keyspace；Classic 调用 `ForJob(jobID)`，因此显式参数不影响结果。参数按值接收，NextGen 可直接把 `String` 移交给辅助函数。 |
| `forJobInKeyspace` | `fn forJobInKeyspace(keyspaceName: String, jobID: i64) -> String` | 私有三段式格式化函数，固定生成 `{keyspace}/{ImportInto}/{jobID}`。 |

`proto::ImportInto` 在本子 crate 的 `lib.rs` 中定义为字符串常量 `"ImportInto"`。`kerneltype::IsNextGen()` 由 `pkg/config/kerneltype/classic.rs` 与 `nextgen.rs` 分别在 feature 条件下提供：默认构建恒为 `false`，启用 `nextgen` 后恒为 `true`，所以这里的模式选择不是运行时可变配置。

## 执行流程

`ForJob(jobID)` 的流程如下：

1. 调用 `kerneltype::IsNextGen()` 判定编译出的内核模式。
2. NextGen 分支调用 `keyspace::GetKeyspaceNameBySettings()` 取得当前全局 keyspace 名称。
3. NextGen 分支把名称和 job ID 交给 `forJobInKeyspace`，返回三段式键。
4. Classic 分支直接通过 `format!` 返回两段式键。

`ForJobInKeyspace(keyspaceName, jobID)` 的流程如下：

1. 同样检查 `kerneltype::IsNextGen()`。
2. NextGen 分支把调用者提供的 `String` 移交给 `forJobInKeyspace`。
3. Classic 分支转调 `ForJob(jobID)`；由于 Classic 的 `ForJob` 不读取 keyspace，传入的 `keyspaceName` 被有意忽略。

`forJobInKeyspace` 不包含分支，只按 keyspace、任务类型和 job ID 的顺序插入 `/`。例如 NextGen 的 `("ks1", 9527)` 得到 `ks1/ImportInto/9527`，Classic 的相同 job ID 得到 `ImportInto/9527`。

## 数据与状态

输入数据只有 `i64` job ID，以及显式入口中的拥有所有权的 `String` keyspace。输出总是新分配的 `String`；函数不缓存结果，也不修改调用者数据。

任务键的格式不变量是：任务类型段固定取 `proto::ImportInto`，job ID 使用 Rust `Display` 的十进制形式。负值会保留负号，`i64::MIN` 可完整渲染。keyspace 被当作不透明字符串原样拼接：空字符串、已有 `/` 或其他特殊内容不会在本模块中规范化或拒绝，因此调用者必须保证名称符合上层 keyspace 约束。

本文件自身没有静态可变状态。只有 `ForJob` 的 NextGen 路径间接读取 `lib.rs` 中由 `OnceLock<RwLock<String>>` 保存的全局 keyspace 名称；`ForJobInKeyspace` 的 NextGen 路径不读取该状态。Classic 路径均不依赖 keyspace 状态。

## 依赖与调用关系

下游依赖均通过 `use crate::{kerneltype, keyspace, proto}` 进入：

- `kerneltype::IsNextGen()` 决定选择两段式还是三段式格式。
- `keyspace::GetKeyspaceNameBySettings()` 只被 `ForJob` 的 NextGen 分支调用，在当前子 crate 中转发到 `config::get_global_keyspace_name()`。
- `proto::ImportInto` 提供稳定的任务类型段。
- Rust 标准库的 `format!` 完成字符串分配与整数格式化。

RustCodeGraph 将本文件标记为被 5 个文件使用。生产调用链中，`pkg/dxf/importinto/job.rs::TaskKey` 转调 `ForJob`，供 IMPORT INTO job 的通用任务定位使用；`pkg/dxf/importinto/jobhistory/history.rs::GetFromHistory` 调用 `ForJobInKeyspace`，然后以生成的键和 `proto::ImportInto` 查询 `mysql.tidb_global_task_history`。其余直接使用来自 `pkg/dxf/importinto/job_testkit_test.rs`、`pkg/dxf/importinto/jobhistory/migration_aster_unit_test.rs` 和本 crate 的 `migration_aster_unit_test.rs`。

crate 边界由 `pkg/dxf/importinto/taskkey/Cargo.toml` 确认；父 crate `astersql-dxf-importinto` 以路径依赖 `taskkey` 子 crate。`pkg/dxf/importinto/jobhistory/lib.rs` 还将 `ForJobInKeyspace` 再导出给历史查询实现和测试使用。

## 错误处理与边界

三个函数均返回裸 `String`，没有 `Result` 或显式错误分支。对任意 `i64` 都能格式化；模块也不会因为 job ID 为负数而拒绝生成键。`format!` 的内存分配失败遵循 Rust 进程级分配失败行为，而不是本模块可恢复错误。

keyspace 不经过转义或分段校验。测试明确证明含 `/` 的 `"tenant/child"` 会原样产生额外路径段；这说明 `/` 在此处只是格式分隔字符，而本模块不承诺可逆解析。空 keyspace 在 NextGen 下会生成以 `/ImportInto/` 开头的字符串，源代码没有禁止这一情况。

`ForJob` 的 NextGen 路径依赖 `lib.rs` 中全局 `RwLock` 的读取封装；该封装使用 `unwrap()`，若锁被 poison 会 panic。此风险属于当前 keyspace 适配层，不是本文件内的显式错误协议。任务键一旦持久化后必须用同一内核模式、同一 keyspace 语义重建，否则查询不到对应任务；`GetFromHistory` 的“未找到”错误是在消费者中产生的。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、文件句柄或网络资源。`ForJobInKeyspace` 和 `forJobInKeyspace` 是只依赖参数与编译模式的纯字符串构造；返回值所有权交给调用者，并在调用者释放时回收。

`ForJob` 的 NextGen 分支会短暂读取全局 keyspace 配置：适配层取得 `RwLock` 读锁、克隆字符串后即释放锁，本文件随后消费该克隆。因而这里不会让锁跨越格式化或存储调用，也不存在本模块管理的长生命周期资源。若其他线程同时更新测试/本地配置，单次调用只会看到某次读锁保护下的完整字符串，但连续两次调用可能得到不同名称；生产代码应避免在同一任务生命周期内改变生成键所依据的 keyspace。

## 与 Go 版本的对应关系

Rust 的三个函数逐一对应 Go `task_key.go` 中的 `ForJob`、`ForJobInKeyspace` 和 `forJobInKeyspace`，分支顺序与格式均一致：

- Go `fmt.Sprintf("%s/%d", proto.ImportInto, jobID)` 对应 Classic 分支的 `format!("{}/{}", proto::ImportInto, jobID)`。
- Go `fmt.Sprintf("%s/%s/%d", keyspaceName, proto.ImportInto, jobID)` 对应私有辅助函数的三段 `format!`。
- 两种实现都只在 NextGen 中使用 keyspace；Classic 的显式 keyspace 参数被忽略。
- Go 的 `int64` 与 Rust 的 `i64` 范围相同，负数和最小值的十进制文本保持一致。

Rust 与 Go 的表面差异是 Rust 显式取得或接收拥有所有权的 `String`，并通过 Cargo feature 选择 `kerneltype` 实现；Go 通过对应构建标签选择内核实现。Rust 文件中的 `allow(non_snake_case)` 是为了保留 Go API 拼写，不代表引入另一套命名或行为。当前代码不是桩：它已由 `job.rs` 和 `jobhistory/history.rs` 的真实逻辑调用。

## 扩展指南

若新增任务键格式或命名空间规则，优先只修改 `ForJob`、`ForJobInKeyspace` 与 `forJobInKeyspace` 中最小必要的分支，并同步 Go 对照实现；不要在消费者中复制格式字符串。任何格式变化都属于持久化兼容性变更，因为已有 `mysql.tidb_global_task` / `_history` 行按旧键查询，必须明确是否需要兼容读取旧格式、迁移数据或版本化键格式。

若增加 keyspace 校验或转义，需要先确认上游 keyspace 命名契约以及是否允许历史上含 `/` 的键；当前独立测试把原样保留视为对齐行为，直接规范化会改变兼容性。若只新增测试，应放在独立的 `migration_aster_unit_test.rs` 或相邻消费者的独立 `*_test.rs`，不要把测试嵌入 `task_key.rs`。至少同步覆盖 Classic 与 NextGen、`ForJob` 的配置读取、显式 keyspace、负 job ID、`i64::MIN`、空值和分隔符边界。

性能上每次调用至少分配一个输出 `String`，`ForJob` 的 NextGen 路径还会克隆全局 keyspace。除非调用频率或剖析数据证明必要，不应通过全局缓存引入失效与并发一致性问题。若更改函数签名，应同时检查 `job.rs::TaskKey`、`jobhistory/history.rs::GetFromHistory`、`jobhistory/lib.rs` 的再导出和所有独立测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 `task_key.rs`、`lib.rs`、迁移测试及 Go 对照均已索引。
- RustCodeGraph `node --file pkg/dxf/importinto/taskkey/task_key.rs`：确认文件共 59 行、3 个函数及 5 个使用文件。
- RustCodeGraph `query` / `explore`：确认 `ForJob` 的调用者包括 `job.rs::TaskKey` 和迁移测试，`ForJobInKeyspace` 的调用者包括 `jobhistory/history.rs::GetFromHistory`、历史相关测试与 job testkit；图的精确 `callers/callees` 子命令未对带路径符号输出明细，因此又用索引源码节点和 `rg` 复核调用点。
- 已读生产与装配文件：`pkg/dxf/importinto/taskkey/task_key.rs`、`lib.rs`、`Cargo.toml`、父级 `pkg/dxf/importinto/Cargo.toml`、`pkg/dxf/importinto/job.rs`、`pkg/dxf/importinto/jobhistory/history.rs`、`pkg/dxf/importinto/jobhistory/lib.rs`、`pkg/config/kerneltype/classic.rs`、`nextgen.rs` 及其 `Cargo.toml`。
- 已读 Go 对照与测试：`pkg/dxf/importinto/taskkey/task_key.go`、`migration_aster_unit_test.rs`、`pkg/dxf/importinto/jobhistory/history_test.rs`、`jobhistory/migration_aster_unit_test.rs`、`pkg/dxf/importinto/job_testkit_test.rs`，并用 `rg` 搜索 Rust/Go 的三个函数名以复核直接引用。
- 独立迁移测试覆盖两种内核格式、配置 keyspace 与显式 keyspace、Classic 忽略 keyspace、负 job ID、含 `/` 的 keyspace 和 `i64::MIN`；本任务按计划不运行 Cargo，验证限于索引、源码事实和文档结构。
