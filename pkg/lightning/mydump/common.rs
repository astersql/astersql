// Copyright 2026 AsterSQL.

// mydump 公共类型：错误枚举与 dump 文件元数据。
//
// Lightning 导入 mydumper/CSV 等数据源时，用这些结构描述文件路径、压缩方式、
// 来源类型（SQL/CSV 等）以及路由扩展列。MydumpError 统一承载 EOF、配置、编码、
// 语法、I/O、路由与 schema 解析失败。

use thiserror::Error;

/// mydump 解析与加载过程中的统一错误类型。
///
/// Eof 表示输入耗尽（正常结束）；其余变体对应配置、字符集、SQL/CSV 语法、
/// 底层 I/O、文件路由与表结构（schema）相关失败。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum MydumpError {
    /// 已到达文件末尾，无更多可解析内容。
    #[error("end of file")]
    Eof,
    /// 导入配置不合法（如空分隔符、并发度为 0）。
    #[error("configuration error: {0}")]
    Configuration(String),
    /// 字符集编解码失败。
    #[error("encoding error: {0}")]
    Encoding(String),
    /// SQL/CSV 词法或语法错误。
    #[error("syntax error: {0}")]
    Syntax(String),
    /// 底层读写失败。
    #[error("I/O error: {0}")]
    Io(String),
    /// 文件路径路由匹配失败。
    #[error("routing error: {0}")]
    Routing(String),
    /// 表/库 schema 文件缺失或解析失败。
    #[error("schema error: {0}")]
    Schema(String),
}

impl From<std::io::Error> for MydumpError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

/// 路由规则附带的扩展列名与常量值，导入时追加到行数据。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtendColumnData {
    /// 扩展列名列表。
    pub columns: Vec<String>,
    /// 与 columns 一一对应的常量值。
    pub values: Vec<String>,
}

/// 单个数据源文件的元信息（路径、大小、类型、压缩、排序键）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileMeta {
    /// 存储中的相对或绝对路径。
    pub path: String,
    /// 文件字节大小（压缩前体积以压缩文件为准）。
    pub file_size: i64,
    /// 估算后的真实导入大小；未压缩或跳过估算时等于 file_size。
    pub real_size: i64,
    /// 来源类型：SchemaSchema / Sql / Csv / Parquet 等。
    pub source_type: crate::SourceType,
    /// 压缩算法：None / Gz / Zstd 等。
    pub compression: crate::Compression,
    /// 分片排序键（如 `0001`），用于同一表多文件有序合并。
    pub sort_key: String,
}

/// 文件元数据加上路由扩展列，供 loader 与 parser 共享。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    /// 路径与类型等基础元数据。
    pub file_meta: FileMeta,
    /// 路由规则指定的扩展列数据。
    pub extend_data: ExtendColumnData,
}
