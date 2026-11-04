# Changelog

Notable changes to Tap. The download is always the latest build
([`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg));
the versions below are how we track releases, and match `Cargo.toml` and the app's
Info.plist.

## Unreleased

## 0.1.2 — 2026-11-04

Settings you can use, more of Search's tab behaviour, and Search's own profile
brought over.

- **Settings shows only what works.** Every row on it now changes what Tap does, at
  once; rows for what Tap can't do yet are no longer shown (their values are still kept
  and still travel in a profile), and pages with none left (Passwords, Extensions,
  Keyboard & Mouse) are out of the rail. Newly working, as Search has them:
  - Tabs: New tab position (a link from another app beside the tab you're on, at the
    start or at the end; ⌘T's at the end unless Leftmost), Leave a new tab when the
    last tab closes, Closing a tab goes back to the one before, Load background tabs
    when you go to them, Close tabs you leave alone (pinned, named and grouped tabs
    stay; ⇧⌘T brings one back).
  - Start with a fresh window: pins, named tabs and tab groups come back, nothing else.
  - Address bar commands: "settings" or "new tab" alone in the field go there.
  - Ask where to save each file.
  - Videos wait for a click, and Page zoom, where every site starts.
  - Remove click trackers from links (gclid, fbclid and the like; utm_ tags stay) and
    Tell sites not to sell or share your data (Global Privacy Control), on macOS 27.
- The last tab that isn't in a group, closed beside groups, leaves a new tab rather than
  opening a group's page, as in Search.
- A profile file now always lists its spaces, so Search's import (#845) brings its
  tabs in too; before, a profile from a Mac with one space came into Search empty.
- `cargo xtask search-profile <file.tapprofile>` packs Search's own profile on this Mac
  (tabs and groups, history, bookmarks, settings) as a Tap profile file to bring in.

## 0.1.1 — 2026-11-04

Settings, profiles you can carry to your other Mac with their tab groups, and Tap
updates itself from now on (0.1.0 doesn't: download this one by hand).

- **Settings** (Tap › Settings…, ⌘,), in a tab of its own as Search's is: every page,
  group and row of Search's Settings, with a field to find a setting. What Tap does
  already works from there: Search with (and your own engine's address), sleeping tabs,
  light or dark, Confirm before quitting, where downloads go, making Tap the default
  browser, clearing history, cookies or the cache, updates. Rows for what Tap can't do
  yet show what is kept for them, dimmed, and say so. Settings use Search's own names
  and values, so a Search profile's settings read as they are.
- **Export and import a whole profile** as one offline file (Search's
  [#845](https://github.com/driceroland/Search/pull/845) format): File › Export Tap
  Profile… writes a `.tapprofile` and offers it to AirDrop, Mail or Messages; on the
  other Mac, open it (or File › Import Profile…) and its tabs come in with their
  groups, folded or open as they were, along with its history, bookmarks and settings.
  Search's `.searchprofile` files come in the same way. Sign-ins, passwords and cookies
  never go in the file. Spaces, pins and extensions of their own come later; a
  profile's tabs from several spaces come in one after another.
- **Tab groups in the row.** A group's name heads its tabs; click it to fold the group
  to its name (and the tab you're on, if it's one of its own) or open it again. Groups
  come back after a relaunch, and a link opened from a grouped tab stays in the group.
  A narrow tab whose icon hasn't arrived yet shows its first letter.
- **Tap updates itself**, as Kil does: it checks a few seconds after it opens and every
  six hours (Settings › About › Install updates on its own), or when you choose Tap ›
  Check for Updates…. A new version is downloaded and checked (its checksum, Developer
  ID and Apple's notarization) and installs when Tap quits; Restart Now does it at
  once and opens the new Tap.

## 0.1.0 — 2026-11-04

The first release: Tap browses, and can be the Mac's browser. Much of Search is still
to come (docs/PLAN.md, Appendix A); Tap doesn't update itself yet.

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
- **Tap can be your Mac's browser.** It is listed under Desktop & Dock › Default web
  browser; links clicked in other apps open in Tap, in the new tab if one is in front,
  and HTML files open from the Finder. PDFs, pictures and text files offer Tap under
  Open With.
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
