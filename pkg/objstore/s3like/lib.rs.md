# `pkg/objstore/s3like/lib.rs`

## 文件定位

S3 公共 crate 根；导出存储、IO、权限、重试与指标，并定义本地 backuppb 配置与 pflag 门面。

## 核心职责

维护具体客户端共同使用的 S3 配置形状与 flag 字符串读写。

## 主要符号

backuppb::S3、pflag::FlagSet、FlagSet::String/Set/GetString，以及各子模块的导出。

## 执行流程

调用者注册 flags，再解析 S3BackendOptions；Apply 校验并写入本文件的 S3；具体客户端读取配置构造请求。

## 数据与状态

SseKmsEncryptionContext 是已规范化且 Base64 编码的字符串；原始 JSON 留在 S3BackendOptions。默认空串不能引入请求头。

## 依赖与调用关系

使用 objectio；store.rs 与 s3store/ossstore 使用此 S3 类型。这里不是外部 kvproto 生成代码；真实外部协议字段在独立 kvproto tag 中移植。

## 错误处理与边界

GetString/Set 对未注册 flag 返回错误；配置语义由 Apply 检查。不可在此重复编码 context。

## 并发与资源生命周期

FlagSet 由单一调用方可变持有；S3 是可克隆配置值，网络连接和上传资源由具体客户端管理。

## 与 Go 版本的对应关系

对应 Go s3like 及 backuppb.S3 的必要公共形状；cb395494e6 的字段18在本地门面对应 SseKmsEncryptionContext。

## 扩展指南

增加配置须同步 Apply、实际请求适配、独立测试和说明。保持原始与已编码值的边界。
