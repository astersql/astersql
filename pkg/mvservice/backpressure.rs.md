# backpressure.rs

## 文件定位

`pkg/mvservice/backpressure.rs` 属于新增物化视图服务框架。

## 核心职责

CPU 与内存背压控制。

## 主要符号

TaskBackpressureController、CPUMemBackpressureController、DEFAULT_DELAY。

## 执行流程

先检查 CPU 支持标志与阈值，再检查非零总内存与成功读取的用量；任一达到阈值返回背压及延迟。

## 数据与状态

阈值小于等于零禁用对应检查；未配置延迟回退 100ms。探针错误和不支持 CPU 不触发相应检查。

## 依赖与调用关系

通过 cpu_crate::GetCPUUsage、memory_crate::meminfo 的真实探针接线；闭包允许替换外部探针边界。

## 错误处理与边界

保留来源提交的错误分支与空状态语义。独立测试在 `commit_7e84fb645a_test.rs`，包含真实入口调用；外部探针与服务发现可替换，内部队列、哈希和任务执行均为实际实现。

## 并发与资源生命周期

通过 cpu_crate::GetCPUUsage、memory_crate::meminfo 的真实探针接线；闭包允许替换外部探针边界。

## 与 Go 版本的对应关系

来源 `7e84fb645ad620aed44570985a64efde83a85db9`：backpressure.go 全部新增 hunk；保留来源 0.8 的 CPU/内存默认阈值常量；构造执行器本身不自动安装控制器。

本提交只导出 crate 框架，和 Go 来源一样尚未接入后续后台调度；不宣称 SQL 生产调度调用已经存在。独立测试通过 crate 的实际入口验证行为。PostgreSQL 方言、协议、类型与事务实现未改动。

## 扩展指南

后续提交可通过公开入口接入调度；先保持本提交测试与 Go 契约，避免递归引入其他服务子系统。修改业务源文件时同步本说明和文档清单。
