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

// 资源组标签（Resource Group Tag）相关单测。
//
// 覆盖 protobuf 编解码、按 key 判定行/索引标签，以及从各类 KV/Coprocessor
// 请求中提取首个 key。资源组标签用于将 SQL 请求归因到资源组（resource group）
// 以便做流量隔离与配额控制。

use protobuf::Message;

use crate::kvproto::{coprocessor, kvrpcpb};
use crate::resource_group_tag::{
    GetFirstKeyFromRequest, GetResourceGroupLabelByKey, Request, RequestPayload,
};
use crate::tipb::{ResourceGroupTag, ResourceGroupTagLabel};

/// 构造带指定 payload 的测试请求。
fn request(payload: RequestPayload) -> Request {
    Request { payload }
}

/// 断言从请求中取出的首 key 与期望一致。
fn assert_first_key(req: &Request, expected: Option<&[u8]>) {
    assert_eq!(GetFirstKeyFromRequest(Some(req)).as_deref(), expected,);
}

/// 验证 ResourceGroupTag protobuf 编解码长度与字段回读。
#[test]
fn test_resource_group_tag_encoding_pb() {
    let digest1 = gen_digest("abc");
    let digest2 = gen_digest("abcdefg");

    let mut resource_tag = ResourceGroupTag::new();
    resource_tag.set_sql_digest(digest1.clone());
    resource_tag.set_plan_digest(digest2.clone());
    // gogo's nullable=false table_id is always emitted, including its zero value.
    resource_tag.set_table_id(0);
    let buf = resource_tag.write_to_bytes().unwrap();
    assert_eq!(buf.len(), 70);

    let mut tag = ResourceGroupTag::new();
    tag.merge_from_bytes(&buf).unwrap();
    assert_eq!(tag.get_sql_digest(), digest1.as_slice());
    assert_eq!(tag.get_plan_digest(), digest2.as_slice());

    let mut resource_tag = ResourceGroupTag::new();
    resource_tag.set_sql_digest(digest1.clone());
    resource_tag.set_table_id(0);
    let buf = resource_tag.write_to_bytes().unwrap();
    assert_eq!(buf.len(), 36);

    let mut tag = ResourceGroupTag::new();
    tag.merge_from_bytes(&buf).unwrap();
    assert_eq!(tag.get_sql_digest(), digest1.as_slice());
    assert!(!tag.has_plan_digest());
}

/// 按编码 key 前缀判断资源组标签是行（row）还是索引（index）。
#[test]
fn test_get_resource_group_label_by_key() {
    assert_eq!(
        GetResourceGroupLabelByKey(&[116, 128, 0, 0, 0, 0, 0, 0, 0, 95, 114]),
        ResourceGroupTagLabel::ResourceGroupTagLabelRow,
    );
    assert_eq!(
        GetResourceGroupLabelByKey(&[
            116, 128, 0, 0, 0, 0, 0, 0, 0, 95, 105, 128, 0, 0, 0, 0, 0, 0, 0,
        ]),
        ResourceGroupTagLabel::ResourceGroupTagLabelIndex,
    );
    assert_eq!(
        GetResourceGroupLabelByKey(&[]),
        ResourceGroupTagLabel::ResourceGroupTagLabelUnknown,
    );
}

/// 覆盖 Get/BatchGet/Scan/Prewrite/Commit/Rollback/Coprocessor 等请求的首 key 提取。
#[test]
fn test_get_first_key_from_request() {
    let test_k1 = b"TEST-1".to_vec();
    let test_k2 = b"TEST-2".to_vec();

    assert_eq!(GetFirstKeyFromRequest(None), None);

    let req = request(RequestPayload::Get(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Get(Some(kvrpcpb::GetRequest::new())));
    assert_first_key(&req, None);
    let mut get = kvrpcpb::GetRequest::new();
    get.set_key(test_k1.clone());
    let req = request(RequestPayload::Get(Some(get)));
    assert_first_key(&req, Some(&test_k1));

    let req = request(RequestPayload::BatchGet(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::BatchGet(Some(
        kvrpcpb::BatchGetRequest::new(),
    )));
    assert_first_key(&req, None);
    let req = request(RequestPayload::BatchGet(Some(
        kvrpcpb::BatchGetRequest::new(),
    )));
    assert_first_key(&req, None);
    let mut batch_get = kvrpcpb::BatchGetRequest::new();
    batch_get.mut_keys().push(test_k2.clone());
    batch_get.mut_keys().push(test_k1.clone());
    let req = request(RequestPayload::BatchGet(Some(batch_get)));
    assert_first_key(&req, Some(&test_k2));

    let req = request(RequestPayload::Scan(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Scan(Some(kvrpcpb::ScanRequest::new())));
    assert_first_key(&req, None);
    let mut scan = kvrpcpb::ScanRequest::new();
    scan.set_start_key(test_k1.clone());
    let req = request(RequestPayload::Scan(Some(scan)));
    assert_first_key(&req, Some(&test_k1));

    let req = request(RequestPayload::Prewrite(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Prewrite(Some(
        kvrpcpb::PrewriteRequest::new(),
    )));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Prewrite(Some(
        kvrpcpb::PrewriteRequest::new(),
    )));
    assert_first_key(&req, None);
    let mut mutation2 = kvrpcpb::Mutation::new();
    mutation2.set_key(test_k2.clone());
    let mut mutation1 = kvrpcpb::Mutation::new();
    mutation1.set_key(test_k1.clone());
    let mut prewrite = kvrpcpb::PrewriteRequest::new();
    prewrite.mut_mutations().push(mutation2);
    prewrite.mut_mutations().push(mutation1);
    let req = request(RequestPayload::Prewrite(Some(prewrite)));
    assert_first_key(&req, Some(&test_k2));

    let req = request(RequestPayload::Commit(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Commit(Some(kvrpcpb::CommitRequest::new())));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Commit(Some(kvrpcpb::CommitRequest::new())));
    assert_first_key(&req, None);
    let mut commit = kvrpcpb::CommitRequest::new();
    commit.mut_keys().push(test_k1.clone());
    commit.mut_keys().push(test_k1.clone());
    let req = request(RequestPayload::Commit(Some(commit)));
    assert_first_key(&req, Some(&test_k1));

    let req = request(RequestPayload::BatchRollback(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::BatchRollback(Some(
        kvrpcpb::BatchRollbackRequest::new(),
    )));
    assert_first_key(&req, None);
    let req = request(RequestPayload::BatchRollback(Some(
        kvrpcpb::BatchRollbackRequest::new(),
    )));
    assert_first_key(&req, None);
    let mut rollback = kvrpcpb::BatchRollbackRequest::new();
    rollback.mut_keys().push(test_k2.clone());
    rollback.mut_keys().push(test_k1.clone());
    let req = request(RequestPayload::BatchRollback(Some(rollback)));
    assert_first_key(&req, Some(&test_k2));

    let req = request(RequestPayload::Coprocessor(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Coprocessor(Some(
        coprocessor::Request::new(),
    )));
    assert_first_key(&req, None);
    let req = request(RequestPayload::Coprocessor(Some(
        coprocessor::Request::new(),
    )));
    assert_first_key(&req, None);
    let mut range = coprocessor::KeyRange::new();
    range.set_start(test_k1.clone());
    let mut cop = coprocessor::Request::new();
    cop.mut_ranges().push(range);
    let req = request(RequestPayload::Coprocessor(Some(cop)));
    assert_first_key(&req, Some(&test_k1));

    let req = request(RequestPayload::BatchCoprocessor(None));
    assert_first_key(&req, None);
    let req = request(RequestPayload::BatchCoprocessor(Some(
        coprocessor::BatchRequest::new(),
    )));
    assert_first_key(&req, None);
    let req = request(RequestPayload::BatchCoprocessor(Some(
        coprocessor::BatchRequest::new(),
    )));
    assert_first_key(&req, None);
    let mut batch = coprocessor::BatchRequest::new();
    batch.mut_regions().push(coprocessor::RegionInfo::new());
    let req = request(RequestPayload::BatchCoprocessor(Some(batch)));
    assert_first_key(&req, None);
    let mut batch = coprocessor::BatchRequest::new();
    batch.mut_regions().push(coprocessor::RegionInfo::new());
    let req = request(RequestPayload::BatchCoprocessor(Some(batch)));
    assert_first_key(&req, None);
    let mut range = coprocessor::KeyRange::new();
    range.set_start(test_k2.clone());
    let mut region = coprocessor::RegionInfo::new();
    region.mut_ranges().push(range);
    let mut batch = coprocessor::BatchRequest::new();
    batch.mut_regions().push(region);
    let req = request(RequestPayload::BatchCoprocessor(Some(batch)));
    assert_first_key(&req, Some(&test_k2));
}

/// 返回与 Go crypto/sha256 一致的硬编码摘要，避免引入 sha2 测试依赖。
fn gen_digest(value: &str) -> Vec<u8> {
    // Hard-coded SHA-256 digests matching Go's crypto/sha256 for the fixtures
    // used below, avoiding a dedicated sha2 test dependency.
    match value {
        "abc" => hex_decode("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "abcdefg" => hex_decode("7d1a54127b222502f5b79b5fb0803061152a44f92b37e23c6527baf665d4da9a"),
        other => panic!("unexpected digest fixture {other}"),
    }
}

/// 将十六进制字符串解码为字节向量。
fn hex_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex nibble"))
        .collect()
}

#[test]
fn test_decode_resource_group_tag_boundaries() {
    use crate::DecodeResourceGroupTag;
    for (wire, expected) in [
        (vec![], None),
        (vec![0x20, 0], None),
        (vec![0x0a, 0], Some(vec![])),
        (vec![0x0a, 1, b'a', 0x0a, 1, b'b'], Some(vec![b'b'])),
        (vec![0x0a, 1, b'a', 0x18, 99], Some(vec![b'a'])),
        (vec![0x3b, 0x30, 1, 0x3c, 0x0a, 1, b'a'], Some(vec![b'a'])),
    ] {
        assert_eq!(
            DecodeResourceGroupTag(&wire).unwrap(),
            expected,
            "{wire:x?}"
        );
    }
    for wire in [
        vec![0],
        vec![0x0a],
        vec![0x0a, 2, 1],
        vec![0x08, 1],
        vec![0x3c],
    ] {
        assert_eq!(
            DecodeResourceGroupTag(&wire).unwrap_err().to_string(),
            format!(
                "invalid resource group tag data {}",
                wire.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )
        );
    }
}

#[test]
fn test_decode_unknown_wire_types_and_truncation() {
    use crate::DecodeResourceGroupTag;
    // Unknown varint, fixed64, bytes, nested groups and fixed32 are all skipped.
    for unknown in [
        vec![0x38, 0xff, 1],
        vec![0x39, 0, 0, 0, 0, 0, 0, 0, 0],
        vec![0x3a, 2, 1, 2],
        vec![0x3b, 0x33, 0x34, 0x3c],
        vec![0x3d, 0, 0, 0, 0],
    ] {
        let mut wire = unknown.clone();
        wire.extend_from_slice(&[0x0a, 1, b'x']);
        assert_eq!(DecodeResourceGroupTag(&wire).unwrap(), Some(vec![b'x']));
        for end in 1..unknown.len() {
            assert!(DecodeResourceGroupTag(&unknown[..end]).is_err());
        }
    }
    for wire in [
        vec![0x3e],
        vec![0x3f],
        vec![0x80; 11],
        vec![0x38, 0x80],
        vec![0x1a, 0],
        vec![0x22, 0],
    ] {
        assert!(DecodeResourceGroupTag(&wire).is_err());
    }
    // Gogo's group skipper tracks depth, not field-number pairing.
    assert_eq!(DecodeResourceGroupTag(&[0x3b, 0x34]).unwrap(), None);
    let mut nested = vec![0x3b; 10_000];
    nested.extend(vec![0x3c; 10_000]);
    assert_eq!(DecodeResourceGroupTag(&nested).unwrap(), None);
}

#[test]
fn test_pessimistic_lock_and_unsupported_request() {
    assert_first_key(&request(RequestPayload::Other), None);
    let mut lock = kvrpcpb::PessimisticLockRequest::new();
    assert_first_key(
        &request(RequestPayload::PessimisticLock(Some(lock.clone()))),
        None,
    );
    let mut mutation = kvrpcpb::Mutation::new();
    lock.mut_mutations().push(mutation.clone());
    mutation.set_key(b"second".to_vec());
    lock.mut_mutations().push(mutation);
    assert_first_key(
        &request(RequestPayload::PessimisticLock(Some(lock.clone()))),
        None,
    );
    lock.mut_mutations()[0].set_key(b"first".to_vec());
    assert_first_key(
        &request(RequestPayload::PessimisticLock(Some(lock))),
        Some(b"first"),
    );
}

#[test]
#[should_panic(expected = "typed nil PessimisticLockRequest")]
fn test_pessimistic_lock_typed_nil_panics_like_go() {
    GetFirstKeyFromRequest(Some(&request(RequestPayload::PessimisticLock(None))));
}

#[test]
fn test_first_key_borrows_request_storage() {
    let mut get = kvrpcpb::GetRequest::new();
    get.set_key(b"borrowed".to_vec());
    let req = request(RequestPayload::Get(Some(get)));
    let RequestPayload::Get(Some(get)) = &req.payload else {
        unreachable!()
    };
    let key = GetFirstKeyFromRequest(Some(&req)).unwrap();
    assert_eq!(key.as_ptr(), get.get_key().as_ptr());
}

/// The same Go slice states in all nine request branches, including an empty
/// first key followed by a nonempty key (which must never be substituted).
#[test]
fn test_in_memory_go_key_presence_and_aliasing() {
    use crate::{
        GetFirstKeyFromRequestMut, InMemoryRequest as R, RequestKey, RequestKeys, RequestMutations,
        RequestRange, RequestRanges, RequestRegions,
    };
    for key in [None, Some(vec![]), Some(b"first".to_vec())] {
        let keyed = RequestKey { key: key.clone() };
        let keys = RequestKeys {
            keys: vec![key.clone(), Some(b"second".to_vec())],
        };
        let mutations = RequestMutations {
            mutations: vec![
                Some(keyed.clone()),
                Some(RequestKey {
                    key: Some(b"second".to_vec()),
                }),
            ],
        };
        let range = RequestRange { start: key.clone() };
        let ranges = RequestRanges {
            ranges: vec![
                Some(range.clone()),
                Some(RequestRange {
                    start: Some(b"second".to_vec()),
                }),
            ],
        };
        let regions = RequestRegions {
            regions: vec![
                Some(ranges.clone()),
                Some(RequestRanges {
                    ranges: vec![Some(RequestRange {
                        start: Some(b"third".to_vec()),
                    })],
                }),
            ],
        };
        for payload in [
            R::Get(Some(keyed)),
            R::BatchGet(Some(keys.clone())),
            R::Scan(Some(range)),
            R::Prewrite(Some(mutations.clone())),
            R::Commit(Some(keys.clone())),
            R::BatchRollback(Some(keys)),
            R::Coprocessor(Some(ranges)),
            R::BatchCoprocessor(Some(regions)),
            R::PessimisticLock(Some(mutations)),
        ] {
            let mut req = request(RequestPayload::InMemory(payload));
            assert_eq!(GetFirstKeyFromRequest(Some(&req)), key.as_deref());
            let read_ptr = GetFirstKeyFromRequest(Some(&req)).map(<[u8]>::as_ptr);
            let writable = GetFirstKeyFromRequestMut(Some(&mut req));
            assert_eq!(writable.as_deref(), key.as_deref());
            assert_eq!(writable.as_deref().map(<[u8]>::as_ptr), read_ptr);
            if let Some(bytes) = writable {
                if !bytes.is_empty() {
                    bytes[0] = b'F';
                }
            }
            let expected = key.as_ref().map(|bytes| {
                let mut bytes = bytes.clone();
                if !bytes.is_empty() {
                    bytes[0] = b'F';
                }
                bytes
            });
            assert_eq!(GetFirstKeyFromRequest(Some(&req)), expected.as_deref());
        }
    }
}

#[test]
fn test_in_memory_go_nil_messages_and_empty_lists() {
    use crate::{
        GetFirstKeyFromRequestMut, InMemoryRequest as R, RequestKey, RequestKeys, RequestMutations,
        RequestRange, RequestRanges, RequestRegions,
    };
    let later = Some(RequestRange {
        start: Some(b"later".to_vec()),
    });
    let ranges = RequestRanges {
        ranges: vec![None, later],
    };
    let mutations = RequestMutations {
        mutations: vec![
            None,
            Some(RequestKey {
                key: Some(b"later".to_vec()),
            }),
        ],
    };
    for payload in [
        R::Get(None),
        R::BatchGet(None),
        R::Scan(None),
        R::Prewrite(None),
        R::Commit(None),
        R::BatchRollback(None),
        R::Coprocessor(None),
        R::BatchCoprocessor(None),
        R::BatchGet(Some(RequestKeys::default())),
        R::Prewrite(Some(RequestMutations::default())),
        R::Commit(Some(RequestKeys::default())),
        R::BatchRollback(Some(RequestKeys::default())),
        R::Coprocessor(Some(RequestRanges::default())),
        R::BatchCoprocessor(Some(RequestRegions::default())),
        R::PessimisticLock(Some(RequestMutations::default())),
        R::Prewrite(Some(mutations)),
        R::Coprocessor(Some(ranges.clone())),
        R::BatchCoprocessor(Some(RequestRegions {
            regions: vec![None, Some(ranges.clone())],
        })),
        R::BatchCoprocessor(Some(RequestRegions {
            regions: vec![Some(RequestRanges::default()), Some(ranges.clone())],
        })),
        R::BatchCoprocessor(Some(RequestRegions {
            regions: vec![Some(ranges)],
        })),
    ] {
        let mut req = request(RequestPayload::InMemory(payload));
        assert_eq!(GetFirstKeyFromRequest(Some(&req)), None, "{req:?}");
        assert_eq!(GetFirstKeyFromRequestMut(Some(&mut req)), None);
    }
}

#[test]
fn test_in_memory_pessimistic_lock_nil_panics_in_both_views() {
    use crate::{GetFirstKeyFromRequestMut, InMemoryRequest, RequestMutations};
    for lock in [
        None,
        Some(RequestMutations {
            mutations: vec![None],
        }),
    ] {
        let mut req = request(RequestPayload::InMemory(InMemoryRequest::PessimisticLock(
            lock,
        )));
        assert!(std::panic::catch_unwind(|| GetFirstKeyFromRequest(Some(&req))).is_err());
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                GetFirstKeyFromRequestMut(Some(&mut req));
            }))
            .is_err()
        );
    }
}

#[test]
fn test_generated_request_mutable_views() {
    use crate::GetFirstKeyFromRequestMut;
    let mut get = kvrpcpb::GetRequest::new();
    get.set_key(b"key".to_vec());
    let mut scan = kvrpcpb::ScanRequest::new();
    scan.set_start_key(b"key".to_vec());
    let mut batch_get = kvrpcpb::BatchGetRequest::new();
    batch_get.mut_keys().push(b"key".to_vec());
    let mut commit = kvrpcpb::CommitRequest::new();
    commit.mut_keys().push(b"key".to_vec());
    let mut rollback = kvrpcpb::BatchRollbackRequest::new();
    rollback.mut_keys().push(b"key".to_vec());
    let mut mutation = kvrpcpb::Mutation::new();
    mutation.set_key(b"key".to_vec());
    let mut prewrite = kvrpcpb::PrewriteRequest::new();
    prewrite.mut_mutations().push(mutation.clone());
    let mut lock = kvrpcpb::PessimisticLockRequest::new();
    lock.mut_mutations().push(mutation);
    let mut range = coprocessor::KeyRange::new();
    range.set_start(b"key".to_vec());
    let mut cop = coprocessor::Request::new();
    cop.mut_ranges().push(range.clone());
    let mut region = coprocessor::RegionInfo::new();
    region.mut_ranges().push(range);
    let mut batch = coprocessor::BatchRequest::new();
    batch.mut_regions().push(region);
    for payload in [
        RequestPayload::Get(Some(get)),
        RequestPayload::BatchGet(Some(batch_get)),
        RequestPayload::Scan(Some(scan)),
        RequestPayload::Prewrite(Some(prewrite)),
        RequestPayload::Commit(Some(commit)),
        RequestPayload::BatchRollback(Some(rollback)),
        RequestPayload::Coprocessor(Some(cop)),
        RequestPayload::BatchCoprocessor(Some(batch)),
        RequestPayload::PessimisticLock(Some(lock)),
    ] {
        let mut req = request(payload);
        let ptr = GetFirstKeyFromRequest(Some(&req)).unwrap().as_ptr();
        let key = GetFirstKeyFromRequestMut(Some(&mut req)).unwrap();
        assert_eq!(key.as_ptr(), ptr);
        key[0] = b'K';
        assert_eq!(GetFirstKeyFromRequest(Some(&req)), Some(b"Key".as_slice()));
    }
    assert_eq!(GetFirstKeyFromRequestMut(None), None);
    assert_eq!(
        GetFirstKeyFromRequestMut(Some(&mut request(RequestPayload::Other))),
        None
    );
}
