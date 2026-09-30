//! One entity, one token: Bonjour instance names and `.local` hostnames in the host domain,
//! and a path's leaf tokenized whole whichever way a line prints it.

use super::*;

const TEST_PROCESS_SECRET: [u8; 32] = [0x6e; 32];

fn context() -> RedactionContext {
    RedactionContext::for_test(TEST_PROCESS_SECRET, "ERR-CONSIST")
}

/// Every `<kind:hash>` token in `line`, hash only.
fn hashes(line: &str) -> Vec<String> {
    Regex::new(r"<[a-z0-9-]+:([0-9a-f]{12})>")
        .expect("valid token regex")
        .captures_iter(line)
        .map(|caps| caps[1].to_string())
        .collect()
}

#[test]
fn a_bonjour_instance_name_is_a_host_token_and_the_service_type_stays() {
    let context = context();
    let prose = context
        .redact_line("call queriers to resolve Naspolya._smb._tcp.local. now")
        .into_owned();
    let keyed = context
        .redact_line(r#"No Keychain credentials: server="Naspolya._smb._tcp.local", share="Multimedia""#)
        .into_owned();
    let bare_host = context.redact_line(r#"Resolved: server="naspolya""#).into_owned();

    for line in [&prose, &keyed, &bare_host] {
        assert!(!line.to_lowercase().contains("naspolya"), "{line}");
        assert!(!line.contains("Multimedia"), "{line}");
    }
    assert!(prose.contains("<host:") && prose.contains(">._smb._tcp.local. now"), "{prose}");
    assert!(keyed.contains(">._smb._tcp.local\""), "{keyed}");
    let token = hashes(&bare_host)[0].clone();
    assert_eq!(hashes(&prose)[0], token, "the instance name correlates with the host: {prose}");
    assert_eq!(hashes(&keyed)[0], token, "{keyed}");
}

#[test]
fn a_bare_service_type_and_multi_label_local_hosts_redact_whole() {
    let context = context();
    assert_eq!(
        context.redact_line("mDNS SearchStarted: _smb._tcp.local."),
        "mDNS SearchStarted: _smb._tcp.local."
    );
    let multi = context.redact_line("dialing nas.home-lab.local:445").into_owned();
    assert!(!multi.contains("nas") && !multi.contains("home-lab"), "{multi}");
    assert!(multi.contains(".local:445"), "{multi}");
}

#[test]
fn a_leaf_with_a_space_gets_one_token_in_every_path_shape() {
    let context = context();
    let url = "sftp://ada@127.0.0.1:12480/srv/data/Anna Kovacs/Medical records";
    let lines = [
        format!("read_directory_with_progress: listing_id=l-1, path={url}"),
        format!("ended in error: PermissionDenied {{ path: {url:?} }} (NeedsAction)"),
        format!("loadDirectory called: path={url:?}, currentLoading=false"),
    ];
    let redacted: Vec<String> = lines.iter().map(|line| context.redact_line(line).into_owned()).collect();
    for line in &redacted {
        for private in ["Anna", "Kovacs", "Medical", "records", "ada"] {
            assert!(!line.contains(private), "{private:?} survived: {line}");
        }
    }
    let leaf = |line: &str| hashes(line).last().cloned();
    assert_eq!(leaf(&redacted[0]), leaf(&redacted[1]), "{redacted:?}");
    assert_eq!(leaf(&redacted[0]), leaf(&redacted[2]), "{redacted:?}");

    // The typed SFTP path (no scheme) shares every segment token with the URL form.
    let typed = context
        .redact_line(r#"SFTP path="/srv/data/Anna Kovacs/Medical records": error_kind=permission_denied"#)
        .into_owned();
    let typed_hashes = hashes(&typed);
    let url_hashes = hashes(&redacted[2]);
    assert_eq!(
        typed_hashes,
        url_hashes[url_hashes.len() - typed_hashes.len()..].to_vec(),
        "{typed} vs {}",
        redacted[2]
    );
}
