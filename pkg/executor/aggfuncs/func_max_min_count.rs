// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::aggfuncs::*;
use crate::builder::{AggImplementation, BuiltAggFunc, ValueKind};
use crate::{DeserializeHelper, SerializeHelper};
use astersql_util_collate as collate;
use astersql_util_serialization::{
    self as ser,
    chunk::{Chunk, Row},
    types,
};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::mem::size_of;

/// Typed evaluators preserve Go's distinct numeric, temporal and collated paths.
pub trait CountValue: Default + Send + 'static {
    fn copy_value(&self) -> Self;
    fn eval(
        arg: &dyn Expression,
        ctx: &dyn EvalContext,
        row: Row,
    ) -> Result<Option<Self>, AggError>;
    fn compare(&self, other: &Self, collator: &dyn collate::Collator) -> Ordering;
    fn retained_size(&self) -> i64 {
        0
    }
    fn write(&self, buffer: Vec<u8>) -> Vec<u8>;
    fn read(buffer: &mut ser::PosAndBuf) -> Self;
}

macro_rules! count_value {
    ($ty:ty, $eval:ident, $convert:expr, $compare:expr, $size:expr, $write:expr, $read:ident) => {
        impl CountValue for $ty {
            fn copy_value(&self) -> Self {
                self.clone()
            }
            fn eval(
                arg: &dyn Expression,
                ctx: &dyn EvalContext,
                row: Row,
            ) -> Result<Option<Self>, AggError> {
                let (value, is_null) = arg.$eval(ctx, row).map_err(|e| AggError(e.to_string()))?;
                Ok((!is_null).then(|| ($convert)(value)))
            }
            fn compare(&self, other: &Self, collator: &dyn collate::Collator) -> Ordering {
                ($compare)(self, other, collator)
            }
            fn retained_size(&self) -> i64 {
                ($size)(self)
            }
            fn write(&self, buffer: Vec<u8>) -> Vec<u8> {
                ($write)(self, buffer)
            }
            fn read(buffer: &mut ser::PosAndBuf) -> Self {
                ser::$read(buffer)
            }
        }
    };
}
fn float_compare(a: f64, b: f64) -> Ordering {
    if a < b || (a.is_nan() && !b.is_nan()) {
        Ordering::Less
    } else if a > b || (!a.is_nan() && b.is_nan()) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}
count_value!(
    i64,
    EvalInt,
    |v| v,
    |a: &i64, b: &i64, _: &dyn collate::Collator| a.cmp(b),
    |_| 0,
    |v: &i64, b| ser::SerializeInt64(*v, b),
    DeserializeInt64
);
count_value!(
    u64,
    EvalInt,
    |v: i64| v as u64,
    |a: &u64, b: &u64, _: &dyn collate::Collator| a.cmp(b),
    |_| 0,
    |v: &u64, b| ser::SerializeUint64(*v, b),
    DeserializeUint64
);
count_value!(
    f32,
    EvalReal,
    |v: f64| v as f32,
    |a: &f32, b: &f32, _: &dyn collate::Collator| float_compare(*a as f64, *b as f64),
    |_| 0,
    |v: &f32, b| ser::SerializeFloat32(*v, b),
    DeserializeFloat32
);
count_value!(
    f64,
    EvalReal,
    |v| v,
    |a: &f64, b: &f64, _: &dyn collate::Collator| float_compare(*a, *b),
    |_| 0,
    |v: &f64, b| ser::SerializeFloat64(*v, b),
    DeserializeFloat64
);
count_value!(
    types::MyDecimal,
    EvalDecimal,
    |v| v,
    |a: &types::MyDecimal, b: &types::MyDecimal, _: &dyn collate::Collator| a.Compare(b).cmp(&0),
    |_| 0,
    |v, b| ser::SerializeMyDecimal(v, b),
    DeserializeMyDecimal
);
count_value!(
    types::Time,
    EvalTime,
    |v| v,
    |a: &types::Time, b: &types::Time, _: &dyn collate::Collator| a.Compare(*b).cmp(&0),
    |_| 0,
    |v: &types::Time, b| ser::SerializeTime(*v, b),
    DeserializeTime
);
count_value!(
    types::Duration,
    EvalDuration,
    |v| v,
    |a: &types::Duration, b: &types::Duration, _: &dyn collate::Collator| a
        .Duration
        .cmp(&b.Duration),
    |_| 0,
    |v: &types::Duration, b| ser::SerializeTypesDuration(*v, b),
    DeserializeTypesDuration
);
count_value!(
    String,
    EvalString,
    |v| v,
    |a: &String, b: &String, c: &dyn collate::Collator| c.Compare(a, b).cmp(&0),
    |v: &String| v.len() as i64,
    |v: &String, b| ser::SerializeString(v, b),
    DeserializeString
);
count_value!(
    types::BinaryJSON,
    EvalJSON,
    |v| v,
    |a: &types::BinaryJSON, b: &types::BinaryJSON, _: &dyn collate::Collator| {
        types::CompareBinaryJSON(a, b).cmp(&0)
    },
    |v: &types::BinaryJSON| v.Value.len() as i64,
    |v, b| ser::SerializeBinaryJSON(v, b),
    DeserializeBinaryJSON
);
impl CountValue for types::VectorFloat32 {
    fn copy_value(&self) -> Self {
        self.Clone()
    }
    fn eval(
        arg: &dyn Expression,
        ctx: &dyn EvalContext,
        row: Row,
    ) -> Result<Option<Self>, AggError> {
        let (v, n) = arg
            .EvalVectorFloat32(ctx, row)
            .map_err(|e| AggError(e.to_string()))?;
        Ok((!n).then_some(v))
    }
    fn compare(&self, other: &Self, _: &dyn collate::Collator) -> Ordering {
        self.Compare(other).cmp(&0)
    }
    fn retained_size(&self) -> i64 {
        self.SerializedSize() as i64
    }
    fn write(&self, b: Vec<u8>) -> Vec<u8> {
        ser::SerializeVectorFloat32(self, b)
    }
    fn read(b: &mut ser::PosAndBuf) -> Self {
        ser::DeserializeVectorFloat32(b)
    }
}
impl CountValue for types::Enum {
    fn copy_value(&self) -> Self {
        self.clone()
    }
    fn eval(
        arg: &dyn Expression,
        ctx: &dyn EvalContext,
        row: Row,
    ) -> Result<Option<Self>, AggError> {
        let value = arg.Eval(ctx, row).map_err(|e| AggError(e.to_string()))?;
        Ok((!value.IsNull()).then(|| value.GetMysqlEnum()))
    }
    fn compare(&self, other: &Self, c: &dyn collate::Collator) -> Ordering {
        c.Compare(&self.Name, &other.Name).cmp(&0)
    }
    fn retained_size(&self) -> i64 {
        self.Name.len() as i64
    }
    fn write(&self, b: Vec<u8>) -> Vec<u8> {
        ser::SerializeEnum(self, b)
    }
    fn read(b: &mut ser::PosAndBuf) -> Self {
        ser::DeserializeEnum(b)
    }
}
impl CountValue for types::Set {
    fn copy_value(&self) -> Self {
        self.clone()
    }
    fn eval(
        arg: &dyn Expression,
        ctx: &dyn EvalContext,
        row: Row,
    ) -> Result<Option<Self>, AggError> {
        let value = arg.Eval(ctx, row).map_err(|e| AggError(e.to_string()))?;
        Ok((!value.IsNull()).then(|| value.GetMysqlSet()))
    }
    fn compare(&self, other: &Self, c: &dyn collate::Collator) -> Ordering {
        c.Compare(&self.Name, &other.Name).cmp(&0)
    }
    fn retained_size(&self) -> i64 {
        self.Name.len() as i64
    }
    fn write(&self, b: Vec<u8>) -> Vec<u8> {
        ser::SerializeSet(self, b)
    }
    fn read(b: &mut ser::PosAndBuf) -> Self {
        ser::DeserializeSet(b)
    }
}

/// Layout mirrors Go partial results: extrema, multiplicity, NULL marker.
#[derive(Clone, Debug)]
#[repr(C)]
pub struct CountPartial<T> {
    pub value: T,
    pub count: i64,
    pub is_null: bool,
}
impl<T: Default> Default for CountPartial<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            count: 0,
            is_null: true,
        }
    }
}
struct Peer<T> {
    value: T,
    indices: VecDeque<u64>,
}
struct CountDeque<T> {
    peers: VecDeque<Peer<T>>,
}
struct SlidingPartial<T> {
    deque: Box<CountDeque<T>>,
    count: i64,
    is_null: bool,
}
impl<T: CountValue> CountDeque<T> {
    fn enqueue(&mut self, index: u64, value: T, is_max: bool, collator: &dyn collate::Collator) {
        while let Some(back) = self.peers.back_mut() {
            let cmp = value.compare(&back.value, collator);
            if (is_max && cmp == Ordering::Greater) || (!is_max && cmp == Ordering::Less) {
                self.peers.pop_back();
                continue;
            }
            if cmp == Ordering::Equal {
                back.indices.push_back(index);
                return;
            }
            break;
        }
        self.peers.push_back(Peer {
            value,
            indices: VecDeque::from([index]),
        });
    }
    fn dequeue(&mut self, boundary: u64) {
        while let Some(front) = self.peers.front_mut() {
            while front.indices.front().is_some_and(|i| *i <= boundary) {
                front.indices.pop_front();
            }
            if !front.indices.is_empty() {
                break;
            }
            self.peers.pop_front();
        }
    }
}
impl<T> SlidingPartial<T> {
    fn refresh(&mut self) {
        self.count = self
            .deque
            .peers
            .front()
            .map_or(0, |p| p.indices.len() as i64);
        self.is_null = self.deque.peers.is_empty();
    }
}

/// Real AggFunc implementation, including evaluated rows, merge and spill.
pub struct CountExtrema<T> {
    pub base: BaseAggFunc,
    is_max: bool,
    collator: Box<dyn collate::Collator>,
    reject_rows: bool,
    sliding: bool,
    start: u64,
    marker: std::marker::PhantomData<T>,
}
impl<T: CountValue> CountExtrema<T> {
    fn new(
        base: BaseAggFunc,
        is_max: bool,
        collation: &str,
        reject_rows: bool,
        sliding: bool,
    ) -> Self {
        Self {
            base,
            is_max,
            collator: collate::GetCollator(collation),
            reject_rows,
            sliding,
            start: 0,
            marker: std::marker::PhantomData,
        }
    }
    fn absorb(&self, dst: &mut CountPartial<T>, value: T, count: i64) -> i64 {
        let cmp = if dst.is_null {
            Ordering::Equal
        } else {
            value.compare(&dst.value, self.collator.as_ref())
        };
        if dst.is_null
            || (self.is_max && cmp == Ordering::Greater)
            || (!self.is_max && cmp == Ordering::Less)
        {
            let delta = value.retained_size() - dst.value.retained_size();
            dst.value = value;
            dst.count = count;
            dst.is_null = false;
            delta
        } else {
            if cmp == Ordering::Equal {
                dst.count = dst.count.wrapping_add(count);
            }
            0
        }
    }
}
impl<T: CountValue> Serializer for CountExtrema<T> {
    fn serialize_partial_result(
        &self,
        pr: &PartialResult,
        chunk: &mut Chunk,
        helper: &mut SerializeHelper,
    ) {
        // Go sliding states inherit a non-sliding serializer and are never spilled.
        let state = pr
            .downcast_ref::<CountPartial<T>>()
            .expect("count extrema spill state");
        chunk.AppendBytes(self.base.ordinal, helper.serialize_count_extrema(state));
    }
    fn deserialize_partial_result(&self, source: &Chunk) -> (Vec<PartialResult>, i64) {
        deserialize_partial_result_common(source, self.base.ordinal, |helper| {
            let Some(state) = helper.deserialize_count_extrema::<T>() else {
                return (None, 0);
            };
            let delta = size_of::<CountPartial<T>>() as i64 + state.value.retained_size();
            (Some(Box::new(state)), delta)
        })
    }
}
impl<T: CountValue> AggFunc for CountExtrema<T> {
    fn alloc_partial_result(&self) -> (PartialResult, i64) {
        if self.sliding {
            (
                Box::new(SlidingPartial::<T> {
                    deque: Box::new(CountDeque {
                        peers: VecDeque::with_capacity(64),
                    }),
                    count: 0,
                    is_null: true,
                }),
                size_of::<SlidingPartial<T>>() as i64 + size_of::<CountDeque<T>>() as i64,
            )
        } else {
            (
                Box::new(CountPartial::<T>::default()),
                size_of::<CountPartial<T>>() as i64,
            )
        }
    }
    fn reset_partial_result(&self, pr: &mut PartialResult) {
        if self.sliding {
            let p = pr.downcast_mut::<SlidingPartial<T>>().unwrap();
            p.deque.peers.clear();
            p.count = 0;
            p.is_null = true;
        } else {
            *pr.downcast_mut::<CountPartial<T>>().unwrap() = CountPartial::default();
        }
    }
    fn update_partial_result(
        &self,
        ctx: &dyn EvalContext,
        rows: &[Row],
        pr: &mut PartialResult,
    ) -> Result<i64, AggError> {
        if self.reject_rows {
            return Err(AggError(format!(
                "row-based final aggregation for {} is unsupported",
                if self.is_max {
                    "max_count"
                } else {
                    "min_count"
                }
            )));
        }
        let mut delta = 0;
        for (i, row) in rows.iter().enumerate() {
            if let Some(value) = T::eval(self.base.args[0].as_ref(), ctx, row.clone())? {
                if self.sliding {
                    pr.downcast_mut::<SlidingPartial<T>>()
                        .unwrap()
                        .deque
                        .enqueue(
                            self.start + i as u64,
                            value,
                            self.is_max,
                            self.collator.as_ref(),
                        );
                } else {
                    delta += self.absorb(pr.downcast_mut::<CountPartial<T>>().unwrap(), value, 1);
                }
            }
        }
        if self.sliding {
            pr.downcast_mut::<SlidingPartial<T>>().unwrap().refresh();
        }
        Ok(delta)
    }
    fn merge_partial_result(
        &self,
        _: &dyn EvalContext,
        src: &PartialResult,
        dst: &mut PartialResult,
    ) -> Result<i64, AggError> {
        let src = src.downcast_ref::<CountPartial<T>>().unwrap();
        if src.is_null {
            return Ok(0);
        }
        Ok(self.absorb(
            dst.downcast_mut::<CountPartial<T>>().unwrap(),
            src.value.copy_value(),
            src.count,
        ))
    }
    fn append_final_result_to_chunk(
        &self,
        _: &dyn EvalContext,
        pr: &PartialResult,
        chunk: &mut Chunk,
    ) -> Result<(), AggError> {
        let (is_null, count) = if self.sliding {
            let p = pr.downcast_ref::<SlidingPartial<T>>().unwrap();
            (p.is_null, p.count)
        } else {
            let p = pr.downcast_ref::<CountPartial<T>>().unwrap();
            (p.is_null, p.count)
        };
        chunk.AppendInt64(self.base.ordinal, if is_null { 0 } else { count });
        Ok(())
    }
}
impl<T: CountValue> SlidingWindowAggFunc for CountExtrema<T> {
    fn slide(
        &self,
        ctx: &dyn EvalContext,
        get_row: &mut dyn FnMut(u64) -> Row,
        last_start: u64,
        last_end: u64,
        shift_start: u64,
        shift_end: u64,
        pr: &mut PartialResult,
    ) -> Result<(), AggError> {
        let p = pr
            .downcast_mut::<SlidingPartial<T>>()
            .expect("sliding count extrema state");
        for i in 0..shift_end {
            if let Some(value) = T::eval(self.base.args[0].as_ref(), ctx, get_row(last_end + i))? {
                p.deque
                    .enqueue(last_end + i, value, self.is_max, self.collator.as_ref());
            }
        }
        if last_start + shift_start >= 1 {
            p.deque.dequeue(last_start + shift_start - 1);
        }
        p.refresh();
        Ok(())
    }
}
impl<T: CountValue> MaxMinSlidingWindowAggFunc for CountExtrema<T> {
    fn set_window_start(&mut self, start: u64) {
        self.start = start;
    }
}

pub trait CountExtremaAgg: AggFunc + SlidingWindowAggFunc + MaxMinSlidingWindowAggFunc {
    fn result_count(&self, state: &PartialResult) -> i64;
}
impl<T: CountValue> CountExtremaAgg for CountExtrema<T> {
    fn result_count(&self, state: &PartialResult) -> i64 {
        let (is_null, count) = if self.sliding {
            let state = state.downcast_ref::<SlidingPartial<T>>().unwrap();
            (state.is_null, state.count)
        } else {
            let state = state.downcast_ref::<CountPartial<T>>().unwrap();
            (state.is_null, state.count)
        };
        if is_null { 0 } else { count }
    }
}

impl BuiltAggFunc {
    /// Instantiate the selected evaluator from the actual argument expressions.
    pub fn instantiate_count_extrema(
        &self,
        args: Vec<Box<dyn Expression>>,
        collation: &str,
    ) -> Option<Box<dyn CountExtremaAgg>> {
        let (kind, is_max, reject_rows, sliding) = match self.implementation {
            AggImplementation::MaxMinCount {
                kind,
                is_max,
                reject_rows,
            } => (kind, is_max, reject_rows, false),
            AggImplementation::SlidingMaxMinCount { kind, is_max } => (kind, is_max, false, true),
            _ => return None,
        };
        let base = BaseAggFunc {
            args,
            ordinal: self.ordinal,
            return_type: None,
        };
        macro_rules! make {
            ($t:ty) => {
                Some(Box::new(CountExtrema::<$t>::new(
                    base,
                    is_max,
                    collation,
                    reject_rows,
                    sliding,
                )) as Box<dyn CountExtremaAgg>)
            };
        }
        match kind {
            ValueKind::Int => make!(i64),
            ValueKind::Uint => make!(u64),
            ValueKind::Float32 => make!(f32),
            ValueKind::Float64 => make!(f64),
            ValueKind::Decimal => make!(types::MyDecimal),
            ValueKind::Time => make!(types::Time),
            ValueKind::Duration => make!(types::Duration),
            ValueKind::String => make!(String),
            ValueKind::Json => make!(types::BinaryJSON),
            ValueKind::Enum => make!(types::Enum),
            ValueKind::Set => make!(types::Set),
            ValueKind::VectorFloat32 => make!(types::VectorFloat32),
        }
    }
}

/// Bridge the SQL descriptor's real field types to the registered evaluator factory.
pub fn build_count_extrema_function(
    context: &dyn EvalContext,
    args: Vec<Box<dyn Expression>>,
    ordinal: usize,
    is_max: bool,
    reject_rows: bool,
    sliding: bool,
) -> Option<Box<dyn CountExtremaAgg>> {
    use crate::builder::*;
    use astersql_expression::{mysql, types as sql_types};
    let field = args.first()?.GetType(context);
    let kind = match field.GetType() {
        mysql::TypeEnum => ValueKind::Enum,
        mysql::TypeSet => ValueKind::Set,
        mysql::TypeBit => ValueKind::String,
        _ => match field.EvalType() {
            sql_types::ETInt => {
                if mysql::HasUnsignedFlag(field.GetFlag()) {
                    ValueKind::Uint
                } else {
                    ValueKind::Int
                }
            }
            sql_types::ETReal => match field.GetType() {
                mysql::TypeFloat => ValueKind::Float32,
                mysql::TypeDouble => ValueKind::Float64,
                _ => return None,
            },
            sql_types::ETDecimal => ValueKind::Decimal,
            sql_types::ETString => ValueKind::String,
            sql_types::ETDatetime | sql_types::ETTimestamp => ValueKind::Time,
            sql_types::ETDuration => ValueKind::Duration,
            sql_types::ETJson => ValueKind::Json,
            sql_types::ETVectorFloat32 => ValueKind::VectorFloat32,
            _ => return None,
        },
    };
    let collation = field.GetCollate().to_owned();
    let implementation = if sliding
        && !reject_rows
        && !matches!(
            kind,
            ValueKind::Enum | ValueKind::Set | ValueKind::Json | ValueKind::VectorFloat32
        ) {
        AggImplementation::SlidingMaxMinCount { kind, is_max }
    } else {
        AggImplementation::MaxMinCount {
            kind,
            is_max,
            reject_rows,
        }
    };
    BuiltAggFunc {
        implementation,
        ordinal,
        argument_count: args.len(),
        order_by: vec![],
        separator: None,
        max_len: None,
        default_value: None,
    }
    .instantiate_count_extrema(args, &collation)
}
