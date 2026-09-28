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
// limitations under the License.

// Affinity Manager：通过 PD HTTP API 或内存 Mock 管理 Affinity Group。
//
// Affinity Group 将一组 key range 绑定为亲和集合；本模块提供创建（幂等）、
// 删除与查询，并在旧版 PD 不兼容时自动回退到「先查全量再过滤」等策略。

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// URL 编码后 ids 查询串的最大允许长度，超出则改走 get-all。
pub const MAX_AFFINITY_GROUP_IDS_QUERY_LEN: usize = 4096;
/// 单次按 ids 查询的最大 Group 数量，超出则改走 get-all。
pub const MAX_AFFINITY_GROUP_IDS_COUNT: usize = 100;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Affinity Group 覆盖的一个半开 key range `[start_key, end_key)`。
pub struct AffinityGroupKeyRange {
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
}

impl AffinityGroupKeyRange {
    /// 由起止 key 构造 key range。
    pub fn new(start_key: impl AsRef<[u8]>, end_key: impl AsRef<[u8]>) -> Self {
        Self {
            start_key: start_key.as_ref().to_vec(),
            end_key: end_key.as_ref().to_vec(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Affinity Group 的标识信息。
pub struct AffinityGroup {
    pub id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Affinity Group 的运行时状态（含 range 数量）。
pub struct AffinityGroupState {
    pub affinity_group: AffinityGroup,
    pub range_count: usize,
}

impl AffinityGroupState {
    /// 由 Group id 与 range 数量构造状态。
    pub fn new(id: impl Into<String>, range_count: usize) -> Self {
        Self {
            affinity_group: AffinityGroup { id: id.into() },
            range_count,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Affinity 操作错误；可携带 PD HTTP 状态码或无状态服务错误标记。
pub struct AffinityError {
    message: String,
    http_service_error: bool,
}

impl AffinityError {
    /// 构造普通错误消息。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            http_service_error: false,
        }
    }

    /// 构造带 HTTP 状态码的 PD API 失败错误（消息格式供后续解析）。
    pub fn http_status(code: u16, text: impl AsRef<str>) -> Self {
        Self::new(format!(
            "request pd http api failed with status: '{code} {}'",
            text.as_ref()
        ))
    }

    /// 构造无 HTTP 状态码的服务层错误（用于兼容性回退判断）。
    pub fn http_service(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            http_service_error: true,
        }
    }

    /// 若消息中含 PD HTTP status，则解析并返回状态码。
    pub fn status_code(&self) -> Option<u16> {
        extract_pd_http_status_code(self)
    }
}

impl Display for AffinityError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for AffinityError {}

/// Go `context.Context` 在 affinity 边界使用的取消与截止期子集。
///
/// Manager 只负责原样传播该对象；具体 PD client 决定如何中断在途请求。
pub trait Context: Send + Sync {
    /// 上下文是否已被取消。
    fn is_cancelled(&self) -> bool;

    /// 可选的绝对截止时刻。
    fn deadline(&self) -> Option<Instant>;
}

/// 永不主动取消且无截止期的根上下文，对应 `context.Background()`。
#[derive(Clone, Copy, Debug, Default)]
pub struct BackgroundContext;

impl Context for BackgroundContext {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }
}

/// Rust boundary for the subset of the PD HTTP client used by this package.
/// PD HTTP 客户端中本包所用子集的 Rust 边界抽象。
pub trait PdClient: Send + Sync {
    fn create_affinity_groups(
        &self,
        ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
        skip_exist_check: bool,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError>;

    fn batch_delete_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
        force: bool,
    ) -> Result<(), AffinityError>;

    fn get_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError>;

    fn get_all_affinity_groups(
        &self,
        ctx: &dyn Context,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError>;
}

/// Affinity Group 生命周期管理接口（创建 / 删除 / 查询）。
pub trait Manager: Send + Sync {
    fn create_affinity_groups_if_not_exists(
        &self,
        ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
    ) -> Result<(), AffinityError>;

    fn delete_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<(), AffinityError>;

    fn get_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError>;
}

/// 基于真实 PD Client 的 Manager 实现。
pub struct PdManager {
    client: Arc<dyn PdClient>,
}

impl PdManager {
    /// 用给定 PD Client 构造 PdManager。
    pub fn new(client: Arc<dyn PdClient>) -> Self {
        Self { client }
    }

    /// 回退路径：先查询已存在 Group，仅创建缺失项。
    fn create_affinity_groups_if_not_exists_by_filtering(
        &self,
        ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
    ) -> Result<(), AffinityError> {
        let mut ids: Vec<_> = groups.keys().cloned().collect();
        ids.sort();
        let existing = self.get_affinity_groups(ctx, &ids)?;
        let missing: HashMap<_, _> = groups
            .iter()
            .filter(|(id, _)| !existing.contains_key(*id))
            .map(|(id, ranges)| (id.clone(), ranges.clone()))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        self.client
            .create_affinity_groups(ctx, &missing, false)
            .map(|_| ())
    }

    /// 回退路径：拉取全部 Group 后按 ids 过滤。
    fn get_affinity_groups_by_scanning_all(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        let all = self.client.get_all_affinity_groups(ctx)?;
        Ok(filter_affinity_groups(&all, ids))
    }
}

impl Manager for PdManager {
    fn create_affinity_groups_if_not_exists(
        &self,
        ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
    ) -> Result<(), AffinityError> {
        if groups.is_empty() {
            return Ok(());
        }
        match self.client.create_affinity_groups(ctx, groups, true) {
            Ok(_) => Ok(()),
            // 旧版 PD 不支持 skip_exist_check 时，改为先查后建缺失项。
            Err(err) if should_fallback_create_affinity_groups(&err) => {
                self.create_affinity_groups_if_not_exists_by_filtering(ctx, groups)
            }
            Err(err) => Err(err),
        }
    }

    fn delete_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<(), AffinityError> {
        if ids.is_empty() {
            return Ok(());
        }
        self.client.batch_delete_affinity_groups(ctx, ids, true)
    }

    fn get_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        // ids 过多或查询串过长：直接全量扫描再过滤。
        if should_use_get_all_affinity_groups(ids) {
            return self.get_affinity_groups_by_scanning_all(ctx, ids);
        }
        match self.client.get_affinity_groups(ctx, ids) {
            Ok(groups) => Ok(filter_affinity_groups(&groups, ids)),
            // 旧版 PD 不支持按 ids 查询时，回退到 get-all。
            Err(err) if should_fallback_get_affinity_groups(&err) => {
                self.get_affinity_groups_by_scanning_all(ctx, ids)
            }
            Err(err) => Err(err),
        }
    }
}

/// 构造包装后的 PdManager（`Arc<dyn Manager>`）。
pub fn new_pd_manager(client: Arc<dyn PdClient>) -> Arc<dyn Manager> {
    Arc::new(PdManager::new(client))
}

/// 计算 ids 作为 form-urlencoded 查询串时的编码长度。
pub fn affinity_group_ids_escaped_query_len(ids: &[String]) -> usize {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for id in ids {
        serializer.append_pair("ids", id);
    }
    serializer.finish().len()
}

/// 判断是否应跳过按 ids 查询、直接扫描全量 Group。
pub fn should_use_get_all_affinity_groups(ids: &[String]) -> bool {
    ids.len() > MAX_AFFINITY_GROUP_IDS_COUNT
        || affinity_group_ids_escaped_query_len(ids) > MAX_AFFINITY_GROUP_IDS_QUERY_LEN
}

/// 从错误消息中解析 `status: 'NNN ...'` 形式的 HTTP 状态码。
pub fn extract_pd_http_status_code(err: &AffinityError) -> Option<u16> {
    let start = err.message.find("status: '")? + "status: '".len();
    let digits: String = err.message[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// 判断错误是否对应指定的 PD HTTP 状态码。
pub fn is_pd_http_status_error(err: &AffinityError, status_code: u16) -> bool {
    extract_pd_http_status_code(err) == Some(status_code)
}

/// 判断是否为无状态码的 PD HTTP 服务错误。
pub fn is_pd_http_service_error_without_status(err: &AffinityError) -> bool {
    extract_pd_http_status_code(err).is_none() && err.http_service_error
}

/// 创建失败时是否应回退到「过滤后重建」策略（400/409/无状态服务错误）。
pub fn should_fallback_create_affinity_groups(err: &AffinityError) -> bool {
    is_pd_http_status_error(err, 400)
        || is_pd_http_status_error(err, 409)
        || is_pd_http_service_error_without_status(err)
}

/// 按 ids 查询失败时是否应回退到全量扫描（400/404/414/无状态服务错误）。
pub fn should_fallback_get_affinity_groups(err: &AffinityError) -> bool {
    is_pd_http_status_error(err, 400)
        || is_pd_http_status_error(err, 404)
        || is_pd_http_status_error(err, 414)
        || is_pd_http_service_error_without_status(err)
}

/// 按 ids 过滤 Group 状态，并对重复 id 去重。
pub fn filter_affinity_groups(
    groups: &HashMap<String, AffinityGroupState>,
    ids: &[String],
) -> HashMap<String, AffinityGroupState> {
    let mut result = HashMap::with_capacity(ids.len());
    let mut seen = HashSet::with_capacity(ids.len());
    for id in ids {
        if seen.insert(id) {
            if let Some(group) = groups.get(id) {
                result.insert(id.clone(), group.clone());
            }
        }
    }
    result
}

#[derive(Default)]
/// 内存版 Mock Manager，供单测与未配置 PD 时使用。
pub struct MockManager {
    groups: RwLock<HashMap<String, AffinityGroupState>>,
}

impl Manager for MockManager {
    fn create_affinity_groups_if_not_exists(
        &self,
        _ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
    ) -> Result<(), AffinityError> {
        let mut stored = self
            .groups
            .write()
            .expect("mock affinity manager lock poisoned");
        for (id, ranges) in groups {
            stored
                .entry(id.clone())
                .or_insert_with(|| AffinityGroupState::new(id, ranges.len()));
        }
        Ok(())
    }

    fn delete_affinity_groups(
        &self,
        _ctx: &dyn Context,
        ids: &[String],
    ) -> Result<(), AffinityError> {
        let mut stored = self
            .groups
            .write()
            .expect("mock affinity manager lock poisoned");
        for id in ids {
            stored.remove(id);
        }
        Ok(())
    }

    fn get_affinity_groups(
        &self,
        _ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        let stored = self
            .groups
            .read()
            .expect("mock affinity manager lock poisoned");
        Ok(filter_affinity_groups(&stored, ids))
    }
}

/// 构造空的 MockManager。
pub fn new_mock_manager() -> Arc<dyn Manager> {
    Arc::new(MockManager::default())
}
