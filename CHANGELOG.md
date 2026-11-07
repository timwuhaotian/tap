# Changelog

Notable changes to Tap. The download is always the latest build
([`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg));
the versions below are how we track releases, and match `Cargo.toml` and the app's
Info.plist.

## Unreleased

## 0.2.15 — 2026-11-07

### Fixed

- **A second Tap no longer kills the agent channel**: starting another copy (or
  running a build from the tree) used to steal the bench socket and leave every
  `tap` command failing until Tap restarted. A socket someone answers on is now
  left alone, quitting takes only its own socket file away, and Tap-isn't-listening
  says to restart Tap.

## 0.2.14 — 2026-11-07

### Changed

- **Login pages fill themselves in**: opening a site with a saved sign-in puts the
  newest account straight into the form, instead of waiting for a click into the
  box. The accounts still hang from a focused box to switch, and Fill in sign-ins
  off stops it.

## 0.2.13 — 2026-11-07

### Added

- **Copy Tab Details**: right-click a tab, or a page, to copy what lets a coding
  agent find exactly that tab: its id for the `tap` command, its address, state and
  HTTP status, its group and place, and what the page says of itself (language,
  canonical address, description, viewport, scroll, the selected text). An address
  that carries a token or a code is pointed out.
- **Copy Element Details**: right-click anything on a page to copy its selector, box,
  style, text and markup, with the `tap` command that finds it.
- **`tap info <id>`** gives an agent the same details.

### Changed

- **A tab keeps its id across a restart**, so an id you gave an agent still finds
  the same page tomorrow, and a closed tab's id is never given to another.

## 0.2.12 — 2026-11-07

### Fixed

- **Video full-screen buttons work**: a player's full-screen button was often
  refused, most visibly with the window itself in full screen. The tab bar's reading
  progress asked the page how far it was scrolled four times a second, and each ask
  cancelled the click that had just happened, as far as the page could tell. The page
  now reports its own scrolling, as Search's does, and a click stays a click.

## 0.2.11 — 2026-11-07

### Fixed

- **Copying the address keeps its scheme**: ⌘C on the whole address in the field
  copies the full page address with its scheme, rather than the trimmed text
  the field shows.

## 0.2.10 — 2026-11-07

### Added

- **Double-click the tab bar to zoom**: a double click on the empty part of the tab row
  fills the screen with the window and, again, puts it back, as a title bar does. It
  follows the system's title-bar double-click setting (Desktop & Dock).

## 0.2.9 — 2026-11-06

### Changed

- **Smoother page loads**: the address bar's loading line glides forward between
  updates instead of jumping, then runs to the end and fades out.
- **A loading transition instead of a white page**: a tab's first page shows a small
  rippling mark and the site's name, then fades into the page once it arrives.
- **A quieter new tab**: the large drawn instrument is replaced by a small ripple mark.
- **Smoother tabs**: hover highlights and the close button fade in and out, and a
  pressed tab sinks and springs back on the display's frames.

## 0.2.8 — 2026-11-06

### Changed

- **Less work during page loads**: unchanged browser state no longer triggers a window
  update, and sign-in detection avoids unnecessary input scans on changing pages.

## 0.2.7 — 2026-11-06

### Fixed

- **Return uses what you typed**: an inline address ending is only accepted with Tab
  or →; Return still opens a suggestion you explicitly picked.
- **Find visited URLs by their path**: a fragment such as `the-pair` now brings up
  matching pages from history, including matches in query strings.

### Changed

- **Smoother browser chrome**: address suggestions fade in and their selection glides;
  tabs and groups slide gently as they enter or leave. Reduce Motion and the light
  motion setting keep these transitions restrained.

## 0.2.6 — 2026-11-05

### Fixed

- **Missing tab icons**: sites such as platform.qianwenai.com recover from blank
  cached icons, and newly fetched icons no longer disappear while being saved.

## 0.2.5 — 2026-11-05

Sites whose only icon is an SVG now show it in the tab.

- **SVG site icons**: a page declaring only an SVG icon (e.g.
  platform.qianwenai.com) gets its icon drawn in an isolated page-scripts-off view
  and kept like any other icon, instead of leaving the tab blank.

## 0.2.4 — 2026-11-05

Dragging a tab no longer drags the window; the empty row stops opening new tabs.

- **Dragging a tab never moves the window**: the window is unmovable, and only the
  empty ends of the tab row move it, so a press meant for a tab stays a tab press
  wherever in the row it lands. Double-clicking the empty row no longer opens a new
  tab either — the empty row is the titlebar, and a blank tab is still one ⌘T away.

## 0.2.3 — 2026-11-05

Moving tabs stay under the hand; ⌘W closes without surprises; the empty row brings tabs back.

- **Drag a moving tab where you see it**: clicks follow the tab's animated position,
  including the gap beside it, instead of taking its destination for the tab and
  sometimes dragging the window.
- **⌘W closes without surprises**: closing a blank tab beside pins or groups keeps the
  window open; closing the last window quits without the explicit-quit confirmation.
- **Right-click the empty tab row to reopen a closed tab**: Reopen Closed Tab is
  available when there is a closed tab to bring back.

## 0.2.2 — 2026-11-05

Bring things over from Chrome or Safari; the chrome lines up; → completes the address.

- **Import from Chrome or Safari**: Chrome profiles are detected locally; Safari data
  comes from an export ZIP. Tap backups and password CSV files remain supported.
  Search and other browser-specific import sources are no longer offered.
- **The chrome's text and pills line up**: a group's chip is as tall as a tab; the
  address no longer jumps above the lock when you click into it, nor a tab's title
  when it opens for editing; every line of chrome text is centred by one rule.
- **→ completes the address**: with the caret at the end of what you typed, the right
  arrow takes the grey completion, as Tab does; elsewhere it moves the caret.

## 0.2.1 — 2026-11-05

A tab is grabbed anywhere in its column, and dropped on a group joins it.

- **A tab is grabbed anywhere in its column**: above, below or between the pills is
  the tab now, not title bar, so dragging a tab no longer drags the window instead.
- **Drag a tab onto a group to join it**: carried over another group's heading or
  tabs, the group lights up in its colour, and letting go puts the tab (or every picked
  tab) into it beside the tab under the pointer. Let go off the row, over the page, it
  joins nothing; in a crowded row, the end past the + still moves the window.

## 0.2.0 — 2026-11-05

Agents browse with Tap; new tabs pile up behind a quiet instrument; a click on any tab lands in the field.

Chrome refinement, no behaviour change.

- **One card, one pill, one shadow**: every floating card and pill now shares
  `palette`'s radius, hairline and shadow instead of each drawing its own.
- **The live tab reads as a plate**: a hairline edge in both themes, and the
  hovered and picked tabs wear one too.

Chrome polish, no behaviour changes: the tab row's plus always shows, presses land at
once under Reduce Motion, the live tab keeps a hover wash, narrow-tab letters and the
load line read better; address-bar progress shows only while loading; tab-menu
dividers, dimming and the colour tick fixed; Settings rows tint fully, explain
themselves and line up; the find bar's count, clicks, dimming and pitch fixed.

- **New tabs pile up and close**: +/⌘T and the "new tab" field command always open a
  fresh blank tab instead of reusing the blank one in front; the cross on a new tab
  closes it, and closing the only tab when it is already new closes the window.
- **A blank tab draws a quiet instrument**: a small isometric signal-bench behind the
  field — a carriage breathing on a 12 s loop, a dial with a breathing needle, one
  softly lit signal dot. Native drawing, no web view; still under Reduce Motion.

A load stops paying for what didn't change: the page-load path is tightened end to
end, behaviour-preserving (docs/PLAN.md §10 "The page-load path").

- A page that keeps loading no longer rewrites `session.json` on every progress tick;
  the session and history files are encoded on the writer thread, not the main one.
- A page that already wears a fresh site icon (kept under 7 days) isn't probed,
  re-downloaded and redrawn again on every visit, and the PNG is written off the main
  thread.
- The web view's observer reads only the property that moved, instead of all seven
  and calling all five model setters, on each of a load's ticks.
- Progress counts as change only in steps the drawn line can show; the tab row and the
  address bar redraw only when something drawn moved, and the reload/stop symbol and
  the window title are touched only on change.
- The chrome's text style and the address bar's badge image are made once, not on
  every draw.
- History's recency score is worked out once per visit when the file is written, not
  inside the sort's comparator.

Coding agents browse with Tap.

- **A `tap` command** for agents and scripts: open a page behind yours, list its
  controls with refs, click, fill and press on them, read its text. Tap starts in the
  background if it isn't running, and a sleeping tab wakes where it stands to be read.
- **Every agent knows**: Tap keeps the command in `~/.local/bin` and a skill in
  `~/.agents/skills` (and Claude Code's), telling an agent to use Tap when a task needs
  a browser.
- **Settings › Let agents use Tap** is on by default; off takes the command and skill
  away. Only scripts running as you can reach Tap's socket.
- **A click on any tab lands in the field**: clicking any tab — the live one or one
  behind — opens the address bar's field with its address selected; a blank tab opens
  it empty with the cursor in it.

## 0.1.8 — 2026-11-05

Picked tabs close together and move together.

- **Picked tabs close together**: ⌘W, or Close N Tabs in the menu of one of them; one
  ⇧⌘T brings them all back.
- **Picked tabs move together**: drag one of them and the others come along, side by
  side, and stay picked.
- **Picked tabs share a group**: right-click one of them and Move to Group takes
  them all — New Group holds them all, a group is off only when every picked tab
  is already in it, and Remove from Group takes them all out.

## 0.1.7 — 2026-11-04

Pick tabs together, choose a group's colour, and the tab you're on glides.

- **Pick several tabs**: ⌘-click tabs one by one, or ⇧-click for a run of them; then
  **⌘C copies their addresses**, one to a line, ready to paste. Esc or a plain click
  lets them go. Right-click one of them for Copy N Addresses too.
- **Choose a group's colour**: right-click its name, then Color, for any of the nine.
- **The tab you're on** sits on a lifted pill that glides to the next tab you pick.
  With the address bar up, the tab itself stays as it is when clicked.

## 0.1.6 — 2026-11-04

The tab row moves, groups have colours, and there's an address bar.

- **Everything in the tab row moves smoothly**: tabs opening, closing and changing
  places glide on one spring; a group folding slides its tabs back under its name, and
  unfolding brings them out again. Reduce Motion in System Settings turns it off.
- **Drag tabs to reorder them**, within their group (or among the tabs in none), and
  **drag a group by its name** to move the whole group past the others.
- **Groups have colours**, as in Chrome: each new group gets one no other group has,
  its name on a chip of that colour and a line of it under its tabs. Groups brought
  over from Chrome keep the colour they had there.
- **An address bar** under the tabs, with back, forward and reload on its left: the
  page's address with its site in bold, a lock (or a warning on plain http, or a
  slashed lock when a secure page pulls in insecure parts), and the load's progress.
  Click the address (or ⌘L) to type; click the lock to see whether the connection is
  secure and who issued the site's certificate. Settings › Appearance › Show the
  address bar hides it, and the tabs and buttons go back to how they were.
- **A group can be ungrouped or closed** now: right-click its name for Rename Group,
  Collapse/Expand, Ungroup Tabs and Close Group (⇧⌘T brings a closed group back whole).
- **Right-click a tab** for Move to Group (a new group, another one, or out of its
  group), Rename and Put to Sleep, beside what was there.

## 0.1.5 — 2026-11-04

Passwords come over, and Tap fills them in.

- **Bring things over is Search's sheet now** (Settings › General › Bring Over):
  every browser found on this Mac with what it holds counted, the profile to bring
  (the one used last, any other, or all of them), and a switch each for passwords,
  bookmarks, history, and open tabs with their groups. Then Bring them in, and what
  came, line by line.
- **Passwords from Chrome, Arc, Brave, Edge, Dia and the other Chromium browsers**:
  macOS asks once for the browser's keychain key, and every saved password comes into
  the macOS keychain under Tap's name, with the sites it was told never to save.
- **Passwords from Search** come as you use them: Search keeps each one as its own
  keychain item, which macOS hands to another app only after asking, one by one. So
  they're offered under sign-in boxes straight away, and the first time you pick one
  macOS asks for it once; from then on it's Tap's.
- **A passwords CSV** (Chrome, Google Password Manager and others export one) comes in
  through the sheet's File….
- **Filling in sign-ins**: click a sign-in box and the accounts kept for the site hang
  from it; a click fills the name and password in, as typing would. A sign-in that asks
  for the name first gets the name, and its password page the right account first.
- **Saving**: after a sign-in that took, a bar offers to save the password, or update
  it when it changed, or never ask on that site. Settings › Passwords switches both
  on and off.
- **Right-click a tab** for Reload, Duplicate, Copy Address, Copy as Markdown Link,
  Close Tabs to the Right, Close Other Tabs, Close and Reopen Closed Tab; right-click a
  group's heading to collapse or expand it.
- Settings' page list lines up: every icon in one column, every name starting at the
  same place.

## 0.1.4 — 2026-11-04

Bring things over finds the other browsers on this Mac by itself.

- **Settings › General › Bring things over** names what it found (Search, and
  Chrome, Arc, Brave, Edge, Dia, Vivaldi, Opera and the other Chromium
  browsers), and its Bring Over menu lists them. Pick one: Tap says what it has,
  then brings it in when you say so. Nothing in the other browser changes.
- **From a Chromium browser**, from the profile you used last: its open tabs and
  their groups, named and folded as they were
  ([#841](https://github.com/driceroland/Search/pull/841)), its history, and its
  bookmarks, kept in a folder of the browser's name for when Tap shows them.
- **From Search**, straight from its folder, no export needed: its tabs and
  groups, history, bookmarks and settings, as its profile file would bring them.
- Tabs already open in Tap aren't opened again, so bringing the same browser over
  twice adds only what's new. Passwords, cookies and extensions stay behind.
  A profile file still comes in through the same menu (From a Profile File…).

## 0.1.3 — 2026-11-04

Twenty-six more Settings rows work, so Settings now shows 53 of its 81 rows and
every one of them does what it says.

- **Privacy**: Block ads and trackers (a built-in rule list in WebKit's own
  blocker, YouTube's ads included, pausable per site), Prevent cross-site
  tracking as in Safari, and Let sites ask to send notifications (asked in
  Tap's name, the answer kept per site). Forget choices clears what sites were
  allowed or refused.
- **Browsing**: search suggestions from your engine as you type (never for an
  address, never from a private tab), Reading mode (⇧⌘R; a site's own Markdown
  when it has one, following light and dark), the floating video (Tap's window
  or the system's picture in picture, out on its own when you leave the tab or
  Tap), Pages at 120 Hz, Darken pages that stay light, Open sized pop-ups in
  small windows, Press Esc twice to leave full screen, and Scroll with the
  middle button.
- **Tabs and the tab bar**: site icons or letters, how far you've read on the
  tab you're on, tabs that give when pressed, tab numbers while ⌘ is held, the
  address on the tab you're on, New tab in a floating field, Reduce animations
  in Low Power Mode, and the downloads button that stays (and with it the
  Keyboard & Mouse page is back in the rail).
- **Passwords** is back in the rail: Open… opens the macOS Passwords app.
  Saving and filling in Tap itself are still to come.
- **About**: Send Feedback opens a draft with the version in it, and Let a
  script drive Tap now really turns the bench socket off and on (a test run's
  socket stays, as it must).
- A bench answer larger than the socket's buffer (the whole of Settings, for
  one) is no longer cut short: the listening socket is nonblocking so the
  switch can close it, and an accepted one no longer inherits that.

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
