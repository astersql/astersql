// Copyright 2026 AsterSQL.

// mydump 单元测试辅助：内存存储与构造 `FileInfo`。
//
// `MemoryStorage` 用互斥的路径→内容映射模拟对象存储，供 reader/region/schema
// 等测试在不访问真实文件系统的情况下打开与列举 dump 文件。

use crate::{Compression, FileInfo, FileMeta, MydumpError, SourceType, Storage};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::Mutex;

/// 测试用内存存储：按路径保存文件字节内容。
#[derive(Default)]
pub struct MemoryStorage {
    /// 路径到文件内容的有序映射（加锁保证并发测试安全）。
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryStorage {
    /// 由路径与内容切片列表构造存储。
    pub fn with(files: &[(&str, &[u8])]) -> Self {
        Self {
            files: Mutex::new(
                files
                    .iter()
                    .map(|(path, data)| ((*path).to_owned(), data.to_vec()))
                    .collect(),
            ),
        }
    }

    /// 插入或覆盖指定路径的文件内容。
    pub fn insert(&self, path: &str, data: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), data.into());
    }
}

impl Storage for MemoryStorage {
    /// 打开路径对应的内存游标；压缩参数在测试中忽略。
    fn open(
        &self,
        path: &str,
        _compression: Compression,
    ) -> Result<Box<dyn Read + Send>, MydumpError> {
        let data = self
            .files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| MydumpError::Io(format!("missing test file {path}")))?;
        Ok(Box::new(Cursor::new(data)))
    }

    /// 列举全部路径及其字节长度。
    fn list(&self) -> Result<Vec<(String, i64)>, MydumpError> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .iter()
            .map(|(path, data)| (path.clone(), data.len() as i64))
            .collect())
    }
}

/// 构造仅填路径与来源类型的 `FileInfo`，其余字段取默认值。
pub fn file(path: &str, source_type: SourceType) -> FileInfo {
    FileInfo {
        file_meta: FileMeta {
            path: path.to_owned(),
            source_type,
            ..Default::default()
        },
        ..Default::default()
    }
}
