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

// 导入 SDK 各组件的可预期 mock（gomock 风格）。
//
// 提供 `MockFileScanner`/`MockJobManager`/`MockSQLGenerator`/`MockSDK`：
// 通过期望列表按方法与业务参数匹配调用并返回预设结果，
// 便于上层测试验证导入编排而不依赖真实存储或数据库。

use astersql_errors as errors;
use astersql_importsdk::{
    FileScanner, GroupStatus, ImportDataSizeEstimate, ImportOptions, JobManager, JobStatus, SDK,
    SQLGenerator, TableMeta,
};
use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// 一次 mock 调用的可观察描述（不含上下文，仅保留业务参数）。
/// An owned description of a call made through an import SDK mock.
///
/// Context values are deliberately omitted because they are transport-scoped,
/// while all business arguments remain exact and observable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MockCall {
    Close,
    CreateSchemaAndTableByName { schema: String, table: String },
    CreateSchemasAndTables,
    EstimateImportDataSize,
    GetTableMetaByName { database: String, table: String },
    GetTableMetas,
    GetTotalSize,
    CancelJob { job_id: i64 },
    GetGroupSummary { group_key: String },
    GetJobStatus { job_id: i64 },
    GetJobsByGroup { group_key: String },
    SubmitJob { query: String },
    GenerateImportSQL { table_meta: String, options: String },
}

/// 仅输出调用名，便于日志与断言消息。
impl fmt::Display for MockCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Close => formatter.write_str("Close"),
            Self::CreateSchemaAndTableByName { .. } => {
                formatter.write_str("CreateSchemaAndTableByName")
            }
            Self::CreateSchemasAndTables => formatter.write_str("CreateSchemasAndTables"),
            Self::EstimateImportDataSize => formatter.write_str("EstimateImportDataSize"),
            Self::GetTableMetaByName { .. } => formatter.write_str("GetTableMetaByName"),
            Self::GetTableMetas => formatter.write_str("GetTableMetas"),
            Self::GetTotalSize => formatter.write_str("GetTotalSize"),
            Self::CancelJob { .. } => formatter.write_str("CancelJob"),
            Self::GetGroupSummary { .. } => formatter.write_str("GetGroupSummary"),
            Self::GetJobStatus { .. } => formatter.write_str("GetJobStatus"),
            Self::GetJobsByGroup { .. } => formatter.write_str("GetJobsByGroup"),
            Self::SubmitJob { .. } => formatter.write_str("SubmitJob"),
            Self::GenerateImportSQL { .. } => formatter.write_str("GenerateImportSQL"),
        }
    }
}

#[derive(Clone)]
/// 与各接口返回类型对应的预设响应载荷。
enum MockResponse {
    Unit(Result<(), errors::SharedError>),
    ImportDataSize(Result<ImportDataSizeEstimate, errors::SharedError>),
    TableMeta(Result<TableMeta, errors::SharedError>),
    TableMetas(Result<Vec<TableMeta>, errors::SharedError>),
    TotalSize(i64),
    GroupStatus(Result<GroupStatus, errors::SharedError>),
    JobStatus(Result<JobStatus, errors::SharedError>),
    JobStatuses(Result<Vec<JobStatus>, errors::SharedError>),
    JobID(Result<i64, errors::SharedError>),
    JobIDParts(i64, Option<errors::SharedError>),
    Dual(Arc<dyn Any + Send + Sync>, Option<errors::SharedError>),
    Dynamic(Arc<dyn Fn(&MockCall, Option<&(dyn Any + Send + Sync)>) -> MockResponse + Send + Sync>),
    SQL(Result<String, errors::SharedError>),
    SQLCallback(
        Arc<
            dyn Fn(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError> + Send + Sync,
        >,
    ),
}

/// 一条期望：预期调用 + 对应响应。
struct Expectation {
    id: usize,
    call: MockCall,
    response: MockResponse,
    matcher: Arc<dyn Fn(&MockCall) -> bool + Send + Sync>,
    context_matcher: Option<Arc<dyn Fn(&(dyn Any + Send + Sync)) -> bool + Send + Sync>>,
    min_calls: usize,
    max_calls: Option<usize>,
    called: usize,
    after: Vec<usize>,
}

#[derive(Default)]
/// 期望队列与已发生调用记录。
struct QueueState {
    expectations: VecDeque<Expectation>,
    calls: Vec<MockCall>,
    next_id: usize,
}

#[derive(Clone, Default)]
/// 线程安全的期望/调用队列，可在多个 mock 句柄间共享。
struct MockQueue {
    state: Arc<Mutex<QueueState>>,
}

/// 配置已登记期望的调用次数和前置依赖。
#[derive(Clone)]
pub struct MockExpectation {
    queue: MockQueue,
    id: usize,
}

impl MockExpectation {
    fn configure(&self, update: impl FnOnce(&mut Expectation)) {
        let mut state = self.queue.lock();
        let expectation = state
            .expectations
            .iter_mut()
            .find(|entry| entry.id == self.id)
            .expect("mock expectation was already removed");
        update(expectation);
    }

    pub fn Times(self, count: usize) -> Self {
        self.configure(|expectation| {
            expectation.min_calls = count;
            expectation.max_calls = Some(count);
        });
        self
    }

    /// 使用调用描述谓词匹配业务参数；可表达 GoMock 自定义 matcher。
    pub fn Matching(self, matcher: impl Fn(&MockCall) -> bool + Send + Sync + 'static) -> Self {
        self.configure(|expectation| expectation.matcher = Arc::new(matcher));
        self
    }

    pub fn ContextMatching(
        self,
        matcher: impl Fn(&(dyn Any + Send + Sync)) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.configure(|expectation| expectation.context_matcher = Some(Arc::new(matcher)));
        self
    }

    /// 覆盖预设响应，分别保存 Go 的值与错误返回位。
    pub fn ReturnParts<T: Any + Send + Sync + Clone>(
        self,
        value: T,
        error: Option<errors::SharedError>,
    ) -> Self {
        self.configure(|expectation| {
            expectation.response = MockResponse::Dual(Arc::new(value), error)
        });
        self
    }

    /// 调用时检查业务参数与 context，并动态返回 Go 的两个返回位。
    pub fn DoAndReturn<T: Any + Send + Sync + Clone>(
        self,
        callback: impl Fn(
            &MockCall,
            Option<&(dyn Any + Send + Sync)>,
        ) -> (T, Option<errors::SharedError>)
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.configure(|expectation| {
            expectation.response = MockResponse::Dynamic(Arc::new(move |call, context| {
                let (value, error) = callback(call, context);
                MockResponse::Dual(Arc::new(value), error)
            }));
        });
        self
    }

    pub fn AnyTimes(self) -> Self {
        self.configure(|expectation| {
            expectation.min_calls = 0;
            expectation.max_calls = None;
        });
        self
    }

    pub fn MinTimes(self, count: usize) -> Self {
        self.configure(|expectation| {
            expectation.min_calls = count;
            if expectation.max_calls == Some(1) {
                expectation.max_calls = None;
            }
        });
        self
    }

    pub fn MaxTimes(self, count: usize) -> Self {
        self.configure(|expectation| {
            if expectation.min_calls == 1 {
                expectation.min_calls = 0;
            }
            expectation.max_calls = Some(count);
        });
        self
    }

    pub fn After(self, prerequisite: &MockExpectation) -> Self {
        assert!(
            Arc::ptr_eq(&self.queue.state, &prerequisite.queue.state),
            "expectations belong to different mocks"
        );
        assert_ne!(
            self.id, prerequisite.id,
            "expectation cannot depend on itself"
        );
        {
            let state = self.queue.lock();
            let mut reachable = vec![prerequisite.id];
            while let Some(id) = reachable.pop() {
                assert_ne!(id, self.id, "cycle in mock call order");
                if let Some(entry) = state.expectations.iter().find(|entry| entry.id == id) {
                    reachable.extend(entry.after.iter().copied());
                }
            }
        }
        self.configure(|expectation| expectation.after.push(prerequisite.id));
        self
    }
}

impl MockQueue {
    /// 获取队列锁；中毒时仍取内层数据，避免测试因 panic 传播中断。
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 追加一条期望到队尾。
    fn expect(&self, call: MockCall, response: MockResponse) -> MockExpectation {
        let exact = call.clone();
        self.expect_matching(call, response, move |actual| *actual == exact)
    }

    fn expect_matching(
        &self,
        call: MockCall,
        response: MockResponse,
        matcher: impl Fn(&MockCall) -> bool + Send + Sync + 'static,
    ) -> MockExpectation {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.expectations.push_back(Expectation {
            id,
            call,
            response,
            matcher: Arc::new(matcher),
            context_matcher: None,
            min_calls: 1,
            max_calls: Some(1),
            called: 0,
            after: Vec::new(),
        });
        MockExpectation {
            queue: self.clone(),
            id,
        }
    }

    fn last_expectation(&self) -> MockExpectation {
        let state = self.lock();
        MockExpectation {
            queue: self.clone(),
            id: state
                .expectations
                .back()
                .expect("no expectation was registered")
                .id,
        }
    }

    /// 记录实际调用，匹配最早登记的相同期望并取出响应。
    fn dispatch(&self, actual: MockCall) -> Result<MockResponse, errors::SharedError> {
        self.dispatch_internal(actual, None)
    }

    fn dispatch_with_context(
        &self,
        actual: MockCall,
        context: &(dyn Any + Send + Sync),
    ) -> Result<MockResponse, errors::SharedError> {
        self.dispatch_internal(actual, Some(context))
    }

    fn dispatch_internal(
        &self,
        actual: MockCall,
        context: Option<&(dyn Any + Send + Sync)>,
    ) -> Result<MockResponse, errors::SharedError> {
        let mut state = self.lock();
        state.calls.push(actual.clone());
        // 无剩余期望：调用意外。
        if state.expectations.is_empty() {
            return Err(errors::New(format!("unexpected mock call: {actual:?}")));
        }
        // GoMock 未设置 InOrder/After 时，不按登记顺序要求调用。
        let Some(index) = state.expectations.iter().position(|expected| {
            (expected.matcher)(&actual)
                && expected
                    .context_matcher
                    .as_ref()
                    .is_none_or(|matcher| context.is_some_and(|ctx| matcher(ctx)))
                && expected.max_calls.is_none_or(|max| expected.called < max)
                && expected.after.iter().all(|id| {
                    state
                        .expectations
                        .iter()
                        .find(|previous| previous.id == *id)
                        .is_none_or(|previous| {
                            previous.max_calls.is_some_and(|max| previous.called >= max)
                        })
                })
        }) else {
            return Err(errors::New(format!(
                "unexpected mock call: got {actual:?}, want one of {:?}",
                state
                    .expectations
                    .iter()
                    .map(|expected| &expected.call)
                    .collect::<Vec<_>>()
            )));
        };
        let expected = &mut state.expectations[index];
        expected.called += 1;
        let response = expected.response.clone();
        if expected.max_calls == Some(expected.called) {
            state.expectations.remove(index);
        }
        drop(state);
        match response {
            MockResponse::Dynamic(callback) => Ok(callback(&actual, context)),
            other => Ok(other),
        }
    }

    /// 已发生的全部调用（按时间序）。
    fn calls(&self) -> Vec<MockCall> {
        self.lock().calls.clone()
    }

    /// 尚未匹配的期望调用列表。
    fn pending_expectations(&self) -> Vec<MockCall> {
        self.lock()
            .expectations
            .iter()
            .filter(|expectation| expectation.called < expectation.min_calls)
            .map(|expectation| expectation.call.clone())
            .collect()
    }

    /// 断言所有期望均已消耗，否则报未满足期望。
    fn verify(&self) -> Result<(), errors::SharedError> {
        let pending = self.pending_expectations();
        if pending.is_empty() {
            Ok(())
        } else {
            Err(errors::New(format!(
                "unmet import SDK mock expectations: {pending:?}"
            )))
        }
    }
}

/// 响应类型与调用不匹配时的错误。
fn wrong_response(call: &MockCall) -> errors::SharedError {
    errors::New(format!("mock response type does not match {call}"))
}

fn dual_result<T: Any + Clone + Send + Sync>(
    value: Arc<dyn Any + Send + Sync>,
    error: Option<errors::SharedError>,
    call: &MockCall,
) -> Result<T, errors::SharedError> {
    if let Some(error) = error {
        return Err(error);
    }
    if let Some(value) = value.downcast_ref::<T>() {
        return Ok(value.clone());
    }
    if let Some(Some(value)) = value.downcast_ref::<Option<T>>() {
        return Ok(value.clone());
    }
    Err(wrong_response(call))
}

fn response_parts<T: Any + Clone + Send + Sync>(
    response: MockResponse,
    call: &MockCall,
) -> (Option<T>, Option<errors::SharedError>) {
    match response {
        MockResponse::Dual(value, error) => {
            let result = value
                .downcast_ref::<T>()
                .cloned()
                .map(Some)
                .or_else(|| value.downcast_ref::<Option<T>>().cloned());
            match result {
                Some(value) => (value, error),
                None => (None, Some(wrong_response(call))),
            }
        }
        MockResponse::JobIDParts(job_id, error) => {
            let value = (&job_id as &dyn Any).downcast_ref::<T>().cloned();
            (value, error)
        }
        other => {
            let result: Option<&dyn Any> = match &other {
                MockResponse::ImportDataSize(result) => Some(result),
                MockResponse::TableMeta(result) => Some(result),
                MockResponse::TableMetas(result) => Some(result),
                MockResponse::GroupStatus(result) => Some(result),
                MockResponse::JobStatus(result) => Some(result),
                MockResponse::JobStatuses(result) => Some(result),
                MockResponse::JobID(result) => Some(result),
                MockResponse::SQL(result) => Some(result),
                _ => None,
            };
            match result.and_then(|any| any.downcast_ref::<Result<T, errors::SharedError>>()) {
                Some(Ok(value)) => (Some(value.clone()), None),
                Some(Err(error)) => (None, Some(error.clone())),
                None => (None, Some(wrong_response(call))),
            }
        }
    }
}

fn dispatch_parts<T: Any + Clone + Send + Sync>(
    queue: &MockQueue,
    call: MockCall,
    context: Option<&(dyn Any + Send + Sync)>,
) -> (Option<T>, Option<errors::SharedError>) {
    match queue.dispatch_internal(call.clone(), context) {
        Ok(response) => response_parts(response, &call),
        Err(error) => (None, Some(error)),
    }
}

/// 用 Debug 字符串作为 TableMeta 期望匹配签名。
fn table_meta_signature(table_meta: &TableMeta) -> String {
    format!("{table_meta:?}")
}

/// 用 Debug 字符串作为 ImportOptions 期望匹配签名。
fn import_options_signature(options: &ImportOptions) -> String {
    format!("{options:?}")
}

/// 为 mock 主体注入 calls/pending_expectations/verify 检查 API。
macro_rules! mock_inspection_api {
    () => {
        pub fn calls(&self) -> Vec<MockCall> {
            self.queue.calls()
        }

        pub fn pending_expectations(&self) -> Vec<MockCall> {
            self.queue.pending_expectations()
        }

        pub fn verify(&self) -> Result<(), errors::SharedError> {
            self.queue.verify()
        }
    };
}

macro_rules! file_scanner_parts_api {
    () => {
        pub fn GetTableMetasParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
        ) -> (Option<Vec<TableMeta>>, Option<errors::SharedError>) {
            dispatch_parts(&self.queue, MockCall::GetTableMetas, Some(ctx))
        }

        pub fn GetTableMetaByNameParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            database: &str,
            table: &str,
        ) -> (Option<TableMeta>, Option<errors::SharedError>) {
            dispatch_parts(
                &self.queue,
                MockCall::GetTableMetaByName {
                    database: database.into(),
                    table: table.into(),
                },
                Some(ctx),
            )
        }

        pub fn EstimateImportDataSizeParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
        ) -> (Option<ImportDataSizeEstimate>, Option<errors::SharedError>) {
            dispatch_parts(&self.queue, MockCall::EstimateImportDataSize, Some(ctx))
        }
    };
}

macro_rules! job_manager_parts_api {
    () => {
        pub fn GetJobStatusParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            job_id: i64,
        ) -> (Option<JobStatus>, Option<errors::SharedError>) {
            dispatch_parts(&self.queue, MockCall::GetJobStatus { job_id }, Some(ctx))
        }

        pub fn GetGroupSummaryParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            group_key: &str,
        ) -> (Option<GroupStatus>, Option<errors::SharedError>) {
            dispatch_parts(
                &self.queue,
                MockCall::GetGroupSummary {
                    group_key: group_key.into(),
                },
                Some(ctx),
            )
        }

        pub fn GetJobsByGroupParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            group_key: &str,
        ) -> (Option<Vec<JobStatus>>, Option<errors::SharedError>) {
            dispatch_parts(
                &self.queue,
                MockCall::GetJobsByGroup {
                    group_key: group_key.into(),
                },
                Some(ctx),
            )
        }
    };
}

macro_rules! sql_generator_parts_api {
    () => {
        pub fn GenerateImportSQLParts(
            &self,
            table_meta: &TableMeta,
            options: &ImportOptions,
        ) -> (String, Option<errors::SharedError>) {
            let call = MockCall::GenerateImportSQL {
                table_meta: table_meta_signature(table_meta),
                options: import_options_signature(options),
            };
            match self.queue.dispatch(call.clone()) {
                Ok(MockResponse::SQLCallback(callback)) => match callback(table_meta, options) {
                    Ok(sql) => (sql, None),
                    Err(error) => (String::new(), Some(error)),
                },
                Ok(response) => {
                    let (value, error) = response_parts::<String>(response, &call);
                    (value.unwrap_or_default(), error)
                }
                Err(error) => (String::new(), Some(error)),
            }
        }
    };
}

macro_rules! file_scanner_parts_trait_api {
    ($mock:ty) => {
        fn GetTableMetasParts(
            &mut self,
            ctx: &(dyn Any + Send + Sync),
        ) -> (Option<Vec<TableMeta>>, Option<errors::SharedError>) {
            <$mock>::GetTableMetasParts(self, ctx)
        }
        fn GetTableMetaByNameParts(
            &mut self,
            ctx: &(dyn Any + Send + Sync),
            db: &str,
            table: &str,
        ) -> (Option<TableMeta>, Option<errors::SharedError>) {
            <$mock>::GetTableMetaByNameParts(self, ctx, db, table)
        }
        fn EstimateImportDataSizeParts(
            &mut self,
            ctx: &(dyn Any + Send + Sync),
        ) -> (Option<ImportDataSizeEstimate>, Option<errors::SharedError>) {
            <$mock>::EstimateImportDataSizeParts(self, ctx)
        }
    };
}

macro_rules! job_manager_parts_trait_api {
    ($mock:ty) => {
        fn GetJobStatusParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            job_id: i64,
        ) -> (Option<JobStatus>, Option<errors::SharedError>) {
            <$mock>::GetJobStatusParts(self, ctx, job_id)
        }
        fn GetGroupSummaryParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            group_key: &str,
        ) -> (Option<GroupStatus>, Option<errors::SharedError>) {
            <$mock>::GetGroupSummaryParts(self, ctx, group_key)
        }
        fn GetJobsByGroupParts(
            &self,
            ctx: &(dyn Any + Send + Sync),
            group_key: &str,
        ) -> (Option<Vec<JobStatus>>, Option<errors::SharedError>) {
            <$mock>::GetJobsByGroupParts(self, ctx, group_key)
        }
    };
}

macro_rules! sql_generator_parts_trait_api {
    ($mock:ty) => {
        fn GenerateImportSQLParts(
            &self,
            table_meta: &TableMeta,
            options: &ImportOptions,
        ) -> (String, Option<errors::SharedError>) {
            <$mock>::GenerateImportSQLParts(self, table_meta, options)
        }
    };
}

/// 为 recorder（EXPECT 句柄）注入同样的检查 API。
macro_rules! recorder_inspection_api {
    () => {
        /// 配置最近登记的调用；用于次数和依赖设置。
        pub fn last_call(&self) -> MockExpectation {
            self.queue.last_expectation()
        }

        pub fn calls(&self) -> Vec<MockCall> {
            self.queue.calls()
        }

        pub fn pending_expectations(&self) -> Vec<MockCall> {
            self.queue.pending_expectations()
        }

        pub fn verify(&self) -> Result<(), errors::SharedError> {
            self.queue.verify()
        }
    };
}

#[derive(Clone)]
/// `FileScanner` 的 mock：共享队列 + EXPECT recorder。
pub struct MockFileScanner {
    queue: MockQueue,
    recorder: MockFileScannerMockRecorder,
}

#[derive(Clone)]
/// 登记 `FileScanner` 方法期望的 recorder。
pub struct MockFileScannerMockRecorder {
    queue: MockQueue,
}

/// 默认构造等价于 `NewMockFileScanner()`。
impl Default for MockFileScanner {
    fn default() -> Self {
        NewMockFileScanner()
    }
}

/// 创建空期望队列的 `MockFileScanner`。
pub fn NewMockFileScanner() -> MockFileScanner {
    let queue = MockQueue::default();
    MockFileScanner {
        recorder: MockFileScannerMockRecorder {
            queue: queue.clone(),
        },
        queue,
    }
}

/// `NewMockFileScanner` 的 snake_case 别名。
pub fn new_mock_file_scanner() -> MockFileScanner {
    NewMockFileScanner()
}

impl MockFileScanner {
    /// 返回用于登记期望的 recorder（gomock 风格大写 EXPECT）。
    pub fn EXPECT(&self) -> &MockFileScannerMockRecorder {
        &self.recorder
    }

    /// `EXPECT` 的小写别名。
    pub fn expect(&self) -> &MockFileScannerMockRecorder {
        self.EXPECT()
    }

    mock_inspection_api!();
    file_scanner_parts_api!();
}

impl MockFileScannerMockRecorder {
    /// 期望一次 Close，并返回给定结果。
    pub fn Close(&self, result: Result<(), errors::SharedError>) -> MockExpectation {
        self.queue
            .expect(MockCall::Close, MockResponse::Unit(result))
    }

    /// 期望按库表名创建 schema/table。
    pub fn CreateSchemaAndTableByName(
        &self,
        schema: impl Into<String>,
        table: impl Into<String>,
        result: Result<(), errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::CreateSchemaAndTableByName {
                schema: schema.into(),
                table: table.into(),
            },
            MockResponse::Unit(result),
        )
    }

    /// 期望创建全部 schema/table。
    pub fn CreateSchemasAndTables(
        &self,
        result: Result<(), errors::SharedError>,
    ) -> MockExpectation {
        self.queue
            .expect(MockCall::CreateSchemasAndTables, MockResponse::Unit(result))
    }

    /// 期望估算导入数据量。
    pub fn EstimateImportDataSize(
        &self,
        result: Result<ImportDataSizeEstimate, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::EstimateImportDataSize,
            MockResponse::ImportDataSize(result),
        )
    }

    /// 期望按名获取表元数据。
    pub fn GetTableMetaByName(
        &self,
        database: impl Into<String>,
        table: impl Into<String>,
        result: Result<TableMeta, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetTableMetaByName {
                database: database.into(),
                table: table.into(),
            },
            MockResponse::TableMeta(result),
        )
    }

    /// 期望获取全部表元数据。
    pub fn GetTableMetas(
        &self,
        result: Result<Vec<TableMeta>, errors::SharedError>,
    ) -> MockExpectation {
        self.queue
            .expect(MockCall::GetTableMetas, MockResponse::TableMetas(result))
    }

    /// 期望返回数据总大小。
    pub fn GetTotalSize(&self, result: i64) -> MockExpectation {
        self.queue
            .expect(MockCall::GetTotalSize, MockResponse::TotalSize(result))
    }

    recorder_inspection_api!();
}

/// 将 trait 调用派发到期望队列并解包对应响应类型。
impl FileScanner for MockFileScanner {
    file_scanner_parts_trait_api!(MockFileScanner);
    fn CreateSchemasAndTables(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<(), errors::SharedError> {
        let call = MockCall::CreateSchemasAndTables;
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::Unit(result) => result,
            _ => Err(wrong_response(&call)),
        }
    }

    fn CreateSchemaAndTableByName(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
        schema: &str,
        table: &str,
    ) -> Result<(), errors::SharedError> {
        let call = MockCall::CreateSchemaAndTableByName {
            schema: schema.to_owned(),
            table: table.to_owned(),
        };
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::Unit(result) => result,
            _ => Err(wrong_response(&call)),
        }
    }

    fn GetTableMetas(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<Vec<TableMeta>, errors::SharedError> {
        let call = MockCall::GetTableMetas;
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::TableMetas(result) => result,
            MockResponse::Dual(value, error) => dual_result::<Vec<TableMeta>>(value, error, &call),
            _ => Err(wrong_response(&call)),
        }
    }

    fn GetTableMetaByName(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
        database: &str,
        table: &str,
    ) -> Result<TableMeta, errors::SharedError> {
        let call = MockCall::GetTableMetaByName {
            database: database.to_owned(),
            table: table.to_owned(),
        };
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::TableMeta(result) => result,
            MockResponse::Dual(value, error) => dual_result::<TableMeta>(value, error, &call),
            _ => Err(wrong_response(&call)),
        }
    }

    fn GetTotalSize(&self, _ctx: &(dyn Any + Send + Sync)) -> i64 {
        let call = MockCall::GetTotalSize;
        match self.queue.dispatch_with_context(call.clone(), _ctx) {
            Ok(MockResponse::TotalSize(result)) => result,
            Ok(_) => panic!("{}", wrong_response(&call)),
            Err(error) => panic!("{error}"),
        }
    }

    fn EstimateImportDataSize(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<ImportDataSizeEstimate, errors::SharedError> {
        let call = MockCall::EstimateImportDataSize;
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::ImportDataSize(result) => result,
            MockResponse::Dual(value, error) => {
                dual_result::<ImportDataSizeEstimate>(value, error, &call)
            }
            _ => Err(wrong_response(&call)),
        }
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        let call = MockCall::Close;
        match self.queue.dispatch(call.clone())? {
            MockResponse::Unit(result) => result,
            _ => Err(wrong_response(&call)),
        }
    }
}

#[derive(Clone)]
/// `JobManager` 的 mock。
pub struct MockJobManager {
    queue: MockQueue,
    recorder: MockJobManagerMockRecorder,
}

#[derive(Clone)]
/// 登记 `JobManager` 方法期望的 recorder。
pub struct MockJobManagerMockRecorder {
    queue: MockQueue,
}

/// 默认构造等价于 `NewMockJobManager()`。
impl Default for MockJobManager {
    fn default() -> Self {
        NewMockJobManager()
    }
}

/// 创建空期望队列的 `MockJobManager`。
pub fn NewMockJobManager() -> MockJobManager {
    let queue = MockQueue::default();
    MockJobManager {
        recorder: MockJobManagerMockRecorder {
            queue: queue.clone(),
        },
        queue,
    }
}

/// `NewMockJobManager` 的 snake_case 别名。
pub fn new_mock_job_manager() -> MockJobManager {
    NewMockJobManager()
}

impl MockJobManager {
    /// 返回 JobManager 期望 recorder。
    pub fn EXPECT(&self) -> &MockJobManagerMockRecorder {
        &self.recorder
    }

    /// `EXPECT` 的小写别名。
    pub fn expect(&self) -> &MockJobManagerMockRecorder {
        self.EXPECT()
    }

    mock_inspection_api!();
    job_manager_parts_api!();
}

impl MockJobManagerMockRecorder {
    /// 期望取消指定 job_id。
    pub fn CancelJob(
        &self,
        job_id: i64,
        result: Result<(), errors::SharedError>,
    ) -> MockExpectation {
        self.queue
            .expect(MockCall::CancelJob { job_id }, MockResponse::Unit(result))
    }

    /// 期望按 group_key 返回组摘要。
    pub fn GetGroupSummary(
        &self,
        group_key: impl Into<String>,
        result: Result<GroupStatus, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetGroupSummary {
                group_key: group_key.into(),
            },
            MockResponse::GroupStatus(result),
        )
    }

    /// 期望查询指定作业状态。
    pub fn GetJobStatus(
        &self,
        job_id: i64,
        result: Result<JobStatus, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetJobStatus { job_id },
            MockResponse::JobStatus(result),
        )
    }

    /// 期望列出组内全部作业。
    pub fn GetJobsByGroup(
        &self,
        group_key: impl Into<String>,
        result: Result<Vec<JobStatus>, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetJobsByGroup {
                group_key: group_key.into(),
            },
            MockResponse::JobStatuses(result),
        )
    }

    /// 期望提交导入 SQL 并返回 job id。
    pub fn SubmitJob(
        &self,
        query: impl Into<String>,
        result: Result<i64, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::SubmitJob {
                query: query.into(),
            },
            MockResponse::JobID(result),
        )
    }

    /// 登记 Go 风格的两个独立返回位。
    pub fn SubmitJobParts(
        &self,
        query: impl Into<String>,
        job_id: i64,
        error: Option<errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::SubmitJob {
                query: query.into(),
            },
            MockResponse::JobIDParts(job_id, error),
        )
    }

    recorder_inspection_api!();
}

/// 将 JobManager 调用委托给共享 dispatch 辅助函数。
impl JobManager for MockJobManager {
    job_manager_parts_trait_api!(MockJobManager);
    fn SubmitJob(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<i64, errors::SharedError> {
        dispatch_submit_job(&self.queue, _ctx, query)
    }

    fn SubmitJobParts(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> (i64, Option<errors::SharedError>) {
        dispatch_submit_job_parts(&self.queue, _ctx, query)
    }

    fn GetJobStatus(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<JobStatus, errors::SharedError> {
        dispatch_get_job_status(&self.queue, _ctx, job_id)
    }

    fn CancelJob(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<(), errors::SharedError> {
        dispatch_cancel_job(&self.queue, _ctx, job_id)
    }

    fn GetGroupSummary(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<GroupStatus, errors::SharedError> {
        dispatch_get_group_summary(&self.queue, _ctx, group_key)
    }

    fn GetJobsByGroup(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<Vec<JobStatus>, errors::SharedError> {
        dispatch_get_jobs_by_group(&self.queue, _ctx, group_key)
    }
}

#[derive(Clone)]
/// `SQLGenerator` 的 mock。
pub struct MockSQLGenerator {
    queue: MockQueue,
    recorder: MockSQLGeneratorMockRecorder,
}

#[derive(Clone)]
/// 登记 `GenerateImportSQL` 期望的 recorder。
pub struct MockSQLGeneratorMockRecorder {
    queue: MockQueue,
}

/// 默认构造等价于 `NewMockSQLGenerator()`。
impl Default for MockSQLGenerator {
    fn default() -> Self {
        NewMockSQLGenerator()
    }
}

/// 创建空期望队列的 `MockSQLGenerator`。
pub fn NewMockSQLGenerator() -> MockSQLGenerator {
    let queue = MockQueue::default();
    MockSQLGenerator {
        recorder: MockSQLGeneratorMockRecorder {
            queue: queue.clone(),
        },
        queue,
    }
}

/// `NewMockSQLGenerator` 的 snake_case 别名。
pub fn new_mock_sql_generator() -> MockSQLGenerator {
    NewMockSQLGenerator()
}

impl MockSQLGenerator {
    /// 返回 SQLGenerator 期望 recorder。
    pub fn EXPECT(&self) -> &MockSQLGeneratorMockRecorder {
        &self.recorder
    }

    /// `EXPECT` 的小写别名。
    pub fn expect(&self) -> &MockSQLGeneratorMockRecorder {
        self.EXPECT()
    }

    mock_inspection_api!();
    sql_generator_parts_api!();
}

impl MockSQLGeneratorMockRecorder {
    /// 用参数谓词和有类型的回调登记 GenerateImportSQL。
    pub fn GenerateImportSQLMatching(
        &self,
        matcher: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
        callback: impl Fn(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError>
        + Send
        + Sync
        + 'static,
    ) -> MockExpectation {
        register_sql_callback(&self.queue, matcher, callback)
    }

    /// 对应 Go 的两个 gomock.Any() 参数。
    pub fn GenerateImportSQLAny(
        &self,
        callback: impl Fn(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError>
        + Send
        + Sync
        + 'static,
    ) -> MockExpectation {
        self.GenerateImportSQLMatching(|_, _| true, callback)
    }

    /// 期望根据表元数据与选项生成 IMPORT SQL。
    pub fn GenerateImportSQL(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
        result: Result<String, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GenerateImportSQL {
                table_meta: table_meta_signature(table_meta),
                options: import_options_signature(options),
            },
            MockResponse::SQL(result),
        )
    }

    recorder_inspection_api!();
}

/// 将 GenerateImportSQL 派发到期望队列。
impl SQLGenerator for MockSQLGenerator {
    sql_generator_parts_trait_api!(MockSQLGenerator);
    fn GenerateImportSQL(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
    ) -> Result<String, errors::SharedError> {
        dispatch_generate_import_sql(&self.queue, table_meta, options)
    }
}

#[derive(Clone)]
/// 聚合 FileScanner + JobManager + SQLGenerator + SDK 的统一 mock。
pub struct MockSDK {
    queue: MockQueue,
    recorder: MockSDKMockRecorder,
}

#[derive(Clone)]
/// 登记 SDK 全部能力期望的 recorder。
pub struct MockSDKMockRecorder {
    queue: MockQueue,
}

/// 默认构造等价于 `NewMockSDK()`。
impl Default for MockSDK {
    fn default() -> Self {
        NewMockSDK()
    }
}

/// 创建空期望队列的 `MockSDK`。
pub fn NewMockSDK() -> MockSDK {
    let queue = MockQueue::default();
    MockSDK {
        recorder: MockSDKMockRecorder {
            queue: queue.clone(),
        },
        queue,
    }
}

/// `NewMockSDK` 的 snake_case 别名。
pub fn new_mock_sdk() -> MockSDK {
    NewMockSDK()
}

impl MockSDK {
    /// 返回 SDK 期望 recorder。
    pub fn EXPECT(&self) -> &MockSDKMockRecorder {
        &self.recorder
    }

    /// `EXPECT` 的小写别名。
    pub fn expect(&self) -> &MockSDKMockRecorder {
        self.EXPECT()
    }

    mock_inspection_api!();
    file_scanner_parts_api!();
    job_manager_parts_api!();
    sql_generator_parts_api!();
}

impl MockSDKMockRecorder {
    /// 用参数谓词和有类型的回调登记 GenerateImportSQL。
    pub fn GenerateImportSQLMatching(
        &self,
        matcher: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
        callback: impl Fn(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError>
        + Send
        + Sync
        + 'static,
    ) -> MockExpectation {
        register_sql_callback(&self.queue, matcher, callback)
    }

    pub fn GenerateImportSQLAny(
        &self,
        callback: impl Fn(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError>
        + Send
        + Sync
        + 'static,
    ) -> MockExpectation {
        self.GenerateImportSQLMatching(|_, _| true, callback)
    }

    /// 期望一次 Close。
    pub fn Close(&self, result: Result<(), errors::SharedError>) -> MockExpectation {
        self.queue
            .expect(MockCall::Close, MockResponse::Unit(result))
    }

    /// 期望按库表名创建 schema/table。
    pub fn CreateSchemaAndTableByName(
        &self,
        schema: impl Into<String>,
        table: impl Into<String>,
        result: Result<(), errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::CreateSchemaAndTableByName {
                schema: schema.into(),
                table: table.into(),
            },
            MockResponse::Unit(result),
        )
    }

    /// 期望创建全部 schema/table。
    pub fn CreateSchemasAndTables(
        &self,
        result: Result<(), errors::SharedError>,
    ) -> MockExpectation {
        self.queue
            .expect(MockCall::CreateSchemasAndTables, MockResponse::Unit(result))
    }

    /// 期望估算导入数据量。
    pub fn EstimateImportDataSize(
        &self,
        result: Result<ImportDataSizeEstimate, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::EstimateImportDataSize,
            MockResponse::ImportDataSize(result),
        )
    }

    /// 期望按名获取表元数据。
    pub fn GetTableMetaByName(
        &self,
        database: impl Into<String>,
        table: impl Into<String>,
        result: Result<TableMeta, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetTableMetaByName {
                database: database.into(),
                table: table.into(),
            },
            MockResponse::TableMeta(result),
        )
    }

    /// 期望获取全部表元数据。
    pub fn GetTableMetas(
        &self,
        result: Result<Vec<TableMeta>, errors::SharedError>,
    ) -> MockExpectation {
        self.queue
            .expect(MockCall::GetTableMetas, MockResponse::TableMetas(result))
    }

    /// 期望返回数据总大小。
    pub fn GetTotalSize(&self, result: i64) -> MockExpectation {
        self.queue
            .expect(MockCall::GetTotalSize, MockResponse::TotalSize(result))
    }

    /// 期望取消指定 job_id。
    pub fn CancelJob(
        &self,
        job_id: i64,
        result: Result<(), errors::SharedError>,
    ) -> MockExpectation {
        self.queue
            .expect(MockCall::CancelJob { job_id }, MockResponse::Unit(result))
    }

    /// 期望按 group_key 返回组摘要。
    pub fn GetGroupSummary(
        &self,
        group_key: impl Into<String>,
        result: Result<GroupStatus, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetGroupSummary {
                group_key: group_key.into(),
            },
            MockResponse::GroupStatus(result),
        )
    }

    /// 期望查询指定作业状态。
    pub fn GetJobStatus(
        &self,
        job_id: i64,
        result: Result<JobStatus, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetJobStatus { job_id },
            MockResponse::JobStatus(result),
        )
    }

    /// 期望列出组内全部作业。
    pub fn GetJobsByGroup(
        &self,
        group_key: impl Into<String>,
        result: Result<Vec<JobStatus>, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GetJobsByGroup {
                group_key: group_key.into(),
            },
            MockResponse::JobStatuses(result),
        )
    }

    /// 期望提交导入 SQL 并返回 job id。
    pub fn SubmitJob(
        &self,
        query: impl Into<String>,
        result: Result<i64, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::SubmitJob {
                query: query.into(),
            },
            MockResponse::JobID(result),
        )
    }

    /// 登记 Go 风格的两个独立返回位。
    pub fn SubmitJobParts(
        &self,
        query: impl Into<String>,
        job_id: i64,
        error: Option<errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::SubmitJob {
                query: query.into(),
            },
            MockResponse::JobIDParts(job_id, error),
        )
    }

    /// 期望根据表元数据与选项生成 IMPORT SQL。
    pub fn GenerateImportSQL(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
        result: Result<String, errors::SharedError>,
    ) -> MockExpectation {
        self.queue.expect(
            MockCall::GenerateImportSQL {
                table_meta: table_meta_signature(table_meta),
                options: import_options_signature(options),
            },
            MockResponse::SQL(result),
        )
    }

    recorder_inspection_api!();
}

/// MockSDK 作为 FileScanner：派发到同一期望队列。
impl FileScanner for MockSDK {
    file_scanner_parts_trait_api!(MockSDK);
    fn CreateSchemasAndTables(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<(), errors::SharedError> {
        let call = MockCall::CreateSchemasAndTables;
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::Unit(result) => result,
            _ => Err(wrong_response(&call)),
        }
    }

    fn CreateSchemaAndTableByName(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
        schema: &str,
        table: &str,
    ) -> Result<(), errors::SharedError> {
        let call = MockCall::CreateSchemaAndTableByName {
            schema: schema.to_owned(),
            table: table.to_owned(),
        };
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::Unit(result) => result,
            _ => Err(wrong_response(&call)),
        }
    }

    fn GetTableMetas(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<Vec<TableMeta>, errors::SharedError> {
        let call = MockCall::GetTableMetas;
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::TableMetas(result) => result,
            MockResponse::Dual(value, error) => dual_result::<Vec<TableMeta>>(value, error, &call),
            _ => Err(wrong_response(&call)),
        }
    }

    fn GetTableMetaByName(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
        database: &str,
        table: &str,
    ) -> Result<TableMeta, errors::SharedError> {
        let call = MockCall::GetTableMetaByName {
            database: database.to_owned(),
            table: table.to_owned(),
        };
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::TableMeta(result) => result,
            MockResponse::Dual(value, error) => dual_result::<TableMeta>(value, error, &call),
            _ => Err(wrong_response(&call)),
        }
    }

    fn GetTotalSize(&self, _ctx: &(dyn Any + Send + Sync)) -> i64 {
        let call = MockCall::GetTotalSize;
        match self.queue.dispatch_with_context(call.clone(), _ctx) {
            Ok(MockResponse::TotalSize(result)) => result,
            Ok(_) => panic!("{}", wrong_response(&call)),
            Err(error) => panic!("{error}"),
        }
    }

    fn EstimateImportDataSize(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<ImportDataSizeEstimate, errors::SharedError> {
        let call = MockCall::EstimateImportDataSize;
        match self.queue.dispatch_with_context(call.clone(), _ctx)? {
            MockResponse::ImportDataSize(result) => result,
            MockResponse::Dual(value, error) => {
                dual_result::<ImportDataSizeEstimate>(value, error, &call)
            }
            _ => Err(wrong_response(&call)),
        }
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        dispatch_close(&self.queue)
    }
}

/// MockSDK 作为 JobManager：复用 dispatch_* 辅助。
impl JobManager for MockSDK {
    job_manager_parts_trait_api!(MockSDK);
    fn SubmitJob(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<i64, errors::SharedError> {
        dispatch_submit_job(&self.queue, _ctx, query)
    }

    fn SubmitJobParts(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> (i64, Option<errors::SharedError>) {
        dispatch_submit_job_parts(&self.queue, _ctx, query)
    }

    fn GetJobStatus(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<JobStatus, errors::SharedError> {
        dispatch_get_job_status(&self.queue, _ctx, job_id)
    }

    fn CancelJob(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<(), errors::SharedError> {
        dispatch_cancel_job(&self.queue, _ctx, job_id)
    }

    fn GetGroupSummary(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<GroupStatus, errors::SharedError> {
        dispatch_get_group_summary(&self.queue, _ctx, group_key)
    }

    fn GetJobsByGroup(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<Vec<JobStatus>, errors::SharedError> {
        dispatch_get_jobs_by_group(&self.queue, _ctx, group_key)
    }
}

/// MockSDK 作为 SQLGenerator。
impl SQLGenerator for MockSDK {
    sql_generator_parts_trait_api!(MockSDK);
    fn GenerateImportSQL(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
    ) -> Result<String, errors::SharedError> {
        dispatch_generate_import_sql(&self.queue, table_meta, options)
    }
}

/// MockSDK 作为 SDK 门面（Close）。
impl SDK for MockSDK {
    fn Close(&mut self) -> Result<(), errors::SharedError> {
        dispatch_close(&self.queue)
    }
}

/// 派发 Close 并解包 Unit 响应。
fn dispatch_close(queue: &MockQueue) -> Result<(), errors::SharedError> {
    let call = MockCall::Close;
    match queue.dispatch(call.clone())? {
        MockResponse::Unit(result) => result,
        _ => Err(wrong_response(&call)),
    }
}

/// 派发 SubmitJob 并解包 JobID 响应。
fn dispatch_submit_job(
    queue: &MockQueue,
    context: &(dyn Any + Send + Sync),
    query: &str,
) -> Result<i64, errors::SharedError> {
    let call = MockCall::SubmitJob {
        query: query.to_owned(),
    };
    match queue.dispatch_with_context(call.clone(), context)? {
        MockResponse::JobID(result) => result,
        MockResponse::Dual(value, error) => dual_result::<i64>(value, error, &call),
        MockResponse::JobIDParts(job_id, Some(error)) => {
            let _ = job_id;
            Err(error)
        }
        MockResponse::JobIDParts(job_id, None) => Ok(job_id),
        _ => Err(wrong_response(&call)),
    }
}

/// 保留 Go 的两个返回位，同时维持原有 Result API。
fn dispatch_submit_job_parts(
    queue: &MockQueue,
    context: &(dyn Any + Send + Sync),
    query: &str,
) -> (i64, Option<errors::SharedError>) {
    let call = MockCall::SubmitJob {
        query: query.to_owned(),
    };
    match queue.dispatch_with_context(call.clone(), context) {
        Ok(MockResponse::JobIDParts(job_id, error)) => (job_id, error),
        Ok(response) => {
            let (value, error) = response_parts::<i64>(response, &call);
            (value.unwrap_or(0), error)
        }
        Err(error) => (0, Some(error)),
    }
}

/// 派发 GetJobStatus 并解包 JobStatus 响应。
fn dispatch_get_job_status(
    queue: &MockQueue,
    context: &(dyn Any + Send + Sync),
    job_id: i64,
) -> Result<JobStatus, errors::SharedError> {
    let call = MockCall::GetJobStatus { job_id };
    match queue.dispatch_with_context(call.clone(), context)? {
        MockResponse::JobStatus(result) => result,
        MockResponse::Dual(value, error) => dual_result::<JobStatus>(value, error, &call),
        _ => Err(wrong_response(&call)),
    }
}

/// 派发 CancelJob 并解包 Unit 响应。
fn dispatch_cancel_job(
    queue: &MockQueue,
    context: &(dyn Any + Send + Sync),
    job_id: i64,
) -> Result<(), errors::SharedError> {
    let call = MockCall::CancelJob { job_id };
    match queue.dispatch_with_context(call.clone(), context)? {
        MockResponse::Unit(result) => result,
        _ => Err(wrong_response(&call)),
    }
}

/// 派发 GetGroupSummary 并解包 GroupStatus 响应。
fn dispatch_get_group_summary(
    queue: &MockQueue,
    context: &(dyn Any + Send + Sync),
    group_key: &str,
) -> Result<GroupStatus, errors::SharedError> {
    let call = MockCall::GetGroupSummary {
        group_key: group_key.to_owned(),
    };
    match queue.dispatch_with_context(call.clone(), context)? {
        MockResponse::GroupStatus(result) => result,
        MockResponse::Dual(value, error) => dual_result::<GroupStatus>(value, error, &call),
        _ => Err(wrong_response(&call)),
    }
}

/// 派发 GetJobsByGroup 并解包作业列表响应。
fn dispatch_get_jobs_by_group(
    queue: &MockQueue,
    context: &(dyn Any + Send + Sync),
    group_key: &str,
) -> Result<Vec<JobStatus>, errors::SharedError> {
    let call = MockCall::GetJobsByGroup {
        group_key: group_key.to_owned(),
    };
    match queue.dispatch_with_context(call.clone(), context)? {
        MockResponse::JobStatuses(result) => result,
        MockResponse::Dual(value, error) => dual_result::<Vec<JobStatus>>(value, error, &call),
        _ => Err(wrong_response(&call)),
    }
}

/// 派发 GenerateImportSQL；用 Debug 签名匹配表元数据与选项。
fn dispatch_generate_import_sql(
    queue: &MockQueue,
    table_meta: &TableMeta,
    options: &ImportOptions,
) -> Result<String, errors::SharedError> {
    let call = MockCall::GenerateImportSQL {
        table_meta: table_meta_signature(table_meta),
        options: import_options_signature(options),
    };
    match queue.dispatch(call.clone())? {
        MockResponse::SQL(result) => result,
        MockResponse::Dual(value, error) => dual_result::<String>(value, error, &call),
        MockResponse::SQLCallback(callback) => callback(table_meta, options),
        _ => Err(wrong_response(&call)),
    }
}

fn register_sql_callback(
    queue: &MockQueue,
    matcher: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
    callback: impl Fn(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError>
    + Send
    + Sync
    + 'static,
) -> MockExpectation {
    queue.expect_matching(
        MockCall::GenerateImportSQL {
            table_meta: "<matcher>".into(),
            options: "<matcher>".into(),
        },
        MockResponse::SQLCallback(Arc::new(callback)),
        move |actual| match actual {
            MockCall::GenerateImportSQL {
                table_meta,
                options,
            } => matcher(table_meta, options),
            _ => false,
        },
    )
}
