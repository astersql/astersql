# `pkg/ingestor/globalsort/orphandata.rs`

## 文件定位

遍历对象存储并排除受独立保留策略管理的前缀，累计对象数和非负字节数。

## 核心职责

- 在扫描前后检查生产者，尽量避免把仍有任务持有的中间数据发布为孤儿。
- 遍历对象存储并排除受独立保留策略管理的前缀，累计对象数和非负字节数。
- 保存至多十个样本名，发布 `GlobalSortOrphanDataSize`，在 owner 停止时移除旧指标序列。

监控不执行对象删除；任务终态清理与过期文件清理分别由 DXF cleaner 驱动。

## 主要符号

- `ActiveProducerChecker`：`Send + Sync` 的生产者查询接口，返回 `anyhow::Result<bool>`；闭包通过 blanket impl 实现。
- `Config`：必需的生产者 checker、每轮读取 URI 的闭包、保留前缀列表，以及可选 `StorageFactory`。
- `StorageFactory`：共享闭包，用对象存储 Context 和 URI 创建 `StorageRef`；默认调用 `NewFromURL`。
- `ScanStats`：`size_bytes`、`object_count`、`sample_objects`、`truncated`；默认全零或空。
- `SAMPLE_OBJECT_LIMIT = 10`：仅限制样本，不限制遍历数量。
- `Monitor::{new,trigger,reset}`：装配、同步扫描发布、移除指标序列。
- `scan_orphan_data`：可独立调用的只读扫描，调用方负责生产者检查和存储生命周期。

## 执行流程

`Monitor::new` 克隆注入工厂或装配默认工厂。`trigger` 每轮先读取 URI；空 URI 直接返回。第一次 checker 必须返回 `Ok(false)`，且 Context 尚未取消，才创建存储并调用 `scan_orphan_data`。扫描返回后无论成功或失败都调用 `storage.Close()`；扫描成功再进行第二次 checker，只有仍为 `Ok(false)` 才把字节数写入无标签 gauge。

生产调用位于 [`../../dxf/framework/scheduler/scheduler_manager.rs`](../../dxf/framework/scheduler/scheduler_manager.rs)：NextGen 模式装配真实 Monitor，Classic 使用 no-op；start/tick 在清理批次之后 trigger，stop/reset 移除旧 owner 指标。manager 使用活动任务表计数而非 unfinished 数量，终态尚未迁入历史前仍阻止扫描，并保留 `conflicted-rows/`。

## 数据与状态

`WalkDir(context, None, callback)` 对每个对象执行前缀字符串匹配，命中任意 `starts_with` 即完全排除。其余对象均增加 object_count，前十个进入样本；第十一个起将 truncated 置 true。负数 size 仍计对象和样本，但不累加字节。

扫描不解析 task ID，不判断对象年龄，也不逐对象查询归属；“孤儿”来自外部生产者检查与保留前缀约定。空前缀会匹配所有路径。样本顺序跟随存储遍历，不能假定排序。Monitor 不缓存 ScanStats，发布的只有总字节数。

## 依赖与调用关系

上游为 DXF owner manager 的 `GlobalSortOrphanDataMonitor` 包装器；它把 scheduler 的共享取消标志转为 objstore Context。生产者 checker 使用 `TaskManager::active_task_count`，存储适配器调用 `GetActiveTaskCountsByKeyspace().Total`。URI 来自 handle 的云存储配置查询；manager 将查询错误转为空 URI。

下游为 `astersql_objstore::storage::{Context,Storage,StorageRef,NewFromURL}` 和 `astersql_metrics::globalsort::GlobalSortOrphanDataSize`。错误用 anyhow 增加上下文。独立测试由 lib.rs 的 `#[cfg(test)]` 模块载入 [`orphandata_test.rs`](orphandata_test.rs)。

## 错误处理与边界

checker、工厂或扫描错误都让 trigger 静默返回并保留旧 gauge，不写零；空 URI、存在生产者或预检取消也一样。底层扫描错误添加 `scan global sort orphan objects` 上下文，默认工厂错误添加 `create global sort orphan-data store`，但 trigger 不向上返回这些错误。

取消在建存储前显式检查，遍历中的取消由 Storage 的 WalkDir 实现承接；扫描结束后没有独立的最终取消检查。双次生产者查询不能检测扫描期间出现又消失的任务（ABA），也不是数据库与对象存储之间的原子快照。不能将指标解释为删除授权或实时一致的容量证明。

## 并发与资源生命周期

trigger 在调用线程同步运行，没有定时器、后台 worker、互斥锁或自动重试。Arc 依赖支持共享，但本模块没有防止多个 trigger 并发发布的机制；owner 调用方应串行驱动生命周期。全量遍历可能阻塞调度 tick，十个样本上限不能缩短扫描。

成功创建存储后，普通 Result 路径均在扫描结束后 Close；这里不是 panic-safe 的专用清理 guard。reset 调用 GaugeVec 的 reset，删除序列而非写零，从而区分“未测量”与“测得零”。manager 的 cancel 仅翻转 initialized，未持有扫描 Context，因此不能据包装器的取消转换推断它能主动中断在途扫描。

## 与 Go 版本的对应关系

Rust 没有 Go Config 的 Logger 或 nil checker/getter 默认值；必需依赖由类型约束表达，测试工厂可显式注入。Go 使用 defer Close，Rust 在扫描调用后显式 Close。Rust 返回公开 ScanStats 和扫描函数，trigger 仍只发布大小。Go owner 从 cleanTaskLoop 的首次与定时清理驱动；Rust 用 manager start/tick，不能将 Go 的独立 goroutine、ticker 和取消生命周期描述为 Rust 已实现能力。

## 扩展指南

扩展保留命名空间时同步 manager 的 retained_prefixes 配置和独立测试，避免将有专属保留策略的冲突文件算入孤儿。改变生产者判断时保留终态清理完成前仍视为活动的约束。

若增加删除或自动回收，需要独立的所有权与竞态协议，不能复用当前两次查询作为安全证明。若增加后台扫描或更强取消，需定义 owner 失效后的发布规则及 reset 与在途 trigger 的同步。新增测试继续放在独立 orphandata_test.rs；失败保留旧值、第二门丢弃和资源关闭均应覆盖。
