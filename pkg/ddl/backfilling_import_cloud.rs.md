# `pkg/ddl/backfilling_import_cloud.rs`

## 文件定位

云端索引回填的同步导入控制面。`CloudImportExecutor` 由 `pkg/ddl/lib.rs` 导出，当前构造调用仅见独立测试；真实 session 云回填入口在 `pkg/session/runtime/modify_column_cloud_executor.rs`，不能把 Go 的调用链当作此泛型实现的生产接线。

## 核心职责

选择索引、合并排序元信息、兼容旧 job keys/时间戳、创建真实 globalsort 引擎并交给后端摄取；成功和失败后关闭调用者持有的引擎。保留动态资源、限速和重复键错误映射。

## 主要符号

`CloudImportExecutor<B>` 保存后端和初始化/运行状态；`CloudImportBackend` 提供对象存储、`alloc_new_ts`、`ingest_external_engine` 和资源回调。`ExternalEngineConfig` 是本地元信息快照；实际引擎使用 globalsort `EngineConfig`。`get_index_info_and_id`、`has_unique_index`、`IngestCollector` 供回填流程复用。

## 执行流程

`new -> init -> run_subtask* -> cleanup`。运行先检查初始化并选择索引，再合并 meta groups；job keys 为空时使用 split keys。TS 为 0 才分配新版本。构造完整配置（热点检测、重复键错误、内存、CPU），创建 `ExternalEngineAdapter` 并直接摄取，不再通过 UUID 注册/查找。摄取后关闭引擎，清运行标志，再返回原摄取错误或成功。

## 数据与状态

合并元信息决定起止键和总字节，KV 条数固定为未知的 0。`initialized` 限制运行入口，`engine_running` 供同步资源回调判断。索引 ID 只用于索引匹配，不再用作外部引擎注册表键；索引名保留在冲突错误中。

## 依赖与调用关系

依赖 crate 内 BackfillSubTaskMeta、SortedKvMeta，以及 astersql-ingestor-globalsort 的存储、配置、引擎和 adapter。后端仅负责外部服务边界与摄取，调用者负责引擎生命周期。完整 native 路径通过 session/runtime 的 import_sst 与 ingestctrl 进入实际 KV store。

## 错误处理与边界

未初始化返回 NotInitialized；时间戳分配失败不创建引擎；普通摄取错误保留文本，重复键附索引名。Close 错误仅记录，不覆盖摄取结果。未知单元素 ID 保留 Go 零值结果，多元素 ID 返回错误；无元素且无索引的历史不变量仍由调用方保证。旧 ExternalEngineNotFound 枚举不再由运行路径构造。

## 并发与资源生命周期

`ExternalEngineGuard` 在作用域退出（包括提前错误与展开）时执行 CloseShared，后端只借用 Arc；关闭错误不覆盖原业务结果。

同步泛型入口用 &mut self 串行控制；真实 globalsort 句柄通过 Arc 共享资源控制。单次摄取返回后关闭本次引擎，即使后端仍保存观察句柄也不能继续加载；重复 close 安全。`cleanup` 关闭后端，不承担外部引擎查表清理。Go 原子引擎指针的并发调度由实际 session CloudStep 的 active 句柄提供，不能将此处布尔值视为跨线程同步。

## 与 Go 版本的对应关系

对齐 373679dd4df08d656e3a95ff0cc0cd245152a9ff 中 RunSubtask 的 7 个 hunk：移除 backend UUID 依赖与注册/查找，旧 TS 分配，直接构造和摄取，调用者关闭。Go 的 logger/import 调整属于语言接线；本文件不重建 DDL job 状态机、持久化、事务或 schema sync。

## 扩展指南

测试保留独立 backfilling_import_cloud_test.rs，覆盖显式/旧 TS、分配失败、普通失败、重复键上下文和真实引擎关闭。新增生产适配器必须沿既有分布式调度入口接线，勿把仅测试可见的泛型构造宣称为生产主链。
