# Security Policy

## Reporting a vulnerability

Report privately through GitHub's
[private vulnerability reporting](https://github.com/voip-rs/freeswitch-esl-tokio/security/advisories/new),
or by email to <jeromepoulin@gmail.com> if you would rather not use GitHub.

Please do not open a public issue for a suspected vulnerability.

Expect an acknowledgement within a week. A confirmed issue is fixed in a
released version before the advisory is published; you will be credited in the
advisory unless you ask otherwise.

## Supported versions

| Version | Supported |
| --- | --- |
| 2.x | yes |
| 1.x | security fixes only |
| < 1.0 | no |

Pre-release versions (`-beta.N`) are not supported; report against the latest
release.

## Scope

This is a client library for the FreeSWITCH Event Socket. Two areas carry the
security weight:

- **Command injection into the wire protocol.** ESL is a text protocol in
  which `\n\n` terminates a command, so a caller-supplied string that reaches
  the socket unvalidated can append arbitrary ESL commands. Any path that gets
  a `\n` or `\r` past validation and onto the wire is a vulnerability in this
  crate.
- **Credential disclosure through logging.** Types that hold passwords or auth
  tokens redact them in `Debug`, and wire logging redacts the `auth` and
  `userauth` commands. A log path that prints a credential in the clear is a
  vulnerability in this crate.

Out of scope, because they are properties of the protocol and the server rather
than of this library:

- ESL transmits the password in cleartext over TCP. Do not expose the event
  socket beyond localhost or a trusted network.
- A FreeSWITCH user who can issue ESL commands can already control the switch.
  This crate does not add a privilege boundary.
- Vulnerabilities in FreeSWITCH itself belong to
  [signalwire/freeswitch](https://github.com/signalwire/freeswitch).
