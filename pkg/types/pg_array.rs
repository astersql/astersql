// Copyright 2026 AsterSQL.

//! Canonical rectangular PostgreSQL array values; SQL NULL is an outer Option.

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgArrayError {
    #[error("array element type is not supported")]
    UnsupportedElement,
    #[error("array dimensions exceed the supported limit")]
    TooManyDimensions,
    #[error("array size exceeds the supported limit")]
    SizeOverflow,
    #[error("array lower bound is too large")]
    BoundsOverflow,
    #[error("array shape and lower bounds have different dimensions")]
    DimensionMismatch,
    #[error("array shape does not match the number of elements")]
    ElementCountMismatch,
}
impl PgArrayError {
    /// Codec-specific malformed-input diagnostics belong to the caller.
    pub fn sqlstate(self) -> Option<&'static str> {
        match self {
            Self::UnsupportedElement => Some("0A000"),
            Self::TooManyDimensions | Self::SizeOverflow | Self::BoundsOverflow => Some("54000"),
            Self::DimensionMismatch | Self::ElementCountMismatch => None,
        }
    }
}

/// Scalar values only: dimensions belong to PgArray, never nested arrays.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PgArrayElement {
    Int4(i32),
}

/// Immutable after validation. The last axis varies fastest in `elements`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgArray {
    element_oid: u32,
    element_typmod: i32,
    shape: Vec<i32>,
    lower_bounds: Vec<i32>,
    elements: Vec<Option<PgArrayElement>>,
}
impl PgArray {
    pub const MAX_DIMENSIONS: usize = 6;
    pub fn new(
        element_oid: u32,
        element_typmod: i32,
        shape: Vec<i32>,
        lower_bounds: Vec<i32>,
        elements: Vec<Option<PgArrayElement>>,
    ) -> Result<Self, PgArrayError> {
        if element_oid != 23 {
            return Err(PgArrayError::UnsupportedElement);
        }
        if shape.len() > Self::MAX_DIMENSIONS {
            return Err(PgArrayError::TooManyDimensions);
        }
        if shape.len() != lower_bounds.len() {
            return Err(PgArrayError::DimensionMismatch);
        }
        // Match ArrayGetNItems' signed int32 product, including validation of
        // later axes even when an earlier axis has length zero.
        let mut count: i32 = if shape.is_empty() { 0 } else { 1 };
        for &length in &shape {
            if length < 0 {
                return Err(PgArrayError::SizeOverflow);
            }
            count = count
                .checked_mul(length)
                .ok_or(PgArrayError::SizeOverflow)?;
        }
        // PostgreSQL MaxArraySize = MaxAllocSize / sizeof(Datum). Also check
        // the actual Rust carrier layout before any future codec allocation.
        let count = count as usize;
        if count > 0x3fffffff / std::mem::size_of::<usize>() {
            return Err(PgArrayError::SizeOverflow);
        }
        std::alloc::Layout::array::<Option<PgArrayElement>>(count)
            .map_err(|_| PgArrayError::SizeOverflow)?;
        for (&length, &lower) in shape.iter().zip(&lower_bounds) {
            // Check the exclusive endpoint, not just the last valid subscript.
            lower
                .checked_add(length)
                .ok_or(PgArrayError::BoundsOverflow)?;
        }
        if count != elements.len() {
            return Err(PgArrayError::ElementCountMismatch);
        }
        let (shape, lower_bounds) = if count == 0 {
            (Vec::new(), Vec::new())
        } else {
            (shape, lower_bounds)
        };
        Ok(Self {
            element_oid,
            element_typmod,
            shape,
            lower_bounds,
            elements,
        })
    }
    pub fn element_oid(&self) -> u32 {
        self.element_oid
    }
    pub fn element_typmod(&self) -> i32 {
        self.element_typmod
    }
    pub fn shape(&self) -> &[i32] {
        &self.shape
    }
    pub fn lower_bounds(&self) -> &[i32] {
        &self.lower_bounds
    }
    pub fn elements(&self) -> &[Option<PgArrayElement>] {
        &self.elements
    }
    pub fn has_nulls(&self) -> bool {
        self.elements.iter().any(Option::is_none)
    }
}
