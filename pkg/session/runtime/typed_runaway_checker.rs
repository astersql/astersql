// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_kv as kv;
use astersql_resourcegroup_runaway as runaway;

pub(super) struct SessionRunawayChecker(pub(super) Arc<runaway::checker::Checker>);

impl kv::resourcegroup::RunawayChecker for SessionRunawayChecker {
    fn BeforeCopRequest(&self, request: &mut kv::resourcegroup::CopRequest) -> Result<(), String> {
        let mut canonical = runaway::CopRequest {
            override_priority: request.priority_low.then_some(1),
            resource_group_name: request.resource_group_name.clone(),
            max_execution_duration_ms: request.max_execution_duration_ms,
        };
        self.0
            .BeforeCopRequest(&mut canonical)
            .map_err(|error| error.to_string())?;
        request.priority_low |= canonical.override_priority == Some(1);
        request.resource_group_name = canonical.resource_group_name;
        request.max_execution_duration_ms = canonical.max_execution_duration_ms;
        Ok(())
    }

    fn CheckThresholds(
        &self,
        ru: Option<&kv::resourcegroup::RUDetails>,
        processed_keys: u64,
        original_error: Option<&str>,
    ) -> Result<(), String> {
        let canonical_ru = ru.map(|ru| runaway::RUDetails {
            read_ru: ru.read_ru,
            write_ru: ru.write_ru,
        });
        let original = original_error.map(|message| runaway::Error::Storage(message.to_owned()));
        match self.0.CheckThresholds(
            canonical_ru.as_ref(),
            processed_keys.min(i64::MAX as u64) as i64,
            original.clone(),
        ) {
            None => Ok(()),
            Some(error) if Some(&error) == original.as_ref() => Ok(()),
            Some(error) => Err(error.to_string()),
        }
    }

    fn CheckAction(&self) -> kv::resourcegroup::RunawayAction {
        match self.0.CheckAction() {
            runaway::RunawayAction::CoolDown => kv::resourcegroup::RunawayAction::CoolDown,
            runaway::RunawayAction::Kill => kv::resourcegroup::RunawayAction::Kill,
            _ => kv::resourcegroup::RunawayAction::None,
        }
    }

    fn ResetTotalProcessedKeys(&self) {
        self.0.ResetTotalProcessedKeys();
    }
}
