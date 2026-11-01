# time_proxy.rs

## 文件定位

`pkg/mvservice/time_proxy.rs` 属于新增物化视图服务框架。

## 核心职责

真实与可控测试时钟代理。

## 主要符号

Clock、Timestamp、MockTimeModule、Timer、RestoreClock、current_clock。

## 执行流程

生产默认使用真实时钟；测试可通过 RAII 安装并恢复 Mock 模块。After 注册截止时间，Set/Advance 触发到期事件；Sleep 可自动推进或等待。Timer 用代数标识隔离重置前的事件，Stop 使旧代失效。

## 数据与状态

Timestamp 为有符号 Unix 纳秒，支持 epoch 前时间；等待器保留截止时间与单次发送端；Timer 状态保留 active/seq，已到期、停止和重置的布尔返回遵循 Go 测试。

## 依赖与调用关系

标准库时间与 crossbeam 单次通道；真实定时器以单调 Instant 截止时间由单个共享调度线程维护（系统墙钟调整不延长任务超时），避免为每个长延迟永久占用一条线程；Mock 只在显式推进时发事件。

## 错误处理与边界

保留来源提交的错误分支与空状态语义。独立测试在 `commit_7e84fb645a_test.rs`，包含真实入口调用；外部探针与服务发现可替换，内部队列、哈希和任务执行均为实际实现。

## 并发与资源生命周期

由文件中的锁或串行调用约束管理状态；所有权释放移除的队列、节点或通知引用。测试时钟全局安装必须串行使用，并通过 RestoreClock 自动恢复。

## 与 Go 版本的对应关系

来源 `7e84fb645ad620aed44570985a64efde83a85db9`：time_proxy.go 和 time_proxy_intest.go 全部新增 hunks；Rust 不依赖 Go intest 编译标签，Mock 是显式选择的测试边界，默认仍为真实时间。

本提交只导出 crate 框架，和 Go 来源一样尚未接入后续后台调度；不宣称 SQL 生产调度调用已经存在。独立测试通过 crate 的实际入口验证行为。PostgreSQL 方言、协议、类型与事务实现未改动。

## 扩展指南

后续提交可通过公开入口接入调度；先保持本提交测试与 Go 契约，避免递归引入其他服务子系统。修改业务源文件时同步本说明和文档清单。
