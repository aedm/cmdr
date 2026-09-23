# Security policy

Thanks for helping keep Cmdr and its users safe. This page says how to report a security problem, what's in scope, and
what happens after you write.

Cmdr is made by Rymdskottkärra AB, a Swedish company, and maintained by one person, David Veszelovszki. The response
times below are sized for that: modest, and ones we can actually keep.

For how Cmdr handles data, signing, updates, and permissions, see the trust page at
[getcmdr.com/trust](https://getcmdr.com/trust).

## How to report a vulnerability

Email **[security@getcmdr.com](mailto:security@getcmdr.com)**. Please don't open a public GitHub issue, discussion, or
pull request for a security problem.

You can also use GitHub's private reporting: the **Report a vulnerability** button on the repository's
[Security tab](https://github.com/vdavid/cmdr/security/advisories/new).

A good report has:

- What the problem is and what an attacker could do with it.
- The Cmdr version (**Cmdr > About Cmdr**) and your macOS version, or the URL for a website or API issue.
- Steps to reproduce, or a proof of concept.
- Whether you've told anyone else, and whether you plan to publish.

Write in English, Swedish, or Hungarian. We don't publish a PGP key, by choice. If you'd rather not send the details by
email, use GitHub's private reporting above: only you and the maintainer can see the report.

## What to expect

- **Acknowledgment within five business days**, so you know a human has read it.
- **A first assessment within 14 days**: whether we can reproduce it, how serious we think it is, and a rough plan.
- **A fix for critical and high-severity issues within 30 days** where that's technically possible, and within 90 days
  for the rest. If a fix will take longer, we'll tell you why and agree on a date with you.
- **Updates at least every 14 days** until the issue is closed.

Once the fix has shipped, we note it in the `### Security` section of the [changelog](CHANGELOG.md) and, with your
permission, credit you by name.

We also publish a [GitHub security advisory](https://github.com/vdavid/cmdr/security/advisories) and request a CVE ID
when a flaw meets either of these:

- Someone can exploit it without already running code on the user's Mac, for example through a file, a network server, a
  web page, or the update channel.
- It exposes passwords, keys, or user data to someone other than the user.

Fixes that only guard against an attacker who already runs code on the Mac get a changelog note, but no advisory.
Security problems on Cmdr's servers (for example `api.getcmdr.com`) have nothing for users to update, so they get no
CVE. If one exposes user data, we tell the people affected whenever it could harm them.

We ask for **coordinated disclosure**: please give us 90 days from your report, or until a fix has shipped, whichever
comes first, before you publish details. Cmdr updates itself automatically, so a running copy picks up a fix within
hours of its release. If an issue is being actively exploited, tell us, and we'll move faster and can agree on an
earlier date.

There's no bug bounty. We can't pay for reports, but we're grateful for them.

## Scope

In scope:

- **The Cmdr desktop app for macOS**, latest released version. That includes the bundled update mechanism, license
  handling, the local MCP server, the AI features, and the file system backends (local, SMB, SFTP, WebDAV, MTP, ADB,
  archives).
- **api.getcmdr.com** (and its older alias `license.getcmdr.com`): licensing, update checks, crash and error reports,
  feedback, and downloads.
- **getcmdr.com**: the website, including `latest.json`, which the updater reads.

Also welcome, on a best-effort basis: `mail.getcmdr.com` (the newsletter) and `comments.getcmdr.com` (blog comments).
They run third-party open-source software, so we may point you to the upstream project too.

Out of scope:

- Services Cmdr uses but doesn't run: Paddle (payments), PostHog, Cloudflare's platform, GitHub, Discord, and the AI
  providers a user connects. Please report those to the vendor.
- Attacks that need an already compromised Mac or account, physical access to an unlocked machine, or root. Cmdr runs as
  the logged-in user and isn't App Sandboxed, so a process already running as that user can do what the user can.
- Denial of service by traffic volume, spam, and social engineering of the maintainer or users.
- Findings from automated scanners without a demonstrated impact, such as a missing security header or a TLS cipher
  preference.
- Versions older than the latest release.

## Supported versions

Only the **latest released version** gets security fixes. Cmdr checks for updates hourly by default and installs them
automatically, so the latest version is what most users run. If you find a problem in an older version, please check
that it still happens in the latest one.

## Safe harbor

If you research and report in good faith under this policy, we won't take legal action against you or ask anyone else
to, and we'll consider your research authorized. Good faith means you:

- Only test against your own installation and your own accounts or licenses.
- Don't access, change, or delete other people's data. If you reach some by accident, stop, don't keep a copy, and tell
  us.
- Don't degrade the service for others (no load testing against api.getcmdr.com or getcmdr.com).
- Give us a reasonable time to fix the problem before you publish, as described above.

If you're unsure whether something is OK, ask first at [security@getcmdr.com](mailto:security@getcmdr.com).

<!-- ⚠️ David: the safe-harbor paragraph is a plain-language template, not reviewed by a lawyer. Get it checked when you do the DPA and site agreement. -->
