# Changelog

Notable changes to Tap. The download is always the latest build
([`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg));
the versions below are how we track releases, and match `Cargo.toml` and the app's
Info.plist.

## Unreleased

- **Tap browses.** One window with Search's row of tabs across the top: the live tab on a
  grey pill, a cross to close, the plus for a new tab, back, forward and reload at the
  far end, and the traffic lights centred in the row. Click the live tab or press ⌘L to
  type an address or a search; ⌘T floats the field over the page and opens a tab only
  when you press Return. ⌘W, ⇧⌘T, ⌃Tab, ⌘1–⌘9, ⌘[ and ⌘] work as in Search. ⌘-click
  opens a link behind the page; a page's new window becomes a tab beside it.
- **The field finishes addresses from your own history**, as Search's does: places you
  have been, ranked by how often and how lately, and a search last when what you typed
  isn't an address. The rest of the best address is drawn grey after the caret, and Tab
  takes it; ↓ and ↑ pick a row, and Return or a click goes there. Nothing is completed
  while an input method is still composing, so pinyin and kana typing are left alone.
  History is kept in Search's own `history.json` format, and never with a password.
- **Pages can ask.** `alert()`, `confirm()` and `prompt()` show the system's sheet, titled
  with the site that asks so a page can't pass its message off as Tap's; file inputs
  open the file panel, naming the site the file goes to; a site behind a name and a
  password asks for them, twice at most. A question from a tab you aren't looking at
  waits until you go to that tab.
- **A tab behind whose page went away** is loaded again when you come back to it, rather
  than in the background.
- **Tabs you aren't using sleep.** A tab left alone for half an hour gives its page back
  and comes back on the same page, with Back and its scroll, when you return to it;
  sooner when macOS is short of memory. Never one on screen, loading, playing sound, on
  a call, downloading, waiting on a question or holding something typed.
- **Find on page.** ⌘F puts a small bar in the page's corner: what you type is found as
  you type it, every match painted and the current one in orange, with "3 of 17" as
  the page counts it. Return and ⌘G go to the next match, ⇧Return and ⇧⌘G to the one
  before; Match Case and Whole Words are under the bar's menu; Esc puts it away and the
  page's own selection back. Search's own page script, run where the page can't see it.
- **Downloads.** A download link, a file the server sends as an attachment, or anything
  WebKit can't show goes into Downloads under its own name, numbered if the name is
  taken; the Downloads stack in the Dock bounces when it arrives. A tab opened only for
  a download closes again.
- **Site icons.** Each tab wears its site's icon, the crispest the page declares or its
  `favicon.ico`, kept small beside the history in Search's own `icons/` format so a tab
  brought back from yesterday has its icon before it loads.
- **Tabs come back.** The session is kept in Search's own format, written as it changes
  and when Tap quits; restored tabs cost nothing until you look at them.
- **The bench**, for scripts and tests: Search's socket protocol on a test run
  (`TAP_PROBE`), driven with `scripts/bench`, so Tap's own window can be checked without
  touching anything else on the Mac.
- **A page that keeps crashing stops reloading itself** (Search #849): it is made again
  twice a minute at most, then says "This page kept crashing." with Try Again.
- **The project.** Plan ([`docs/PLAN.md`](docs/PLAN.md)), license, contribution guide,
  code of conduct, security policy, and issue and pull request templates.
- **The icon.** "Bolt T": a heavy T whose stem breaks into a lightning bolt, on a warm
  plate. It is drawn from `packaging/icon.svg` with `cargo xtask icon`; debug builds get a
  graphite plate.
- **The release flow**, as in Kil: `scripts/bundle.sh` and `scripts/release.sh` build,
  sign with Developer ID, notarize and staple locally. CI checks formatting, lints and
  tests, and never builds releases.
