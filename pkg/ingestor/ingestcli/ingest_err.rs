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

// TiKV ingest 错误分类与 errorpb 解码。
//
// 将 HTTP 响应体中的 `errorpb.Error`（protobuf）解成 `ErrorPb`，再按与 Go 相同的
// 优先级映射为带稳定 RFC code 的 `IngestAPIError`，供上层决定重试或重启 Region 扫描。
// Region：TiKV 键范围分片；EpochNotMatch 表示分片元数据版本已变。

use astersql_ingestor_errdef as errdef;

/// The subset of errorpb.Error needed by ingest retry classification. It is
/// decoded from the real protobuf wire response by `decode_error_pb`.
/// ingest 重试分类所需的 errorpb.Error 子集，由 `decode_error_pb` 从 protobuf 线格式解码。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ErrorPb {
    pub message: String,
    pub not_leader: bool,
    pub region_not_found: bool,
    pub key_not_in_region: bool,
    pub epoch_not_match: bool,
    pub current_regions: Vec<crate::Region>,
    pub server_is_busy: bool,
    pub stale_command: bool,
    pub store_not_match: bool,
    pub raft_entry_too_large: bool,
    pub max_timestamp_not_synced: bool,
    pub read_index_not_ready: bool,
    pub proposal_in_merging_mode: bool,
    pub data_is_not_ready: bool,
    pub region_not_initialized: bool,
    pub disk_full: bool,
    pub recovery_in_progress: bool,
    pub flashback_in_progress: bool,
    pub flashback_not_prepared: bool,
    pub is_witness: bool,
    pub mismatch_peer_id: bool,
    pub bucket_version_not_match: bool,
    pub undetermined_result: bool,
}

/// A categorized error retains the stable errdef RFC code while adding the
/// TiKV response text, equivalent to GenWithStack in the Go implementation.
/// 带稳定 RFC code 的分类错误，并附加 TiKV 响应原文，等价于 Go 的 GenWithStack。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CategorizedError {
    pub category: errdef::NormalizedError,
    pub detail: String,
}

impl CategorizedError {
    /// 按 RFC code 判断是否属于同一错误类别。
    pub fn is(&self, category: &errdef::NormalizedError) -> bool {
        self.category.RFCCode() == category.RFCCode()
    }
}

impl std::fmt::Display for CategorizedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(formatter, "{}", self.category)
        } else {
            write!(formatter, "{}: {}", self.category, self.detail)
        }
    }
}

impl std::error::Error for CategorizedError {}

/// IngestAPIError is returned when transport succeeded but TiKV reported a
/// region error. `new_region` is populated only for EpochNotMatch.
/// 传输成功但 TiKV 返回 Region 错误时抛出；仅 EpochNotMatch 时填充 `new_region`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestAPIError {
    pub err: CategorizedError,
    pub new_region: Option<crate::RegionInfo>,
}

impl std::fmt::Display for IngestAPIError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.err)
    }
}

impl std::error::Error for IngestAPIError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.err)
    }
}

impl IngestAPIError {
    /// 返回内层分类错误（Go 风格 Cause）。
    pub fn Cause(&self) -> &CategorizedError {
        &self.err
    }

    /// 返回内层分类错误（Go 风格 Unwrap）。
    pub fn Unwrap(&self) -> &CategorizedError {
        &self.err
    }
}

/// 从 EpochNotMatch 附带的 Region 列表中提取可用 RegionInfo 的回调类型。
pub type RegionExtractFn = dyn Fn(&[crate::Region]) -> Option<crate::RegionInfo>;

/// NewIngestAPIError keeps the exact priority of the Go switch statement.
/// 按与 Go switch 完全相同的优先级，将 ErrorPb 映射为 IngestAPIError。
pub fn NewIngestAPIError(
    error_pb: &ErrorPb,
    extract_region_fn: Option<&RegionExtractFn>,
) -> IngestAPIError {
    let mut new_region = None;
    // 优先级：NotLeader → EpochNotMatch → raft dropped → busy → RegionNotFound →
    // ReadIndexNotReady → DiskFull → 其余归为 IngestFailed。
    let category = if error_pb.not_leader {
        &errdef::ErrKVNotLeader
    } else if error_pb.epoch_not_match {
        if let Some(extract) = extract_region_fn {
            new_region = extract(&error_pb.current_regions);
        }
        &errdef::ErrKVEpochNotMatch
    } else if error_pb.message.contains("raft: proposal dropped") {
        &errdef::ErrKVRaftProposalDropped
    } else if error_pb.server_is_busy {
        &errdef::ErrKVServerIsBusy
    } else if error_pb.region_not_found {
        &errdef::ErrKVRegionNotFound
    } else if error_pb.read_index_not_ready {
        &errdef::ErrKVReadIndexNotReady
    } else if error_pb.disk_full {
        &errdef::ErrKVDiskFull
    } else {
        &errdef::ErrKVIngestFailed
    };
    // 通用 IngestFailed 用 getIngestFailedMsg 补全变体名；其余直接用 message。
    let detail = if category.RFCCode() == errdef::ErrKVIngestFailed.RFCCode() {
        getIngestFailedMsg(error_pb)
    } else {
        error_pb.message.clone()
    };

    IngestAPIError {
        err: CategorizedError {
            category: category.clone(),
            detail,
        },
        new_region,
    }
}

/// Adds a variant name for errorpb messages whose original message may be empty.
/// 当原始 message 可能为空时，为 errorpb 变体补上类型名，形如 "KeyNotInRegion ..."。
pub fn getIngestFailedMsg(error_pb: &ErrorPb) -> String {
    let error_type = if error_pb.key_not_in_region {
        "KeyNotInRegion"
    } else if error_pb.stale_command {
        "StaleCommand"
    } else if error_pb.store_not_match {
        "StoreNotMatch"
    } else if error_pb.raft_entry_too_large {
        "RaftEntryTooLarge"
    } else if error_pb.max_timestamp_not_synced {
        "MaxTimestampNotSynced"
    } else if error_pb.proposal_in_merging_mode {
        "ProposalInMergingMode"
    } else if error_pb.data_is_not_ready {
        "DataIsNotReady"
    } else if error_pb.region_not_initialized {
        "RegionNotInitialized"
    } else if error_pb.recovery_in_progress {
        "RecoveryInProgress"
    } else if error_pb.flashback_in_progress {
        "FlashbackInProgress"
    } else if error_pb.flashback_not_prepared {
        "FlashbackNotPrepared"
    } else if error_pb.is_witness {
        "IsWitness"
    } else if error_pb.mismatch_peer_id {
        "MismatchPeerId"
    } else if error_pb.bucket_version_not_match {
        "BucketVersionNotMatch"
    } else if error_pb.undetermined_result {
        "UndeterminedResult"
    } else {
        ""
    };
    match (error_type.is_empty(), error_pb.message.is_empty()) {
        (true, _) => error_pb.message.clone(),
        (false, true) => error_type.to_owned(),
        (false, false) => format!("{error_type} {}", error_pb.message),
    }
}

/// 从 protobuf 线格式解码 errorpb.Error 子集。
pub(crate) fn decode_error_pb(data: &[u8]) -> Result<ErrorPb, crate::Error> {
    let mut result = ErrorPb::default();
    for (field, wire, value) in fields(data)? {
        // field 1 = message（length-delimited string）。
        if field == 1 && wire == 2 {
            result.message = String::from_utf8(value.to_vec())
                .map_err(|error| crate::Error::Protobuf(error.to_string()))?;
            continue;
        }
        // 其余仅处理 wire type 2（嵌套消息/标志位子消息存在即置 true）。
        if wire != 2 {
            continue;
        }
        match field {
            2 => result.not_leader = true,
            3 => result.region_not_found = true,
            4 => result.key_not_in_region = true,
            5 => {
                result.epoch_not_match = true;
                result.current_regions = decode_epoch_not_match(value)?;
            }
            6 => result.server_is_busy = true,
            7 => result.stale_command = true,
            8 => result.store_not_match = true,
            9 => result.raft_entry_too_large = true,
            10 => result.max_timestamp_not_synced = true,
            11 => result.read_index_not_ready = true,
            12 => result.proposal_in_merging_mode = true,
            13 => result.data_is_not_ready = true,
            14 => result.region_not_initialized = true,
            15 => result.disk_full = true,
            16 => result.recovery_in_progress = true,
            17 => result.flashback_in_progress = true,
            18 => result.flashback_not_prepared = true,
            19 => result.is_witness = true,
            20 => result.mismatch_peer_id = true,
            21 => result.bucket_version_not_match = true,
            22 => result.undetermined_result = true,
            _ => {}
        }
    }
    Ok(result)
}

/// 解码 EpochNotMatch 子消息中的 current_regions 列表。
fn decode_epoch_not_match(data: &[u8]) -> Result<Vec<crate::Region>, crate::Error> {
    fields(data)?
        .into_iter()
        .filter(|(field, wire, _)| *field == 1 && *wire == 2)
        .map(|(_, _, value)| decode_region(value))
        .collect()
}

/// 解码 metapb.Region。
fn decode_region(data: &[u8]) -> Result<crate::Region, crate::Error> {
    let mut result = crate::Region::default();
    for (field, wire, value) in fields(data)? {
        match (field, wire) {
            (1, 0) => result.id = decode_varint_value(value)?,
            (2, 2) => result.start_key = value.to_vec(),
            (3, 2) => result.end_key = value.to_vec(),
            (4, 2) => result.region_epoch = Some(decode_epoch(value)?),
            (5, 2) => result.peers.push(decode_peer(value)?),
            _ => {}
        }
    }
    Ok(result)
}

/// 解码 RegionEpoch（conf_ver / version）。
fn decode_epoch(data: &[u8]) -> Result<crate::RegionEpoch, crate::Error> {
    let mut result = crate::RegionEpoch::default();
    for (field, wire, value) in fields(data)? {
        match (field, wire) {
            (1, 0) => result.conf_ver = decode_varint_value(value)?,
            (2, 0) => result.version = decode_varint_value(value)?,
            _ => {}
        }
    }
    Ok(result)
}

/// 解码 Peer（id / store_id）。
fn decode_peer(data: &[u8]) -> Result<crate::Peer, crate::Error> {
    let mut result = crate::Peer::default();
    for (field, wire, value) in fields(data)? {
        match (field, wire) {
            (1, 0) => result.id = decode_varint_value(value)?,
            (2, 0) => result.store_id = decode_varint_value(value)?,
            _ => {}
        }
    }
    Ok(result)
}

/// 解析 protobuf 字段序列为 (field_number, wire_type, value_bytes)。
fn fields(data: &[u8]) -> Result<Vec<(u64, u8, &[u8])>, crate::Error> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (tag, used) = decode_varint(&data[offset..])?;
        offset += used;
        let wire = (tag & 7) as u8;
        let field = tag >> 3;
        // Match generated protobuf decoders: field number zero is never valid,
        // and generated Go code stores it in an int32 before dispatch.
        if field == 0 || field > i32::MAX as u64 {
            return Err(invalid_wire());
        }
        let start = offset;
        match wire {
            0 => {
                let (_, used) = decode_varint(&data[offset..])?;
                offset += used;
            }
            1 => offset = offset.checked_add(8).ok_or_else(invalid_wire)?,
            2 => {
                let (length, used) = decode_varint(&data[offset..])?;
                offset += used;
                let content_start = offset;
                offset = offset
                    .checked_add(length as usize)
                    .ok_or_else(invalid_wire)?;
                if offset > data.len() {
                    return Err(invalid_wire());
                }
                result.push((field, wire, &data[content_start..offset]));
                continue;
            }
            5 => offset = offset.checked_add(4).ok_or_else(invalid_wire)?,
            _ => return Err(invalid_wire()),
        }
        if offset > data.len() {
            return Err(invalid_wire());
        }
        result.push((field, wire, &data[start..offset]));
    }
    Ok(result)
}

/// 将已切出的 varint 字节片解码为数值。
fn decode_varint_value(data: &[u8]) -> Result<u64, crate::Error> {
    decode_varint(data).map(|(value, _)| value)
}

/// 解码 protobuf varint，返回 (值, 消耗字节数)。
fn decode_varint(data: &[u8]) -> Result<(u64, usize), crate::Error> {
    let mut value = 0_u64;
    for (index, byte) in data.iter().copied().take(10).enumerate() {
        // A u64 varint's tenth byte may contain only its final (64th) bit.
        // gogo/protobuf's generated Go decoder rejects larger values as an
        // integer overflow; accepting them here could misclassify a response.
        if index == 9 && byte > 1 {
            return Err(invalid_wire());
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(invalid_wire())
}

/// 构造畸形 errorpb 响应错误。
fn invalid_wire() -> crate::Error {
    crate::Error::Protobuf("malformed errorpb.Error response".to_owned())
}
