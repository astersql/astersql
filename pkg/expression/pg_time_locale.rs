// Copyright 2026 AsterSQL.
//! Locale names owned by one operation; never changes process setlocale.
use std::ffi::{CStr, CString};
use types_dependency::pg_temporal::PgTemporalError;
// Darwin exports this POSIX function in xlocale.h, but libc does not declare it.
unsafe extern "C" {
    fn nl_langinfo_l(item: libc::nl_item, locale: libc::locale_t) -> *mut libc::c_char;
}
struct Locale(libc::locale_t);
impl Drop for Locale {
    fn drop(&mut self) {
        // SAFETY: newlocale returned this unique handle; no borrowed name survives it.
        unsafe {
            libc::freelocale(self.0);
        }
    }
}
impl Locale {
    fn new(name: &str) -> Result<Self, PgTemporalError> {
        if name.is_empty() {
            return Err(PgTemporalError::Unsupported);
        }
        let name = CString::new(name).map_err(|_| PgTemporalError::Unsupported)?;
        // SAFETY: valid terminated name, null base requests a fresh independent locale.
        let value = unsafe {
            libc::newlocale(
                libc::LC_TIME_MASK | libc::LC_CTYPE_MASK,
                name.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        if value.is_null() {
            Err(PgTemporalError::Unsupported)
        } else {
            Ok(Self(value))
        }
    }
    fn item(&self, item: libc::nl_item) -> &[u8] {
        // SAFETY: the handle is live and item is a platform langinfo constant.
        unsafe { CStr::from_ptr(nl_langinfo_l(item, self.0)).to_bytes() }
    }
}
pub fn validate(name: &str) -> bool {
    Locale::new(name).is_ok()
}
pub(crate) fn name(
    locale: &str,
    month: bool,
    short: bool,
    index: usize,
) -> Result<String, PgTemporalError> {
    if index >= if month { 12 } else { 7 } {
        return Err(PgTemporalError::Range);
    }
    let locale = Locale::new(locale)?;
    let base = match (month, short) {
        (true, false) => libc::MON_1,
        (true, true) => libc::ABMON_1,
        (false, false) => libc::DAY_1,
        (false, true) => libc::ABDAY_1,
    };
    let bytes = locale.item(base + index as libc::nl_item);
    if let Ok(value) = std::str::from_utf8(bytes) {
        return Ok(value.into());
    }
    let encoding = encoding_rs::Encoding::for_label(locale.item(libc::CODESET))
        .ok_or(PgTemporalError::Unsupported)?;
    let (value, _, errors) = encoding.decode(bytes);
    if errors {
        Err(PgTemporalError::Unsupported)
    } else {
        Ok(value.into_owned())
    }
}
