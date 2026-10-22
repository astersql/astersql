# `br/pkg/task/restore_name_route.rs`

## 文件定位

扩展 `RestoreConfig`，为 snapshot/PiTR 提供名称路由配置及名称计划构建 API。

## 核心职责

每次从当前公开 `Rename` 字段重新解析路由；限制支持的恢复配置与 schema，生成十六进制指纹，并把选中备份对象解析为不可变源到稳定目标的计划。

## 主要符号

`get_name_router`、`has_name_routing`、`is_partial_restore`、`name_route_fingerprint`、`validate_name_routing` 是配置方法。`RestoreNamePlanEntry` 保存 source/target，`RestoreNamePlan` 持有 entries；`build_restore_name_plan` 是纯函数。私有 schema 检查拒绝系统库、临时系统库及检查点库。

## 执行流程

`get_name_router` 解析后检查每条规则两端 schema。`validate_name_routing` 即使无规则也先解析；有规则时拒绝 NoSchema、WithSysTable、raw、txn 和 EBS。计划构建先 `validate_targets`，再逐对象路由、记录规则命中情况；任何规则未命中所选对象时返回错误。

## 数据与状态

不缓存 router，程序修改 `Rename` 后不会复用旧解析。`is_partial_restore` 为显式 filter 或存在 Rename。无规则的配置指纹为空字符串；非空规则指纹为 router 的 32 字节摘要的 hex 表达。entries 保留输入顺序和源元数据名称，不进行 DDL 或 ID 分配。

## 依赖与调用关系

依赖 restore::nameroute、checkpoint 的三种库名前缀、task::restore 的命令与 flag 常量及 task 错误类型。冻结 `restore.rs` 的配置哈希逻辑调用 `get_name_router`，配置解析调用 `validate_name_routing("")`；`stream.rs` 将 `name_route_fingerprint` 传入日志检查点。`build_restore_name_plan` 只有 `restore_name_route_test.rs` 调用；本文件没有从该计划到快照建表或日志映射的生产桥接。

## 错误处理与边界

系统 schema 包括 mysql、information_schema、performance_schema、metrics_schema、inspection_schema、sys；比较前小写化，还拒绝 `__tidb_br_temporary_` 和三种 checkpoint schema 前缀。`validate_name_routing` 的 raw/txn 拒绝取决于传入命令名，不能把空字符串调用当作这些命令都已校验。命中标记使用 `eq_ignore_ascii_case`，而 router 键用 Unicode 小写，对非 ASCII 名称须保留此差异。

## 并发与资源生命周期

纯同步值计算，不创建线程或占用连接。规则与对象结果使用 Vec；失败时丢弃未返回的部分计划，无外部副作用。每次获取 router 都重新解析、排序和计算摘要。

## 与 Go 版本的对应关系

Go 还有围绕恢复元数据构建具体库表计划的编排，本 Rust 文件只提供通用 ObjectName 计划；不能将 Go 的完整目标 schema 创建/复用与 PiTR 接线写成 Rust 已实现。Rust 普通 String 与 Go CIStr、ASCII 命中检查也有表示差异。

## 扩展指南

新增 schema 禁止项应覆盖源和目标；修改规则集必须连同命令哈希、检查点及 ID map 指纹兼容审查。补生产调用时分别证明名称计划进入目标 DDL、源 ID/文件保留与日志映射，不应只增加 pub use。测试保持在独立 `restore_name_route_test.rs`。
