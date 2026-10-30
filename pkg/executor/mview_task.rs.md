# `pkg/executor/mview_task.rs`

## 文件定位

物化视图维护时间工具，属于来源 a87bf8db6499ac38075781cef7357709f07ea002 的局部增量。

## 核心职责

计算非负微秒时长、固定六位小数秒数和历史时间微秒截断；None 表示 Go zero time，缺少端点或结束早于开始返回零。

## 主要符号

`duration_microseconds_between`、`format_duration_seconds_from_microseconds`、`format_duration_seconds_between`、`format_duration_seconds`、`hist_time`。

## 执行流程

本模块经 lib.rs 导出，由真实 MV refresh 历史与 canonical mlog_purge 的 purge_history_duration 共同消费；独立测试覆盖缺失/倒序端点、微秒截断与饱和边界。

## 数据与状态

步骤和时间计算使用调用者提供的数据，不持久化数据库状态。步骤集合与计划行由调用方拥有。

## 依赖与调用关系

本模块经 lib.rs 导出，由真实 MV refresh 历史与 canonical mlog_purge 的 purge_history_duration 共同消费；独立测试覆盖缺失/倒序端点、微秒截断与饱和边界。

## 错误处理与边界

无事务、网络或存储错误处理。时间倒序和缺失端点归零；两个 SystemTime 的差值按 Go time.Duration 最大值饱和。负微秒输入格式化为零；微秒截断遵守 Unix epoch 前后的一致绝对时间边界。

## 并发与资源生命周期

纯数据 helper 没有后台线程、连接、锁或事务资源。输入时间按值传入，输出不借用外部资源。

## 与 Go 版本的对应关系

`pkg/executor/mview_task.go` 中新增时间函数 hunk，以及 `mlog_purge.go` 的工具提取。SystemTime 是绝对时间，时区转换不改变其值；SQL 的时区展示仍归 session。

## 扩展指南

保留来源的全部分支与步骤顺序；新增 runtime 接线前先补真实存储与事务测试，不能用本模块的纯计算测试替代行为验收。独立测试位于同目录测试文件，不放入生产源文件。
