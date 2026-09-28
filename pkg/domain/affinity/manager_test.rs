// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// PdManager 行为单测。
//
// 用可编排结果的 MockPdClient 验证：skip_exist_check 创建、冲突回退、
// 查询过滤、按查询串长度 / HTTP 错误回退到 get-all 等路径。

#![allow(non_snake_case)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use astersql_domain_affinity::manager::{
    AffinityError, AffinityGroupKeyRange, AffinityGroupState, BackgroundContext, Context,
    MAX_AFFINITY_GROUP_IDS_COUNT, MAX_AFFINITY_GROUP_IDS_QUERY_LEN, Manager, PdClient, PdManager,
    affinity_group_ids_escaped_query_len, is_pd_http_status_error,
    should_use_get_all_affinity_groups,
};

static BACKGROUND_CONTEXT: BackgroundContext = BackgroundContext;

fn context() -> &'static dyn Context {
    &BACKGROUND_CONTEXT
}

/// 创建请求中的 Group id -> key ranges 映射别名。
type Groups = HashMap<String, Vec<AffinityGroupKeyRange>>;
/// Group 状态映射别名。
type States = HashMap<String, AffinityGroupState>;

#[derive(Default)]
/// 可记录调用并按队列返回预设结果的 PD Client Mock。
struct MockPdClient {
    create_calls: Mutex<Vec<(Groups, bool)>>,
    get_calls: Mutex<Vec<Vec<String>>>,
    get_all_calls: Mutex<usize>,
    create_results: Mutex<VecDeque<Result<States, AffinityError>>>,
    get_results: Mutex<VecDeque<Result<States, AffinityError>>>,
    get_all_results: Mutex<VecDeque<Result<States, AffinityError>>>,
}

impl PdClient for MockPdClient {
    fn create_affinity_groups(
        &self,
        _ctx: &dyn Context,
        groups: &Groups,
        skip_exist_check: bool,
    ) -> Result<States, AffinityError> {
        self.create_calls
            .lock()
            .unwrap()
            .push((groups.clone(), skip_exist_check));
        self.create_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(HashMap::new()))
    }

    fn batch_delete_affinity_groups(
        &self,
        _ctx: &dyn Context,
        _ids: &[String],
        _force: bool,
    ) -> Result<(), AffinityError> {
        Ok(())
    }

    fn get_affinity_groups(
        &self,
        _ctx: &dyn Context,
        ids: &[String],
    ) -> Result<States, AffinityError> {
        self.get_calls.lock().unwrap().push(ids.to_vec());
        self.get_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(HashMap::new()))
    }

    fn get_all_affinity_groups(&self, _ctx: &dyn Context) -> Result<States, AffinityError> {
        *self.get_all_calls.lock().unwrap() += 1;
        self.get_all_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(HashMap::new()))
    }
}

/// 构造仅含一个 range 的列表。
fn key_range(start: &[u8], end: &[u8]) -> Vec<AffinityGroupKeyRange> {
    vec![AffinityGroupKeyRange::new(start, end)]
}

/// 构造两个示例 Affinity Group。
fn two_groups() -> Groups {
    HashMap::from([
        ("g1".into(), key_range(b"a", b"b")),
        ("g2".into(), key_range(b"c", b"d")),
    ])
}

/// 构造 range_count 为 0 的 Group 状态。
fn state(id: &str) -> AffinityGroupState {
    AffinityGroupState::new(id, 0)
}

/// 构造带 HTTP 状态码的 PD API 错误（消息格式对齐生产路径）。
fn status_error(code: u16, text: &str) -> AffinityError {
    AffinityError::new(format!(
        "request pd http api failed with status: '{code} {text}', body: 'test'"
    ))
}

#[test]
/// 验证首次创建走 skip_exist_check=true，且不触发 get。
fn TestCreateAffinityGroupsIfNotExistsUseSkipExistCheck() {
    let client = Arc::new(MockPdClient::default());
    let manager = PdManager::new(client.clone());
    let groups = HashMap::from([("g1".into(), key_range(b"a", b"b"))]);

    manager
        .create_affinity_groups_if_not_exists(context(), &groups)
        .unwrap();

    assert_eq!(*client.create_calls.lock().unwrap(), vec![(groups, true)]);
    assert!(client.get_calls.lock().unwrap().is_empty());
}

#[test]
/// 验证 409 冲突时回退：仅创建尚不存在的 Group。
fn TestCreateAffinityGroupsIfNotExistsFallbackWhenSkipExistRejected() {
    let client = Arc::new(MockPdClient::default());
    client
        .create_results
        .lock()
        .unwrap()
        .push_back(Err(status_error(409, "Conflict")));
    client
        .get_results
        .lock()
        .unwrap()
        .push_back(Ok(HashMap::from([("g1".into(), state("g1"))])));
    let manager = PdManager::new(client.clone());
    let groups = two_groups();

    manager
        .create_affinity_groups_if_not_exists(context(), &groups)
        .unwrap();

    assert_eq!(
        *client.create_calls.lock().unwrap(),
        vec![
            (groups.clone(), true),
            (HashMap::from([("g2".into(), groups["g2"].clone())]), false),
        ]
    );
    assert_eq!(
        *client.get_calls.lock().unwrap(),
        vec![vec!["g1".to_string(), "g2".to_string()]]
    );
}

#[test]
/// 验证无状态码的 HTTP 服务错误也会触发创建回退。
fn TestCreateAffinityGroupsIfNotExistsFallbackForHTTPServiceError() {
    let client = Arc::new(MockPdClient::default());
    client
        .create_results
        .lock()
        .unwrap()
        .push_back(Err(AffinityError::http_service("mock service error")));
    client
        .get_results
        .lock()
        .unwrap()
        .push_back(Ok(HashMap::from([("g1".into(), state("g1"))])));
    let manager = PdManager::new(client.clone());
    let groups = two_groups();

    manager
        .create_affinity_groups_if_not_exists(context(), &groups)
        .unwrap();

    assert_eq!(client.create_calls.lock().unwrap().len(), 2);
    assert_eq!(
        client.create_calls.lock().unwrap()[1],
        (HashMap::from([("g2".into(), groups["g2"].clone())]), false)
    );
}

#[test]
/// 验证非兼容性错误（如 500）不回退，直接返回。
fn TestCreateAffinityGroupsIfNotExistsDoNotFallbackForNonCompatibilityError() {
    let client = Arc::new(MockPdClient::default());
    let expected = status_error(500, "Internal Server Error");
    client
        .create_results
        .lock()
        .unwrap()
        .push_back(Err(expected.clone()));
    let manager = PdManager::new(client.clone());

    let actual = manager
        .create_affinity_groups_if_not_exists(
            context(),
            &HashMap::from([("g1".into(), key_range(b"a", b"b"))]),
        )
        .unwrap_err();

    assert_eq!(actual, expected);
    assert!(client.get_calls.lock().unwrap().is_empty());
}

#[test]
/// 验证直连 get 结果会按请求 ids 过滤。
fn TestGetAffinityGroupsFilterDirectResponse() {
    let client = Arc::new(MockPdClient::default());
    client
        .get_results
        .lock()
        .unwrap()
        .push_back(Ok(HashMap::from([
            ("g1".into(), state("g1")),
            ("g2".into(), state("g2")),
            ("g3".into(), state("g3")),
        ])));
    let manager = PdManager::new(client.clone());
    let ids = vec!["g1".into(), "g2".into()];

    let result = manager.get_affinity_groups(context(), &ids).unwrap();

    assert_eq!(result.len(), 2);
    assert!(result.contains_key("g1"));
    assert!(result.contains_key("g2"));
    assert!(!result.contains_key("g3"));
    assert_eq!(*client.get_calls.lock().unwrap(), vec![ids]);
}

#[test]
/// 验证编码后查询串过长时直接走 get-all，不调用按 ids 接口。
fn TestGetAffinityGroupsFallbackByEscapedQueryLen() {
    let client = Arc::new(MockPdClient::default());
    let id = "/".repeat(1365);
    client
        .get_all_results
        .lock()
        .unwrap()
        .push_back(Ok(HashMap::from([
            (id.clone(), state(&id)),
            ("other".into(), state("other")),
        ])));
    let manager = PdManager::new(client.clone());

    let result = manager
        .get_affinity_groups(context(), std::slice::from_ref(&id))
        .unwrap();

    assert_eq!(result, HashMap::from([(id.clone(), state(&id))]));
    assert!(client.get_calls.lock().unwrap().is_empty());
    assert_eq!(*client.get_all_calls.lock().unwrap(), 1);
}

#[test]
/// 验证旧 PD 返回 400 时回退到 get-all。
fn TestGetAffinityGroupsFallbackWhenIDsQueryUnsupported() {
    let client = Arc::new(MockPdClient::default());
    client
        .get_results
        .lock()
        .unwrap()
        .push_back(Err(status_error(400, "Bad Request")));
    client
        .get_all_results
        .lock()
        .unwrap()
        .push_back(Ok(HashMap::from([
            ("g1".into(), state("g1")),
            ("g2".into(), state("g2")),
        ])));
    let manager = PdManager::new(client.clone());

    let result = manager
        .get_affinity_groups(context(), &["g1".into()])
        .unwrap();

    assert_eq!(result, HashMap::from([("g1".into(), state("g1"))]));
    assert_eq!(*client.get_all_calls.lock().unwrap(), 1);
}

#[test]
/// 验证无状态码服务错误时查询也会回退到 get-all。
fn TestGetAffinityGroupsFallbackForHTTPServiceError() {
    let client = Arc::new(MockPdClient::default());
    client
        .get_results
        .lock()
        .unwrap()
        .push_back(Err(AffinityError::http_service("mock service error")));
    client
        .get_all_results
        .lock()
        .unwrap()
        .push_back(Ok(HashMap::from([
            ("g1".into(), state("g1")),
            ("g2".into(), state("g2")),
        ])));
    let manager = PdManager::new(client.clone());

    let result = manager
        .get_affinity_groups(context(), &["g1".into()])
        .unwrap();

    assert_eq!(result, HashMap::from([("g1".into(), state("g1"))]));
    assert_eq!(*client.get_all_calls.lock().unwrap(), 1);
}

#[test]
/// 验证 URL 编码查询长度边界与 Go 侧常量一致。
fn TestAffinityGroupIDsEscapedQueryLenBoundary() {
    assert_eq!(
        affinity_group_ids_escaped_query_len(&["/".repeat(1364)]),
        MAX_AFFINITY_GROUP_IDS_QUERY_LEN
    );
    assert_eq!(
        affinity_group_ids_escaped_query_len(&["/".repeat(1365)]),
        MAX_AFFINITY_GROUP_IDS_QUERY_LEN + 3
    );
}

#[test]
/// 验证 Group 数量超过阈值时改走 get-all。
fn TestShouldUseGetAllAffinityGroupsByIDCount() {
    let ids: Vec<_> = (0..=MAX_AFFINITY_GROUP_IDS_COUNT)
        .map(|i| format!("g{i}"))
        .collect();
    assert!(should_use_get_all_affinity_groups(&ids));
}

#[test]
/// 验证 HTTP 状态错误只按状态码匹配，不依赖文案。
fn TestIsPDHTTPStatusErrorMatchByCodeOnly() {
    let err = AffinityError::new(
        "request pd http api failed with status: '400 UnknownText', body: 'test'",
    );
    assert!(is_pd_http_status_error(&err, 400));
    assert!(!is_pd_http_status_error(&err, 409));
}
