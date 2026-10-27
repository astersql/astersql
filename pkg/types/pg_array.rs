// Copyright 2026 AsterSQL.

//! Canonical rectangular PostgreSQL array values; SQL NULL is an outer Option.

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgArrayError {
    #[error("array element type is not supported")]
    UnsupportedElement,
    #[error("array element does not match its declared type")]
    ElementTypeMismatch,
    #[error("array element contains NUL")]
    InvalidEncoding,
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
            Self::InvalidEncoding => Some("22021"),
            Self::TooManyDimensions | Self::SizeOverflow | Self::BoundsOverflow => Some("54000"),
            Self::DimensionMismatch | Self::ElementCountMismatch | Self::ElementTypeMismatch => {
                None
            }
        }
    }
}

/// Container failures are distinct from delegated scalar input errors.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgArrayTextError<E> {
    #[error("malformed array literal")]
    Syntax,
    #[error("array upper bound is less than its lower bound")]
    Bounds,
    #[error("array input is not valid UTF-8 text")]
    Encoding,
    #[error("{0}")]
    Array(PgArrayError),
    #[error("array element input failed")]
    Element(E),
}
impl<E> PgArrayTextError<E> {
    /// Element diagnostics retain the caller's scalar error and SQLSTATE.
    pub fn sqlstate(&self) -> Option<&'static str> {
        match self {
            Self::Syntax => Some("22P02"),
            Self::Bounds => Some("2202E"),
            Self::Encoding => Some("22021"),
            Self::Array(e) => e.sqlstate(),
            Self::Element(_) => None,
        }
    }
}

/// Scalar values only: dimensions belong to PgArray, never nested arrays.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PgArrayElement {
    Int4(i32),
    Text(String),
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
        if !matches!(element_oid, 23 | 25) {
            return Err(PgArrayError::UnsupportedElement);
        }
        Self::validate_layout(&shape, &lower_bounds, elements.len())?;
        for element in elements.iter().flatten() {
            match (element_oid, element) {
                (23, PgArrayElement::Int4(_)) => {}
                (25, PgArrayElement::Text(s)) => {
                    if s.contains('\0') {
                        return Err(PgArrayError::InvalidEncoding);
                    }
                }
                _ => return Err(PgArrayError::ElementTypeMismatch),
            }
        }
        let (shape, lower_bounds) = if elements.is_empty() {
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
    fn validate_layout(
        shape: &[i32],
        lower_bounds: &[i32],
        element_count: usize,
    ) -> Result<(), PgArrayError> {
        if shape.len() > Self::MAX_DIMENSIONS {
            return Err(PgArrayError::TooManyDimensions);
        }
        if shape.len() != lower_bounds.len() {
            return Err(PgArrayError::DimensionMismatch);
        }
        // Match ArrayGetNItems' signed int32 product, including validation of
        // later axes even when an earlier axis has length zero.
        let mut count: i32 = if shape.is_empty() { 0 } else { 1 };
        for &length in shape {
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
        for (&length, &lower) in shape.iter().zip(lower_bounds) {
            // Check the exclusive endpoint, not just the last valid subscript.
            lower
                .checked_add(length)
                .ok_or(PgArrayError::BoundsOverflow)?;
        }
        if count != element_count {
            return Err(PgArrayError::ElementCountMismatch);
        }
        Ok(())
    }

    /// Parse a UTF-8 Bind payload with array_nulls=on, without SQL unescaping.
    /// The callback receives unescaped, non-NULL elements and the element typmod.
    /// All syntax, shape and bound checks finish before any scalar input call.
    /// Scalar SQLSTATEs stay in Element(E), unchanged; callers supply their codec.
    pub fn from_text_with<E>(
        bytes: &[u8],
        element_oid: u32,
        element_typmod: i32,
        delimiter: u8,
        mut input: impl FnMut(&str, i32) -> Result<PgArrayElement, E>,
    ) -> Result<Self, PgArrayTextError<E>> {
        use PgArrayTextError as Error;
        std::str::from_utf8(bytes).map_err(|_| Error::Encoding)?;
        if bytes.contains(&0) {
            return Err(Error::Encoding);
        }
        if !matches!(element_oid, 23 | 25) {
            return Err(Error::Array(PgArrayError::UnsupportedElement));
        }
        // A catalog delimiter is one ASCII punctuation character. Other
        // delimiter conventions are deliberately outside this UTF-8 codec.
        if !array_delimiter(delimiter) {
            return Err(Error::Array(PgArrayError::UnsupportedElement));
        }
        let mut parser = TextParser {
            bytes,
            pos: 0,
            delimiter,
            elements: Vec::new(),
        };
        let (declared_shape, lower) = parser.dimensions()?;
        let shape = parser.array(0)?;
        parser.space();
        if parser.pos != bytes.len() || (!declared_shape.is_empty() && declared_shape != shape) {
            return Err(Error::Syntax);
        }
        let lower = if lower.is_empty() {
            vec![1; shape.len()]
        } else {
            lower
        };
        Self::validate_layout(&shape, &lower, parser.elements.len()).map_err(Error::Array)?;
        let elements = parser
            .elements
            .into_iter()
            .map(|value| {
                value
                    .map(|s| input(&s, element_typmod))
                    .transpose()
                    .map_err(Error::Element)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(element_oid, element_typmod, shape, lower, elements).map_err(Error::Array)
    }

    /// Format the canonical value using the element family's text output.
    /// Emit all dimensions when any lower bound differs from one. SQL NULL
    /// remains outside this value; only NULL elements are written here.
    pub fn to_text(&self, delimiter: u8) -> Result<String, PgArrayError> {
        use std::fmt::Write;

        if !array_delimiter(delimiter) {
            return Err(PgArrayError::UnsupportedElement);
        }
        if self.elements.is_empty() {
            return Ok("{}".into());
        }
        let mut output = String::new();
        if self.lower_bounds.iter().any(|&lower| lower != 1) {
            for (&lower, &length) in self.lower_bounds.iter().zip(&self.shape) {
                // Construction checked the exclusive endpoint for overflow.
                write!(output, "[{lower}:{}]", lower + length - 1).unwrap();
            }
            output.push('=');
        }
        self.write_text_axis(0, &mut self.elements.iter(), delimiter, &mut output);
        Ok(output)
    }

    fn write_text_axis(
        &self,
        axis: usize,
        elements: &mut std::slice::Iter<'_, Option<PgArrayElement>>,
        delimiter: u8,
        output: &mut String,
    ) {
        output.push('{');
        for index in 0..self.shape[axis] {
            if index != 0 {
                output.push(char::from(delimiter));
            }
            if axis + 1 < self.shape.len() {
                self.write_text_axis(axis + 1, elements, delimiter, output);
                continue;
            }
            // Private immutable fields guarantee the shape consumes exactly
            // the validated row-major elements, and recursion is at most six.
            match elements.next().unwrap() {
                None => output.push_str("NULL"),
                Some(PgArrayElement::Int4(value)) => {
                    write_array_element_text(&value.to_string(), delimiter, output);
                }
                Some(PgArrayElement::Text(value)) => {
                    write_array_element_text(value, delimiter, output)
                }
            }
        }
        output.push('}');
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

// Token storage is local to one parse; no partially converted array escapes.
struct TextParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    delimiter: u8,
    elements: Vec<Option<String>>,
}
impl TextParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn space(&mut self) {
        while self.peek().is_some_and(array_space) {
            self.pos += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn bound<E>(&mut self) -> Result<i32, PgArrayTextError<E>> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        let digits = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == digits {
            return Err(PgArrayTextError::Syntax);
        }
        // UTF-8 was checked at entry; this substring consists only of ASCII.
        std::str::from_utf8(&self.bytes[start..self.pos])
            .unwrap()
            .parse()
            .map_err(|_| PgArrayTextError::Array(PgArrayError::BoundsOverflow))
    }
    fn dimensions<E>(&mut self) -> Result<(Vec<i32>, Vec<i32>), PgArrayTextError<E>> {
        use PgArrayTextError as Error;
        let mut shape = Vec::new();
        let mut lower = Vec::new();
        self.space();
        while self.take(b'[') {
            if shape.len() == PgArray::MAX_DIMENSIONS {
                return Err(Error::Array(PgArrayError::TooManyDimensions));
            }
            let first = self.bound()?;
            let (lo, hi) = if self.take(b':') {
                (first, self.bound()?)
            } else {
                (1, first)
            };
            if !self.take(b']') {
                return Err(Error::Syntax);
            }
            if hi < lo {
                return Err(Error::Bounds);
            }
            if hi == i32::MAX {
                return Err(Error::Array(PgArrayError::BoundsOverflow));
            }
            let length = hi
                .checked_sub(lo)
                .and_then(|n| n.checked_add(1))
                .ok_or(Error::Array(PgArrayError::SizeOverflow))?;
            shape.push(length);
            lower.push(lo);
            self.space();
        }
        if !shape.is_empty() && !self.take(b'=') {
            return Err(Error::Syntax);
        }
        self.space();
        Ok((shape, lower))
    }
    fn array<E>(&mut self, depth: usize) -> Result<Vec<i32>, PgArrayTextError<E>> {
        use PgArrayTextError as Error;
        if depth == PgArray::MAX_DIMENSIONS {
            return Err(Error::Array(PgArrayError::TooManyDimensions));
        }
        if !self.take(b'{') {
            return Err(Error::Syntax);
        }
        self.space();
        if self.take(b'}') {
            return Ok(vec![0]);
        }
        let nested = self.peek() == Some(b'{');
        let mut child_shape = None;
        let mut count = 0i32;
        loop {
            self.space();
            if nested {
                let shape = self.array(depth + 1)?;
                if child_shape.as_ref().is_some_and(|s| s != &shape) {
                    return Err(Error::Syntax);
                }
                child_shape = Some(shape);
            } else {
                let value = self.element()?;
                self.elements.push(value);
            }
            count = count
                .checked_add(1)
                .ok_or(Error::Array(PgArrayError::SizeOverflow))?;
            self.space();
            if self.take(b'}') {
                break;
            }
            if !self.take(self.delimiter) {
                return Err(Error::Syntax);
            }
        }
        let mut shape = vec![count];
        shape.extend(child_shape.unwrap_or_default());
        Ok(shape)
    }
    fn element<E>(&mut self) -> Result<Option<String>, PgArrayTextError<E>> {
        use PgArrayTextError as Error;
        let quoted = self.take(b'"');
        let mut escaped = false;
        let mut output = Vec::new();
        let mut significant = 0;
        loop {
            let b = self.peek().ok_or(Error::Syntax)?;
            if b == b'\\' {
                self.pos += 1;
                output.push(self.peek().ok_or(Error::Syntax)?);
                self.pos += 1;
                significant = output.len();
                escaped = true;
            } else if quoted {
                self.pos += 1;
                if b == b'"' {
                    self.space();
                    if !self
                        .peek()
                        .is_some_and(|b| b == self.delimiter || b == b'}')
                    {
                        return Err(Error::Syntax);
                    }
                    break;
                }
                output.push(b);
            } else if b == self.delimiter || b == b'}' {
                if output.is_empty() {
                    return Err(Error::Syntax);
                }
                output.truncate(significant);
                break;
            } else {
                if matches!(b, b'{' | b'"') {
                    return Err(Error::Syntax);
                }
                output.push(b);
                self.pos += 1;
                if !array_space(b) {
                    significant = output.len();
                }
            }
        }
        if !quoted && !escaped && output.eq_ignore_ascii_case(b"NULL") {
            Ok(None)
        } else {
            // Array escaping removes ASCII backslashes, never continuation bytes.
            Ok(Some(
                String::from_utf8(output).map_err(|_| Error::Encoding)?,
            ))
        }
    }
}

fn array_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn array_delimiter(b: u8) -> bool {
    b.is_ascii_punctuation() && !matches!(b, b'{' | b'}' | b'"' | b'\\')
}

// Quote the scalar output, including integers when a supplied delimiter is '-'.
fn write_array_element_text(value: &str, delimiter: u8, output: &mut String) {
    let quote = value.is_empty()
        || value.eq_ignore_ascii_case("NULL")
        || value
            .bytes()
            .any(|b| b == delimiter || matches!(b, b'{' | b'}' | b'"' | b'\\') || array_space(b));
    if quote {
        output.push('"');
    }
    for ch in value.chars() {
        if matches!(ch, '"' | '\\') {
            output.push('\\');
        }
        output.push(ch);
    }
    if quote {
        output.push('"');
    }
}
