# dumpling/export/stubs.rs

## 文件定位

export crate 的历史 SQL、MySQL、存储、HTTP、指标及 PD 替身集合。

## 核心职责

为现有导出代码提供局部依赖类型及可注入测试状态；不表示真实外部服务实现。

## 主要符号

MysqlConfig 保存连接参数，TLS 与 AllowFallbackToPlaintext 承载 MySQL TLS 策略；DB、Conn、Rows 与 PDClient 提供现有内存行为。

## 执行流程

Config::GetDriverConfig 创建 MysqlConfig；现有 openDB 忽略配置返回 DB::new。真实 TLS 升级入口在 config.rs 的 MysqlTLSConfig::connect。

## 数据与状态

MysqlConfig 持有可克隆策略和连接字段；其他替身通过共享锁、原子计数和内存集合保存状态。

## 依赖与调用关系

lib.rs 将本模块再导出供 config、dump、sql、consistency 使用。TLS 字段引用 crate::MysqlTLSConfig，无独立 TLS 实现。

## 错误处理与边界

Error/MySQLError 承载本地错误；FormatDSN 掩盖密码。openDB 不是网络驱动，不执行握手或验证服务端。

## 并发与资源生命周期

共享替身状态由 Arc/Mutex/原子字段协调；MysqlConfig 是值对象，TLS socket 生命周期归实际 connect 调用方。

## 与 Go 版本的对应关系

来源 02dffa3bcf3fe6663f5d66fd9914588b3102c73a 的必要局部接线仅增加 MysqlConfig 的 TLS 和 fallback 字段，以承载 Go driverCfg 的策略。其他 SQL/PD 替身是历史缺口，本任务不扩展。

## 扩展指南

真实 SQL 驱动应另行对齐连接、查询和资源语义；不可将本文件的内存替身当成真实服务验收。测试放独立文件。
