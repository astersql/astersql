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

// 资源组标签解码、键 Label 判定与 TiKV RPC 首键提取。
//
// 资源组（Resource Group）按租户/业务隔离资源；标签随请求下发，含 SQL digest
// 等归因信息。本模块解码 tipb.ResourceGroupTag，按行/索引键打 Label，并从
// Get/Scan/Prewrite 等请求变体中取出第一个键供资源统计使用。

#![allow(non_snake_case)]

use crate::kvproto::{coprocessor, kvrpcpb};
use crate::rowindexcodec;
use crate::tipb::ResourceGroupTagLabel;

/// tipb 资源组标签字节无法解码时的错误（消息中含十六进制原文）。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid resource group tag data {encoded}")]
pub struct DecodeResourceGroupTagError {
    encoded: String,
}

// DecodeResourceGroupTag decodes a resource group tag and returns the SQL digest.
/// 解码资源组标签，成功时返回其中的 SQL digest（空输入为 None）。
pub fn DecodeResourceGroupTag(data: &[u8]) -> Result<Option<Vec<u8>>, DecodeResourceGroupTagError> {
    if data.is_empty() {
        return Ok(None);
    }

    decode_tag(data).ok_or_else(|| DecodeResourceGroupTagError {
        encoded: data.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}

// Match tipb's gogo decoder, including unknown groups unsupported by protobuf 2.8.
fn decode_tag(mut data: &[u8]) -> Option<Option<Vec<u8>>> {
    let mut digest = None;
    while !data.is_empty() {
        let tag = take_varint(&mut data)?;
        let field = (tag >> 3) as i32;
        let wire = tag & 7;
        if field <= 0 || wire == 4 {
            return None;
        }
        match field {
            1 | 2 | 5 => {
                if wire != 2 {
                    return None;
                }
                let len = usize::try_from(take_varint(&mut data)?).ok()?;
                let bytes = data.get(..len)?;
                if field == 1 {
                    digest = Some(bytes.to_vec());
                }
                data = &data[len..];
            }
            3 | 4 => {
                if wire != 0 {
                    return None;
                }
                take_varint(&mut data)?;
            }
            _ => skip_unknown(&mut data, wire)?,
        }
    }
    Some(digest)
}

fn take_varint(data: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = data.split_first()?;
        *data = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

// Gogo skips groups by depth (without requiring matching start/end field numbers).
// Iterate instead of recursing so deeply nested unknown data cannot overflow the stack.
fn skip_unknown(data: &mut &[u8], mut wire: u64) -> Option<()> {
    let mut depth = 0usize;
    loop {
        let len = match wire {
            0 => {
                take_varint(data)?;
                0
            }
            1 => 8,
            2 => usize::try_from(take_varint(data)?).ok()?,
            3 => {
                depth = depth.checked_add(1)?;
                0
            }
            4 => {
                depth = depth.checked_sub(1)?;
                0
            }
            5 => 4,
            _ => return None,
        };
        *data = data.get(len..)?;
        if depth == 0 {
            return Some(());
        }
        wire = take_varint(data)? & 7;
    }
}

// GetResourceGroupLabelByKey determines the ResourceGroupTagLabel of key.
/// 根据键前缀判定资源组 Label：行键 / 索引键 / 未知。
pub fn GetResourceGroupLabelByKey(key: &[u8]) -> ResourceGroupTagLabel {
    match rowindexcodec::GetKeyKind(key) {
        rowindexcodec::KeyKindRow => ResourceGroupTagLabel::ResourceGroupTagLabelRow,
        rowindexcodec::KeyKindIndex => ResourceGroupTagLabel::ResourceGroupTagLabelIndex,
        _ => ResourceGroupTagLabel::ResourceGroupTagLabelUnknown,
    }
}

/// Rust counterpart of the Go `tikvrpc.Request` interface payload.
/// 对应 Go `tikvrpc.Request` 载荷的枚举，覆盖常见 KV/Coprocessor 请求。
#[derive(Clone, Debug)]
pub enum RequestPayload {
    Get(Option<kvrpcpb::GetRequest>),
    BatchGet(Option<kvrpcpb::BatchGetRequest>),
    Scan(Option<kvrpcpb::ScanRequest>),
    /// 两阶段提交（2PC）预写阶段请求。
    Prewrite(Option<kvrpcpb::PrewriteRequest>),
    /// 两阶段提交提交阶段请求。
    Commit(Option<kvrpcpb::CommitRequest>),
    BatchRollback(Option<kvrpcpb::BatchRollbackRequest>),
    Coprocessor(Option<coprocessor::Request>),
    BatchCoprocessor(Option<coprocessor::BatchRequest>),
    PessimisticLock(Option<kvrpcpb::PessimisticLockRequest>),
    /// Lossless key projection of an in-memory Go request, before protobuf encoding.
    /// Use this for nil versus non-nil empty keys and nullable repeated messages;
    /// those distinctions are not representable on the proto3 wire.
    InMemory(InMemoryRequest),
    Other,
}

/// 包装 RequestPayload，便于按可选引用传入 GetFirstKeyFromRequest。
#[derive(Clone, Debug)]
pub struct Request {
    pub payload: RequestPayload,
}

/// Generated proto3 scalar bytes have no presence; absent/empty normalize to nil.
fn non_empty(bytes: &[u8]) -> Option<&[u8]> {
    (!bytes.is_empty()).then_some(bytes)
}

// GetFirstKeyFromRequest gets the first key from a TiKV RPC request.
/// 从各类 TiKV RPC 请求中提取第一个可用于归因的键。
pub fn GetFirstKeyFromRequest(req: Option<&Request>) -> Option<&[u8]> {
    match &req?.payload {
        RequestPayload::Get(request) => request.as_ref().and_then(|r| non_empty(r.get_key())),
        RequestPayload::BatchGet(request) => request
            .as_ref()
            .and_then(|r| r.get_keys().first())
            .map(Vec::as_slice),
        RequestPayload::Scan(request) => {
            request.as_ref().and_then(|r| non_empty(r.get_start_key()))
        }
        RequestPayload::Prewrite(request) => request
            .as_ref()
            .and_then(|r| r.get_mutations().first())
            .and_then(|mutation| non_empty(mutation.get_key())),
        RequestPayload::Commit(request) => request
            .as_ref()
            .and_then(|r| r.get_keys().first())
            .map(Vec::as_slice),
        RequestPayload::BatchRollback(request) => request
            .as_ref()
            .and_then(|r| r.get_keys().first())
            .map(Vec::as_slice),
        RequestPayload::Coprocessor(request) => request
            .as_ref()
            .and_then(|r| r.get_ranges().first())
            .and_then(|range| non_empty(range.get_start())),
        // Batch Cop：取首个 Region 的首个 range 起点。
        RequestPayload::BatchCoprocessor(request) => request
            .as_ref()
            .and_then(|r| r.get_regions().first())
            .and_then(|region| region.get_ranges().first())
            .and_then(|range| non_empty(range.get_start())),
        RequestPayload::PessimisticLock(request) => {
            // Go 对类型化 nil 会 panic；此处同样要求 Option 内已有请求体。
            let request = request
                .as_ref()
                .expect("Go would panic on a typed nil PessimisticLockRequest");
            request
                .get_mutations()
                .first()
                .and_then(|mutation| non_empty(mutation.get_key()))
        }
        RequestPayload::InMemory(request) => request.first_key(),
        RequestPayload::Other => None,
    }
}

/// Key fields inspected by GetFirstKeyFromRequest. None is a nil Go []byte;
/// Some(Vec::new()) is a non-nil empty slice. Other RPC fields are not inspected.
#[derive(Clone, Debug, Default)]
pub struct RequestKey {
    pub key: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestKeys {
    pub keys: Vec<Option<Vec<u8>>>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestMutations {
    pub mutations: Vec<Option<RequestKey>>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestRange {
    pub start: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestRanges {
    pub ranges: Vec<Option<RequestRange>>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestRegions {
    pub regions: Vec<Option<RequestRanges>>,
}

/// Exact projection of the fields read by the Go type switch. Outer None means
/// a typed nil request; nullable messages and byte slices remain distinct.
/// Keeping this projection before serialization preserves states that protobuf
/// generated messages necessarily erase. Lists retain order and nil elements.
#[derive(Clone, Debug)]
pub enum InMemoryRequest {
    Get(Option<RequestKey>),
    BatchGet(Option<RequestKeys>),
    Scan(Option<RequestRange>),
    Prewrite(Option<RequestMutations>),
    Commit(Option<RequestKeys>),
    BatchRollback(Option<RequestKeys>),
    Coprocessor(Option<RequestRanges>),
    BatchCoprocessor(Option<RequestRegions>),
    PessimisticLock(Option<RequestMutations>),
}

impl InMemoryRequest {
    fn first_key(&self) -> Option<&[u8]> {
        match self {
            Self::Get(r) => r.as_ref()?.key.as_deref(),
            Self::Scan(r) => r.as_ref()?.start.as_deref(),
            Self::BatchGet(r) | Self::Commit(r) | Self::BatchRollback(r) => {
                r.as_ref()?.keys.first()?.as_deref()
            }
            Self::Prewrite(r) => r.as_ref()?.mutations.first()?.as_ref()?.key.as_deref(),
            Self::Coprocessor(r) => r.as_ref()?.ranges.first()?.as_ref()?.start.as_deref(),
            Self::BatchCoprocessor(r) => r
                .as_ref()?
                .regions
                .first()?
                .as_ref()?
                .ranges
                .first()?
                .as_ref()?
                .start
                .as_deref(),
            Self::PessimisticLock(r) => r
                .as_ref()
                .expect("Go would panic on a typed nil PessimisticLockRequest")
                .mutations
                .first()?
                .as_ref()
                .expect("Go would panic on a nil first PessimisticLock mutation")
                .key
                .as_deref(),
        }
    }

    fn first_key_mut(&mut self) -> Option<&mut [u8]> {
        match self {
            Self::Get(r) => r.as_mut()?.key.as_deref_mut(),
            Self::Scan(r) => r.as_mut()?.start.as_deref_mut(),
            Self::BatchGet(r) | Self::Commit(r) | Self::BatchRollback(r) => {
                r.as_mut()?.keys.first_mut()?.as_deref_mut()
            }
            Self::Prewrite(r) => r
                .as_mut()?
                .mutations
                .first_mut()?
                .as_mut()?
                .key
                .as_deref_mut(),
            Self::Coprocessor(r) => r
                .as_mut()?
                .ranges
                .first_mut()?
                .as_mut()?
                .start
                .as_deref_mut(),
            Self::BatchCoprocessor(r) => r
                .as_mut()?
                .regions
                .first_mut()?
                .as_mut()?
                .ranges
                .first_mut()?
                .as_mut()?
                .start
                .as_deref_mut(),
            Self::PessimisticLock(r) => r
                .as_mut()
                .expect("Go would panic on a typed nil PessimisticLockRequest")
                .mutations
                .first_mut()?
                .as_mut()
                .expect("Go would panic on a nil first PessimisticLock mutation")
                .key
                .as_deref_mut(),
        }
    }
}

/// Writable counterpart to the borrowed first-key view: modifications update
/// the request's original bytes, just as writing through Go's returned slice.
pub fn GetFirstKeyFromRequestMut(req: Option<&mut Request>) -> Option<&mut [u8]> {
    fn non_empty(bytes: &mut Vec<u8>) -> Option<&mut [u8]> {
        (!bytes.is_empty()).then_some(bytes.as_mut_slice())
    }
    match &mut req?.payload {
        RequestPayload::Get(r) => non_empty(r.as_mut()?.mut_key()),
        RequestPayload::Scan(r) => non_empty(r.as_mut()?.mut_start_key()),
        RequestPayload::BatchGet(r) => Some(r.as_mut()?.mut_keys().first_mut()?.as_mut_slice()),
        RequestPayload::Commit(r) => Some(r.as_mut()?.mut_keys().first_mut()?.as_mut_slice()),
        RequestPayload::BatchRollback(r) => {
            Some(r.as_mut()?.mut_keys().first_mut()?.as_mut_slice())
        }
        RequestPayload::Prewrite(r) => {
            non_empty(r.as_mut()?.mut_mutations().first_mut()?.mut_key())
        }
        RequestPayload::Coprocessor(r) => {
            non_empty(r.as_mut()?.mut_ranges().first_mut()?.mut_start())
        }
        RequestPayload::BatchCoprocessor(r) => non_empty(
            r.as_mut()?
                .mut_regions()
                .first_mut()?
                .mut_ranges()
                .first_mut()?
                .mut_start(),
        ),
        RequestPayload::PessimisticLock(r) => non_empty(
            r.as_mut()
                .expect("Go would panic on a typed nil PessimisticLockRequest")
                .mut_mutations()
                .first_mut()?
                .mut_key(),
        ),
        RequestPayload::InMemory(r) => r.first_key_mut(),
        RequestPayload::Other => None,
    }
}
