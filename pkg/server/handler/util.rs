// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Status/TiKV HTTP handler 共用工具：路径参数名、查询串键、响应写出与表名解析。
//
// 常量名对齐 Go `handler` 包中的路由变量与 query 字段，便于对照迁移。

/// 路径变量：数据库名。
pub const DB_NAME: &str = "db";
/// 路径变量：十六进制编码的 key。
pub const HEX_KEY: &str = "hexKey";
/// 路径变量：索引名。
pub const INDEX_NAME: &str = "index";
/// 路径变量：行 handle（行标识，聚簇/非聚簇主键或 _tidb_rowid）。
pub const HANDLE: &str = "handle";
/// 路径变量：Region ID（Region 是 TiKV 数据分片单位）。
pub const REGION_ID: &str = "regionID";
/// 路径变量：事务 startTS（事务开始时间戳，用于 MVCC 版本定位）。
pub const START_TS: &str = "startTS";
/// 路径变量：表名。
pub const TABLE_NAME: &str = "table";
/// 路径变量：表 ID。
pub const TABLE_ID: &str = "tableID";
/// 路径变量：列 ID。
pub const COLUMN_ID: &str = "colID";
/// 路径变量：列类型编码。
pub const COLUMN_TP: &str = "colTp";
/// 路径变量：列 flag。
pub const COLUMN_FLAG: &str = "colFlag";
/// 路径变量：列长度。
pub const COLUMN_LEN: &str = "colLen";
/// 查询参数：base64 编码的行二进制。
pub const ROW_BIN: &str = "rowBin";
/// 路径变量：统计信息快照标识。
pub const SNAPSHOT: &str = "snapshot";
/// 路径变量：导出文件名。
pub const FILE_NAME: &str = "filename";
/// 查询参数：是否导出分区统计。
pub const DUMP_PARTITION_STATS: &str = "dumpPartitionStats";
/// 查询参数：区间起点。
pub const BEGIN: &str = "begin";
/// 查询参数：区间终点。
pub const END: &str = "end";

// For extract task handler.
/// Extract 任务类型。
pub const TYPE: &str = "type";
/// 是否执行 dump。
pub const IS_DUMP: &str = "isDump";
/// 是否跳过统计信息。
pub const IS_SKIP_STATS: &str = "isSkipStats";
/// 是否历史视图。
pub const IS_HISTORY_VIEW: &str = "isHistoryView";

// For query string.
/// 查询串：单个 table_id。
pub const TABLE_ID_QUERY: &str = "table_id";
/// 查询串：多个 table_id。
pub const TABLE_IDS_QUERY: &str = "table_ids";
/// 查询串：仅返回 id/name。
pub const ID_NAME_ONLY: &str = "id_name_only";
/// 查询串：结果条数上限。
pub const LIMIT: &str = "limit";
/// 查询串：DDL/任务起始 job id。
pub const JOB_ID: &str = "start_job_id";
/// 路径/查询：操作名（如 upgrade 的 start/finish/show）。
pub const OPERATION: &str = "op";
/// 查询串：秒数。
pub const SECONDS: &str = "seconds";

/// HTTP 响应头 Content-Type。
pub const HEADER_CONTENT_TYPE: &str = "Content-Type";
/// JSON 媒体类型。
pub const CONTENT_TYPE_JSON: &str = "application/json";
/// HTTP 400 Bad Request。
pub const STATUS_BAD_REQUEST: u16 = 400;
/// HTTP 200 OK。
pub const STATUS_OK: u16 = 200;

// Go-compatible exported spellings retained for callers that use the package
// names directly; the snake-case constants above are idiomatic Rust aliases.
#[allow(non_upper_case_globals)]
pub const DBName: &str = DB_NAME;
#[allow(non_upper_case_globals)]
pub const HexKey: &str = HEX_KEY;
#[allow(non_upper_case_globals)]
pub const IndexName: &str = INDEX_NAME;
#[allow(non_upper_case_globals)]
pub const Handle: &str = HANDLE;
#[allow(non_upper_case_globals)]
pub const RegionID: &str = REGION_ID;
#[allow(non_upper_case_globals)]
pub const StartTS: &str = START_TS;
#[allow(non_upper_case_globals)]
pub const TableName: &str = TABLE_NAME;
#[allow(non_upper_case_globals)]
pub const TableID: &str = TABLE_ID;
#[allow(non_upper_case_globals)]
pub const ColumnID: &str = COLUMN_ID;
#[allow(non_upper_case_globals)]
pub const ColumnTp: &str = COLUMN_TP;
#[allow(non_upper_case_globals)]
pub const ColumnFlag: &str = COLUMN_FLAG;
#[allow(non_upper_case_globals)]
pub const ColumnLen: &str = COLUMN_LEN;
#[allow(non_upper_case_globals)]
pub const RowBin: &str = ROW_BIN;
#[allow(non_upper_case_globals)]
pub const Snapshot: &str = SNAPSHOT;
#[allow(non_upper_case_globals)]
pub const FileName: &str = FILE_NAME;
#[allow(non_upper_case_globals)]
pub const DumpPartitionStats: &str = DUMP_PARTITION_STATS;
#[allow(non_upper_case_globals)]
pub const Begin: &str = BEGIN;
#[allow(non_upper_case_globals)]
pub const End: &str = END;
#[allow(non_upper_case_globals)]
pub const Type: &str = TYPE;
#[allow(non_upper_case_globals)]
pub const IsDump: &str = IS_DUMP;
#[allow(non_upper_case_globals)]
pub const IsSkipStats: &str = IS_SKIP_STATS;
#[allow(non_upper_case_globals)]
pub const IsHistoryView: &str = IS_HISTORY_VIEW;
#[allow(non_upper_case_globals)]
pub const TableIDQuery: &str = TABLE_ID_QUERY;
#[allow(non_upper_case_globals)]
pub const TableIDsQuery: &str = TABLE_IDS_QUERY;
#[allow(non_upper_case_globals)]
pub const IDNameOnly: &str = ID_NAME_ONLY;
#[allow(non_upper_case_globals)]
pub const Limit: &str = LIMIT;
#[allow(non_upper_case_globals)]
pub const JobID: &str = JOB_ID;
#[allow(non_upper_case_globals)]
pub const Operation: &str = OPERATION;
#[allow(non_upper_case_globals)]
pub const Seconds: &str = SECONDS;
#[allow(non_upper_case_globals)]
pub const HeaderContentType: &str = HEADER_CONTENT_TYPE;
#[allow(non_upper_case_globals)]
pub const ContentTypeJSON: &str = CONTENT_TYPE_JSON;

/// WriteError 对应 Go 的默认 400 错误写出。
pub fn WriteError(w: &mut ResponseWriter, err: Error) {
    WriteErrorWithCode(w, STATUS_BAD_REQUEST, err);
}

/// WriteErrorWithCode 先写 HTTP 状态码，再写错误字符串，并保留 terror.Log 记录语义。
pub fn WriteErrorWithCode(w: &mut ResponseWriter, status_code: u16, err: Error) {
    w.write_header(status_code);
    if let Err(write_err) = w.write(err.message.as_bytes()) {
        terror_log(write_err);
    }
    terror_log(err);
}

/// WriteData 对应 Go 的 json.MarshalIndent + application/json 响应。
pub fn WriteData<T: JsonValue>(w: &mut ResponseWriter, data: T) {
    let js = match json_marshal_indent(data, "", " ") {
        Ok(v) => v,
        Err(err) => {
            WriteError(w, err);
            return;
        }
    };
    // 写 body 前设置 Content-Type；保持 Go handler 的响应顺序。
    w.header_set(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON);
    w.write_header(STATUS_OK);
    if let Err(err) = w.write(js.as_bytes()) {
        terror_log(err);
    }
}

/// ExtractTableAndPartitionName 从 `table(partition)` 形态提取表名和分区名。
pub fn ExtractTableAndPartitionName(input: &str) -> (String, String) {
    let Some(start) = input.find('(') else {
        return (input.to_owned(), String::new());
    };
    let Some(end) = input.find(')') else {
        return (input.to_owned(), String::new());
    };
    // Go 假设表名/分区名不会包含 '('；这里保留相同的简单切分策略。
    (input[..start].to_owned(), input[start + 1..end].to_owned())
}

/// 可被 `WriteData` 序列化的 JSON 值。
pub trait JsonValue {
    fn to_json(&self) -> String;
}
impl JsonValue for String {
    fn to_json(&self) -> String {
        json_string(self)
    }
}
impl JsonValue for &str {
    fn to_json(&self) -> String {
        json_string(self)
    }
}

/// HTTP 响应写出器占位类型。
#[derive(Default)]
pub struct ResponseWriter {
    pub(crate) status: Option<u16>,
    pub(crate) content_type: Option<String>,
    pub(crate) body: Vec<u8>,
}
/// 可写出的错误，携带消息字符串。
#[derive(Debug)]
pub struct Error {
    /// 错误文案。
    pub message: String,
}
impl ResponseWriter {
    /// 写出 HTTP 状态码。
    fn write_header(&mut self, status: u16) {
        self.status = Some(status);
    }
    /// 写出响应 body。
    fn write(&mut self, body: &[u8]) -> Result<(), Error> {
        self.body.extend_from_slice(body);
        Ok(())
    }
    /// 设置响应头。
    fn header_set(&mut self, name: &str, value: &str) {
        if name == HEADER_CONTENT_TYPE {
            self.content_type = Some(value.to_owned());
        }
    }

    /// 已写入的 HTTP 状态码，供 status server 适配不同 handler writer 时读取。
    pub fn status_code(&self) -> Option<u16> {
        self.status
    }

    /// 已写入的响应正文，供 status server 适配不同 handler writer 时读取。
    pub fn body_bytes(&self) -> &[u8] {
        &self.body
    }
}
impl Error {
    /// 由消息构造错误。
    pub(crate) fn new<T: Into<String>>(message: T) -> Error {
        Error {
            message: message.into(),
        }
    }
}

/// JSON 字符串序列化；handler 当前写出的基础值均为字符串。
fn json_marshal_indent<T: JsonValue>(value: T, _: &str, _: &str) -> Result<String, Error> {
    Ok(value.to_json())
}

pub(crate) fn json_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c.is_control() => output.push_str(&format!("\\u{:04x}", c as u32)),
            c => output.push(c),
        }
    }
    output.push('"');
    output
}
/// 对齐 Go `terror.Log` 的错误记录占位。
fn terror_log(_: Error) {}
