## What this changes

<!-- One or two sentences: what a user can do now, or what no longer goes wrong. Link the issue: "Fixes #123". -->

## Why

<!-- The problem it solves, or the reason for the approach if there were other options. -->

## How it was checked

<!-- Tick what applies and say how, for example "typed in a 20,000-line file, no stutter". -->

- [ ] `cargo test --workspace` passes
- [ ] I tried it in the running editor (`cargo run -p forge-app -- path\to\folder`), not only in tests
- [ ] New behaviour has a test (a unit test, or a throwaway project for something only the window shows)
- [ ] No new compiler warnings (`cargo build -p forge-app`)

## Screenshots

<!-- For anything you can see: before and after. Delete this section otherwise. -->

## Performance

<!-- Anything that runs on every keystroke, every frame or at startup: run with FORGE_PROFILE=1 and give the numbers before and after.
     Benchmarks live in `crates/*/tests/bench.rs`. Write "no effect" if it does not touch those paths. -->

## Documentation

- [ ] README updated (the feature's section, or the "not built yet" list if something is still missing)
- [ ] `docs/FEATURES.xlsx` updated (status, the "Verified" column, and a row for any gap left)
- [ ] Not needed (a fix with no visible change)

## Reviewers: look here first

<!-- Risky parts, things you were unsure about, follow-up work you left out on purpose. -->
