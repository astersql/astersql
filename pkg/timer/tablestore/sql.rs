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

// Timer 表存储的 SQL 拼装与 TIMER_EXT JSON 编解码。
//
// 将 `TimerRecord` / 查询条件 / 更新字段转换为带 `%?` 占位符的 SQL 与 `SqlArg` 参数列表；
// `TIMER_EXT` 列以 JSON 存放 tags、手动触发请求与事件扩展信息。

use astersql_timer_api as api;
use std::collections::BTreeMap;
use std::time::Duration;

/// SQL 绑定参数的类型化表示，对应会话层 `SqlValue`。
#[derive(Clone, Debug, PartialEq)]
pub enum SqlArg {
    Null,
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    I64(i64),
    U64(u64),
    Json(String),
}

/// `TIMER_EXT` JSON 的内存结构：标签、手动请求与事件扩展。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimerExt {
    /// 定时器标签列表，用于按 tag 过滤。
    pub tags: Vec<String>,
    /// 手动触发请求（若有）。
    pub manual: Option<ManualRequestObj>,
    /// 当前事件的扩展字段（若有）。
    pub event: Option<EventExtObj>,
}

/// 手动触发请求在 JSON 中的字段布局（可选字段用 `Option`）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ManualRequestObj {
    pub request_id: Option<String>,
    pub request_time_unix: Option<i64>,
    pub timeout_sec: Option<i64>,
    pub processed: Option<bool>,
    pub event_id: Option<String>,
}

/// 事件扩展：关联的手动请求 ID 与水位线（watermark，已处理进度时间戳）Unix 秒。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventExtObj {
    pub manual_request_id: Option<String>,
    pub watermark_unix: Option<i64>,
}

/// 生成反引号包裹的 `` `db`.`table` `` 标识符。
pub fn indentString(db_name: &str, table_name: &str) -> String {
    format!("`{db_name}`.`{table_name}`")
}

/// 将 API 层 `ManualRequest` 转为 JSON 对象；全默认值时返回 `None`（存 null）。
pub fn newManualRequestObj(manual: &api::ManualRequest) -> Option<ManualRequestObj> {
    if manual == &api::ManualRequest::default() {
        return None;
    }
    Some(ManualRequestObj {
        request_id: (!manual.ManualRequestID.is_empty()).then(|| manual.ManualRequestID.clone()),
        request_time_unix: manual.ManualRequestTime.map(|value| value.timestamp()),
        timeout_sec: (manual.ManualTimeout != Duration::ZERO)
            .then(|| manual.ManualTimeout.as_secs() as i64),
        processed: manual.ManualProcessed.then_some(true),
        event_id: (!manual.ManualEventID.is_empty()).then(|| manual.ManualEventID.clone()),
    })
}

impl ManualRequestObj {
    /// 从 JSON 对象还原为 API 层 `ManualRequest`。
    pub fn ToManualRequest(&self) -> api::ManualRequest {
        api::ManualRequest {
            ManualRequestID: self.request_id.clone().unwrap_or_default(),
            ManualRequestTime: self.request_time_unix.and_then(timestamp_from_unix),
            ManualTimeout: self
                .timeout_sec
                .and_then(|value| u64::try_from(value).ok())
                .map(Duration::from_secs)
                .unwrap_or_default(),
            ManualProcessed: self.processed.unwrap_or_default(),
            ManualEventID: self.event_id.clone().unwrap_or_default(),
        }
    }
}

/// 将 API 层 `EventExtra` 转为 JSON 对象；全默认值时返回 `None`。
pub fn newEventExtObj(event: &api::EventExtra) -> Option<EventExtObj> {
    if event == &api::EventExtra::default() {
        return None;
    }
    Some(EventExtObj {
        manual_request_id: (!event.EventManualRequestID.is_empty())
            .then(|| event.EventManualRequestID.clone()),
        watermark_unix: event.EventWatermark.map(|value| value.timestamp()),
    })
}

impl EventExtObj {
    /// 从 JSON 对象还原为 API 层 `EventExtra`。
    pub fn ToEventExtra(&self) -> api::EventExtra {
        api::EventExtra {
            EventManualRequestID: self.manual_request_id.clone().unwrap_or_default(),
            EventWatermark: self.watermark_unix.and_then(timestamp_from_unix),
        }
    }
}

/// 拼装 INSERT 语句：列顺序与 Go 版一致，时间列用 `FROM_UNIXTIME` 或 NULL 占位。
pub fn buildInsertTimerSQL(
    db_name: &str,
    table_name: &str,
    record: &api::TimerRecord,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    let (watermark_format, watermark) = timestamp_argument(record.Watermark.as_ref());
    let (event_start_format, event_start) = timestamp_argument(record.EventStart.as_ref());
    // 空状态默认 IDLE，与调度器空闲语义一致。
    let event_status = if record.EventStatus.is_empty() {
        api::SchedEventIdle
    } else {
        &record.EventStatus
    };
    let ext = TimerExt {
        tags: record.Tags.clone(),
        manual: newManualRequestObj(&record.ManualRequest),
        event: newEventExtObj(&record.EventExtra),
    };
    let ext = encode_timer_ext(&ext);
    let sql = format!(
        "INSERT INTO {} (NAMESPACE, TIMER_KEY, TIMER_DATA, TIMEZONE, SCHED_POLICY_TYPE, SCHED_POLICY_EXPR, HOOK_CLASS, WATERMARK, ENABLE, TIMER_EXT, EVENT_ID, EVENT_STATUS, EVENT_START, EVENT_DATA, SUMMARY_DATA, VERSION) VALUES (%?, %?, %?, %?, %?, %?, %?, {}, %?, JSON_MERGE_PATCH('{{}}', %?), %?, %?, {}, %?, %?, 1)",
        indentString(db_name, table_name),
        watermark_format,
        event_start_format
    );
    Ok((
        sql,
        vec![
            SqlArg::String(record.Namespace.clone()),
            SqlArg::String(record.Key.clone()),
            SqlArg::Bytes(record.Data.clone()),
            SqlArg::String(record.TimeZone.clone()),
            SqlArg::String(record.SchedPolicyType.clone()),
            SqlArg::String(record.SchedPolicyExpr.clone()),
            SqlArg::String(record.HookClass.clone()),
            watermark,
            SqlArg::Bool(record.Enable),
            SqlArg::Json(ext),
            SqlArg::String(record.EventID.clone()),
            SqlArg::String(event_status.to_string()),
            event_start,
            SqlArg::Bytes(record.EventData.clone()),
            SqlArg::Bytes(record.SummaryData.clone()),
        ],
    ))
}

/// 有时间戳则用 `FROM_UNIXTIME(%?)`，否则占位符绑定 NULL。
fn timestamp_argument(value: Option<&api::Timestamp>) -> (&'static str, SqlArg) {
    match value {
        Some(value) => ("FROM_UNIXTIME(%?)", SqlArg::I64(value.timestamp())),
        None => ("%?", SqlArg::Null),
    }
}

/// 拼装 SELECT 全列查询，WHERE 由 `buildCondCriteria` 生成。
pub fn buildSelectTimerSQL(
    db_name: &str,
    table_name: &str,
    cond: Option<&dyn api::Cond>,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    let (criteria, args) = buildCondCriteria(cond, Vec::with_capacity(8))?;
    Ok((
        format!(
            "SELECT ID, NAMESPACE, TIMER_KEY, TIMER_DATA, TIMEZONE, SCHED_POLICY_TYPE, SCHED_POLICY_EXPR, HOOK_CLASS, WATERMARK, ENABLE, TIMER_EXT, EVENT_STATUS, EVENT_ID, EVENT_DATA, EVENT_START, SUMMARY_DATA, CREATE_TIME, UPDATE_TIME, VERSION FROM {} WHERE {}",
            indentString(db_name, table_name),
            criteria
        ),
        args,
    ))
}

/// 将 `Cond` 树转为 SQL 谓词；无条件时返回恒真 `"1"`。
pub fn buildCondCriteria(
    cond: Option<&dyn api::Cond>,
    args: Vec<SqlArg>,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    let Some(cond) = cond else {
        return Ok(("1".to_string(), args));
    };
    if let Some(cond) = cond.as_any().downcast_ref::<api::TimerCond>() {
        return buildTimerCondCriteria(cond, args);
    }
    if let Some(operator) = cond.as_any().downcast_ref::<api::Operator>() {
        return buildOperatorCriteria(operator, args);
    }
    Err(api::TimerError::message("unsupported condition type"))
}

/// 将 `TimerCond` 各可选字段拼成 AND 连接的等值/前缀/JSON 标签谓词。
pub fn buildTimerCondCriteria(
    cond: &api::TimerCond,
    mut args: Vec<SqlArg>,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    let mut items = Vec::new();
    if let Some(value) = cond.ID.Get() {
        items.push("ID = %?");
        args.push(SqlArg::String(value.clone()));
    }
    if let Some(value) = cond.Namespace.Get() {
        items.push("NAMESPACE = %?");
        args.push(SqlArg::String(value.clone()));
    }
    if let Some(value) = cond.Key.Get() {
        // KeyPrefix：LIKE 'value%'；否则精确匹配。
        if cond.KeyPrefix {
            items.push("TIMER_KEY LIKE %?");
            args.push(SqlArg::String(format!("{value}%")));
        } else {
            items.push("TIMER_KEY = %?");
            args.push(SqlArg::String(value.clone()));
        }
    }
    if let Some(tags) = cond.Tags.Get().filter(|tags| !tags.is_empty()) {
        // 标签过滤：先确认 tags 字段存在，再用 JSON_CONTAINS 做包含判定。
        items.push("JSON_EXTRACT(TIMER_EXT, '$.tags') IS NOT NULL");
        items.push("JSON_CONTAINS((TIMER_EXT->'$.tags'), %?)");
        args.push(SqlArg::Json(encode_string_array(tags)));
    }
    Ok((
        if items.is_empty() {
            "1".to_string()
        } else {
            items.join(" AND ")
        },
        args,
    ))
}

/// 递归拼装 AND/OR/`Not` 组合条件；多子条件时加括号，并对恒真/恒假做 Not 化简。
pub fn buildOperatorCriteria(
    operator: &api::Operator,
    mut args: Vec<SqlArg>,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    if operator.Children.is_empty() {
        return Err(api::TimerError::message("children should not be empty"));
    }
    let separator = match operator.Op {
        api::OperatorTp::OperatorAnd => " AND ",
        api::OperatorTp::OperatorOr => " OR ",
    };
    let mut criteria_list = Vec::with_capacity(operator.Children.len());
    for child in &operator.Children {
        let (mut criteria, next_args) = buildCondCriteria(Some(child.as_ref()), args)?;
        args = next_args;
        // 多子条件时加括号，避免 AND/OR 优先级歧义。
        if operator.Children.len() > 1 && criteria != "1" && criteria != "0" {
            criteria = format!("({criteria})");
        }
        criteria_list.push(criteria);
    }
    let mut criteria = criteria_list.join(separator);
    if operator.Not {
        // 对恒真/恒假做布尔取反化简，其余包在 `!(...)` 中。
        criteria = match criteria.as_str() {
            "0" => "1".to_string(),
            "1" => "0".to_string(),
            _ => format!("!({criteria})"),
        };
    }
    Ok((criteria, args))
}

/// 拼装按 ID 更新的 UPDATE，SET 子句由 `buildUpdateCriteria` 生成。
pub fn buildUpdateTimerSQL(
    db_name: &str,
    table_name: &str,
    timer_id: &str,
    update: &api::TimerUpdate,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    let (criteria, mut args) = buildUpdateCriteria(update, Vec::with_capacity(6))?;
    args.push(SqlArg::String(timer_id.to_string()));
    Ok((
        format!(
            "UPDATE {} SET {} WHERE ID = %?",
            indentString(db_name, table_name),
            criteria
        ),
        args,
    ))
}

/// 将 `TimerUpdate` 中出现的字段拼成 SET 列表，并始终递增 VERSION。
pub fn buildUpdateCriteria(
    update: &api::TimerUpdate,
    mut args: Vec<SqlArg>,
) -> api::TimerResult<(String, Vec<SqlArg>)> {
    let mut fields = Vec::new();
    if let Some(value) = update.Enable.Get() {
        fields.push("ENABLE = %?");
        args.push(SqlArg::Bool(*value));
    }
    // TIMER_EXT 子字段先收集，最后用 JSON_MERGE_PATCH 一次性合并。
    let mut ext = BTreeMap::new();
    if let Some(value) = update.Tags.Get() {
        ext.insert(
            "tags",
            if value.is_empty() {
                "null".to_string()
            } else {
                encode_string_array(value)
            },
        );
    }
    if let Some(value) = update.ManualRequest.Get() {
        ext.insert(
            "manual",
            newManualRequestObj(value)
                .as_ref()
                .map(encode_manual)
                .unwrap_or_else(|| "null".to_string()),
        );
    }
    if let Some(value) = update.EventExtra.Get() {
        ext.insert(
            "event",
            newEventExtObj(value)
                .as_ref()
                .map(encode_event)
                .unwrap_or_else(|| "null".to_string()),
        );
    }
    macro_rules! string_field {
        ($value:expr, $sql:literal) => {
            if let Some(value) = $value.Get() {
                fields.push($sql);
                args.push(SqlArg::String(value.clone()));
            }
        };
    }
    string_field!(update.TimeZone, "TIMEZONE = %?");
    string_field!(update.SchedPolicyType, "SCHED_POLICY_TYPE = %?");
    string_field!(update.SchedPolicyExpr, "SCHED_POLICY_EXPR = %?");
    string_field!(update.EventStatus, "EVENT_STATUS = %?");
    string_field!(update.EventID, "EVENT_ID = %?");
    if let Some(value) = update.EventData.Get() {
        fields.push("EVENT_DATA = %?");
        args.push(SqlArg::Bytes(value.clone()));
    }
    append_optional_timestamp(
        &mut fields,
        &mut args,
        "EVENT_START",
        update.EventStart.Get(),
    );
    append_optional_timestamp(&mut fields, &mut args, "WATERMARK", update.Watermark.Get());
    if let Some(value) = update.SummaryData.Get() {
        fields.push("SUMMARY_DATA = %?");
        args.push(SqlArg::Bytes(value.clone()));
    }
    if !ext.is_empty() {
        // 用 MERGE_PATCH 局部更新 tags/manual/event，避免覆盖未改字段。
        fields.push("TIMER_EXT = JSON_MERGE_PATCH(TIMER_EXT, %?)");
        args.push(SqlArg::Json(format!(
            "{{{}}}",
            ext.into_iter()
                .map(|(key, value)| format!("\"{key}\":{value}"))
                .collect::<Vec<_>>()
                .join(",")
        )));
    }
    // 乐观并发：每次更新递增版本号。
    fields.push("VERSION = VERSION + 1");
    Ok((fields.join(", "), args))
}

/// 将 Unix 秒转为 `Timestamp`（相对“当前整秒”做加减，避免依赖绝对时钟类型）。
fn timestamp_from_unix(value: i64) -> Option<api::Timestamp> {
    let now = api::now_timestamp();
    let base = now - Duration::from_nanos(now.timestamp_subsec_nanos() as u64);
    let current = base.timestamp();
    if value >= current {
        Some(base + Duration::from_secs((value - current) as u64))
    } else {
        Some(base - Duration::from_secs((current - value) as u64))
    }
}

/// JSON 字符串字面量转义（含控制字符的 `\uXXXX`）。
fn encode_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '<' => output.push_str("\\u003c"),
            '>' => output.push_str("\\u003e"),
            '&' => output.push_str("\\u0026"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            value if value.is_control() => output.push_str(&format!("\\u{:04x}", value as u32)),
            value => output.push(value),
        }
    }
    output.push('"');
    output
}

/// 将字符串切片编码为 JSON 数组文本。
fn encode_string_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| encode_string(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// 将手动请求对象编码为 JSON 对象文本；未赋值字段按 Go struct 编码为 `null`。
fn encode_manual(value: &ManualRequestObj) -> String {
    format!(
        concat!(
            "{{\"request_id\":{},",
            "\"request_time_unix\":{},",
            "\"timeout_sec\":{},",
            "\"processed\":{},",
            "\"event_id\":{}}}"
        ),
        value
            .request_id
            .as_deref()
            .map(encode_string)
            .unwrap_or_else(|| "null".to_string()),
        value
            .request_time_unix
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
        value
            .timeout_sec
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
        value
            .processed
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
        value
            .event_id
            .as_deref()
            .map(encode_string)
            .unwrap_or_else(|| "null".to_string()),
    )
}

/// 将事件扩展编码为 JSON 对象文本。
fn encode_event(value: &EventExtObj) -> String {
    format!(
        "{{\"manual_request_id\":{},\"watermark_unix\":{}}}",
        value
            .manual_request_id
            .as_deref()
            .map(encode_string)
            .unwrap_or_else(|| "null".to_string()),
        value
            .watermark_unix
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
    )
}

/// 将完整 `TimerExt` 编码为写入 `TIMER_EXT` 的 JSON 文本。
pub fn encode_timer_ext(value: &TimerExt) -> String {
    let mut fields = Vec::new();
    if !value.tags.is_empty() {
        fields.push(format!("\"tags\":{}", encode_string_array(&value.tags)));
    }
    if let Some(value) = &value.manual {
        fields.push(format!("\"manual\":{}", encode_manual(value)));
    }
    if let Some(value) = &value.event {
        fields.push(format!("\"event\":{}", encode_event(value)));
    }
    format!("{{{}}}", fields.join(","))
}

/// 轻量 JSON AST，仅覆盖 TIMER_EXT 所需的标量/数组/对象。
#[derive(Clone, Debug)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

/// 解析 `TIMER_EXT` JSON 文本为 `TimerExt`；要求根为对象且无尾随数据。
pub fn decode_timer_ext(input: &str) -> api::TimerResult<TimerExt> {
    let mut parser = JsonParser {
        input: input.as_bytes(),
        offset: 0,
    };
    let JsonValue::Object(root) = parser.parse_value()? else {
        return Err(api::TimerError::message(
            "TIMER_EXT should be a JSON object",
        ));
    };
    parser.skip_whitespace();
    if parser.offset != parser.input.len() {
        return Err(api::TimerError::message("trailing TIMER_EXT JSON data"));
    }
    // tags：缺省或 null → 空列表；否则必须是字符串数组。
    let tags = match root.get("tags") {
        None | Some(JsonValue::Null) => Vec::new(),
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(value) => Ok(value.clone()),
                _ => Err(api::TimerError::message("timer tag should be a string")),
            })
            .collect::<api::TimerResult<Vec<_>>>()?,
        _ => return Err(api::TimerError::message("tags should be an array")),
    };
    let manual = object_field(&root, "manual")?
        .map(|value| {
            Ok(ManualRequestObj {
                request_id: string_value(value.get("request_id"), "request_id")?,
                request_time_unix: number_value(
                    value.get("request_time_unix"),
                    "request_time_unix",
                )?,
                timeout_sec: number_value(value.get("timeout_sec"), "timeout_sec")?,
                processed: bool_value(value.get("processed"), "processed")?,
                event_id: string_value(value.get("event_id"), "event_id")?,
            })
        })
        .transpose()?;
    let event = object_field(&root, "event")?
        .map(|value| {
            Ok(EventExtObj {
                manual_request_id: string_value(
                    value.get("manual_request_id"),
                    "manual_request_id",
                )?,
                watermark_unix: number_value(value.get("watermark_unix"), "watermark_unix")?,
            })
        })
        .transpose()?;
    Ok(TimerExt {
        tags,
        manual,
        event,
    })
}

/// 从对象中取嵌套对象字段；null/缺失 → `None`。
fn object_field<'a>(
    root: &'a BTreeMap<String, JsonValue>,
    key: &str,
) -> api::TimerResult<Option<&'a BTreeMap<String, JsonValue>>> {
    match root.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Object(value)) => Ok(Some(value)),
        _ => Err(api::TimerError::message(format!(
            "{key} should be an object"
        ))),
    }
}

/// 仅当值为 JSON 字符串时取出。
fn string_value(value: Option<&JsonValue>, field: &str) -> api::TimerResult<Option<String>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        _ => Err(api::TimerError::message(format!(
            "{field} should be a string"
        ))),
    }
}
/// 仅当值为 JSON 整数时取出。
fn number_value(value: Option<&JsonValue>, field: &str) -> api::TimerResult<Option<i64>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(value)) => Ok(Some(*value)),
        _ => Err(api::TimerError::message(format!(
            "{field} should be an integer"
        ))),
    }
}
/// 仅当值为 JSON 布尔时取出。
fn bool_value(value: Option<&JsonValue>, field: &str) -> api::TimerResult<Option<bool>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(value)) => Ok(Some(*value)),
        _ => Err(api::TimerError::message(format!(
            "{field} should be a boolean"
        ))),
    }
}

/// 手写递归下降 JSON 解析器，避免引入完整 serde 依赖。
struct JsonParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl JsonParser<'_> {
    /// 跳过 ASCII 空白。
    fn skip_whitespace(&mut self) {
        while self
            .input
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }
    /// 按首字节分派解析 null/bool/string/array/object/number。
    fn parse_value(&mut self) -> api::TimerResult<JsonValue> {
        self.skip_whitespace();
        match self.input.get(self.offset) {
            Some(b'n') => {
                self.expect(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.expect(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.expect(b"false")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(JsonValue::Number),
            _ => Err(api::TimerError::message("invalid TIMER_EXT JSON value")),
        }
    }
    /// 期望并消费精确字节序列（如 `null`/`true`）。
    fn expect(&mut self, value: &[u8]) -> api::TimerResult<()> {
        if self.input.get(self.offset..self.offset + value.len()) == Some(value) {
            self.offset += value.len();
            Ok(())
        } else {
            Err(api::TimerError::message("invalid TIMER_EXT JSON token"))
        }
    }
    /// 解析 JSON 字符串，支持常见转义与 `\uXXXX`。
    fn parse_string(&mut self) -> api::TimerResult<String> {
        self.offset += 1;
        let mut output = String::new();
        while let Some(byte) = self.input.get(self.offset).copied() {
            self.offset += 1;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = *self
                        .input
                        .get(self.offset)
                        .ok_or_else(|| api::TimerError::message("unterminated JSON escape"))?;
                    self.offset += 1;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{8}'),
                        b'f' => output.push('\u{c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let digits = self
                                .input
                                .get(self.offset..self.offset + 4)
                                .ok_or_else(|| api::TimerError::message("short unicode escape"))?;
                            let digits = std::str::from_utf8(digits)
                                .map_err(|_| api::TimerError::message("invalid unicode escape"))?;
                            let value = u16::from_str_radix(digits, 16)
                                .map_err(|_| api::TimerError::message("invalid unicode escape"))?;
                            self.offset += 4;
                            let value = if (0xd800..=0xdbff).contains(&value) {
                                let low = self
                                    .input
                                    .get(self.offset..self.offset + 6)
                                    .filter(|candidate| candidate.starts_with(b"\\u"))
                                    .and_then(|candidate| std::str::from_utf8(&candidate[2..]).ok())
                                    .and_then(|digits| u16::from_str_radix(digits, 16).ok());
                                if let Some(low) = low.filter(|low| (0xdc00..=0xdfff).contains(low))
                                {
                                    self.offset += 6;
                                    0x10000
                                        + (((value as u32 - 0xd800) << 10) | (low as u32 - 0xdc00))
                                } else {
                                    char::REPLACEMENT_CHARACTER as u32
                                }
                            } else if (0xdc00..=0xdfff).contains(&value) {
                                char::REPLACEMENT_CHARACTER as u32
                            } else {
                                value as u32
                            };
                            output.push(char::from_u32(value).ok_or_else(|| {
                                api::TimerError::message("invalid unicode scalar")
                            })?);
                        }
                        _ => return Err(api::TimerError::message("invalid JSON escape")),
                    }
                }
                value if value < 0x20 => {
                    return Err(api::TimerError::message("control character in JSON string"));
                }
                value if value.is_ascii() => output.push(value as char),
                _ => {
                    self.offset -= 1;
                    let remaining = std::str::from_utf8(&self.input[self.offset..])
                        .map_err(|_| api::TimerError::message("invalid UTF-8 in JSON string"))?;
                    let character = remaining.chars().next().unwrap();
                    output.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }
        Err(api::TimerError::message("unterminated JSON string"))
    }
    /// 解析有符号整数（TIMER_EXT 不需要浮点）。
    fn parse_number(&mut self) -> api::TimerResult<i64> {
        let start = self.offset;
        if self.input.get(self.offset) == Some(&b'-') {
            self.offset += 1;
        }
        while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
            self.offset += 1;
        }
        std::str::from_utf8(&self.input[start..self.offset])
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| api::TimerError::message("invalid JSON integer"))
    }
    /// 解析 JSON 数组。
    fn parse_array(&mut self) -> api::TimerResult<JsonValue> {
        self.offset += 1;
        let mut values = Vec::new();
        loop {
            self.skip_whitespace();
            if self.input.get(self.offset) == Some(&b']') {
                self.offset += 1;
                return Ok(JsonValue::Array(values));
            }
            values.push(self.parse_value()?);
            self.skip_whitespace();
            match self.input.get(self.offset) {
                Some(b',') => {
                    self.offset += 1;
                    self.skip_whitespace();
                    if self.input.get(self.offset) == Some(&b']') {
                        return Err(api::TimerError::message("invalid JSON array"));
                    }
                }
                Some(b']') => {
                    self.offset += 1;
                    return Ok(JsonValue::Array(values));
                }
                _ => return Err(api::TimerError::message("invalid JSON array")),
            }
        }
    }
    /// 解析 JSON 对象（键按插入顺序存入 `BTreeMap`）。
    fn parse_object(&mut self) -> api::TimerResult<JsonValue> {
        self.offset += 1;
        let mut values = BTreeMap::new();
        loop {
            self.skip_whitespace();
            if self.input.get(self.offset) == Some(&b'}') {
                self.offset += 1;
                return Ok(JsonValue::Object(values));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.input.get(self.offset) != Some(&b':') {
                return Err(api::TimerError::message("missing JSON object colon"));
            }
            self.offset += 1;
            values.insert(key, self.parse_value()?);
            self.skip_whitespace();
            match self.input.get(self.offset) {
                Some(b',') => {
                    self.offset += 1;
                    self.skip_whitespace();
                    if self.input.get(self.offset) == Some(&b'}') {
                        return Err(api::TimerError::message("invalid JSON object"));
                    }
                }
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(JsonValue::Object(values));
                }
                _ => return Err(api::TimerError::message("invalid JSON object")),
            }
        }
    }
}

/// 可选时间戳字段：`Some(Some(ts))` 写 FROM_UNIXTIME；`Some(None)` 写 NULL；`None` 跳过。
fn append_optional_timestamp(
    fields: &mut Vec<&'static str>,
    args: &mut Vec<SqlArg>,
    column: &'static str,
    value: Option<&Option<api::Timestamp>>,
) {
    match value {
        Some(Some(value)) => {
            fields.push(match column {
                "EVENT_START" => "EVENT_START = FROM_UNIXTIME(%?)",
                _ => "WATERMARK = FROM_UNIXTIME(%?)",
            });
            args.push(SqlArg::I64(value.timestamp()));
        }
        Some(None) => fields.push(match column {
            "EVENT_START" => "EVENT_START = NULL",
            _ => "WATERMARK = NULL",
        }),
        None => {}
    }
}

/// 拼装按 ID 删除的 DELETE 语句。
pub fn buildDeleteTimerSQL(
    db_name: &str,
    table_name: &str,
    timer_id: &str,
) -> (String, Vec<SqlArg>) {
    (
        format!(
            "DELETE FROM {} WHERE ID = %?",
            indentString(db_name, table_name)
        ),
        vec![SqlArg::String(timer_id.to_string())],
    )
}
