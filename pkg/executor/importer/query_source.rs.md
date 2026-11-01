# `pkg/executor/importer/query_source.rs`

## 文件定位

IMPORT FROM SELECT 的可序列化捕获模型和 worker 恢复、MVCC 最小 schema 重建，来源 964af3fa4679579548ffdfee4fb995cf1fcf037b。

## 核心职责

保留读取版本、schemaVersion、编码时间、排序来源表与会话快照；不复制 planner 或事务实现。

## 主要符号

QuerySource、SourceTable、QuerySessionStates、IMPORT_QUERY_WORKER_VARS、RestoreStates、BuildMinimalInfoSchema、BuildMinimalInfoSchemaWithCharsetPolicy。

## 执行流程

捕获由具体会话提供；恢复先 clone 状态并过滤 worker 决定的变量/内部变量，再替换状态位及清空警告，委派 decoder。重建从 SnapshotReader 逐库逐表读取、校验 revision 后用既有 Builder 构造。旧版本 UTF8 升级和大小写归一化按 Go 顺序执行，默认使用 Go 的 UTF8MB4 兼容策略，显式 policy 入口支持禁用该选项。

## 数据与状态

捕获快照可重用，RestoreStates 不改变原 SystemVars、Status 或 Warnings。表按照提交会话解析的 ID/revision 保留。

## 依赖与调用关系

lib.rs 导出；Plan.QuerySource 持有 Arc；ConcreteSession 实现 QuerySessionStates 并从 IMPORT 生产路径调用。依赖既有 sessionstates JSON、meta SnapshotReader、InfoSchema Builder。

## 错误处理与边界

元数据读取错误原样转字符串传播；数据库缺失、表缺失或 revision 不符返回来源定义的错误。状态 setter 错误向上传播，不伪造成功。

## 并发与资源生命周期

函数同步执行，SnapshotReader 独占不可变 KV snapshot；捕获数据只读共享，过滤使用独立克隆，无后台线程和全局锁。

## 与 Go 版本的对应关系

对应 query_source.go 的数据模型、RestoreStates 和 BuildMinimalInfoSchema；parser/捕获与 planner 的具体接线在 runtime/import_query_source.rs。父版本的完整会话迁移框架不在这里重建。

## 扩展指南

增加 worker 自有变量时维护排除表和真实会话回归；扩展 metadata 归一化必须保持 Go 版本/配置边界。
