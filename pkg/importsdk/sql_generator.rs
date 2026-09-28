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

// IMPORT INTO SQL 生成器。
//
// 根据表元数据（库名、表名、通配路径）与导入选项，拼接 TiDB `IMPORT INTO ... FROM ...`
// 语句；CSV 专用选项仅在 Format 为 `"csv"` 时附加。

use crate::{CSVConfig, ErrMultipleFieldsDefinedNullBy, ImportOptions, TableMeta};
use astersql_errors as errors;

/// 从表元数据与导入选项生成 `IMPORT INTO` SQL 的 trait。
// SQLGenerator 对应 Go 接口，约束调用方从表元数据和导入选项生成 SQL。
pub trait SQLGenerator: Send + Sync {
    fn GenerateImportSQL(
        &self,
        tableMeta: &TableMeta,
        options: &ImportOptions,
    ) -> Result<String, errors::SharedError>;
    fn GenerateImportSQLParts(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
    ) -> (String, Option<errors::SharedError>) {
        match self.GenerateImportSQL(table_meta, options) {
            Ok(sql) => (sql, None),
            Err(error) => (String::new(), Some(error)),
        }
    }
}

/// 无状态 SQL 生成器实现。
// sqlGenerator 对应 Go 的无状态实现；生成过程不保存共享状态。
pub struct sqlGenerator;

/// 创建 SQL 生成器，返回 trait 对象以隐藏具体类型。
// NewSQLGenerator 对应 Go 构造函数，返回接口对象以隐藏具体实现。
pub fn NewSQLGenerator() -> Box<dyn SQLGenerator> {
    Box::new(sqlGenerator)
}

impl SQLGenerator for sqlGenerator {
    // GenerateImportSQL 按 Go 顺序拼接库表、数据路径、格式与 WITH 选项。
    fn GenerateImportSQL(
        &self,
        tableMeta: &TableMeta,
        options: &ImportOptions,
    ) -> Result<String, errors::SharedError> {
        let mut sb = String::from("IMPORT INTO ");
        sb.push_str(&escapeIdentifier(&tableMeta.Database));
        sb.push('.');
        sb.push_str(&escapeIdentifier(&tableMeta.Table));

        let mut path = tableMeta.WildcardPath.clone();
        if !options.ResourceParameters.is_empty() {
            // Go 只在 URL 解析成功时附加资源参数；解析失败不会中止 SQL 生成。
            if let Ok(mut parsed) = url::Url::parse(&path) {
                let query = match parsed.query() {
                    Some(raw) if !raw.is_empty() => {
                        format!("{}&{}", raw, options.ResourceParameters)
                    }
                    _ => options.ResourceParameters.clone(),
                };
                parsed.set_query(Some(&query));
                path = parsed.to_string();
            } else if isValidRelativeURL(&path) {
                // `net/url.Parse` also accepts relative and scheme-relative references,
                // while `url::Url::parse` intentionally accepts absolute URLs only.
                let fragmentOffset = path.find('#').unwrap_or(path.len());
                let queryOffset = path[..fragmentOffset].find('?');
                let insertOffset = match queryOffset {
                    Some(offset) if offset + 1 < fragmentOffset => {
                        path.insert(fragmentOffset, '&');
                        fragmentOffset + 1
                    }
                    Some(_) => fragmentOffset,
                    None => {
                        path.insert(fragmentOffset, '?');
                        fragmentOffset + 1
                    }
                };
                path.insert_str(insertOffset, &options.ResourceParameters);
            }
        }

        sb.push_str(" FROM '");
        sb.push_str(&path);
        sb.push('\'');

        if !options.Format.is_empty() {
            sb.push_str(" FORMAT '");
            sb.push_str(&options.Format);
            sb.push('\'');
        }

        // 选项构造可能因 CSV 空值定义不合法而失败，沿用 Go 的立即返回语义。
        let opts = self.buildOptions(options)?;
        if !opts.is_empty() {
            sb.push_str(" WITH ");
            sb.push_str(&opts.join(", "));
        }
        Ok(sb)
    }
}

impl sqlGenerator {
    // buildOptions 对应 Go 的通用选项收集器，仅输出显式启用或非零的字段。
    fn buildOptions(&self, options: &ImportOptions) -> Result<Vec<String>, errors::SharedError> {
        let mut opts = Vec::new();
        if options.Thread > 0 {
            opts.push(format!("THREAD={}", options.Thread));
        }
        if !options.DiskQuota.is_empty() {
            opts.push(format!("DISK_QUOTA='{}'", options.DiskQuota));
        }
        if !options.MaxWriteSpeed.is_empty() {
            opts.push(format!("MAX_WRITE_SPEED='{}'", options.MaxWriteSpeed));
        }
        if options.SplitFile {
            opts.push("SPLIT_FILE".to_owned());
        }
        if options.RecordErrors > 0 {
            opts.push(format!("RECORD_ERRORS={}", options.RecordErrors));
        }
        if options.Detached {
            opts.push("DETACHED".to_owned());
        }
        if !options.CloudStorageURI.is_empty() {
            opts.push(format!("CLOUD_STORAGE_URI='{}'", options.CloudStorageURI));
        }
        if !options.GroupKey.is_empty() {
            opts.push(format!("GROUP_KEY='{}'", escapeString(&options.GroupKey)));
        }
        if options.SkipRows > 0 {
            opts.push(format!("SKIP_ROWS={}", options.SkipRows));
        }
        if !options.CharacterSet.is_empty() {
            opts.push(format!(
                "CHARACTER_SET='{}'",
                escapeString(&options.CharacterSet)
            ));
        }
        if !options.ChecksumTable.is_empty() {
            opts.push(format!(
                "CHECKSUM_TABLE='{}'",
                escapeString(&options.ChecksumTable)
            ));
        }
        if options.DisableTiKVImportMode {
            opts.push("DISABLE_TIKV_IMPORT_MODE".to_owned());
        }
        if options.DisablePrecheck {
            opts.push("DISABLE_PRECHECK".to_owned());
        }

        // CSV 专用配置只在 Format 精确为 "csv" 时生效，保持 Go 的大小写行为。
        if let Some(csvConfig) = options
            .CSVConfig
            .as_ref()
            .filter(|_| options.Format == "csv")
        {
            opts.extend(self.buildCSVOptions(csvConfig)?);
        }
        Ok(opts)
    }

    // buildCSVOptions 对应 Go 的 CSV 分隔符、包围符、转义符及空值配置翻译。
    fn buildCSVOptions(&self, csvConfig: &CSVConfig) -> Result<Vec<String>, errors::SharedError> {
        let mut opts = Vec::new();
        if !csvConfig.FieldsTerminatedBy.is_empty() {
            opts.push(format!(
                "FIELDS_TERMINATED_BY='{}'",
                escapeString(&csvConfig.FieldsTerminatedBy)
            ));
        }
        if !csvConfig.FieldsEnclosedBy.is_empty() {
            opts.push(format!(
                "FIELDS_ENCLOSED_BY='{}'",
                escapeString(&csvConfig.FieldsEnclosedBy)
            ));
        }
        if !csvConfig.FieldsEscapedBy.is_empty() {
            opts.push(format!(
                "FIELDS_ESCAPED_BY='{}'",
                escapeString(&csvConfig.FieldsEscapedBy)
            ));
        }
        if !csvConfig.LinesTerminatedBy.is_empty() {
            opts.push(format!(
                "LINES_TERMINATED_BY='{}'",
                escapeString(&csvConfig.LinesTerminatedBy)
            ));
        }
        if !csvConfig.FieldNullDefinedBy.is_empty() {
            // Go 仅接受一个空值标记；多个标记返回包级哨兵错误。
            if csvConfig.FieldNullDefinedBy.len() > 1 {
                return Err((*ErrMultipleFieldsDefinedNullBy).clone());
            }
            opts.push(format!(
                "FIELDS_DEFINED_NULL_BY='{}'",
                escapeString(&csvConfig.FieldNullDefinedBy[0])
            ));
        }
        Ok(opts)
    }
}

// escapeIdentifier 对应 Go 标识符转义：反引号包裹并把内部反引号加倍。
fn escapeIdentifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

// escapeString 对应 Go SQL 字符串转义，先处理反斜杠，再处理单引号。
fn escapeString(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}

// Go's `net/url.Parse` accepts relative references but rejects control bytes,
// malformed percent escapes, and a colon in the first relative path segment.
fn isValidRelativeURL(value: &str) -> bool {
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return false;
    }

    let bytes = value.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            if offset + 2 >= bytes.len()
                || !bytes[offset + 1].is_ascii_hexdigit()
                || !bytes[offset + 2].is_ascii_hexdigit()
            {
                return false;
            }
            offset += 3;
        } else {
            offset += 1;
        }
    }

    let pathEnd = value.find(['?', '#']).unwrap_or(value.len());
    let firstSegmentEnd = value[..pathEnd].find('/').unwrap_or(pathEnd);
    value.starts_with("//") || !value[..firstSegmentEnd].contains(':')
}
