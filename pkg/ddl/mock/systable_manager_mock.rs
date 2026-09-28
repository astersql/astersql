// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 系统表管理器（systable manager）的 GoMock 风格替身。
//
// 系统表管理器从 DDL 系统表读取 Job、MDL（Metadata Lock，元数据锁）版本、
// 最小 job ID，以及是否存在 flashback cluster（集群闪回）类作业。测试通过
// `MockManager` + `Controller` 预设这些查询的返回值。

use crate::{Argument, Controller, Matcher, MockError, ReturnValue};

pub use ddl_systable::{Context, JobWrapper, Manager, Session};

fn production_error(error: MockError) -> ddl_systable::Error {
    ddl_systable::Error::Execute(error.to_string())
}

fn manager_return<T>(
    result: Result<T, ddl_systable::Error>,
    success: impl FnOnce(T) -> ReturnValue,
) -> Result<ReturnValue, MockError> {
    Ok(match result {
        Ok(value) => success(value),
        Err(error) => ReturnValue::ManagerError(error),
    })
}

/// 由共享 `Controller` 驱动的 `Manager` Mock。
pub struct MockManager {
    controller: Controller,
    recorder: MockManagerRecorder,
}

/// 录制 `MockManager` 各方法期望调用的辅助对象。
#[derive(Clone)]
pub struct MockManagerRecorder {
    controller: Controller,
}

/// 用给定控制器构造 Mock 系统表管理器。
pub fn new_mock_manager(controller: Controller) -> MockManager {
    MockManager {
        recorder: MockManagerRecorder {
            controller: controller.clone(),
        },
        controller,
    }
}

impl MockManager {
    /// 返回用于录制期望的 recorder。
    pub fn expect(&self) -> &MockManagerRecorder {
        &self.recorder
    }

    /// 类型标记方法：表明实现为 Mock。
    pub fn is_mock(&self) {}

    /// 转发到共享 Controller 的实际调用入口。
    fn call(
        &self,
        method: &'static str,
        arguments: Vec<Argument>,
    ) -> Result<ReturnValue, MockError> {
        self.controller.call(method, arguments)
    }
}

impl Manager for MockManager {
    fn get_job_by_id(
        &self,
        context: &Context,
        job_id: i64,
    ) -> Result<JobWrapper, ddl_systable::Error> {
        // 参数顺序对齐 Go：request_id、job_id。
        match self
            .call(
                "GetJobByID",
                vec![
                    Argument::Text(context.request_id.clone()),
                    Argument::Int(job_id),
                ],
            )
            .map_err(production_error)?
        {
            ReturnValue::Job(bytes) => Ok(JobWrapper {
                job: ddl_systable::Job::decode(&bytes)
                    .map_err(|error| ddl_systable::Error::Decode(error.to_string()))?,
                bytes,
            }),
            ReturnValue::ManagerError(error) => Err(error),
            _ => Err(ddl_systable::Error::Decode(
                "GetJobByID returned the wrong type".into(),
            )),
        }
    }

    fn get_job_bytes_by_id_with_session(
        &self,
        context: &Context,
        _session: &mut dyn Session,
        job_id: i64,
    ) -> Result<Vec<u8>, ddl_systable::Error> {
        match self
            .call(
                "GetJobBytesByIDWithSe",
                vec![
                    Argument::Text(context.request_id.clone()),
                    Argument::Session,
                    Argument::Int(job_id),
                ],
            )
            .map_err(production_error)?
        {
            ReturnValue::Bytes(bytes) => Ok(bytes),
            ReturnValue::ManagerError(error) => Err(error),
            _ => Err(ddl_systable::Error::Decode(
                "GetJobBytesByIDWithSe returned the wrong type".into(),
            )),
        }
    }

    fn get_mdl_version(&self, context: &Context, job_id: i64) -> Result<i64, ddl_systable::Error> {
        match self
            .call(
                "GetMDLVer",
                vec![
                    Argument::Text(context.request_id.clone()),
                    Argument::Int(job_id),
                ],
            )
            .map_err(production_error)?
        {
            ReturnValue::Int(value) => Ok(value),
            ReturnValue::ManagerError(error) => Err(error),
            _ => Err(ddl_systable::Error::Decode(
                "GetMDLVer returned the wrong type".into(),
            )),
        }
    }

    fn get_min_job_id(&self, context: &Context, job_id: i64) -> Result<i64, ddl_systable::Error> {
        match self
            .call(
                "GetMinJobID",
                vec![
                    Argument::Text(context.request_id.clone()),
                    Argument::Int(job_id),
                ],
            )
            .map_err(production_error)?
        {
            ReturnValue::Int(value) => Ok(value),
            ReturnValue::ManagerError(error) => Err(error),
            _ => Err(ddl_systable::Error::Decode(
                "GetMinJobID returned the wrong type".into(),
            )),
        }
    }

    fn has_flashback_cluster_job(
        &self,
        context: &Context,
        min_job_id: i64,
    ) -> Result<bool, ddl_systable::Error> {
        match self
            .call(
                "HasFlashbackClusterJob",
                vec![
                    Argument::Text(context.request_id.clone()),
                    Argument::Int(min_job_id),
                ],
            )
            .map_err(production_error)?
        {
            ReturnValue::Bool(value) => Ok(value),
            ReturnValue::ManagerError(error) => Err(error),
            _ => Err(ddl_systable::Error::Decode(
                "HasFlashbackClusterJob returned the wrong type".into(),
            )),
        }
    }
}

impl MockManagerRecorder {
    /// 录制 `GetJobByID` 的期望参数与返回 Job。
    pub fn get_job_by_id(
        &self,
        context: Matcher,
        job_id: Matcher,
        result: Result<JobWrapper, ddl_systable::Error>,
    ) {
        self.controller.record(
            "GetJobByID",
            vec![context, job_id],
            manager_return(result, |job| ReturnValue::Job(job.bytes)),
        );
    }

    /// 录制 `GetJobBytesByIDWithSe` 的期望参数与返回字节。
    pub fn get_job_bytes_by_id_with_session(
        &self,
        context: Matcher,
        session: Matcher,
        job_id: Matcher,
        result: Result<Vec<u8>, ddl_systable::Error>,
    ) {
        self.controller.record(
            "GetJobBytesByIDWithSe",
            vec![context, session, job_id],
            manager_return(result, ReturnValue::Bytes),
        );
    }

    /// 录制 `GetMDLVer` 的期望参数与返回版本号。
    pub fn get_mdl_version(
        &self,
        context: Matcher,
        job_id: Matcher,
        result: Result<i64, ddl_systable::Error>,
    ) {
        self.controller.record(
            "GetMDLVer",
            vec![context, job_id],
            manager_return(result, ReturnValue::Int),
        );
    }

    /// 录制 `GetMinJobID` 的期望参数与返回最小 job ID。
    pub fn get_min_job_id(
        &self,
        context: Matcher,
        job_id: Matcher,
        result: Result<i64, ddl_systable::Error>,
    ) {
        self.controller.record(
            "GetMinJobID",
            vec![context, job_id],
            manager_return(result, ReturnValue::Int),
        );
    }

    /// 录制 `HasFlashbackClusterJob` 的期望参数与布尔结果。
    pub fn has_flashback_cluster_job(
        &self,
        context: Matcher,
        min_job_id: Matcher,
        result: Result<bool, ddl_systable::Error>,
    ) {
        self.controller.record(
            "HasFlashbackClusterJob",
            vec![context, min_job_id],
            manager_return(result, ReturnValue::Bool),
        );
    }
}
