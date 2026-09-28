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

// PhysicalTableSample returns sampled rows from a base table to its parent.

use std::sync::Arc;

use base::{ContextRef, PhysicalPlan as _};

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// Physical table-sample plan, matching the fields of the Go operator.
pub struct PhysicalTableSample {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub TableSampleInfo: Option<Arc<tablesampler::TableSampleInfo>>,
    pub TableInfo: Option<Arc<dyn table::Table>>,
    pub PhysicalTableID: i64,
    pub Desc: bool,
}

impl PhysicalTableSample {
    pub fn New(ctx: ContextRef, physical_table_id: i64, desc: bool) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeTableSample,
                0,
            )),
            TableSampleInfo: None,
            TableInfo: None,
            PhysicalTableID: physical_table_id,
            Desc: desc,
        }
    }

    pub fn WithTableSampleInfo(mut self, info: Arc<tablesampler::TableSampleInfo>) -> Self {
        self.TableSampleInfo = Some(info);
        self
    }

    /// Initializes metadata and the Go-defined one-row cardinality estimate.
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP(plancodec::TypeTableSample);
        plan.Plan.SetQueryBlockOffset(offset);
        base::PhysicalPlan::set_stats(
            plan,
            property::StatsInfo {
                RowCount: 1.0,
                ..Default::default()
            },
        );
        self
    }

    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            TableSampleInfo: self.TableSampleInfo.clone(),
            TableInfo: self.TableInfo.clone(),
            PhysicalTableID: self.PhysicalTableID,
            Desc: self.Desc,
        })
    }

    pub fn ExplainInfo(&self) -> String {
        String::new()
    }

    /// Matches Go: producer + table interface + bool + optional sample-info payload.
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + std::mem::size_of::<Option<Arc<dyn table::Table>>>() as i64
            + std::mem::size_of::<bool>() as i64
            + self
                .TableSampleInfo
                .as_deref()
                .map_or(0, tablesampler::TableSampleInfo::MemoryUsage)
    }
}
