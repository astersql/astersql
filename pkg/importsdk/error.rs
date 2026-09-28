// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Import SDK 共享哨兵错误定义。
//
// 以 `LazyLock` 惰性构造 [`astersql_errors::SharedError`]，供扫描、建库建表、
// 任务管理与存储后端路径统一 annotate 包装。消息文案与 Go 侧保持一致。

use astersql_errors as errors;
use std::sync::LazyLock;

// ErrNoDatabasesFound 表示 dump 来源中没有可识别的数据库。
/// dump 来源中没有可识别的数据库。
pub static ErrNoDatabasesFound: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("no databases found in the source path"));

// ErrSchemaNotFound 表示 dump 来源中不存在目标 schema。
/// dump 来源中不存在目标 schema（数据库）。
pub static ErrSchemaNotFound: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("schema not found"));

// ErrTableNotFound 表示 dump 来源中不存在目标表。
/// dump 来源中不存在目标表。
pub static ErrTableNotFound: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("table not found"));

// ErrNoTableDataFiles 表示表没有数据文件，因而不能继续处理。
/// 表没有数据文件，因而不能继续处理。
pub static ErrNoTableDataFiles: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("no data files for table"));

// ErrWildcardNotSpecific 表示通配符无法唯一覆盖目标表文件。
/// 无法为表的数据文件生成唯一通配符路径模式。
pub static ErrWildcardNotSpecific: LazyLock<errors::SharedError> = LazyLock::new(|| {
    errors::New("cannot generate a unique wildcard pattern for the table's data files")
});

// ErrJobNotFound 表示找不到导入任务。
/// 找不到导入任务（job）。
pub static ErrJobNotFound: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("job not found"));

// ErrNoJobIDReturned 表示提交语句没有返回任务 ID。
/// 提交导入语句后未返回任务 ID。
pub static ErrNoJobIDReturned: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("no job id returned"));

// ErrInvalidOptions 表示调用方提供的选项无效。
/// 调用方提供的导入选项无效。
pub static ErrInvalidOptions: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("invalid options"));

// ErrMultipleFieldsDefinedNullBy 表示同时给出了多个不受支持的 FIELDS_DEFINED_NULL_BY 值。
/// IMPORT INTO 仅支持单个 `FIELDS_DEFINED_NULL_BY` 值。
pub static ErrMultipleFieldsDefinedNullBy: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("IMPORT INTO only supports one FIELDS_DEFINED_NULL_BY value"));

// ErrParseStorageURL 表示存储后端 URL 无法解析。
/// 存储后端 URL 无法解析。
pub static ErrParseStorageURL: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("failed to parse storage backend URL"));

// ErrCreateExternalStorage 表示无法创建外部存储客户端。
/// 无法创建外部存储（S3/OSS/本地等）客户端。
pub static ErrCreateExternalStorage: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("failed to create external storage"));

// ErrCreateLoader 表示无法创建 MyDump loader。
/// 无法创建 MyDump loader（扫描 dump 目录的加载器）。
pub static ErrCreateLoader: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("failed to create MyDump loader"));

// ErrCreateSchema 表示创建数据库和表结构失败。
/// 在目标集群创建数据库与表结构失败。
pub static ErrCreateSchema: LazyLock<errors::SharedError> =
    LazyLock::new(|| errors::New("failed to create schemas and tables"));
