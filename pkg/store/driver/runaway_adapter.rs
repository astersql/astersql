// Copyright 2026 AsterSQL.

use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use astersql_kv as kv;
use astersql_store_copr as copr;

pub(crate) struct KVRunawayChecker {
    inner: kv::resourcegroup::SharedRunawayChecker,
}

impl KVRunawayChecker {
    pub(crate) fn new(
        inner: kv::resourcegroup::SharedRunawayChecker,
    ) -> Arc<dyn copr::RunawayChecker> {
        Arc::new(Self { inner })
    }
}

impl Debug for KVRunawayChecker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("KVRunawayChecker")
    }
}

impl copr::RunawayChecker for KVRunawayChecker {
    fn before_cop_request(&self, wire: &mut copr::CopWireRequest) -> copr::BatchResult<()> {
        let mut request = kv::resourcegroup::CopRequest {
            priority_low: wire.priority == copr::Priority::Low,
            resource_group_name: wire.resource_group_name.clone(),
            max_execution_duration_ms: wire.max_execution_duration_ms,
        };
        self.inner
            .BeforeCopRequest(&mut request)
            .map_err(|_| copr::BatchError::QueryInterrupted)?;
        if request.priority_low {
            wire.priority = copr::Priority::Low;
        }
        wire.resource_group_name = request.resource_group_name;
        wire.max_execution_duration_ms = request.max_execution_duration_ms;
        Ok(())
    }

    fn check_thresholds(
        &self,
        ru: Option<&copr::CopRUDetails>,
        processed_keys: u64,
        error: Option<&copr::BatchError>,
    ) -> copr::BatchResult<()> {
        let ru = ru.map(|ru| kv::resourcegroup::RUDetails {
            read_ru: ru.read_ru,
            write_ru: ru.write_ru,
        });
        let original_error = error.map(ToString::to_string);
        self.inner
            .CheckThresholds(ru.as_ref(), processed_keys, original_error.as_deref())
            .map_err(|_| copr::BatchError::QueryInterrupted)
    }

    fn reset_total_processed_keys(&self) {
        self.inner.ResetTotalProcessedKeys();
    }

    fn check_action(&self) -> copr::RunawayAction {
        match self.inner.CheckAction() {
            kv::resourcegroup::RunawayAction::CoolDown => copr::RunawayAction::CoolDown,
            kv::resourcegroup::RunawayAction::Kill => copr::RunawayAction::Kill,
            kv::resourcegroup::RunawayAction::None => copr::RunawayAction::None,
        }
    }
}

pub(crate) struct KVCopRUInterceptor {
    inner: kv::resourcegroup::SharedCopRUInterceptor,
}

impl KVCopRUInterceptor {
    pub(crate) fn new(
        inner: kv::resourcegroup::SharedCopRUInterceptor,
    ) -> Arc<dyn copr::CopRUInterceptor> {
        Arc::new(Self { inner })
    }

    fn request_info(
        task: &copr::CopTask,
        wire: &copr::CopWireRequest,
    ) -> kv::resourcegroup::CopRPCRequestInfo {
        kv::resourcegroup::CopRPCRequestInfo {
            resource_group_name: wire.resource_group_name.clone(),
            request_type: format!("{:?}", wire.request_type),
            region_id: task.region.id,
            store_address: task.store_address.clone(),
            data_bytes: wire.data.len(),
            priority_low: wire.priority == copr::Priority::Low,
        }
    }
}

impl Debug for KVCopRUInterceptor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("KVCopRUInterceptor")
    }
}

impl copr::CopRUInterceptor for KVCopRUInterceptor {
    fn on_request_wait(
        &self,
        task: &copr::CopTask,
        wire: &copr::CopWireRequest,
    ) -> copr::BatchResult<copr::CopRUDetails> {
        self.inner
            .OnRequestWait(&Self::request_info(task, wire))
            .map(|details| copr::CopRUDetails {
                read_ru: details.read_ru,
                write_ru: details.write_ru,
            })
            .map_err(copr::BatchError::OtherResponse)
    }

    fn on_response_wait(
        &self,
        task: &copr::CopTask,
        wire: &copr::CopWireRequest,
        response: &copr::CopProtocolResponse,
    ) -> copr::BatchResult<copr::CopRUDetails> {
        let error = response
            .region_error
            .as_ref()
            .cloned()
            .or_else(|| (!response.other_error.is_empty()).then(|| response.other_error.clone()))
            .or_else(|| response.locked.as_ref().map(|_| "locked".to_owned()));
        let response_info = kv::resourcegroup::CopRPCResponseInfo {
            region_id: task.region.id,
            data_bytes: response.data.len()
                + response
                    .batch_responses
                    .values()
                    .map(|child| child.data.len())
                    .sum::<usize>(),
            processed_keys: response.scanned_keys
                + response
                    .batch_responses
                    .values()
                    .map(|child| child.scanned_keys)
                    .sum::<u64>(),
            error,
        };
        self.inner
            .OnResponseWait(&Self::request_info(task, wire), &response_info)
            .map(|details| copr::CopRUDetails {
                read_ru: details.read_ru,
                write_ru: details.write_ru,
            })
            .map_err(copr::BatchError::OtherResponse)
    }
}
