# Tap — notes for agents

- [`docs/PLAN.md`](docs/PLAN.md) is the plan and the spec, and the **only** plan
  document: record every planning change, decision, milestone status and parity status
  there. Don't create other plan or notes files.
- Tap is a Rust port of Search (`~/code/Search`, driceroland/Search). Search's behaviour at
  the plan's pinned version is the reference; `CONTRIBUTING.md` has the checks and
  conventions.
- Work and commit directly on `main`; don't create branches.
- **To release** ("发版", "new patch version", "打包"), follow plan §12 step by step:
  bump, changelog, README, `scripts/release.sh`, install into `/Applications`, commit
  `Tap x.y.z: …` with `releases/`, push. `releases/` is in git from 0.1.0 on.
