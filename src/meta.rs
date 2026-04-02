//! Meta protocol parser and types for memcached meta commands.
//!
//! Meta commands use two-letter prefixes (mg, ms, md, ma, mn, me) and
//! a flag-based system.  This module contains the pure parser and types;
//! runtime handlers live in `ascii.rs` alongside the classic ASCII handlers.

use super::ascii::validate_key;

// ── Meta command types ──────────────────────────────────────────────

/// A parsed meta protocol flag with optional token argument.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct MetaFlag {
    pub ch: u8,
    pub token: Option<String>,
}

/// Parsed meta command.
#[derive(Debug, PartialEq)]
pub(crate) enum MetaCmd<'a> {
    /// mg <key> [flags]*
    Get { key: &'a [u8], flags: Vec<MetaFlag> },
    /// ms <key> <datalen> [flags]*
    Set { key: &'a [u8], datalen: u32, flags: Vec<MetaFlag> },
    /// md <key> [flags]*
    Delete { key: &'a [u8], flags: Vec<MetaFlag> },
    /// ma <key> [flags]*
    Arithmetic { key: &'a [u8], flags: Vec<MetaFlag> },
    /// mn [flags]*
    Noop { flags: Vec<MetaFlag> },
    /// me <key> [flags]*
    Debug { key: &'a [u8], flags: Vec<MetaFlag> },
}

#[derive(Debug)]
pub(crate) enum MetaParseResult<'a> {
    Ok(MetaCmd<'a>),
    ClientError(#[allow(dead_code)] String),
    /// ms needs a data block of this size
    NeedData(MetaCmd<'a>, u32),
}

// ── Flag parsing ────────────────────────────────────────────────────

/// Parse meta flags from a slice of whitespace-separated tokens.
/// Each flag is a single character optionally followed by a token argument
/// (attached directly, no space).  E.g., "v", "T300", "Oopaque123".
pub(crate) fn parse_meta_flags(tokens: &[&str]) -> Result<Vec<MetaFlag>, String> {
    let mut flags = Vec::new();
    for &tok in tokens {
        if tok.is_empty() { continue; }
        let bytes = tok.as_bytes();
        let ch = bytes[0];
        let token = if bytes.len() > 1 {
            Some(std::str::from_utf8(&bytes[1..])
                .map_err(|_| "bad flag token encoding".to_string())?
                .to_string())
        } else {
            None
        };
        flags.push(MetaFlag { ch, token });
    }
    Ok(flags)
}

/// Check if a flag character is in the set.
pub(crate) fn has_flag(flags: &[MetaFlag], ch: u8) -> bool {
    flags.iter().any(|f| f.ch == ch)
}

/// Get the token for a flag, if present.
pub(crate) fn get_flag_token(flags: &[MetaFlag], ch: u8) -> Option<&str> {
    flags.iter().find(|f| f.ch == ch).and_then(|f| f.token.as_deref())
}

// ── Supported flag validation ───────────────────────────────────────

/// Flags that are silently ignored on all commands (proxy hints).
const IGNORED_FLAGS: &[u8] = b"PL";

/// Validate flags for mg. Returns error message if unsupported flag found.
pub(crate) fn validate_mg_flags(flags: &[MetaFlag]) -> Result<(), String> {
    const SUPPORTED: &[u8] = b"vcfksOqtT";
    for f in flags {
        if SUPPORTED.contains(&f.ch) || IGNORED_FLAGS.contains(&f.ch) { continue; }
        return Err(format!("unsupported meta flag '{}'", f.ch as char));
    }
    validate_numeric_tokens(flags, b"T")?;
    Ok(())
}

/// Validate flags for ms. Also validates M mode token and numeric tokens.
pub(crate) fn validate_ms_flags(flags: &[MetaFlag]) -> Result<(), String> {
    const SUPPORTED: &[u8] = b"FTCqOkM";
    for f in flags {
        if SUPPORTED.contains(&f.ch) || IGNORED_FLAGS.contains(&f.ch) { continue; }
        return Err(format!("unsupported meta flag '{}'", f.ch as char));
    }
    // M requires a token; bare M is rejected.
    validate_mode_token_present(flags)?;
    // Validate M mode token: only S, E, A, P, R allowed.
    if let Some(mode) = get_flag_token(flags, b'M') {
        match mode {
            "S" | "E" | "A" | "P" | "R" => {}
            _ => return Err(format!("unsupported ms mode '{mode}'")),
        }
        // Append/Prepend don't support F (client flags) or T (TTL).
        if (mode == "A" || mode == "P") && (has_flag(flags, b'F') || has_flag(flags, b'T')) {
            return Err(format!("flags F/T not supported with ms mode '{mode}'"));
        }
    }
    validate_numeric_tokens(flags, b"FTC")?;
    Ok(())
}

/// Validate flags for md.
pub(crate) fn validate_md_flags(flags: &[MetaFlag]) -> Result<(), String> {
    const SUPPORTED: &[u8] = b"CqOk";
    for f in flags {
        if SUPPORTED.contains(&f.ch) || IGNORED_FLAGS.contains(&f.ch) { continue; }
        return Err(format!("unsupported meta flag '{}'", f.ch as char));
    }
    validate_numeric_tokens(flags, b"C")?;
    Ok(())
}

/// Validate flags for ma.
pub(crate) fn validate_ma_flags(flags: &[MetaFlag]) -> Result<(), String> {
    const SUPPORTED: &[u8] = b"DJNqOkvcM";
    for f in flags {
        if SUPPORTED.contains(&f.ch) || IGNORED_FLAGS.contains(&f.ch) { continue; }
        return Err(format!("unsupported meta flag '{}'", f.ch as char));
    }
    // M requires a token; bare M is rejected.
    validate_mode_token_present(flags)?;
    // Validate M mode token: only I, D allowed.
    if let Some(mode) = get_flag_token(flags, b'M') {
        match mode {
            "I" | "D" => {}
            _ => return Err(format!("unsupported ma mode '{mode}'")),
        }
    }
    validate_numeric_tokens(flags, b"DJN")?;
    Ok(())
}

/// Validate flags for mn. Only O (opaque) is supported.
pub(crate) fn validate_mn_flags(flags: &[MetaFlag]) -> Result<(), String> {
    const SUPPORTED: &[u8] = b"O";
    for f in flags {
        if SUPPORTED.contains(&f.ch) || IGNORED_FLAGS.contains(&f.ch) { continue; }
        return Err(format!("unsupported meta flag '{}'", f.ch as char));
    }
    Ok(())
}

/// Validate flags for me. Only O, k, q are supported.
pub(crate) fn validate_me_flags(flags: &[MetaFlag]) -> Result<(), String> {
    const SUPPORTED: &[u8] = b"Okq";
    for f in flags {
        if SUPPORTED.contains(&f.ch) || IGNORED_FLAGS.contains(&f.ch) { continue; }
        return Err(format!("unsupported meta flag '{}'", f.ch as char));
    }
    Ok(())
}

/// Validate that flag tokens expected to be numeric have a valid unsigned integer token.
/// Bare flags (no token) are rejected — numeric flags require an explicit value.
fn validate_numeric_tokens(flags: &[MetaFlag], numeric_flags: &[u8]) -> Result<(), String> {
    for f in flags {
        if numeric_flags.contains(&f.ch) {
            match &f.token {
                Some(tok) => {
                    if tok.parse::<u64>().is_err() {
                        return Err(format!(
                            "bad numeric value '{}' for flag '{}'",
                            tok, f.ch as char
                        ));
                    }
                }
                None => {
                    return Err(format!(
                        "flag '{}' requires a numeric token",
                        f.ch as char
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Validate that an M (mode) flag has an explicit token. Bare M is rejected.
fn validate_mode_token_present(flags: &[MetaFlag]) -> Result<(), String> {
    for f in flags {
        if f.ch == b'M' && f.token.is_none() {
            return Err("flag 'M' requires a mode token".to_string());
        }
    }
    Ok(())
}



// ── Command parsing ─────────────────────────────────────────────────

/// Parse a meta command line (already stripped of \r\n).
pub(crate) fn parse_meta_command(line: &[u8]) -> MetaParseResult<'_> {
    let line_str = match std::str::from_utf8(line) {
        Ok(s) => s,
        Err(_) => return MetaParseResult::ClientError("bad command line format".into()),
    };
    let tokens: Vec<&str> = line_str.split_whitespace().collect();
    if tokens.is_empty() {
        return MetaParseResult::ClientError("bad command line format".into());
    }

    match tokens[0] {
        "mn" => {
            let flags = match parse_meta_flags(&tokens[1..]) {
                Ok(f) => f,
                Err(e) => return MetaParseResult::ClientError(e),
            };
            MetaParseResult::Ok(MetaCmd::Noop { flags })
        }
        "mg" | "me" | "md" | "ma" => {
            if tokens.len() < 2 {
                return MetaParseResult::ClientError("bad command line format".into());
            }
            let key = tokens[1].as_bytes();
            if let Err(e) = validate_key(key) {
                return MetaParseResult::ClientError(e);
            }
            let flags = match parse_meta_flags(&tokens[2..]) {
                Ok(f) => f,
                Err(e) => return MetaParseResult::ClientError(e),
            };
            match tokens[0] {
                "mg" => MetaParseResult::Ok(MetaCmd::Get { key, flags }),
                "me" => MetaParseResult::Ok(MetaCmd::Debug { key, flags }),
                "md" => MetaParseResult::Ok(MetaCmd::Delete { key, flags }),
                "ma" => MetaParseResult::Ok(MetaCmd::Arithmetic { key, flags }),
                _ => unreachable!(),
            }
        }
        "ms" => parse_meta_set(&tokens),
        _ => MetaParseResult::ClientError("bad command line format".into()),
    }
}

/// Parse `ms <key> <datalen> [flags]*`.
fn parse_meta_set<'a>(tokens: &[&'a str]) -> MetaParseResult<'a> {
    if tokens.len() < 3 {
        return MetaParseResult::ClientError("bad command line format".into());
    }
    let key = tokens[1].as_bytes();
    if let Err(e) = validate_key(key) {
        return MetaParseResult::ClientError(e);
    }
    let datalen = match tokens[2].parse::<u32>() {
        Ok(n) => n,
        Err(_) => return MetaParseResult::ClientError("bad command line format".into()),
    };
    let flags = match parse_meta_flags(&tokens[3..]) {
        Ok(f) => f,
        Err(e) => return MetaParseResult::ClientError(e),
    };
    MetaParseResult::NeedData(MetaCmd::Set { key, datalen, flags }, datalen)
}

// ── Response helpers ────────────────────────────────────────────────

/// Write the common flag echo suffix for a meta response.
/// This writes the O (opaque) and k (key) flag echoes.
pub(crate) fn write_meta_flag_echo(out: &mut Vec<u8>, flags: &[MetaFlag], key: &[u8]) {
    if let Some(opaque) = get_flag_token(flags, b'O') {
        out.push(b' ');
        out.push(b'O');
        out.extend_from_slice(opaque.as_bytes());
    }
    if has_flag(flags, b'k') {
        out.push(b' ');
        out.push(b'k');
        out.extend_from_slice(key);
    }
}

// ── Unit tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mn() {
        match parse_meta_command(b"mn") {
            MetaParseResult::Ok(MetaCmd::Noop { flags }) => assert!(flags.is_empty()),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_mn_with_opaque() {
        match parse_meta_command(b"mn Otoken123") {
            MetaParseResult::Ok(MetaCmd::Noop { flags }) => {
                assert_eq!(flags.len(), 1);
                assert_eq!(flags[0].ch, b'O');
                assert_eq!(flags[0].token.as_deref(), Some("token123"));
            }
            other => panic!("expected Noop with O flag, got {other:?}"),
        }
    }

    #[test]
    fn parse_mg_basic() {
        match parse_meta_command(b"mg mykey v f c") {
            MetaParseResult::Ok(MetaCmd::Get { key, flags }) => {
                assert_eq!(key, b"mykey");
                assert_eq!(flags.len(), 3);
                assert!(has_flag(&flags, b'v'));
                assert!(has_flag(&flags, b'f'));
                assert!(has_flag(&flags, b'c'));
            }
            other => panic!("expected Get, got {other:?}"),
        }
    }

    #[test]
    fn parse_mg_with_ttl_update() {
        match parse_meta_command(b"mg mykey v T300") {
            MetaParseResult::Ok(MetaCmd::Get { key, flags }) => {
                assert_eq!(key, b"mykey");
                assert!(has_flag(&flags, b'v'));
                assert_eq!(get_flag_token(&flags, b'T'), Some("300"));
            }
            other => panic!("expected Get, got {other:?}"),
        }
    }

    #[test]
    fn parse_ms_basic() {
        match parse_meta_command(b"ms mykey 5 F123 T300") {
            MetaParseResult::NeedData(MetaCmd::Set { key, datalen, flags }, 5) => {
                assert_eq!(key, b"mykey");
                assert_eq!(datalen, 5);
                assert_eq!(get_flag_token(&flags, b'F'), Some("123"));
                assert_eq!(get_flag_token(&flags, b'T'), Some("300"));
            }
            other => panic!("expected Set NeedData, got {other:?}"),
        }
    }

    #[test]
    fn parse_md_basic() {
        match parse_meta_command(b"md mykey") {
            MetaParseResult::Ok(MetaCmd::Delete { key, flags }) => {
                assert_eq!(key, b"mykey");
                assert!(flags.is_empty());
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn parse_ma_basic() {
        match parse_meta_command(b"ma counter D10 MI") {
            MetaParseResult::Ok(MetaCmd::Arithmetic { key, flags }) => {
                assert_eq!(key, b"counter");
                assert_eq!(get_flag_token(&flags, b'D'), Some("10"));
                assert_eq!(get_flag_token(&flags, b'M'), Some("I"));
            }
            other => panic!("expected Arithmetic, got {other:?}"),
        }
    }

    #[test]
    fn parse_me_basic() {
        match parse_meta_command(b"me mykey") {
            MetaParseResult::Ok(MetaCmd::Debug { key, .. }) => {
                assert_eq!(key, b"mykey");
            }
            other => panic!("expected Debug, got {other:?}"),
        }
    }

    #[test]
    fn flag_validation_mg() {
        let flags = parse_meta_flags(&["v", "f", "c", "k", "s", "T300", "t", "q", "Oabc"]).unwrap();
        assert!(validate_mg_flags(&flags).is_ok());

        let bad = parse_meta_flags(&["v", "N30"]).unwrap();
        assert!(validate_mg_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_ms() {
        let flags = parse_meta_flags(&["F123", "T300", "C5", "q", "k", "MS"]).unwrap();
        assert!(validate_ms_flags(&flags).is_ok());

        let bad = parse_meta_flags(&["v"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_ms_rejects_unknown_mode() {
        let bad = parse_meta_flags(&["MX"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_ms_rejects_bad_numeric() {
        let bad = parse_meta_flags(&["Tabc"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_md() {
        let flags = parse_meta_flags(&["C5", "q", "k"]).unwrap();
        assert!(validate_md_flags(&flags).is_ok());

        let bad = parse_meta_flags(&["I"]).unwrap();
        assert!(validate_md_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_md_rejects_bad_cas() {
        let bad = parse_meta_flags(&["Cabc"]).unwrap();
        assert!(validate_md_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_ma() {
        let flags = parse_meta_flags(&["D10", "J0", "N300", "q", "v", "c", "MI"]).unwrap();
        assert!(validate_ma_flags(&flags).is_ok());
    }

    #[test]
    fn flag_validation_ma_rejects_unknown_mode() {
        let bad = parse_meta_flags(&["MX"]).unwrap();
        assert!(validate_ma_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_ma_rejects_t_flag() {
        // t flag removed from ma — not accurately implementable.
        let bad = parse_meta_flags(&["t"]).unwrap();
        assert!(validate_ma_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_ma_rejects_bad_delta() {
        let bad = parse_meta_flags(&["Dabc"]).unwrap();
        assert!(validate_ma_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_mn() {
        let flags = parse_meta_flags(&["Oabc"]).unwrap();
        assert!(validate_mn_flags(&flags).is_ok());

        let bad = parse_meta_flags(&["v"]).unwrap();
        assert!(validate_mn_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_me() {
        let flags = parse_meta_flags(&["Oabc", "k", "q"]).unwrap();
        assert!(validate_me_flags(&flags).is_ok());

        let bad = parse_meta_flags(&["v"]).unwrap();
        assert!(validate_me_flags(&bad).is_err());
    }

    #[test]
    fn flag_validation_mg_rejects_bad_ttl() {
        let bad = parse_meta_flags(&["Tabc"]).unwrap();
        assert!(validate_mg_flags(&bad).is_err());
    }

    #[test]
    fn reject_bare_numeric_flags() {
        // Bare T (no token) must be rejected.
        let bad = parse_meta_flags(&["T"]).unwrap();
        assert!(validate_mg_flags(&bad).is_err());

        // Bare F on ms must be rejected.
        let bad = parse_meta_flags(&["F", "MS"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());

        // Bare C on md must be rejected.
        let bad = parse_meta_flags(&["C"]).unwrap();
        assert!(validate_md_flags(&bad).is_err());

        // Bare D on ma must be rejected.
        let bad = parse_meta_flags(&["D", "MI"]).unwrap();
        assert!(validate_ma_flags(&bad).is_err());
    }

    #[test]
    fn reject_bare_m_flag() {
        // Bare M (no mode token) on ms must be rejected.
        let bad = parse_meta_flags(&["M"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());

        // Bare M on ma must be rejected.
        let bad = parse_meta_flags(&["M"]).unwrap();
        assert!(validate_ma_flags(&bad).is_err());
    }

    #[test]
    fn reject_ft_on_ms_append_prepend() {
        // F on ms M=A must be rejected.
        let bad = parse_meta_flags(&["MA", "F123"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());

        // T on ms M=P must be rejected.
        let bad = parse_meta_flags(&["MP", "T300"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());

        // Both F and T on ms M=A must be rejected.
        let bad = parse_meta_flags(&["MA", "F0", "T0"]).unwrap();
        assert!(validate_ms_flags(&bad).is_err());

        // But F and T on ms M=S is fine.
        let ok = parse_meta_flags(&["MS", "F0", "T0"]).unwrap();
        assert!(validate_ms_flags(&ok).is_ok());
    }

    #[test]
    fn ignored_proxy_flags() {
        let flags = parse_meta_flags(&["v", "P", "L"]).unwrap();
        assert!(validate_mg_flags(&flags).is_ok());
    }

    #[test]
    fn missing_key_error() {
        match parse_meta_command(b"mg") {
            MetaParseResult::ClientError(_) => {}
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn write_flag_echo_opaque_and_key() {
        let flags = parse_meta_flags(&["v", "Otoken42", "k"]).unwrap();
        let mut out = Vec::new();
        write_meta_flag_echo(&mut out, &flags, b"mykey");
        assert_eq!(&out, b" Otoken42 kmykey");
    }

    #[test]
    fn write_flag_echo_empty() {
        let flags = parse_meta_flags(&["v"]).unwrap();
        let mut out = Vec::new();
        write_meta_flag_echo(&mut out, &flags, b"mykey");
        assert!(out.is_empty());
    }
}
