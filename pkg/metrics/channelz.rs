// Copyright 2026 AsterSQL.
// Copyright 2026 TiKV Authors.
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

//! Prometheus collector for grpcio C-core channelz snapshots.
//!
//! 本模块从 grpcio C-core 的 channelz 接口分页读取通道、子通道与套接字快照，
//! 过滤 TiDB 自身用于采集 channelz 的内部连接，再转换为 Prometheus 指标族。
//! 遍历期间的抓取或 JSON 解析失败不会中断采集，而是累计到专用错误计数器中。

use prometheus::core::{Collector, Desc};
use prometheus::proto::{
    Counter as ProtoCounter, Gauge as ProtoGauge, LabelPair, Metric, MetricFamily, MetricType,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

const CHANNEL_CALLS: (&str, &str) = (
    "tidb_grpc_channelz_channel_calls_total",
    "Total calls observed by the channelz channel or subchannel.",
);
const CHANNEL_LAST_CALL: (&str, &str) = (
    "tidb_grpc_channelz_channel_last_call_started_timestamp_seconds",
    "Unix timestamp of the last call started on the channelz channel or subchannel.",
);
const SOCKET_STREAMS: (&str, &str) = (
    "tidb_grpc_channelz_socket_streams_total",
    "Total streams observed by the channelz socket.",
);
const SOCKET_MESSAGES: (&str, &str) = (
    "tidb_grpc_channelz_socket_messages_total",
    "Total messages observed by the channelz socket.",
);
const SOCKET_KEEPALIVES: (&str, &str) = (
    "tidb_grpc_channelz_socket_keepalives_total",
    "Total keepalive pings sent on the channelz socket.",
);
const SOCKET_LAST_STREAM: (&str, &str) = (
    "tidb_grpc_channelz_socket_last_stream_created_timestamp_seconds",
    "Unix timestamp of the last stream created on the channelz socket.",
);
const SOCKET_LAST_MESSAGE: (&str, &str) = (
    "tidb_grpc_channelz_socket_last_message_timestamp_seconds",
    "Unix timestamp of the last message activity observed on the channelz socket.",
);
const SOCKET_FLOW_WINDOW: (&str, &str) = (
    "tidb_grpc_channelz_socket_flow_control_window_bytes",
    "HTTP/2 flow control window exposed by the channelz socket.",
);
const FETCH_ERRORS: (&str, &str) = (
    "tidb_grpc_channelz_fetch_errors_total",
    "Total RPC fetch errors encountered by the channelz collector.",
);

#[derive(Clone)]
/// channelz 快照的 Prometheus 采集器。
///
/// 克隆实例共享指标描述、错误计数和身份句柄，便于注册侧识别同一采集器。
pub(crate) struct ChannelzCollector {
    descs: Arc<Vec<Desc>>,
    errors: Arc<FetchErrors>,
    handle: Arc<()>,
}

impl ChannelzCollector {
    /// 创建采集器并预先校验全部 Prometheus 指标描述。
    pub(crate) fn new() -> prometheus::Result<Self> {
        Ok(Self {
            descs: Arc::new(channelz_descs()?),
            errors: Arc::new(FetchErrors::default()),
            handle: Arc::new(()),
        })
    }

    /// 返回所有克隆实例共享的稳定身份，不暴露内部句柄本身。
    pub(crate) fn handle_id(&self) -> usize {
        Arc::as_ptr(&self.handle) as usize
    }

    /// 从指定快照源完成一次遍历，并将扁平样本组装为指标族。
    fn collect_from(&self, source: &dyn SnapshotSource) -> Vec<MetricFamily> {
        let mut walker = Walker::new(source, &self.errors);
        walker.walk_top_channels();
        walker.add_fetch_errors();
        families(walker.samples)
    }
}

impl Collector for ChannelzCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.descs.iter().collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        static GRPC_ENV: LazyLock<grpcio::Environment> =
            LazyLock::new(|| grpcio::Environment::new(1));
        let _keep_alive = &*GRPC_ENV;
        self.collect_from(&SystemSource)
    }
}

#[derive(Default)]
/// 按 channelz RPC 分类累计抓取或解析失败次数。
///
/// 计数跨多次采集共享；这里只要求原子累加，无需借助顺序保证同步其他状态。
struct FetchErrors {
    top_channels: AtomicU64,
    channels: AtomicU64,
    subchannels: AtomicU64,
    sockets: AtomicU64,
}

/// channelz 快照读取接口，用于隔离 grpcio C-core 调用并支持静态快照测试。
trait SnapshotSource: Send + Sync {
    fn top_channels(&self, start_id: u64) -> Option<String>;
    fn channel(&self, id: u64) -> Option<String>;
    fn subchannel(&self, id: u64) -> Option<String>;
    fn socket(&self, id: u64) -> Option<String>;
}

/// 直接调用 grpcio 全局 channelz 接口的生产快照源。
struct SystemSource;

impl SnapshotSource for SystemSource {
    fn top_channels(&self, start_id: u64) -> Option<String> {
        nonempty(grpcio::channelz::get_top_channels(start_id, str::to_owned))
    }

    fn channel(&self, id: u64) -> Option<String> {
        nonempty(grpcio::channelz::get_channel(id, str::to_owned))
    }

    fn subchannel(&self, id: u64) -> Option<String> {
        nonempty(grpcio::channelz::get_subchannel(id, str::to_owned))
    }

    fn socket(&self, id: u64) -> Option<String> {
        nonempty(grpcio::channelz::get_socket(id, str::to_owned))
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
/// Prometheus 样本类型，也是后续指标族分组键的一部分。
enum SampleKind {
    Counter,
    Gauge,
}

/// 尚未编码为 protobuf 的单条指标样本。
struct Sample {
    family: &'static str,
    help: &'static str,
    kind: SampleKind,
    labels: Vec<(&'static str, String)>,
    value: f64,
}

/// 沿 channelz 引用关系遍历快照并收集样本。
///
/// 三组 `seen_*` 集合既防止拓扑中的重复引用产生重复指标，也避免环形引用递归不止。
struct Walker<'a> {
    source: &'a dyn SnapshotSource,
    errors: &'a FetchErrors,
    samples: Vec<Sample>,
    seen_channels: HashSet<u64>,
    seen_subchannels: HashSet<u64>,
    seen_sockets: HashSet<u64>,
}

impl<'a> Walker<'a> {
    fn new(source: &'a dyn SnapshotSource, errors: &'a FetchErrors) -> Self {
        Self {
            source,
            errors,
            samples: Vec::new(),
            seen_channels: HashSet::new(),
            seen_subchannels: HashSet::new(),
            seen_sockets: HashSet::new(),
        }
    }

    fn walk_top_channels(&mut self) {
        let mut start_id = 0;
        loop {
            let Some(payload) = self.source.top_channels(start_id) else {
                self.errors.top_channels.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Ok(root) = serde_json::from_str::<Value>(&payload) else {
                self.errors.top_channels.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let channels = root["channel"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let mut max_id = start_id;
            for channel in channels {
                max_id = max_id.max(id(&channel["ref"], "channelId").unwrap_or(0));
                self.walk_channel(channel, false);
            }
            // 除服务端明确标记结束外，空页或游标没有前进也必须终止，避免异常响应导致死循环。
            if root["end"].as_bool().unwrap_or(false) || channels.is_empty() || max_id <= start_id {
                return;
            }
            start_id = max_id + 1;
        }
    }

    fn walk_channel(&mut self, node: &Value, is_subchannel: bool) {
        let id_key = if is_subchannel {
            "subchannelId"
        } else {
            "channelId"
        };
        let Some(node_id) = id(&node["ref"], id_key) else {
            return;
        };
        if node_id == 0 {
            return;
        }
        let seen = if is_subchannel {
            &mut self.seen_subchannels
        } else {
            &mut self.seen_channels
        };
        if !seen.insert(node_id) {
            return;
        }

        let target = node["data"]["target"].as_str().unwrap_or_default();
        // 排除采集器自身建立的内部通道，避免监控行为污染被观测数据。
        if crate::metrics::is_internal_channelz_target(target) {
            return;
        }
        let channel_refs = array(&node["channelRef"]);
        let subchannel_refs = array(&node["subchannelRef"]);
        let socket_refs = array(&node["socketRef"]);
        if is_subchannel
            && !socket_refs.is_empty()
            && channel_refs.is_empty()
            && subchannel_refs.is_empty()
        {
            // 只在连接落到套接字的叶子子通道上记录调用指标，避免父级聚合值被重复计入。
            self.add_channel_samples(node_id, target, &node["data"]);
        }

        for reference in channel_refs {
            if let Some(child_id) = id(reference, "channelId") {
                self.walk_fetched_channel(child_id, false);
            }
        }
        for reference in subchannel_refs {
            if let Some(child_id) = id(reference, "subchannelId") {
                self.walk_fetched_channel(child_id, true);
            }
        }
        for reference in socket_refs {
            if let Some(socket_id) = id(reference, "socketId") {
                self.walk_socket(socket_id);
            }
        }
    }

    fn walk_fetched_channel(&mut self, node_id: u64, is_subchannel: bool) {
        let payload = if is_subchannel {
            self.source.subchannel(node_id)
        } else {
            self.source.channel(node_id)
        };
        let Some(payload) = payload else {
            self.channel_error(is_subchannel);
            return;
        };
        let Ok(root) = serde_json::from_str::<Value>(&payload) else {
            self.channel_error(is_subchannel);
            return;
        };
        self.walk_channel(
            &root[if is_subchannel {
                "subchannel"
            } else {
                "channel"
            }],
            is_subchannel,
        );
    }

    fn channel_error(&self, is_subchannel: bool) {
        if is_subchannel {
            self.errors.subchannels.fetch_add(1, Ordering::Relaxed);
        } else {
            self.errors.channels.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn walk_socket(&mut self, socket_id: u64) {
        if socket_id == 0 {
            return;
        }
        if !self.seen_sockets.insert(socket_id) {
            return;
        }
        let Some(payload) = self.source.socket(socket_id) else {
            self.errors.sockets.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let Ok(root) = serde_json::from_str::<Value>(&payload) else {
            self.errors.sockets.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let socket = &root["socket"];
        let remote = format_address(&socket["remote"]);
        // 内部套接字可能缺少结构化远端地址，因此同时传入格式化地址和 remoteName 判定。
        if crate::metrics::is_internal_channelz_socket(
            (!socket["remote"].is_null()).then_some(remote.as_str()),
            socket["remoteName"].as_str().unwrap_or_default(),
        ) {
            return;
        }
        self.add_socket_samples(
            socket_id,
            &format_address(&socket["local"]),
            &remote,
            &socket["data"],
        );
    }

    fn add_channel_samples(&mut self, node_id: u64, target: &str, data: &Value) {
        if data.is_null() {
            return;
        }
        for (metric_type, key) in [
            ("started", "callsStarted"),
            ("succeeded", "callsSucceeded"),
            ("failed", "callsFailed"),
        ] {
            self.samples.push(Sample {
                family: CHANNEL_CALLS.0,
                help: CHANNEL_CALLS.1,
                kind: SampleKind::Counter,
                labels: vec![
                    ("kind", "subchannel".to_owned()),
                    ("id", node_id.to_string()),
                    ("target", target.to_owned()),
                    ("type", metric_type.to_owned()),
                ],
                value: number(&data[key]),
            });
        }
        if let Some(value) = timestamp(&data["lastCallStartedTimestamp"]) {
            self.samples.push(Sample {
                family: CHANNEL_LAST_CALL.0,
                help: CHANNEL_LAST_CALL.1,
                kind: SampleKind::Gauge,
                labels: vec![
                    ("kind", "subchannel".to_owned()),
                    ("id", node_id.to_string()),
                    ("target", target.to_owned()),
                ],
                value,
            });
        }
    }

    fn add_socket_samples(&mut self, socket_id: u64, local: &str, remote: &str, data: &Value) {
        if data.is_null() {
            return;
        }
        let base = || {
            vec![
                ("id", socket_id.to_string()),
                ("local", local.to_owned()),
                ("remote", remote.to_owned()),
            ]
        };
        for (metric_type, key) in [
            ("started", "streamsStarted"),
            ("succeeded", "streamsSucceeded"),
            ("failed", "streamsFailed"),
        ] {
            let mut labels = base();
            labels.push(("type", metric_type.to_owned()));
            self.samples.push(Sample {
                family: SOCKET_STREAMS.0,
                help: SOCKET_STREAMS.1,
                kind: SampleKind::Counter,
                labels,
                value: number(&data[key]),
            });
        }
        for (direction, key) in [("sent", "messagesSent"), ("received", "messagesReceived")] {
            let mut labels = base();
            labels.push(("direction", direction.to_owned()));
            self.samples.push(Sample {
                family: SOCKET_MESSAGES.0,
                help: SOCKET_MESSAGES.1,
                kind: SampleKind::Counter,
                labels,
                value: number(&data[key]),
            });
        }
        self.samples.push(Sample {
            family: SOCKET_KEEPALIVES.0,
            help: SOCKET_KEEPALIVES.1,
            kind: SampleKind::Counter,
            labels: base(),
            value: number(&data["keepAlivesSent"]),
        });
        for (side, key) in [
            ("local", "lastLocalStreamCreatedTimestamp"),
            ("remote", "lastRemoteStreamCreatedTimestamp"),
        ] {
            if let Some(value) = timestamp(&data[key]).filter(|value| *value != 0.0) {
                let mut labels = base();
                labels.push(("side", side.to_owned()));
                self.samples.push(Sample {
                    family: SOCKET_LAST_STREAM.0,
                    help: SOCKET_LAST_STREAM.1,
                    kind: SampleKind::Gauge,
                    labels,
                    value,
                });
            }
        }
        for (direction, key) in [
            ("sent", "lastMessageSentTimestamp"),
            ("received", "lastMessageReceivedTimestamp"),
        ] {
            if let Some(value) = timestamp(&data[key]) {
                let mut labels = base();
                labels.push(("direction", direction.to_owned()));
                self.samples.push(Sample {
                    family: SOCKET_LAST_MESSAGE.0,
                    help: SOCKET_LAST_MESSAGE.1,
                    kind: SampleKind::Gauge,
                    labels,
                    value,
                });
            }
        }
        for (side, key) in [
            ("local", "localFlowControlWindow"),
            ("remote", "remoteFlowControlWindow"),
        ] {
            if let Some(value) = data[key].get("value").map(number) {
                let mut labels = base();
                labels.push(("side", side.to_owned()));
                self.samples.push(Sample {
                    family: SOCKET_FLOW_WINDOW.0,
                    help: SOCKET_FLOW_WINDOW.1,
                    kind: SampleKind::Gauge,
                    labels,
                    value,
                });
            }
        }
    }

    fn add_fetch_errors(&mut self) {
        for (rpc, value) in [
            (
                "GetTopChannels",
                self.errors.top_channels.load(Ordering::Relaxed),
            ),
            ("GetChannel", self.errors.channels.load(Ordering::Relaxed)),
            (
                "GetSubchannel",
                self.errors.subchannels.load(Ordering::Relaxed),
            ),
            ("GetSocket", self.errors.sockets.load(Ordering::Relaxed)),
        ] {
            self.samples.push(Sample {
                family: FETCH_ERRORS.0,
                help: FETCH_ERRORS.1,
                kind: SampleKind::Counter,
                labels: vec![("rpc", rpc.to_owned())],
                value: value as f64,
            });
        }
    }
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// 兼容 channelz JSON 将标识符编码为字符串或无符号整数的两种形式。
fn id(value: &Value, key: &str) -> Option<u64> {
    value[key]
        .as_str()
        .and_then(|value| value.parse().ok())
        .or_else(|| value[key].as_u64())
}

/// 宽容读取 channelz 数值；缺失或格式错误时按零暴露，保持本轮采集可用。
fn number(value: &Value) -> f64 {
    value
        .as_str()
        .and_then(|value| value.parse().ok())
        .or_else(|| value.as_f64())
        .unwrap_or(0.0)
}

/// 构造采集器声明的全部指标描述，并集中固定每个指标族的标签集合。
fn channelz_descs() -> prometheus::Result<Vec<Desc>> {
    [
        (CHANNEL_CALLS, &["kind", "id", "target", "type"][..]),
        (CHANNEL_LAST_CALL, &["kind", "id", "target"][..]),
        (SOCKET_STREAMS, &["id", "local", "remote", "type"][..]),
        (SOCKET_MESSAGES, &["id", "local", "remote", "direction"][..]),
        (SOCKET_KEEPALIVES, &["id", "local", "remote"][..]),
        (SOCKET_LAST_STREAM, &["id", "local", "remote", "side"][..]),
        (
            SOCKET_LAST_MESSAGE,
            &["id", "local", "remote", "direction"][..],
        ),
        (SOCKET_FLOW_WINDOW, &["id", "local", "remote", "side"][..]),
        (FETCH_ERRORS, &["rpc"][..]),
    ]
    .into_iter()
    .map(|((name, help), labels)| {
        Desc::new(
            name.to_owned(),
            help.to_owned(),
            labels.iter().map(|label| (*label).to_owned()).collect(),
            HashMap::new(),
        )
    })
    .collect()
}

/// 按名称、帮助文本和类型归并样本，再编码为 Prometheus protobuf 指标族。
fn families(samples: Vec<Sample>) -> Vec<MetricFamily> {
    let mut groups: BTreeMap<(&str, &str, SampleKind), Vec<Sample>> = BTreeMap::new();
    for sample in samples {
        groups
            .entry((sample.family, sample.help, sample.kind))
            .or_default()
            .push(sample);
    }
    groups
        .into_iter()
        .map(|((name, help, kind), samples)| {
            let mut family = MetricFamily::default();
            family.set_name(name.to_owned());
            family.set_help(help.to_owned());
            family.set_field_type(match kind {
                SampleKind::Counter => MetricType::COUNTER,
                SampleKind::Gauge => MetricType::GAUGE,
            });
            family.set_metric(
                samples
                    .into_iter()
                    .map(|sample| {
                        let mut metric = Metric::default();
                        metric.set_label(
                            sample
                                .labels
                                .into_iter()
                                .map(|(name, value)| {
                                    let mut pair = LabelPair::default();
                                    pair.set_name(name.to_owned());
                                    pair.set_value(value);
                                    pair
                                })
                                .collect(),
                        );
                        match kind {
                            SampleKind::Counter => {
                                let mut counter = ProtoCounter::default();
                                counter.set_value(sample.value);
                                metric.set_counter(counter);
                            }
                            SampleKind::Gauge => {
                                let mut gauge = ProtoGauge::default();
                                gauge.set_value(sample.value);
                                metric.set_gauge(gauge);
                            }
                        }
                        metric
                    })
                    .collect(),
            );
            family
        })
        .collect()
}

/// 将 channelz 使用的 UTC RFC 3339 时间戳转换为 Unix 秒，保留小数秒。
fn timestamp(value: &Value) -> Option<f64> {
    let value = value.as_str()?;
    let (date, time) = value.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.split(':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next()?.parse::<i64>().ok()?;
    let second = time.next()?;
    let (second, fraction) = second
        .split_once('.')
        .map(|(whole, fraction)| (whole, format!("0.{fraction}")))
        .unwrap_or((second, "0".to_owned()));
    Some(
        (days_from_civil(year, month, day) * 86_400
            + hour * 3_600
            + minute * 60
            + second.parse::<i64>().ok()?) as f64
            + fraction.parse::<f64>().ok()?,
    )
}

/// 计算公历日期相对 Unix 纪元的天数，供时间戳转换使用。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// 将 channelz 的命名地址、Unix 套接字或 TCP/IP 地址统一为指标标签文本。
fn format_address(value: &Value) -> String {
    if let Some(name) = value["other_address"]["name"].as_str() {
        return name.to_owned();
    }
    if let Some(filename) = value["uds_address"]["filename"].as_str() {
        return filename.to_owned();
    }
    let tcp = &value["tcpip_address"];
    if !tcp.is_object() {
        return String::new();
    }
    let port = tcp["port"]
        .as_str()
        .and_then(|port| port.parse::<i64>().ok())
        .or_else(|| tcp["port"].as_i64())
        .unwrap_or_default();
    let encoded = tcp["ip_address"].as_str().unwrap_or_default();
    let Some(bytes) = decode_base64(encoded) else {
        return String::new();
    };
    let ip = match bytes.as_slice() {
        [a, b, c, d] => format!("{a}.{b}.{c}.{d}"),
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] => {
            format!("{a}.{b}.{c}.{d}")
        }
        bytes if bytes.len() == 16 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(bytes);
            std::net::Ipv6Addr::from(octets).to_string()
        }
        [] => "<nil>".to_owned(),
        _ => bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
    };
    if port < 0 {
        ip
    } else if ip.contains(':') {
        format!("[{ip}]:{port}")
    } else {
        format!("{ip}:{port}")
    }
}

/// 解码 channelz TCP/IP 地址字段使用的标准 Base64；非法字符直接视为无地址。
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut output = Vec::new();
    for byte in value.bytes().take_while(|byte| *byte != b'=') {
        let decoded = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(decoded);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            output.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }
    Some(output)
}

#[cfg(test)]
/// 由调用方提供固定 JSON 的快照源，避免测试依赖运行中的 grpcio 环境。
struct StaticSource {
    top: String,
    nodes: HashMap<(&'static str, u64), String>,
}

#[cfg(test)]
impl SnapshotSource for StaticSource {
    fn top_channels(&self, _start_id: u64) -> Option<String> {
        Some(self.top.clone())
    }

    fn channel(&self, id: u64) -> Option<String> {
        self.nodes.get(&("channel", id)).cloned()
    }

    fn subchannel(&self, id: u64) -> Option<String> {
        self.nodes.get(&("subchannel", id)).cloned()
    }

    fn socket(&self, id: u64) -> Option<String> {
        self.nodes.get(&("socket", id)).cloned()
    }
}

#[cfg(test)]
/// 从静态拓扑和节点快照执行与生产采集一致的转换流程。
pub(crate) fn collect_snapshots_for_test(
    top_channels: &str,
    snapshots: &[(&str, u64, &str)],
) -> Vec<MetricFamily> {
    let source = StaticSource {
        top: top_channels.to_owned(),
        nodes: snapshots
            .iter()
            .map(|(kind, id, payload)| {
                let kind = match *kind {
                    "channel" => "channel",
                    "subchannel" => "subchannel",
                    "socket" => "socket",
                    other => panic!("unknown channelz snapshot kind: {other}"),
                };
                ((kind, *id), (*payload).to_owned())
            })
            .collect(),
    };
    ChannelzCollector::new()
        .expect("valid channelz descriptors")
        .collect_from(&source)
}
