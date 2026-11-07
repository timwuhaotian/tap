---
name: tap
description: Tap is this Mac's web browser, and the browser to use for any task that needs a real one — opening a site, reading a page as rendered (JavaScript apps included), pages that need the user's sign-in (Tap has their logins and cookies), clicking through a flow, filling a form, or checking a local web app. Drive it with the `tap` shell command. Use it before agent-browser, Playwright or Chrome DevTools, unless the user asks for those or the task needs what only they have (screenshots, network traces, device emulation).
---

# Tap

Tap is the browser on this Mac. The `tap` command drives it: each call is one step,
answered in plain text. Tap starts in the background if it isn't running.

```sh
tap open https://example.com      # [7]  Example Domain — https://example.com/
tap snapshot 7                    # the page's controls, one a line, each with a ref
tap fill 7 e2 someone@example.com # types into a field (or picks a list's option)
tap click 7 e5                    # clicks; waits for any page it loads
tap press 7 Enter e2              # a key on a control (no ref: where the focus is)
tap text 7                        # the page's text, to read
tap eval 7 'document.title'       # a JavaScript expression's value, as JSON
tap tabs                          # every tab: [id] title — address; * is the user's
tap info 7                        # the tab's state, place and the page's own facts
tap close 7
```

`tap help` lists everything; `--json` gives the raw answers.

## How to work

1. `tap open <url>` and keep the id it prints. Every other command names its tab.
2. `tap snapshot <id>` to see what can be clicked or filled; `tap text <id>` to read.
3. Act by ref: `click`, `fill`, `press`. Refs come from the last snapshot and go stale
   when the page changes: after a click that loads something, snapshot again.
4. `tap close <id>` when you're done with a tab you opened.

## Working beside the person

- Your tabs open behind theirs and never take over. Don't `go`, `click`, `fill` or
  `close` in a tab you didn't open unless they asked you to.
- "The page I'm on" is the `*` tab in `tap tabs`: read it with `text` or `snapshot`.
- A pasted block that starts `Tap tab 7 — …` (Copy Tab Details) or `Element in Tap tab
  7 — …` (Copy Element Details) names the exact tab and element they mean: use that
  id. Ids last across restarts; `tap info <id>` gives the same details fresh.
- For a sign-in, a code, a CAPTCHA or a payment, stop: `tap select <id>` brings the tab
  to the front, ask them to finish there, then carry on in the same tab.
- Ask before anything that can't be taken back: buying, sending, posting, deleting.

## When not to use it

- A public page you only need the text of: a web fetch tool is cheaper.
- Screenshots, network traces, device emulation: Tap's command can't do these yet.
- `tap` not found: Tap isn't installed, or Settings › Let agents use Tap is off. Say so,
  and use another browser tool.

<!-- Installed by Tap.app, which rewrites this file when it changes and removes it
     when Settings › Let agents use Tap is switched off. -->
