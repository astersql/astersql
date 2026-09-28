// Copyright 2021-present PingCAP, Inc.
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
// limitations under the License.

// TiKV Region/请求错误的 protobuf compact text 序列化与 `PBError` 包装。
//
// `errorpb::Error` 描述 NotLeader、EpochNotMatch、ServerIsBusy 等 Region 级错误；
// 本模块按 Go protobuf CompactText 风格输出，供错误接口与单测对齐。
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use crate::errorpb;
use protobuf::ProtobufEnum;
use std::fmt::Write as _;

/// 将 errorpb::Error 序列化为与 Go CompactTextString 对齐的文本。
fn compact_text_string(err: &errorpb::Error) -> String {
    let mut output = String::new();
    string_field(&mut output, "message", &err.message);
    optional_message(
        &mut output,
        "not_leader",
        err.not_leader.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
            optional_message(out, "leader", value.leader.as_ref(), peer_fields);
        },
    );
    optional_message(
        &mut output,
        "region_not_found",
        err.region_not_found.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "key_not_in_region",
        err.key_not_in_region.as_ref(),
        |out, value| {
            bytes_field(out, "key", &value.key);
            uint64_field(out, "region_id", value.region_id);
            bytes_field(out, "start_key", &value.start_key);
            bytes_field(out, "end_key", &value.end_key);
        },
    );
    optional_message(
        &mut output,
        "epoch_not_match",
        err.epoch_not_match.as_ref(),
        |out, value| {
            for region in &value.current_regions {
                message(out, "current_regions", region, region_fields);
            }
        },
    );
    optional_message(
        &mut output,
        "server_is_busy",
        err.server_is_busy.as_ref(),
        |out, value| {
            string_field(out, "reason", &value.reason);
            uint64_field(out, "backoff_ms", value.backoff_ms);
            uint32_field(out, "estimated_wait_ms", value.estimated_wait_ms);
            uint64_field(out, "applied_index", value.applied_index);
        },
    );
    optional_message(
        &mut output,
        "stale_command",
        err.stale_command.as_ref(),
        |_, _| {},
    );
    optional_message(
        &mut output,
        "store_not_match",
        err.store_not_match.as_ref(),
        |out, value| {
            uint64_field(out, "request_store_id", value.request_store_id);
            uint64_field(out, "actual_store_id", value.actual_store_id);
        },
    );
    optional_message(
        &mut output,
        "raft_entry_too_large",
        err.raft_entry_too_large.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
            uint64_field(out, "entry_size", value.entry_size);
        },
    );
    optional_message(
        &mut output,
        "max_timestamp_not_synced",
        err.max_timestamp_not_synced.as_ref(),
        |_, _| {},
    );
    optional_message(
        &mut output,
        "read_index_not_ready",
        err.read_index_not_ready.as_ref(),
        |out, value| {
            string_field(out, "reason", &value.reason);
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "proposal_in_merging_mode",
        err.proposal_in_merging_mode.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "data_is_not_ready",
        err.data_is_not_ready.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
            uint64_field(out, "peer_id", value.peer_id);
            uint64_field(out, "safe_ts", value.safe_ts);
        },
    );
    optional_message(
        &mut output,
        "region_not_initialized",
        err.region_not_initialized.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "disk_full",
        err.disk_full.as_ref(),
        |out, value| {
            for store_id in &value.store_id {
                uint64_repeated_field(out, "store_id", *store_id);
            }
            string_field(out, "reason", &value.reason);
        },
    );
    optional_message(
        &mut output,
        "RecoveryInProgress",
        err.recovery_in_progress.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "FlashbackInProgress",
        err.flashback_in_progress.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
            uint64_field(out, "flashback_start_ts", value.flashback_start_ts);
        },
    );
    optional_message(
        &mut output,
        "FlashbackNotPrepared",
        err.flashback_not_prepared.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "is_witness",
        err.is_witness.as_ref(),
        |out, value| {
            uint64_field(out, "region_id", value.region_id);
        },
    );
    optional_message(
        &mut output,
        "mismatch_peer_id",
        err.mismatch_peer_id.as_ref(),
        |out, value| {
            uint64_field(out, "request_peer_id", value.request_peer_id);
            uint64_field(out, "store_peer_id", value.store_peer_id);
        },
    );
    optional_message(
        &mut output,
        "bucket_version_not_match",
        err.bucket_version_not_match.as_ref(),
        |out, value| {
            uint64_field(out, "version", value.version);
            for key in &value.keys {
                bytes_field(out, "keys", key);
            }
        },
    );
    optional_message(
        &mut output,
        "undetermined_result",
        err.undetermined_result.as_ref(),
        |out, value| {
            string_field(out, "message", &value.message);
        },
    );
    output
}

/// 可选嵌套消息：仅在 `Some` 时写入 `name:<fields> `。
fn optional_message<T>(
    output: &mut String,
    name: &str,
    value: Option<&T>,
    fields: impl FnOnce(&mut String, &T),
) {
    if let Some(value) = value {
        message(output, name, value, fields);
    }
}

/// 写入嵌套消息块：`name:<fields> `。
fn message<T>(output: &mut String, name: &str, value: &T, fields: impl FnOnce(&mut String, &T)) {
    write!(output, "{name}:<").expect("writing to String cannot fail");
    fields(output, value);
    output.push_str("> ");
}

/// 非空字符串字段；值经 protobuf quote_escape 转义。
fn string_field(output: &mut String, name: &str, value: &str) {
    if !value.is_empty() {
        write!(
            output,
            "{name}:{} ",
            protobuf::text_format::quote_escape_bytes(value.as_bytes())
        )
        .expect("writing to String cannot fail");
    }
}

/// 非空字节字段；同样使用 quote_escape。
fn bytes_field(output: &mut String, name: &str, value: &[u8]) {
    if !value.is_empty() {
        write!(
            output,
            "{name}:{} ",
            protobuf::text_format::quote_escape_bytes(value)
        )
        .expect("writing to String cannot fail");
    }
}

/// 非零 u64 字段（零值按 protobuf 默认省略）。
fn uint64_field(output: &mut String, name: &str, value: u64) {
    if value != 0 {
        uint64_repeated_field(output, name, value);
    }
}

/// 重复的 u64 字段，始终写出（含零值）。
fn uint64_repeated_field(output: &mut String, name: &str, value: u64) {
    write!(output, "{name}:{value} ").expect("writing to String cannot fail");
}

/// 非零 u32 字段。
fn uint32_field(output: &mut String, name: &str, value: u32) {
    if value != 0 {
        write!(output, "{name}:{value} ").expect("writing to String cannot fail");
    }
}

/// 仅在 true 时写出布尔字段。
fn bool_field(output: &mut String, name: &str, value: bool) {
    if value {
        write!(output, "{name}:true ").expect("writing to String cannot fail");
    }
}

/// 序列化 metapb::Peer（副本）字段。
fn peer_fields(output: &mut String, peer: &kvproto::metapb::Peer) {
    uint64_field(output, "id", peer.id);
    uint64_field(output, "store_id", peer.store_id);
    if peer.role != kvproto::metapb::PeerRole::Voter {
        write!(output, "role:{} ", peer.role.descriptor().name())
            .expect("writing to String cannot fail");
    }
    bool_field(output, "is_witness", peer.is_witness);
}

/// 序列化 metapb::Region（键空间分片元数据）字段。
fn region_fields(output: &mut String, region: &kvproto::metapb::Region) {
    uint64_field(output, "id", region.id);
    bytes_field(output, "start_key", &region.start_key);
    bytes_field(output, "end_key", &region.end_key);
    optional_message(
        output,
        "region_epoch",
        region.region_epoch.as_ref(),
        |out, epoch| {
            uint64_field(out, "conf_ver", epoch.conf_ver);
            uint64_field(out, "version", epoch.version);
        },
    );
    for peer in &region.peers {
        message(output, "peers", peer, peer_fields);
    }
    optional_message(
        output,
        "encryption_meta",
        region.encryption_meta.as_ref(),
        |out, meta| {
            uint64_field(out, "key_id", meta.key_id);
            bytes_field(out, "iv", &meta.iv);
        },
    );
    bool_field(output, "is_in_flashback", region.is_in_flashback);
    uint64_field(output, "flashback_start_ts", region.flashback_start_ts);
}

// PBError is a implementation of error.
// PBError 对应 Go 结构体，包装 errorpb.Error 指针作为 error 接口实现。
/// TiKV protobuf 请求错误的包装类型。
#[derive(Clone, Debug, Default)]
pub struct PBError {
    pub RequestErr: Option<errorpb::Error>,
}

impl PBError {
    // Error implements the error.
    pub fn Error(&self) -> String {
        match &self.RequestErr {
            Some(err) => compact_text_string(err),
            None => "<nil>".to_owned(),
        }
    }
}

/// Display 委托给 `Error()`，与 Go error.Error 字符串一致。
impl std::fmt::Display for PBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.Error())
    }
}

/// 实现标准 Error，便于作为错误链节点。
impl std::error::Error for PBError {}
