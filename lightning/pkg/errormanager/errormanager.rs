// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 自动补充的本模块把导入过程中的错误和冲突统一收口到任务级 schema。
// 注释重点是阈值怎样消费、SQL 怎样组合、以及与 Go 的可观测语义对齐。
// 自动补充的这个文件承载当前模块的主要语义边界。
// 注释重点是数据流、配额边界、SQL 模板和对 Go 契约的对齐关系。
// 本次只增加注释，不改变任何运行时逻辑或测试行为。
// 因此这些说明会围绕“为什么这样写”而不是重复语法。
//! Error manager: create task error tables, decrement error thresholds,
//! record type/conflict/duplicate errors, resolve replace-mode conflicts,
//! and render an error summary. Mirrors `lightning/pkg/errormanager`.

use std::sync::{Arc, Mutex};

use crate::Result;
use crate::atomic;
use crate::common;
use crate::config;
use crate::context;
use crate::encode;
use crate::errors;
use crate::kv;
use crate::log;
use crate::logutil;
use crate::multierr;
use crate::mysql;
use crate::pretty_table;
use crate::redact;
use crate::sql::{self, SqlValue};
use crate::tablecodec;
use crate::tables;
use crate::tidbtbl;
use crate::tikverr;
use crate::types;
use crate::util;
use crate::zap;

// 自动补充的`createSchema` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createSchema: &str = "
		CREATE SCHEMA IF NOT EXISTS %s;
	";

// 自动补充的`syntaxErrorTableName` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const syntaxErrorTableName: &str = "syntax_error_v2";
const typeErrorTableName: &str = "type_error_v2";
/// ConflictErrorTableName is the table name for duplicate detection.
pub const ConflictErrorTableName: &str = "conflict_error_v4";
// 自动补充的`DupRecordTableName` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
/// DupRecordTableName is the table name to record duplicate data that displayed to user.
pub const DupRecordTableName: &str = "conflict_records_v2";
/// ConflictViewName is the view name for presenting the union information of ConflictErrorTable and DupRecordTable.
pub const ConflictViewName: &str = "conflict_view";

// 自动补充的`createSyntaxErrorTable` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createSyntaxErrorTable: &str = "
		CREATE TABLE IF NOT EXISTS %s.syntax_error_v2 (
			id 	    	bigint PRIMARY KEY AUTO_INCREMENT,
			task_id     bigint NOT NULL,
			create_time datetime(6) NOT NULL DEFAULT now(6),
			table_name  varchar(261) NOT NULL,
			path        varchar(2048) NOT NULL,
			offset      bigint NOT NULL,
			error       text NOT NULL,
			context     text
		);
	";

// 自动补充的`createTypeErrorTable` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createTypeErrorTable: &str = "
		CREATE TABLE IF NOT EXISTS %s.type_error_v2 (
			id		    bigint PRIMARY KEY AUTO_INCREMENT,
			task_id     bigint NOT NULL,
			create_time datetime(6) NOT NULL DEFAULT now(6),
			table_name  varchar(261) NOT NULL,
			path        varchar(2048) NOT NULL,
			offset      bigint NOT NULL,
			error       text NOT NULL,
			row_data    text NOT NULL
		);
	";

// 自动补充的`createConflictErrorTable` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createConflictErrorTable: &str = "
		CREATE TABLE IF NOT EXISTS %s.conflict_error_v4 (
			id          bigint PRIMARY KEY AUTO_INCREMENT,
			task_id     bigint NOT NULL,
			create_time datetime(6) NOT NULL DEFAULT now(6),
			table_name  varchar(261) NOT NULL,
			index_name  varchar(128) NOT NULL,
			key_data    text COMMENT 'decoded from raw_key, human readable only, not for machine use',
			row_data    text COMMENT 'decoded from raw_row, human readable only, not for machine use',
			raw_key     mediumblob NOT NULL COMMENT 'the conflicted key',
			raw_value   mediumblob NOT NULL COMMENT 'the value of the conflicted key',
			raw_handle  mediumblob NOT NULL COMMENT 'the data handle derived from the conflicted key or value',
			raw_row     mediumblob NOT NULL COMMENT 'the data retrieved from the handle',
			kv_type     tinyint(1) NOT NULL COMMENT '0 for index kv, 1 for data kv, 2 for additionally inserted data kv',
			INDEX (task_id, table_name),
			INDEX (index_name),
			INDEX (table_name, index_name),
			INDEX (kv_type)
		);
	";

// 自动补充的`createDupRecordTableName` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createDupRecordTableName: &str = "
		CREATE TABLE IF NOT EXISTS %s.conflict_records_v2 (
			id          bigint PRIMARY KEY AUTO_INCREMENT,
			task_id     bigint NOT NULL,
			create_time datetime(6) NOT NULL DEFAULT now(6),
			table_name  varchar(261) NOT NULL,
			path        varchar(2048) NOT NULL,
			offset      bigint NOT NULL,
			error       text NOT NULL,
			row_id 	    bigint NOT NULL COMMENT 'the row id of the conflicted row',
			row_data    text NOT NULL COMMENT 'the row data of the conflicted row',
			KEY (task_id, table_name)
		);
	";

// 自动补充的`createConflictV1View` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createConflictV1View: &str = "
    	CREATE OR REPLACE VIEW %s.conflict_view
			AS SELECT 0 AS is_precheck_conflict, task_id, create_time, table_name, index_name, key_data, row_data,
			raw_key, raw_value, raw_handle, raw_row, kv_type, NULL AS path, NULL AS offset, NULL AS error, NULL AS row_id
			FROM %s.conflict_error_v4;
	";

// 自动补充的`createConflictV2View` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createConflictV2View: &str = "
    	CREATE OR REPLACE VIEW %s.conflict_view
			AS SELECT 1 AS is_precheck_conflict, task_id, create_time, table_name, NULL AS index_name, NULL AS key_data,
			row_data, NULL AS raw_key, NULL AS raw_value, NULL AS raw_handle, NULL AS raw_row, NULL AS kv_type, path,
			offset, error, row_id FROM %s.conflict_records_v2;
	";

// 自动补充的`createConflictV1V2View` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const createConflictV1V2View: &str = "
    	CREATE OR REPLACE VIEW %s.conflict_view
			AS SELECT 0 AS is_precheck_conflict, task_id, create_time, table_name, index_name, key_data, row_data,
			raw_key, raw_value, raw_handle, raw_row, kv_type, NULL AS path, NULL AS offset, NULL AS error, NULL AS row_id
			FROM %s.conflict_error_v4
			UNION ALL SELECT 1 AS is_precheck_conflict, task_id, create_time, table_name, NULL AS index_name, NULL AS key_data,
			row_data, NULL AS raw_key, NULL AS raw_value, NULL AS raw_handle, NULL AS raw_row, NULL AS kv_type, path,
			offset, error, row_id FROM %s.conflict_records_v2;
	";

// 自动补充的`insertIntoTypeError` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const insertIntoTypeError: &str = "
		INSERT INTO %s.type_error_v2
		(task_id, table_name, path, offset, error, row_data)
		VALUES (?, ?, ?, ?, ?, ?);
	";

// 自动补充的`insertIntoConflictErrorData` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const insertIntoConflictErrorData: &str = "
		INSERT INTO %s.conflict_error_v4
		(task_id, table_name, index_name, key_data, row_data, raw_key, raw_value, raw_handle, raw_row, kv_type)
		VALUES
	";

// 自动补充的`sqlValuesConflictErrorData` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const sqlValuesConflictErrorData: &str = "(?,?,'PRIMARY',?,?,?,?,raw_key,raw_value,?)";

const insertIntoConflictErrorIndex: &str = "
		INSERT INTO %s.conflict_error_v4
		(task_id, table_name, index_name, key_data, row_data, raw_key, raw_value, raw_handle, raw_row, kv_type)
		VALUES
	";

// 自动补充的`sqlValuesConflictErrorIndex` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const sqlValuesConflictErrorIndex: &str = "(?,?,?,?,?,?,?,?,?,?)";

const selectIndexConflictKeysReplace: &str = "
		SELECT id, raw_key, index_name, raw_value, raw_handle
		FROM %s.conflict_error_v4
		WHERE table_name = ? AND kv_type = 0 AND id >= ? and id < ?
		ORDER BY id LIMIT ?;
	";

// 自动补充的`selectDataConflictKeysReplace` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const selectDataConflictKeysReplace: &str = "
		SELECT id, raw_key, raw_value
		FROM %s.conflict_error_v4
		WHERE table_name = ? AND kv_type <> 0 AND id >= ? and id < ?
		ORDER BY id LIMIT ?;
	";

// 自动补充的`deleteNullDataRow` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const deleteNullDataRow: &str = "
		DELETE FROM %s.conflict_error_v4
		WHERE kv_type = 2
		LIMIT ?;
	";

// 自动补充的`insertIntoDupRecord` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
const insertIntoDupRecord: &str = "
		INSERT INTO %s.conflict_records_v2
		(task_id, table_name, path, offset, error, row_id, row_data)
		VALUES (?, ?, ?, ?, ?, ?, ?);
	";

// 自动补充的`ErrorManager` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
/// ErrorManager records errors during the import process.
pub struct ErrorManager {
    pub db: Option<sql::DB>,
    pub taskID: i64,
    pub schema: String,
    pub configError: config::MaxError,
    pub remainingError: config::MaxError,
    pub configConflict: config::Conflict,
    pub conflictErrRemain: atomic::Int64,
    pub conflictRecordsRemain: atomic::Int64,
    pub conflictV1Enabled: bool,
    pub conflictV2Enabled: bool,
    pub logger: log::Logger,
    pub recordErrorOnce: atomic::Bool,
    /// Optional encode map shared with kv stub encoder (tests / replace path).
    pub encode_map: Arc<Mutex<Vec<(Vec<u8>, Vec<kv::KvPair>)>>>,
}

// 自动补充的下面的 `impl ErrorManager` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl ErrorManager {
    /// TypeErrorsRemain returns the number of type errors that can be recorded.
    pub fn TypeErrorsRemain(&self) -> i64 {
        self.remainingError.Type.Load()
    }

    // 自动补充的`ConflictErrorsRemain` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// ConflictErrorsRemain returns the number of conflict errors that can be recorded.
    pub fn ConflictErrorsRemain(&self) -> i64 {
        self.conflictErrRemain.Load()
    }

    // 自动补充的`ConflictRecordsRemain` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// ConflictRecordsRemain returns the number of errors that need be recorded.
    pub fn ConflictRecordsRemain(&self) -> i64 {
        self.conflictRecordsRemain.Load()
    }

    // 自动补充的`RecordErrorOnce` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordErrorOnce returns if RecordDuplicateOnce has been called. Not that this
    /// method is not atomic with RecordDuplicateOnce.
    pub fn RecordErrorOnce(&self) -> bool {
        self.recordErrorOnce.Load()
    }

    // 自动补充的`Init` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// Init creates the schemas and tables to store the task information.
    pub fn Init(&self, ctx: context::Context) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };

        let exec = common::SQLWithRetry {
            DB: db.clone(),
            Logger: self.logger.clone(),
            HideQueryLog: false,
        };

        let mut sqls: Vec<(&str, &str)> = Vec::new();
        sqls.push(("create task info schema", createSchema));
        if self.remainingError.Syntax.Load() > 0 {
            sqls.push(("create syntax error table", createSyntaxErrorTable));
        }
        if self.remainingError.Type.Load() > 0 {
            sqls.push(("create type error table", createTypeErrorTable));
        }
        if self.conflictV1Enabled {
            sqls.push(("create conflict error table", createConflictErrorTable));
        }
        if self.conflictV2Enabled {
            sqls.push(("create duplicate records table", createDupRecordTableName));
        }

        // No need to create task info schema if no error is allowed.
        if sqls.len() == 1 {
            return Ok(());
        }

        for (name, sql) in sqls {
            // trim spaces for unit test pattern matching
            let q = common::SprintfWithIdentifiers(sql, &[&self.schema]);
            exec.Exec(ctx, name, q.trim().to_string(), &[])?;
        }

        if self.conflictV1Enabled && self.conflictV2Enabled {
            let q = common::SprintfWithIdentifiers(
                createConflictV1V2View,
                &[&self.schema, &self.schema, &self.schema],
            );
            exec.Exec(ctx, "create conflict view", q.trim().to_string(), &[])?;
        } else if self.conflictV1Enabled {
            let q =
                common::SprintfWithIdentifiers(createConflictV1View, &[&self.schema, &self.schema]);
            exec.Exec(ctx, "create conflict view", q.trim().to_string(), &[])?;
        } else if self.conflictV2Enabled {
            let q =
                common::SprintfWithIdentifiers(createConflictV2View, &[&self.schema, &self.schema]);
            exec.Exec(ctx, "create conflict view", q.trim().to_string(), &[])?;
        }

        Ok(())
    }

    // 自动补充的`RecordTypeError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordTypeError records a type error.
    /// If the number of recorded type errors exceed the max-error count, also returns `err` directly.
    pub fn RecordTypeError(
        &self,
        ctx: context::Context,
        mut logger: log::Logger,
        tableName: &str,
        path: &str,
        offset: i64,
        rowText: &str,
        mut encodeErr: errors::Error,
    ) -> Result<()> {
        // elide the encode error if needed.
        if self.remainingError.Type.Dec() < 0 {
            let threshold = self.configError.Type.Load();
            if threshold > 0 {
                encodeErr = errors::Annotatef(
                    encodeErr,
                    format!(
                        "The number of type errors exceeds the threshold configured by `max-error.type`: '{threshold}'"
                    ),
                );
            }
            return Err(encodeErr);
        }

        if let Some(db) = &self.db {
            let errMsg = encodeErr.Error();
            logger = logger.With(&[
                zap::Int64("offset", offset),
                zap::String("row", &redact::Value(rowText)),
                zap::String("message", &errMsg),
            ]);

            let exec = common::SQLWithRetry {
                DB: db.clone(),
                Logger: logger,
                HideQueryLog: redact::NeedRedact(),
            };
            let q = common::SprintfWithIdentifiers(insertIntoTypeError, &[&self.schema]);
            if let Err(err) = exec.Exec(
                ctx,
                "insert type error record",
                q,
                &[
                    SqlValue::from(self.taskID),
                    SqlValue::from(tableName),
                    SqlValue::from(path),
                    SqlValue::from(offset),
                    SqlValue::from(errMsg.as_str()),
                    SqlValue::from(rowText),
                ],
            ) {
                return Err(multierr::Append(encodeErr, err));
            }
        }
        Ok(())
    }

    // 自动补充的`RecordDataConflictError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordDataConflictError records a data conflict error.
    pub fn RecordDataConflictError(
        &self,
        ctx: context::Context,
        logger: log::Logger,
        tableName: &str,
        conflictInfos: &[DataConflictInfo],
    ) -> Result<()> {
        let mut gerr: Option<errors::Error> = None;
        if conflictInfos.is_empty() {
            return Ok(());
        }

        if self.conflictErrRemain.Sub(conflictInfos.len() as i64) < 0 {
            let threshold = self.configConflict.Threshold;
            // Still need to record this batch of conflict records, and then return this error at last.
            gerr = Some(errors::Errorf(format!(
                "The number of conflict errors exceeds the threshold configured by `conflict.threshold`: '{threshold}'"
            )));
        }

        let Some(db) = &self.db else {
            return match gerr {
                Some(e) => Err(e),
                None => Ok(()),
            };
        };

        let exec = common::SQLWithRetry {
            DB: db.clone(),
            Logger: logger,
            HideQueryLog: redact::NeedRedact(),
        };
        let schema = self.schema.clone();
        let task_id = self.taskID;
        let table_name = tableName.to_string();
        let infos = conflictInfos.to_vec();
        if let Err(err) = exec.Transact(ctx, "insert data conflict error record", |c, txn| {
            let mut sb = String::new();
            common::FprintfWithIdentifiers(&mut sb, insertIntoConflictErrorData, &[&schema])?;
            let mut sqlArgs: Vec<SqlValue> = Vec::new();
            for (i, conflictInfo) in infos.iter().enumerate() {
                if i > 0 {
                    sb.push(',');
                }
                sb.push_str(sqlValuesConflictErrorData);
                sqlArgs.push(SqlValue::from(task_id));
                sqlArgs.push(SqlValue::from(table_name.as_str()));
                sqlArgs.push(SqlValue::from(conflictInfo.KeyData.as_str()));
                sqlArgs.push(SqlValue::from(conflictInfo.Row.as_str()));
                sqlArgs.push(SqlValue::from(conflictInfo.RawKey.as_slice()));
                sqlArgs.push(SqlValue::from(conflictInfo.RawValue.as_slice()));
                sqlArgs.push(SqlValue::from(tablecodec::IsRecordKey(
                    &conflictInfo.RawKey,
                )));
            }
            txn.ExecContext(c, &sb, &sqlArgs)?;
            Ok(())
        }) {
            gerr = Some(err);
        }
        match gerr {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    // 自动补充的`RecordIndexConflictError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordIndexConflictError records a index conflict error.
    pub fn RecordIndexConflictError(
        &self,
        ctx: context::Context,
        logger: log::Logger,
        tableName: &str,
        indexNames: &[String],
        conflictInfos: &[DataConflictInfo],
        rawHandles: &[Vec<u8>],
        rawRows: &[Vec<u8>],
    ) -> Result<()> {
        let mut gerr: Option<errors::Error> = None;
        if conflictInfos.is_empty() {
            return Ok(());
        }

        if self.conflictErrRemain.Sub(conflictInfos.len() as i64) < 0 {
            let threshold = self.configConflict.Threshold;
            gerr = Some(errors::Errorf(format!(
                "The number of conflict errors exceeds the threshold configured by `conflict.threshold`: '{threshold}'"
            )));
        }

        let Some(db) = &self.db else {
            return match gerr {
                Some(e) => Err(e),
                None => Ok(()),
            };
        };

        let exec = common::SQLWithRetry {
            DB: db.clone(),
            Logger: logger,
            HideQueryLog: redact::NeedRedact(),
        };
        let schema = self.schema.clone();
        let task_id = self.taskID;
        let table_name = tableName.to_string();
        let infos = conflictInfos.to_vec();
        let index_names = indexNames.to_vec();
        let raw_handles = rawHandles.to_vec();
        let raw_rows = rawRows.to_vec();
        if let Err(err) = exec.Transact(ctx, "insert index conflict error record", |c, txn| {
            let mut sb = String::new();
            common::FprintfWithIdentifiers(&mut sb, insertIntoConflictErrorIndex, &[&schema])?;
            let mut sqlArgs: Vec<SqlValue> = Vec::new();
            for (i, conflictInfo) in infos.iter().enumerate() {
                if i > 0 {
                    sb.push(',');
                }
                sb.push_str(sqlValuesConflictErrorIndex);
                sqlArgs.push(SqlValue::from(task_id));
                sqlArgs.push(SqlValue::from(table_name.as_str()));
                sqlArgs.push(SqlValue::from(index_names[i].as_str()));
                sqlArgs.push(SqlValue::from(conflictInfo.KeyData.as_str()));
                sqlArgs.push(SqlValue::from(conflictInfo.Row.as_str()));
                sqlArgs.push(SqlValue::from(conflictInfo.RawKey.as_slice()));
                sqlArgs.push(SqlValue::from(conflictInfo.RawValue.as_slice()));
                sqlArgs.push(SqlValue::from(raw_handles[i].as_slice()));
                sqlArgs.push(SqlValue::from(raw_rows[i].as_slice()));
                sqlArgs.push(SqlValue::from(tablecodec::IsRecordKey(
                    &conflictInfo.RawKey,
                )));
            }
            txn.ExecContext(c, &sb, &sqlArgs)?;
            Ok(())
        }) {
            gerr = Some(err);
        }
        match gerr {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    // 自动补充的`ReplaceConflictKeys` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// ReplaceConflictKeys query all conflicting rows (handle and their
    /// values) from the current error report and resolve them
    /// by replacing the necessary rows and reserving the others.
    ///
    /// Concurrency mirrors Go's channel + errgroup work-splitting via a
    /// work queue (same split rule: when `end-start > rowLimit`, push
    /// `[mid,end)` back for another worker). Pool size controls parallelism
    /// of the logical split; the algorithm itself is unchanged.
    pub fn ReplaceConflictKeys<F1, F2>(
        &self,
        ctx: context::Context,
        tbl: tidbtbl::Table,
        tableName: &str,
        pool: &util::WorkerPool,
        fnGetLatest: F1,
        fnDeleteKeys: F2,
    ) -> Result<()>
    where
        F1: Fn(context::Context, &[u8]) -> Result<Vec<u8>> + Send + Sync,
        F2: Fn(context::Context, &[Vec<u8>]) -> Result<()> + Send + Sync,
    {
        let Some(db) = &self.db else {
            return Ok(());
        };

        let exec = common::SQLWithRetry {
            DB: db.clone(),
            Logger: self.logger.clone(),
            HideQueryLog: redact::NeedRedact(),
        };

        // 自动补充的`rowLimit` 是当前流程依赖的固定片段。
        // 它通常被用来组装 SQL、保持对外命名或描述状态语义。
        // 单独拆出常量能降低不同分支重复拼装字符串的风险。
        // 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
        // 理解它时要结合后续使用它的方法一起看。
        const rowLimit: i64 = 1000;
        let splitRemaining = |start: i64, end: i64| -> Vec<[i64; 2]> {
            if start >= end {
                return Vec::new();
            }
            if end - start > rowLimit {
                let mid = start + (end - start) / 2;
                vec![[start, mid], [mid, end]]
            } else {
                vec![[start, end]]
            }
        };

        // ---- index KV phase ----
        pool.RunDynamic([0, i64::MAX], |[start, end]| {
            let sessionOpts = encode::SessionOptions {
                SQLMode: mysql::ModeStrictAllTables,
            };
            let encoder = kv::NewBaseKVEncoderWithMap(
                &encode::EncodingConfig {
                    Table: tbl.clone(),
                    SessionOptions: sessionOpts,
                    Logger: self.logger.clone(),
                },
                self.encode_map.clone(),
            )?;

            let mut handleKeys: Vec<Vec<u8>> = Vec::new();
            let mut insertRows: Vec<[Vec<u8>; 2]> = Vec::new();
            let q = common::SprintfWithIdentifiers(selectIndexConflictKeysReplace, &[&self.schema]);
            let mut indexKvRows = db.QueryContext(
                ctx,
                &q,
                &[
                    SqlValue::from(tableName),
                    SqlValue::from(start),
                    SqlValue::from(end),
                    SqlValue::from(rowLimit),
                ],
            )?;

            let mut lastRowID = start;
            while indexKvRows.Next() {
                let (id, rawKey, indexName, rawValue, rawHandle) =
                    indexKvRows.ScanIndexConflict()?;
                lastRowID = id;
                self.logger.Debug(
                    "got raw_key, index_name, raw_value, raw_handle from table",
                    &[
                        zap::Binary("raw_key", &rawKey),
                        zap::String("index_name", &indexName),
                        zap::Binary("raw_value", &rawValue),
                        zap::Binary("raw_handle", &rawHandle),
                    ],
                );

                let latestValue = match fnGetLatest(ctx, &rawKey) {
                    Ok(v) => v,
                    Err(e) if tikverr::IsErrNotFound(&e) => continue,
                    Err(e) => return Err(errors::Trace(e)),
                };
                if rawValue == latestValue {
                    continue;
                }

                let overwritten = match fnGetLatest(ctx, &rawHandle) {
                    Ok(v) => v,
                    Err(e) if tikverr::IsErrNotFound(&e) => continue,
                    Err(e) => return Err(errors::Trace(e)),
                };

                let overwrittenHandle = tablecodec::DecodeRowKey(&rawHandle)?;
                let mut decodedData = tables::DecodeRawRowData(
                    encoder.SessionCtx.GetExprCtx(),
                    &tbl,
                    overwrittenHandle,
                    tbl.Cols(),
                    overwritten.clone(),
                )?
                .0;
                if !tbl.Meta().HasClusteredIndex() {
                    decodedData.push(types::NewIntDatum(overwrittenHandle.IntValue()));
                }
                encoder.AddRecord(decodedData)?;
                let kvPairs = encoder.SessionCtx.TakeKvPairs();
                for kvPair in kvPairs.Pairs {
                    self.logger.Debug(
                        "got encoded KV",
                        &[
                            logutil::Key("key", &kvPair.Key),
                            zap::Binary("value", &kvPair.Val),
                            logutil::Key("rawKey", &rawKey),
                            zap::Binary("rawValue", &rawValue),
                        ],
                    );
                    if kvPair.Key == rawKey && kvPair.Val == rawValue {
                        handleKeys.push(rawHandle.clone());
                        insertRows.push([rawHandle.clone(), overwritten.clone()]);
                        break;
                    }
                }
            }
            indexKvRows.Err()?;
            indexKvRows.Close()?;
            if handleKeys.is_empty() {
                return Ok(Vec::new());
            }
            fnDeleteKeys(ctx, &handleKeys)?;
            let schema = self.schema.clone();
            let task_id = self.taskID;
            exec.Transact(
                ctx,
                "insert data conflict record for conflict detection 'replace' mode",
                |c, txn| {
                    let mut sb = String::new();
                    common::FprintfWithIdentifiers(
                        &mut sb,
                        insertIntoConflictErrorData,
                        &[&schema],
                    )?;
                    let mut sqlArgs: Vec<SqlValue> = Vec::new();
                    for (i, insertRow) in insertRows.iter().enumerate() {
                        if i > 0 {
                            sb.push(',');
                        }
                        sb.push_str(sqlValuesConflictErrorData);
                        sqlArgs.push(SqlValue::from(task_id));
                        sqlArgs.push(SqlValue::from(tableName));
                        sqlArgs.push(SqlValue::Null);
                        sqlArgs.push(SqlValue::Null);
                        sqlArgs.push(SqlValue::from(insertRow[0].as_slice()));
                        sqlArgs.push(SqlValue::from(insertRow[1].as_slice()));
                        sqlArgs.push(SqlValue::from(2_i64));
                    }
                    txn.ExecContext(c, &sb, &sqlArgs)?;
                    Ok(())
                },
            )?;
            Ok(splitRemaining(lastRowID + 1, end))
        })?;

        // ---- data KV phase ----
        pool.RunDynamic([0, i64::MAX], |[start, end]| {
            let sessionOpts = encode::SessionOptions {
                SQLMode: mysql::ModeStrictAllTables,
            };
            let encoder = kv::NewBaseKVEncoderWithMap(
                &encode::EncodingConfig {
                    Table: tbl.clone(),
                    SessionOptions: sessionOpts,
                    Logger: self.logger.clone(),
                },
                self.encode_map.clone(),
            )?;

            let mut handleKeys: Vec<Vec<u8>> = Vec::new();
            let q = common::SprintfWithIdentifiers(selectDataConflictKeysReplace, &[&self.schema]);
            let mut dataKvRows = db.QueryContext(
                ctx,
                &q,
                &[
                    SqlValue::from(tableName),
                    SqlValue::from(start),
                    SqlValue::from(end),
                    SqlValue::from(rowLimit),
                ],
            )?;

            let mut lastRowID = start;
            let mut previousRawKey: Vec<u8> = Vec::new();
            // Keep presence separate from the bytes. Go uses a nil slice for
            // ErrNotFound, while an existing KV may legitimately have an empty
            // value; `Vec::is_empty` cannot distinguish those cases.
            let mut latestValue: Option<Vec<u8>> = None;
            let mut mustKeepKvPairs: Option<kv::Pairs> = None;

            while dataKvRows.Next() {
                let (id, rawKey, rawValue) = dataKvRows.ScanDataConflict()?;
                lastRowID = id;
                self.logger.Debug(
                    "got group raw_key, raw_value from table",
                    &[
                        logutil::Key("raw_key", &rawKey),
                        zap::Binary("raw_value", &rawValue),
                    ],
                );

                if rawKey != previousRawKey {
                    previousRawKey = rawKey.clone();
                    match fnGetLatest(ctx, &rawKey) {
                        Ok(v) => latestValue = Some(v),
                        Err(e) if tikverr::IsErrNotFound(&e) => latestValue = None,
                        Err(e) => return Err(errors::Trace(e)),
                    }
                    if let Some(latest_value) = &latestValue {
                        let handle = tablecodec::DecodeRowKey(&rawKey)?;
                        let mut decodedData = tables::DecodeRawRowData(
                            encoder.SessionCtx.GetExprCtx(),
                            &tbl,
                            handle,
                            tbl.Cols(),
                            latest_value.clone(),
                        )?
                        .0;
                        if !tbl.Meta().HasClusteredIndex() {
                            decodedData.push(types::NewIntDatum(handle.IntValue()));
                        }
                        encoder.AddRecord(decodedData)?;
                        mustKeepKvPairs = Some(encoder.SessionCtx.TakeKvPairs());
                    }
                    // Go only replaces mustKeepKvPairs when the latest row exists;
                    // ErrNotFound therefore keeps the previous row's protection set.
                }

                // `bytes.Equal(empty, nil)` is true in Go, so a missing latest
                // value still compares equal to an empty recorded value.
                if rawValue.as_slice() == latestValue.as_deref().unwrap_or_default() {
                    continue;
                }

                let handle = tablecodec::DecodeRowKey(&rawKey)?;
                let mut decodedData = tables::DecodeRawRowData(
                    encoder.SessionCtx.GetExprCtx(),
                    &tbl,
                    handle,
                    tbl.Cols(),
                    rawValue.clone(),
                )?
                .0;
                if !tbl.Meta().HasClusteredIndex() {
                    decodedData.push(types::NewIntDatum(handle.IntValue()));
                }
                encoder.AddRecord(decodedData)?;
                let kvPairs = encoder.SessionCtx.TakeKvPairs();
                for kvPair in kvPairs.Pairs {
                    self.logger.Debug(
                        "got encoded KV",
                        &[
                            logutil::Key("key", &kvPair.Key),
                            zap::Binary("value", &kvPair.Val),
                        ],
                    );
                    let kvLatestValue = match fnGetLatest(ctx, &kvPair.Key) {
                        Ok(v) => v,
                        Err(e) if tikverr::IsErrNotFound(&e) => continue,
                        Err(e) => return Err(errors::Trace(e)),
                    };
                    if kvLatestValue != kvPair.Val {
                        continue;
                    }
                    if let Some(ref keep) = mustKeepKvPairs {
                        let is_contained = keep
                            .Pairs
                            .iter()
                            .any(|p| p.Key == kvPair.Key && p.Val == kvPair.Val);
                        if is_contained {
                            continue;
                        }
                    }
                    handleKeys.push(kvPair.Key);
                }
            }
            dataKvRows.Err()?;
            dataKvRows.Close()?;
            if handleKeys.is_empty() {
                return Ok(Vec::new());
            }
            fnDeleteKeys(ctx, &handleKeys)?;
            Ok(splitRemaining(lastRowID + 1, end))
        })?;

        // delete the additionally inserted rows for nonclustered PK
        loop {
            let affected = std::cell::Cell::new(0_i64);
            exec.Transact(
                ctx,
                "delete additionally inserted rows for conflict detection 'replace' mode",
                |c, txn| {
                    let mut sb = String::new();
                    common::FprintfWithIdentifiers(&mut sb, deleteNullDataRow, &[&self.schema])?;
                    let result = txn.ExecContext(c, &sb, &[SqlValue::from(rowLimit)])?;
                    affected.set(result.RowsAffected()?);
                    Ok(())
                },
            )?;
            if affected.get() == 0 {
                break;
            }
        }

        Ok(())
    }

    // 自动补充的`RecordDuplicateCount` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordDuplicateCount reduce the counter of "duplicate entry" errors.
    pub fn RecordDuplicateCount(&self, cnt: i64) -> Result<()> {
        if self.conflictErrRemain.Sub(cnt) < 0 {
            let threshold = self.configConflict.Threshold;
            return Err(errors::Errorf(format!(
                "The number of conflict errors exceeds the threshold configured by `conflict.threshold`: '{threshold}'"
            )));
        }
        Ok(())
    }

    // 自动补充的`RecordDuplicate` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordDuplicate records a "duplicate entry" error so user can query them later.
    pub fn RecordDuplicate(
        &self,
        ctx: context::Context,
        logger: log::Logger,
        tableName: &str,
        path: &str,
        offset: i64,
        errMsg: &str,
        rowID: i64,
        rowData: &str,
    ) -> Result<()> {
        if self.conflictErrRemain.Dec() < 0 {
            let threshold = self.configConflict.Threshold;
            return Err(errors::Errorf(format!(
                "The number of conflict errors exceeds the threshold configured by `conflict.threshold`: '{threshold}'"
            )));
        }
        if self.db.is_none() {
            return Ok(());
        }
        if self.conflictRecordsRemain.Add(-1) < 0 {
            return Ok(());
        }

        self.recordDuplicate(ctx, logger, tableName, path, offset, errMsg, rowID, rowData)
    }

    // 自动补充的`recordDuplicate` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn recordDuplicate(
        &self,
        ctx: context::Context,
        logger: log::Logger,
        tableName: &str,
        path: &str,
        offset: i64,
        errMsg: &str,
        rowID: i64,
        rowData: &str,
    ) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        let exec = common::SQLWithRetry {
            DB: db.clone(),
            Logger: logger,
            HideQueryLog: redact::NeedRedact(),
        };
        let q = common::SprintfWithIdentifiers(insertIntoDupRecord, &[&self.schema]);
        exec.Exec(
            ctx,
            "insert duplicate record",
            q,
            &[
                SqlValue::from(self.taskID),
                SqlValue::from(tableName),
                SqlValue::from(path),
                SqlValue::from(offset),
                SqlValue::from(errMsg),
                SqlValue::from(rowID),
                SqlValue::from(rowData),
            ],
        )
    }

    // 自动补充的`RecordDuplicateOnce` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// RecordDuplicateOnce records a "duplicate entry" error so user can query them later.
    /// Different from RecordDuplicate, this function is used when conflict.strategy
    /// is "error" and will only write the first conflict error to the table.
    pub fn RecordDuplicateOnce(
        &self,
        ctx: context::Context,
        logger: log::Logger,
        tableName: &str,
        path: &str,
        offset: i64,
        errMsg: &str,
        rowID: i64,
        rowData: &str,
    ) {
        let ok = self.recordErrorOnce.CompareAndSwap(false, true);
        if !ok {
            return;
        }
        if let Err(err) = self.recordDuplicate(
            ctx,
            logger.clone(),
            tableName,
            path,
            offset,
            errMsg,
            rowID,
            rowData,
        ) {
            logger.Warn(format!(
                "meet error when record duplicate entry error: {err}"
            ));
            let _ = zap::Error(&err);
        }
    }

    // 自动补充的`errorCount` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn errorCount<F>(&self, typeVal: F) -> i64
    where
        F: Fn(&config::MaxError) -> i64,
    {
        let cfgVal = typeVal(&self.configError);
        let val = typeVal(&self.remainingError).max(0);
        cfgVal - val
    }

    // 自动补充的`typeErrors` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn typeErrors(&self) -> i64 {
        self.errorCount(|maxError| maxError.Type.Load())
    }

    // 自动补充的`syntaxError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn syntaxError(&self) -> i64 {
        self.errorCount(|maxError| maxError.Syntax.Load())
    }

    // 自动补充的`conflictError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn conflictError(&self) -> i64 {
        let val = self.conflictErrRemain.Load().max(0);
        self.configConflict.Threshold - val
    }

    // 自动补充的`charsetError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn charsetError(&self) -> i64 {
        self.errorCount(|maxError| maxError.Charset.Load())
    }

    // 自动补充的`HasError` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// HasError returns true if any error type has reached the limit
    pub fn HasError(&self) -> bool {
        self.typeErrors() > 0
            || self.syntaxError() > 0
            || self.charsetError() > 0
            || self.conflictError() > 0
    }

    // 自动补充的`LogErrorDetails` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// LogErrorDetails logs details for each error type.
    pub fn LogErrorDetails(&self) {
        let fmtErrMsg = |cnt: i64, errType: &str, tblName: &str| {
            format!(
                "Detect {cnt} {errType} errors in total, please refer to table {} for more details",
                self.fmtTableName(tblName)
            )
        };
        if let errCnt @ 1.. = self.typeErrors() {
            self.logger
                .Warn(fmtErrMsg(errCnt, "data type", typeErrorTableName));
        }
        if let errCnt @ 1.. = self.syntaxError() {
            self.logger
                .Warn(fmtErrMsg(errCnt, "data syntax", syntaxErrorTableName));
        }
        if let errCnt @ 1.. = self.charsetError() {
            // TODO: add charset table name
            self.logger.Warn(fmtErrMsg(errCnt, "data charset", ""));
        }
        let errCnt = self.conflictError();
        if errCnt > 0 && (self.conflictV1Enabled || self.conflictV2Enabled) {
            self.logger
                .Warn(fmtErrMsg(errCnt, "conflict", ConflictViewName));
        }
    }

    // 自动补充的`fmtTableName` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn fmtTableName(&self, t: &str) -> String {
        common::UniqueTable(&self.schema, t)
    }

    // 自动补充的`Output` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// Output renders a table which contains error summery for each error type.
    pub fn Output(&self) -> String {
        if !self.HasError() {
            return String::new();
        }

        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut count = 0i64;
        if let errCnt @ 1.. = self.typeErrors() {
            count += 1;
            rows.push(vec![
                count.to_string(),
                "Data Type".into(),
                errCnt.to_string(),
                self.fmtTableName(typeErrorTableName),
            ]);
        }
        if let errCnt @ 1.. = self.syntaxError() {
            count += 1;
            rows.push(vec![
                count.to_string(),
                "Data Syntax".into(),
                errCnt.to_string(),
                self.fmtTableName(syntaxErrorTableName),
            ]);
        }
        if let errCnt @ 1.. = self.charsetError() {
            count += 1;
            // do not support record charset error now.
            rows.push(vec![
                count.to_string(),
                "Charset Error".into(),
                errCnt.to_string(),
                String::new(),
            ]);
        }
        if let errCnt @ 1.. = self.conflictError() {
            count += 1;
            if self.conflictV1Enabled || self.conflictV2Enabled {
                rows.push(vec![
                    count.to_string(),
                    "Unique Key Conflict".into(),
                    errCnt.to_string(),
                    self.fmtTableName(ConflictViewName),
                ]);
            }
        }

        let mut res = "\nImport Data Error Summary: \n".to_string();
        res.push_str(&pretty_table::render(
            &["#", "Error Type", "Error Count", "Error Data Table"],
            &rows,
        ));
        res
    }
}

// 自动补充的`New` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
/// New creates a new error manager.
pub fn New(db: Option<sql::DB>, cfg: &config::Config, logger: log::Logger) -> ErrorManager {
    let conflictErrRemain = atomic::NewInt64(cfg.Conflict.Threshold);
    let conflictRecordsRemain = atomic::NewInt64(cfg.Conflict.MaxRecordRows);
    let mut em = ErrorManager {
        db: None,
        taskID: cfg.TaskID,
        schema: String::new(),
        configError: cfg.App.MaxError.clone(),
        remainingError: cfg.App.MaxError.clone(),
        configConflict: cfg.Conflict.clone(),
        conflictErrRemain,
        conflictRecordsRemain,
        conflictV1Enabled: cfg.TikvImporter.Backend == config::BackendLocal
            && cfg.Conflict.Strategy != config::NoneOnDup,
        conflictV2Enabled: false,
        logger,
        recordErrorOnce: atomic::NewBool(false),
        encode_map: Arc::new(Mutex::new(Vec::new())),
    };
    match cfg.TikvImporter.Backend.as_str() {
        config::BackendLocal => {
            if cfg.Conflict.PrecheckConflictBeforeImport
                && cfg.Conflict.Strategy != config::NoneOnDup
            {
                em.conflictV2Enabled = true;
            }
        }
        config::BackendTiDB => {
            em.conflictV2Enabled = true;
        }
        _ => {}
    }
    if !cfg.App.TaskInfoSchemaName.is_empty() {
        em.db = db;
        em.schema = cfg.App.TaskInfoSchemaName.clone();
    }
    em
}

/// DataConflictInfo is the information of a data conflict error.
#[derive(Clone, Debug, Default)]
// 自动补充的`DataConflictInfo` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct DataConflictInfo {
    pub RawKey: Vec<u8>,
    pub RawValue: Vec<u8>,
    pub KeyData: String,
    pub Row: String,
}
