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

// AsterSQL 迁移回归：资源组标签解码、键 Label 与请求首键提取。
//
// 覆盖 tipb 线格式、行/索引键分类，以及 Get/Scan/Prewrite 等各 RPC 分支，
// 行为对齐 Go `pkg/util/resourcegrouptag`。

use crate::kvproto::{coprocessor, kvrpcpb};
use crate::resource_group_tag::{
    DecodeResourceGroupTag, GetFirstKeyFromRequest, GetResourceGroupLabelByKey, Request,
    RequestPayload,
};
use crate::tipb::{ResourceGroupTag, ResourceGroupTagLabel};
use protobuf::Message;

/// 空输入返回 None；合法 tipb 返回 sql_digest；畸形字节返回错误。
#[test]
fn migration_decodes_tipb_wire_data_and_rejects_malformed_input() {
    assert_eq!(DecodeResourceGroupTag(&[]).unwrap(), None);

    let digest = vec![0x12; 32];
    let mut tag = ResourceGroupTag::new();
    tag.set_sql_digest(digest.clone());
    tag.set_plan_digest(vec![0x34; 32]);
    let encoded = tag.write_to_bytes().unwrap();
    assert_eq!(encoded.len(), 68);
    assert_eq!(DecodeResourceGroupTag(&encoded).unwrap(), Some(digest));

    let error = DecodeResourceGroupTag(&[0xff, 0x01]).unwrap_err();
    assert_eq!(error.to_string(), "invalid resource group tag data ff01");
}

/// 行键 `_r`、索引键 `_i` 与空键的 Label 分类对照 Go。
#[test]
fn migration_classifies_row_index_and_unknown_keys_like_go() {
    let row = [116, 128, 0, 0, 0, 0, 0, 0, 0, 95, 114];
    let index = [
        116, 128, 0, 0, 0, 0, 0, 0, 0, 95, 105, 128, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(
        GetResourceGroupLabelByKey(&row),
        ResourceGroupTagLabel::ResourceGroupTagLabelRow
    );
    assert_eq!(
        GetResourceGroupLabelByKey(&index),
        ResourceGroupTagLabel::ResourceGroupTagLabelIndex
    );
    assert_eq!(
        GetResourceGroupLabelByKey(b""),
        ResourceGroupTagLabel::ResourceGroupTagLabelUnknown
    );
}

/// 遍历 Go 侧所有请求分支，断言首键提取规则一致。
#[test]
fn migration_extracts_first_key_from_every_go_request_branch() {
    let k1 = b"TEST-1".to_vec();
    let k2 = b"TEST-2".to_vec();
    assert_eq!(GetFirstKeyFromRequest(None), None);

    let request = |payload| Request { payload };
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Get(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Get(Some(
            kvrpcpb::GetRequest::new(),
        ))))),
        None
    );
    let mut get = kvrpcpb::GetRequest::new();
    get.set_key(k1.clone());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Get(Some(get))))),
        Some(k1.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchGet(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchGet(Some(
            kvrpcpb::BatchGetRequest::new(),
        ))))),
        None
    );
    let mut batch_get = kvrpcpb::BatchGetRequest::new();
    batch_get.set_keys(vec![k2.clone(), k1.clone()].into());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchGet(Some(batch_get))))),
        Some(k2.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Scan(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Scan(Some(
            kvrpcpb::ScanRequest::new(),
        ))))),
        None
    );
    let mut scan = kvrpcpb::ScanRequest::new();
    scan.set_start_key(k1.clone());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Scan(Some(scan))))),
        Some(k1.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Prewrite(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Prewrite(Some(
            kvrpcpb::PrewriteRequest::new(),
        ))))),
        None
    );
    let mut mutation = kvrpcpb::Mutation::new();
    mutation.set_key(k2.clone());
    let mut prewrite = kvrpcpb::PrewriteRequest::new();
    prewrite.mut_mutations().push(mutation.clone());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Prewrite(Some(prewrite))))),
        Some(k2.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Commit(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Commit(Some(
            kvrpcpb::CommitRequest::new(),
        ))))),
        None
    );
    let mut commit = kvrpcpb::CommitRequest::new();
    commit.set_keys(vec![k1.clone(), k2.clone()].into());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Commit(Some(commit))))),
        Some(k1.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchRollback(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchRollback(Some(
            kvrpcpb::BatchRollbackRequest::new(),
        ))))),
        None
    );
    let mut rollback = kvrpcpb::BatchRollbackRequest::new();
    rollback.set_keys(vec![k2.clone(), k1.clone()].into());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchRollback(Some(
            rollback
        ))))),
        Some(k2.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Coprocessor(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Coprocessor(Some(
            coprocessor::Request::new(),
        ))))),
        None
    );
    let mut range = coprocessor::KeyRange::new();
    range.set_start(k1.clone());
    let mut cop = coprocessor::Request::new();
    cop.mut_ranges().push(range.clone());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Coprocessor(Some(cop))))),
        Some(k1.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchCoprocessor(None)))),
        None
    );
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchCoprocessor(Some(
            coprocessor::BatchRequest::new(),
        ))))),
        None
    );
    let mut empty_region_request = coprocessor::BatchRequest::new();
    empty_region_request
        .mut_regions()
        .push(coprocessor::RegionInfo::new());
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchCoprocessor(Some(
            empty_region_request,
        ))))),
        None
    );
    let mut region = coprocessor::RegionInfo::new();
    region.mut_ranges().push(range);
    let mut batch_cop = coprocessor::BatchRequest::new();
    batch_cop.mut_regions().push(region);
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::BatchCoprocessor(Some(
            batch_cop
        ))))),
        Some(k1.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::PessimisticLock(Some(
            kvrpcpb::PessimisticLockRequest::new(),
        ))))),
        None
    );
    let mut pessimistic = kvrpcpb::PessimisticLockRequest::new();
    pessimistic.mut_mutations().push(mutation);
    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::PessimisticLock(Some(
            pessimistic
        ))))),
        Some(k2.as_slice())
    );

    assert_eq!(
        GetFirstKeyFromRequest(Some(&request(RequestPayload::Other))),
        None
    );
}
