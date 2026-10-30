# `pkg/ingestor/ingestctrl/lib.rs`

## 文件定位

pkg/ingestor/ingestctrl/lib.rs 是crate入口，同时实现共享错误、数据与取消类型，不能按纯装配文件排除。

## 核心职责

导出引擎/导入pipeline/region worker模块；统一Error与Result；提供EngineId、KvPair、KeyRange、重复键策略、统计及CancellationToken。

## 主要符号

Error、EngineId::new、KvPair::size、KeyRange::overlaps、DuplicateResolution、ConflictInfo、EngineFileSize、CancellationToken。

## 执行流程

调用者创建engine标识与半开range，将有序KV交给local backend；worker通过取消令牌check检查生命周期。错误保留其类别与文本，经Error::source连接上层分类器。

## 数据与状态

EngineId用AtomicU64分配进程内递增值并以32位十六进制显示；KvPair拥有key/value；空end表示无穷上界；取消令牌包含自身AtomicBool、可选父令牌和worker context。

## 依赖与调用关系

local/engine/import_pipeline/job_worker共用这些类型；本来源session/import_sst把HTTP磁盘满错误转换为KVDiskFull(message)，CloudStep经ExecutorError.with_source交给DXF业务重试。

## 错误处理与边界

KVDiskFull保存原错误详情，source返回errdef::ErrKVDiskFull，避免只靠消息匹配。Cancelled、Conflict、DiskQuotaExceeded、参数/数据/IO/未找到/锁毒化/Retryable/Timeout仍保留既有分支。

## 并发与资源生命周期

共享取消标志用Acquire/Release读写；父取消及worker context取消沿子令牌传播。令牌本身不拥有线程或HTTP连接，资源由local backend/worker管理。

## 与 Go 版本的对应关系

对应pkg/ingestor/ingestctrl共享类型；KVDiskFull源链是0766a90d525d4e78b2739f170157408e39f1ec87之NextGen disk-full重试所需的限定前置接线，由用户明确授权。不表示完整Go控制器新增实现。

## 扩展指南

新增错误需保留类别与详情，更新Display/source并验证不把同文案不同类别误判；不要令不可重试错误进入本地无限重试。

## 验证依据

session的port_0766a90d52_整链2项与taskexecutor真实retry指标回归均通过；错误保留类别及详情，失败后任务仍Running，容量恢复后原任务成功。
