// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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
// DXF 计量（metering）数据模型与增量计算。
//
// 将各 recorder 累计的对象存储/集群读写计数与字节数，
// 相对上次 flush（刷盘/上报）快照算出正增量，组装成计量 SDK 可接受的 `MeterItem`。

// limitations under the License.

use std::collections::HashMap;

/// MeterItem 字段名：对象存储 GET 请求次数。
pub const GET_REQUESTS_FIELD: &str = "get_requests";
/// MeterItem 字段名：对象存储 PUT 请求次数。
pub const PUT_REQUESTS_FIELD: &str = "put_requests";
/// MeterItem 字段名：对象存储读字节数。
pub const OBJ_STORE_READ_BYTES_FIELD: &str = "obj_store_read_bytes";
/// MeterItem 字段名：对象存储写字节数。
pub const OBJ_STORE_WRITE_BYTES_FIELD: &str = "obj_store_write_bytes";
/// MeterItem 字段名：集群（TiKV 等）读字节数。
pub const CLUSTER_READ_BYTES_FIELD: &str = "cluster_read_bytes";
/// MeterItem 字段名：集群写字节数。
pub const CLUSTER_WRITE_BYTES_FIELD: &str = "cluster_write_bytes";

/// 处理行数（与 Go `RowCountField` 对齐）。
/// RowCountField represents the number of rows processed.
pub const RowCountField: &str = "row_count";
/// 写入集群的数据 KV 字节数。
/// DataKVBytesField represents the bytes of data KV ingested into the cluster.
pub const DataKVBytesField: &str = "data_kv_bytes";
/// 写入集群的索引 KV 字节数。
/// IndexKVBytesField represents the bytes of index KV ingested into the cluster.
pub const IndexKVBytesField: &str = "index_kv_bytes";
/// 任务所需 slot（槽位）数。
/// RequiredSlotsField represents the required slots of the task.
pub const RequiredSlotsField: &str = "required_slots";
/// 任务执行期间使用的最大节点数。
/// MaxNodeCountField represents the maximum number of nodes used during the task.
pub const MaxNodeCountField: &str = "max_node_count";
/// 任务持续时长（秒）。
/// DurationSecondsField represents the duration of the task in seconds.
pub const DurationSecondsField: &str = "duration_seconds";

/// 计量 SDK `map[string]any` 载荷可接受的值类型。
/// Values accepted by the metering SDK's `map[string]any` payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeterValue {
    /// 字符串值。
    String(String),
    /// 有符号 64 位整数。
    I64(i64),
    /// 无符号 64 位整数。
    U64(u64),
}

/// 从 `&str` 构造字符串型计量值。
impl From<&str> for MeterValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

/// 从 `String` 构造字符串型计量值。
impl From<String> for MeterValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

/// 从 `i64` 构造整型计量值。
impl From<i64> for MeterValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

/// 从 `u64` 构造无符号整型计量值。
impl From<u64> for MeterValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

/// 单条计量记录：字段名到值的映射（对齐 Go `map[string]any`）。
pub type MeterItem = HashMap<String, MeterValue>;

/// 单个 recorder 累计的计量数据（含任务元信息与计数）。
/// Data represents one recorder's accumulated metering data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Data {
    /// 累计计数与字节数。
    values: DataValues,
    /// DXF 任务 ID。
    task_id: i64,
    /// Keyspace（多租户命名空间，用作 cluster_id）。
    keyspace: String,
    /// 任务类型字符串。
    task_type: String,
}

/// 单调递增的计量计数器快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataValues {
    /// 对象存储 GET 请求累计次数。
    pub get_requests: u64,
    /// 对象存储 PUT 请求累计次数。
    pub put_requests: u64,
    /// 对象存储读字节累计。
    pub obj_store_read_bytes: u64,
    /// 对象存储写字节累计。
    pub obj_store_write_bytes: u64,
    /// 集群读字节累计。
    pub cluster_read_bytes: u64,
    /// 集群写字节累计。
    pub cluster_write_bytes: u64,
}

impl Data {
    /// 构造带任务元信息与计数值的计量数据。
    pub fn new(
        task_id: i64,
        keyspace: impl Into<String>,
        task_type: impl Into<String>,
        values: DataValues,
    ) -> Self {
        Self {
            values,
            task_id,
            keyspace: keyspace.into(),
            task_type: task_type.into(),
        }
    }

    /// 返回累计计数值引用。
    pub fn values(&self) -> &DataValues {
        &self.values
    }

    /// 返回任务 ID。
    pub fn task_id(&self) -> i64 {
        self.task_id
    }

    /// 返回 keyspace。
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// 返回任务类型。
    pub fn task_type(&self) -> &str {
        &self.task_type
    }

    /// 仅比较累计计数值，不比较任务元信息（与 Go 一致）。
    /// Go intentionally compares only accumulated values, not task metadata.
    pub fn equals(&self, other: &Data) -> bool {
        self.values == other.values
    }

    /// 相对上次 scrape/flush 快照计算正增量；无变化时返回 `None`。
    /// Calculates the positive delta from the last scraped snapshot.
    pub fn cal_meter_data_item(&self, other: &Data) -> Option<MeterItem> {
        if self.equals(other) {
            return None;
        }

        let mut item = GetBaseMeterItem(self.task_id, &self.keyspace, &self.task_type);
        insert_positive_delta(
            &mut item,
            GET_REQUESTS_FIELD,
            self.values.get_requests,
            other.values.get_requests,
        );
        insert_positive_delta(
            &mut item,
            PUT_REQUESTS_FIELD,
            self.values.put_requests,
            other.values.put_requests,
        );
        insert_positive_delta(
            &mut item,
            OBJ_STORE_READ_BYTES_FIELD,
            self.values.obj_store_read_bytes,
            other.values.obj_store_read_bytes,
        );
        insert_positive_delta(
            &mut item,
            OBJ_STORE_WRITE_BYTES_FIELD,
            self.values.obj_store_write_bytes,
            other.values.obj_store_write_bytes,
        );
        insert_positive_delta(
            &mut item,
            CLUSTER_READ_BYTES_FIELD,
            self.values.cluster_read_bytes,
            other.values.cluster_read_bytes,
        );
        insert_positive_delta(
            &mut item,
            CLUSTER_WRITE_BYTES_FIELD,
            self.values.cluster_write_bytes,
            other.values.cluster_write_bytes,
        );
        Some(item)
    }
}

/// 将 `current - previous` 的正增量写入 item；零增量不写入。
fn insert_positive_delta(item: &mut MeterItem, field: &str, current: u64, previous: u64) {
    // Go's uint64 subtraction wraps even though these counters are expected to
    // be monotonic. Preserve that boundary behavior instead of saturating.
    let delta = current.wrapping_sub(previous);
    if delta > 0 {
        item.insert(field.to_owned(), delta.into());
    }
}

/// 将字节数按 docker/go-units `BytesSize` 的二进制单位与四位有效数字格式化。
fn byte_size(value: u64) -> String {
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];

    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    let exponent = if size == 0.0 {
        0
    } else {
        size.abs().log10().floor() as i32
    };
    let digits = if exponent < -4 || exponent >= 4 {
        let formatted = format!("{size:.3e}");
        let (mantissa, exponent) = formatted
            .split_once('e')
            .expect("Rust scientific formatting always has an exponent");
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let exponent: i32 = exponent
            .parse()
            .expect("Rust scientific formatting always has an integer exponent");
        format!("{mantissa}e{exponent:+03}")
    } else {
        let decimal_places = (3 - exponent).max(0) as usize;
        let formatted = format!("{size:.decimal_places$}");
        if formatted.contains('.') {
            formatted
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned()
        } else {
            formatted
        }
    };
    format!("{digits}{}", UNITS[unit])
}

/// 人类可读的计量数据摘要，用于日志。
impl std::fmt::Display for Data {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{id: {}, keyspace: {}, type: {}, requests{{get: {}, put: {}}}, obj_store{{r: {}, w: {}}}, cluster{{r: {}, w: {}}}",
            self.task_id,
            self.keyspace,
            self.task_type,
            self.values.get_requests,
            self.values.put_requests,
            byte_size(self.values.obj_store_read_bytes),
            byte_size(self.values.obj_store_write_bytes),
            byte_size(self.values.cluster_read_bytes),
            byte_size(self.values.cluster_write_bytes),
        )
    }
}

/// 构造每条 DXF 计量记录共有的基础字段（version/source/task 等）。
/// GetBaseMeterItem returns the fields shared by every DXF metering item.
pub fn GetBaseMeterItem(
    task_id: i64,
    keyspace: impl Into<String>,
    task_type: impl Into<String>,
) -> MeterItem {
    HashMap::from([
        ("version".to_owned(), "1".into()),
        ("source_name".to_owned(), "dxf".into()),
        ("task_id".to_owned(), task_id.into()),
        ("cluster_id".to_owned(), keyspace.into().into()),
        ("task_type".to_owned(), task_type.into().into()),
    ])
}
