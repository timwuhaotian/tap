# Security policy

## Supported versions

Only the latest release, [`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg)
and `main`, gets security fixes. Until the first release, that means `main`.

## Reporting a vulnerability

Please **don't open a public issue.** Report it privately through GitHub:
[**Report a vulnerability**](https://github.com/timwuhaotian/tap/security/advisories/new).

Include what an attacker could do, the steps to reproduce it, and the macOS version and
Tap build. You'll get a reply within a week. Once a fix is released, the advisory is
published with credit to you, unless you'd rather not be named.

## Scope

A web page can't do more in Tap than WebKit allows it. A flaw inside WebKit itself
belongs to Apple; report it at [security.apple.com](https://security.apple.com). These are
Tap's, for example:

- A page, or an extension, reaching what Tap keeps for you: saved passwords and passkeys,
  history, other profiles' or private tabs' data, or files outside what you chose.
- A page's script calling Tap's own message handlers, or the native side of an
  extension shim, in a way it shouldn't be able to.
- The address field or the window showing one site while another is loaded (spoofing).
- The bench socket in `~/Library/Application Support/Tap/` accepting another user.
- An update installed without its checksum, Developer ID signature or notarization
  holding, or a release DMG or app whose signature or notarization doesn't hold.
