root='/Users/veszelovszki/projects-git/vdavid/cmdr/.claude/worktrees/pii-redaction/apps/desktop/'
p=root+'src-tauri/src/redact/detail.rs'; s=open(p).read()
def rep(o,n):
    global s
    assert s.count(o)==1,o[:60]; s=s.replace(o,n)
rep('''    let mut redacted = String::with_capacity(text.len());
    for line in text.split_inclusive('\\n') {''','''    let text = redact_json_identity_pairs(&text, context);
    let mut redacted = String::with_capacity(text.len());
    for line in text.split_inclusive('\\n') {''')
rep('''/// Tokenize every absolute path the line scanner left alone''','''/// Tokenize the values of identity-keyed JSON pairs (`"server":"NASPOLYA"`): the frontend logs
/// a typed error as `JSON.stringify(error)`, whose keys say what each value is, in whatever
/// spelling the error carried. Runs before the ordinary scan, which then leaves the tokens be.
fn redact_json_identity_pairs(text: &str, context: &RedactionContext) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r#""(?P<key>server|host|hostname|share|volumeName|volumeId|serverId|deviceId|user|username|path|name)"\\s*:\\s*"(?P<value>(?:[^"\\\\]|\\\\.)*)""#,
        )
        .expect("valid JSON identity-pair regex")
    });
    re.replace_all(text, |caps: &Captures<'_>| {
        let key = caps.name("key").map_or("", |m| m.as_str());
        let value = caps.name("value").map_or("", |m| m.as_str());
        let token = match key {
            "server" | "host" | "hostname" => identity_field_token("host", value, Some(context)),
            "share" => identity_field_token("share", value, Some(context)),
            "user" | "username" => identity_token("user", TokenDomain::Userinfo, value, Some(context)),
            "path" => redact_typed_path(value, Some(context), true),
            "name" => redact_leaf(value, has_extension_like_suffix(value), Some(context)),
            _ => identity_field_token(key, value, Some(context)),
        };
        format!(r#""{key}":"{token}""#)
    })
    .into_owned()
}

/// Tokenize every absolute path the line scanner left alone''')
open(p,'w').write(s)
p=root+'src/lib/logging/log-bridge.ts'; s=open(p).read()
o='''  // External text (an error message, a server's answer): redacted and capped in a report.
  detail: 'detail','''
assert o in s
s=s.replace(o,'''  // External text (an error message, a server's answer, a stringified typed error): redacted
  // and capped in a report, with identity-keyed JSON pairs tokenized.
  detail: 'detail',
  error: 'detail',
  err: 'detail',
  result: 'detail',''')
open(p,'w').write(s)
