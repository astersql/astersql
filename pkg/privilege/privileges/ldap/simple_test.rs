// Copyright 2026 AsterSQL.

#[cfg(test)]
mod tests {
    use crate::simple::LdapSimpleAuthImpl;

    #[test]
    fn simple_password_preserves_non_utf8_bytes_like_go() {
        let password = [0xff, 0x80, b'a', 0];
        assert_eq!(
            LdapSimpleAuthImpl::password_bytes(&password).unwrap(),
            &[0xff, 0x80, b'a']
        );
    }
}
