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

// Affinity 迁移期 Aster 单元测试。
//
// 覆盖 PdManager 创建回退、查询过滤 / 全量扫描、Mock 幂等，
// 以及包级 `delete_groups_with_retry` 的强制删除与重试语义。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_domain_affinity::manager::{
    AffinityError, AffinityGroupKeyRange, AffinityGroupState, BackgroundContext, Context,
    MAX_AFFINITY_GROUP_IDS_COUNT, MAX_AFFINITY_GROUP_IDS_QUERY_LEN, Manager, PdClient, PdManager,
    affinity_group_ids_escaped_query_len, filter_affinity_groups, new_mock_manager,
    should_use_get_all_affinity_groups,
};

static BACKGROUND_CONTEXT: BackgroundContext = BackgroundContext;

fn context() -> &'static dyn Context {
    &BACKGROUND_CONTEXT
}

#[derive(Default)]
/// 记录调用参数并可预设返回值的 PD Client。
struct RecordingClient {
    create_calls: Mutex<Vec<(Vec<String>, bool)>>,
    delete_calls: Mutex<Vec<(Vec<String>, bool)>>,
    get_calls: Mutex<Vec<Vec<String>>>,
    get_all_calls: Mutex<usize>,
    create_results: Mutex<Vec<Result<HashMap<String, AffinityGroupState>, AffinityError>>>,
    delete_results: Mutex<Vec<Result<(), AffinityError>>>,
    get_result: Mutex<Option<Result<HashMap<String, AffinityGroupState>, AffinityError>>>,
    all_groups: Mutex<HashMap<String, AffinityGroupState>>,
}

impl PdClient for RecordingClient {
    fn create_affinity_groups(
        &self,
        _ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
        skip_exist_check: bool,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        let mut ids: Vec<_> = groups.keys().cloned().collect();
        ids.sort();
        self.create_calls
            .lock()
            .unwrap()
            .push((ids, skip_exist_check));
        let mut results = self.create_results.lock().unwrap();
        if results.is_empty() {
            Ok(HashMap::new())
        } else {
            results.remove(0)
        }
    }

    fn batch_delete_affinity_groups(
        &self,
        _ctx: &dyn Context,
        ids: &[String],
        force: bool,
    ) -> Result<(), AffinityError> {
        self.delete_calls
            .lock()
            .unwrap()
            .push((ids.to_vec(), force));
        let mut results = self.delete_results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }

    fn get_affinity_groups(
        &self,
        _ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        self.get_calls.lock().unwrap().push(ids.to_vec());
        self.get_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Ok(HashMap::new()))
    }

    fn get_all_affinity_groups(
        &self,
        _ctx: &dyn Context,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        *self.get_all_calls.lock().unwrap() += 1;
        Ok(self.all_groups.lock().unwrap().clone())
    }
}

/// 构造简易 Group 状态。
fn state(id: &str) -> AffinityGroupState {
    AffinityGroupState::new(id, 0)
}

/// 构造两个示例 Group。
fn groups() -> HashMap<String, Vec<AffinityGroupKeyRange>> {
    HashMap::from([
        ("g1".into(), vec![AffinityGroupKeyRange::new(b"a", b"b")]),
        ("g2".into(), vec![AffinityGroupKeyRange::new(b"c", b"d")]),
    ])
}

#[test]
/// 首次创建应带 skip_exist_check=true。
fn create_uses_skip_exist_check_on_first_attempt() {
    let client = Arc::new(RecordingClient::default());
    let manager = PdManager::new(client.clone());
    manager
        .create_affinity_groups_if_not_exists(context(), &groups())
        .unwrap();
    assert_eq!(
        *client.create_calls.lock().unwrap(),
        vec![(vec!["g1".into(), "g2".into()], true)]
    );
    assert!(client.get_calls.lock().unwrap().is_empty());
}

#[test]
/// 409 后回退，仅创建缺失 Group。
fn create_falls_back_and_only_creates_missing_groups() {
    let client = Arc::new(RecordingClient::default());
    client
        .create_results
        .lock()
        .unwrap()
        .push(Err(AffinityError::http_status(409, "conflict")));
    *client.get_result.lock().unwrap() = Some(Ok(HashMap::from([("g1".into(), state("g1"))])));
    let manager = PdManager::new(client.clone());
    manager
        .create_affinity_groups_if_not_exists(context(), &groups())
        .unwrap();
    assert_eq!(
        *client.create_calls.lock().unwrap(),
        vec![
            (vec!["g1".into(), "g2".into()], true),
            (vec!["g2".into()], false)
        ]
    );
}

#[test]
/// 非兼容性状态码（500）不回退。
fn create_does_not_fallback_for_unrelated_status() {
    let client = Arc::new(RecordingClient::default());
    client
        .create_results
        .lock()
        .unwrap()
        .push(Err(AffinityError::http_status(500, "failure")));
    let manager = PdManager::new(client.clone());
    assert_eq!(
        manager
            .create_affinity_groups_if_not_exists(context(), &groups())
            .unwrap_err()
            .status_code(),
        Some(500)
    );
    assert!(client.get_calls.lock().unwrap().is_empty());
}

#[test]
/// 无状态码服务错误触发创建回退。
fn create_falls_back_for_http_service_error_without_status() {
    let client = Arc::new(RecordingClient::default());
    client
        .create_results
        .lock()
        .unwrap()
        .push(Err(AffinityError::http_service("service error")));
    *client.get_result.lock().unwrap() = Some(Ok(HashMap::from([("g1".into(), state("g1"))])));
    let manager = PdManager::new(client.clone());
    manager
        .create_affinity_groups_if_not_exists(context(), &groups())
        .unwrap();
    assert_eq!(client.create_calls.lock().unwrap().len(), 2);
    assert_eq!(
        client.create_calls.lock().unwrap()[1],
        (vec!["g2".into()], false)
    );
}

#[test]
/// 直连结果过滤，且旧 PD 400 时回退 get-all。
fn get_filters_direct_response_and_falls_back_for_old_pd() {
    let client = Arc::new(RecordingClient::default());
    *client.get_result.lock().unwrap() = Some(Ok(HashMap::from([
        ("g1".into(), state("g1")),
        ("g2".into(), state("g2")),
        ("g3".into(), state("g3")),
    ])));
    let manager = PdManager::new(client.clone());
    let direct = manager
        .get_affinity_groups(context(), &["g1".into(), "g2".into()])
        .unwrap();
    assert_eq!(direct.len(), 2);
    assert!(!direct.contains_key("g3"));

    *client.get_result.lock().unwrap() =
        Some(Err(AffinityError::http_status(400, "unsupported ids")));
    *client.all_groups.lock().unwrap() =
        HashMap::from([("g1".into(), state("g1")), ("g2".into(), state("g2"))]);
    let fallback = manager
        .get_affinity_groups(context(), &["g1".into()])
        .unwrap();
    assert_eq!(fallback.keys().collect::<Vec<_>>(), vec!["g1"]);
    assert_eq!(*client.get_all_calls.lock().unwrap(), 1);
}

#[test]
/// 编码长度与数量阈值边界对齐 Go。
fn escaped_query_and_count_boundaries_match_go() {
    assert_eq!(
        affinity_group_ids_escaped_query_len(&["/".repeat(1364)]),
        MAX_AFFINITY_GROUP_IDS_QUERY_LEN
    );
    assert_eq!(
        affinity_group_ids_escaped_query_len(&["/".repeat(1365)]),
        MAX_AFFINITY_GROUP_IDS_QUERY_LEN + 3
    );
    let ids: Vec<_> = (0..=MAX_AFFINITY_GROUP_IDS_COUNT)
        .map(|i| format!("g{i}"))
        .collect();
    assert!(should_use_get_all_affinity_groups(&ids));
}

#[test]
/// 超长查询串直接全量扫描，不调用 ids 接口。
fn oversized_query_scans_all_without_calling_ids_endpoint() {
    let client = Arc::new(RecordingClient::default());
    let id = "/".repeat(1365);
    *client.all_groups.lock().unwrap() =
        HashMap::from([(id.clone(), state(&id)), ("other".into(), state("other"))]);
    let manager = PdManager::new(client.clone());
    let result = manager
        .get_affinity_groups(context(), &[id.clone()])
        .unwrap();
    assert_eq!(result, HashMap::from([(id, state(&"/".repeat(1365)))]));
    assert!(client.get_calls.lock().unwrap().is_empty());
}

#[test]
/// filter 去重；Mock 对已存在 Group 的创建保持幂等。
fn filtering_deduplicates_ids_and_mock_is_idempotent() {
    let filtered = filter_affinity_groups(
        &HashMap::from([("g1".into(), state("g1")), ("other".into(), state("other"))]),
        &["g1".into(), "g1".into(), "missing".into()],
    );
    assert_eq!(filtered, HashMap::from([("g1".into(), state("g1"))]));

    let manager = new_mock_manager();
    manager
        .create_affinity_groups_if_not_exists(context(), &groups())
        .unwrap();
    let replacement = HashMap::from([(
        "g1".into(),
        vec![
            AffinityGroupKeyRange::new(b"x", b"y"),
            AffinityGroupKeyRange::new(b"y", b"z"),
        ],
    )]);
    manager
        .create_affinity_groups_if_not_exists(context(), &replacement)
        .unwrap();
    let result = manager
        .get_affinity_groups(context(), &["g1".into()])
        .unwrap();
    assert_eq!(result["g1"].range_count, 1);
}

#[test]
/// 包级删除带重试，且始终 force=true 清理 PD。
fn package_delete_retries_and_always_forces_pd_cleanup() {
    use astersql_domain_affinity::interface::{delete_groups_with_retry, init_manager};

    let _guard = crate::interface_test::lock_package_state();
    let client = Arc::new(RecordingClient::default());
    client.delete_results.lock().unwrap().extend([
        Err(AffinityError::new("first")),
        Err(AffinityError::new("second")),
        Ok(()),
    ]);
    init_manager(Some(client.clone()));
    delete_groups_with_retry(context(), &["g1".into()]).unwrap();
    assert_eq!(
        *client.delete_calls.lock().unwrap(),
        vec![
            (vec!["g1".into()], true),
            (vec!["g1".into()], true),
            (vec!["g1".into()], true)
        ]
    );
    init_manager(None);
}
