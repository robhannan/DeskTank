---
name: desktank-developer
description: Coordinate a DeskTank feature or fix through implementation parcels, integrated verification, independent review and CI.
---

The orchestrator owns one increment and its PR through hand-off. Prepare the
branch once, integrate the parcels, and run the release checks on the combined
change. A delegated worker follows the Implementation parcel section and
the Build conventions relevant to its assignment.

## Assigning work

Keep a small change and its tests together. Split a larger change at boundaries
that can be verified independently. Sequence dependencies. Run parcels in
parallel only when their writes are disjoint and their checks can run safely
in the selected checkouts.

Choose Sol for Codex, Sonnet for Claude, or Flash for Gemini implementation.
In T3 Code, select the available model ID from `orchestrator_capabilities`
and pass it explicitly to `delegate_task` with `role: "implementation"`.
Outside T3 Code invoke the named `desktank-developer` agent. The orchestrator
resolves design and scope questions. Gemini Pro is an option for that work.

Give each worker a self-contained brief using these fields:

- **Outcome** — one observable change and its acceptance criterion.
- **Scope** — owned files or symbols, dependencies and relevant constraints.
- **Context** — the decision or source pointers needed for this parcel.
- **Verification and hand-off** — focused check, expected result and stopping point.

Point to this skill's Implementation parcel section. Send relevant excerpts
and paths so workers can start without replaying the parent conversation.

## Implementation parcel

Work in the assigned checkout and scope. Apply the relevant Build conventions
and run the focused check in the brief. Stop at the acceptance criterion or
when progress needs a design or scope decision. Return changed paths, the
check and its result, and any blocking question with the evidence needed to
decide it. Leave integration, PR operations, review dispatch and CI to the
orchestrator.

## Before you start

1. Read `AGENTS.md`, the relevant GitHub Issue (work package, goal,
   acceptance criteria), the matching files in `docs/spec/` and
   `docs/notes/`, and the code you will touch. Reference `Closes #<n>`
   (or `Part of #<n>`) in your PR description rather than editing a
   central changelog.
2. `git fetch origin` and branch from the latest `origin/main` (or from the
   lower PR's remote-tracking branch `origin/<lower-branch>` when stacking
   onto an open PR):
   `git checkout -b feat/<short-slug> origin/<base>` (where `<base>` is `main`
   or `<lower-branch>`). Never commit to `main`.
   If the working tree has changes you did not make, stop and report.
3. If the Cargo workspace or CI doesn't exist yet on `main`, the first PR
   must be the scaffold (workspace, toolchain, lints, `deny.toml`, CI);
   don't build features until it has merged. Install missing tools such as
   `cargo-deny` with `cargo install --locked`.

## Build

- Small, focused commits. Stage files explicitly (no `git add -A`/`.`), check
  `git status --short` before each commit, and keep local agent settings and worktrees out of commits.
  Use the active agent's co-author trailer when it provides one.
- Idiomatic, modern Rust: 2024 edition, the pinned toolchain, workspace
  lints (clippy pedantic). Match the surrounding code: comment density,
  naming, module layout. No `unwrap`/`expect` outside tests and
  `main` start-up; return typed errors (`thiserror` in libraries).
- Keep the simulation and rendering core free of OS-specific code.
  Platform integration (wallpaper layer, screensaver bundle, lock screen,
  overlay window) lives behind small `cfg(target_os = …)` modules or
  per-platform crates. `unsafe` only at FFI boundaries, each block with a
  `// SAFETY:` comment.
- **Resources are a feature.** DeskTank must stay cheap to leave running.
  Don't add busy loops or uncapped redraws; throttle or pause rendering
  when hidden, occluded or on battery. If you change the render loop,
  frame pacing or asset loading, measure idle CPU and resident memory in
  release mode before and after (e.g. `top -pid <pid>` / Activity
  Monitor on macOS) and put the numbers in the PR.
- New dependencies must earn their place: prefer small crates, disable
  unneeded default features, and make sure `cargo deny check` passes.
- Add tests for new logic. Keep the simulation deterministic for a given
  seed and time step, so tests can assert on exact or tightly bounded
  results; never depend on wall-clock time or an unseeded RNG in tests.
- The orchestrator verifies the integrated change before pushing:
  `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
- Update the relevant file in `docs/spec/` (or add a new numbered ADR in
  `docs/decisions/`), `docs/notes/` (only for durable technical gotchas),
  and `README.md` if usage changed, in the same PR.

## Show it working

Check visuals in the real app, in every display mode your change touches.
Put scratch output under `target/shots/` (`mkdir -p target/shots`):

- Use the app's capture mode if `AGENTS.md` documents one; otherwise
  `screencapture -x target/shots/x.png` (macOS). For motion, record
  frames or a short screen capture and convert to a GIF:
  `ffmpeg -framerate 30 -i dir/frame_%04d.png -vf "scale=1000:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer" out.gif`
- Look at the frames yourself (Read the PNGs) before claiming it works.
- You can only run macOS locally. For Windows/Linux-specific code, rely on
  CI and say plainly in the PR that it was not run on those platforms.
- Publish images to the orphan `pr-assets` branch, checked out as a
  worktree at `target/pr-assets`. If it is missing, run
  `git worktree add target/pr-assets pr-assets`; if the branch doesn't
  exist yet, create it as an orphan (`git worktree add --orphan -b
  pr-assets target/pr-assets`). Pull, add the files under
  `<feature-slug>/`, commit and push. Reference them as
  `https://github.com/robhannan/DeskTank/blob/pr-assets/<path>?raw=true`.

## Pull request

- In T3 Code, when `pull_request_*` MCP tools (`pull_request_create`,
  `pull_request_read`, `pull_request_respond`, `ci_logs`, `ci_rerun`) are
  available, use them instead of `gh`. Otherwise, always use
  `env -u GITHUB_TOKEN gh …`, so an ambient token for another account is
  never used. If that is refused (seen with an emoji in the title), run
  `GITHUB_TOKEN= gh …` instead.
- Write the PR body to a new file with a name unique to this PR, never a
  shared path such as the session scratchpad's `body.md`. Use
  `mkdir -p target/pr-bodies` and `target/pr-bodies/<branch with / replaced
  by ->.md`. Check it with `head` before `gh pr create`. After creating the
  PR, confirm the posted body matches the file exactly:
  `diff <(gh pr view <n> --json body -q .body) <file>`. If a command that
  both writes the file and creates the PR is refused, rewrite the file
  before retrying the create.
- Start the PR title with one category emoji. Squash merges use the
  title as the commit subject on `main`, so history stays scannable:
  ⚡️ feature, 🐛 bug fix, 🏎️ performance, ♻️ refactor, 📝 docs,
  ✅ tests only, 👷 CI, 🔧 config/tooling, 💄 UI polish, 🎨 graphics
  (art, sprites, shaders, how the tank looks), 🏷️ tag/release.
  Copy the emoji exactly as shown (some include the U+FE0F variation
  selector). Pick the dominant category. If a change fixes wrong
  behaviour, use 🐛 even when the fix is a speed-up; use 🏎️ only for
  speed with no behaviour change.
- Push the branch and open a PR against `main` (or against the lower PR's
  branch when stacking — prefer stacking when multiple PRs are in flight or
  related). Fill in the sections of
  `.github/pull_request_template.md` (summary, screenshot or GIF, resource
  use, testing) and delete its HTML comments. Keep the body to about 300
  words and say plainly what you could not verify. End with a “🤖 Generated with” credit naming the agent used.
- Don't hard-wrap PR bodies or PR comments: GitHub renders single
  newlines as line breaks, so write each paragraph or bullet on one
  line (code blocks excepted).

## Review

1. Run an adversarial review on a lower-cost model: in T3 Code (when
   `delegate_task` is available), call `delegate_task` with
   `role: "review"` and a lower-cost model from `orchestrator_capabilities`
   (e.g. `gemini-3.8-flash` or `claude-haiku-5-5`); outside T3 Code, spawn
   the named `desktank-reviewer` subagent. Give it the diff command
   (`git diff <base>...HEAD`, e.g. `origin/main...HEAD`), context and focus areas
   (always include resource use and cross-platform correctness), and
   include the reviewer model selected for this run in the brief. Have the independent reviewer return its findings to the orchestrator.
   If a usage limit prevents the reviewer from starting, dispatch a fresh
   review to an available lower-cost model from a different provider. In
   T3 Code, select it from `orchestrator_capabilities` and start a new
   `delegate_task` with `role: "review"`. Keep the same diff and focus
   areas and update the model in the brief. If another provider also hits
   a usage limit at startup, try each remaining eligible provider once.
   If none can start, report which providers were tried and why review
   is unavailable.

   The orchestrator posts one PR comment beginning with this two-line GitHub alert.

   ```markdown
   > [!NOTE]
   > 🤖 <model> Reviewbot
   ```

   Replace `<model>` with the model that performed the review (for example, `GPT-6 Luna`,
   `Gemini 3.8 Flash` or `Claude Haiku 5.5`). Use the actual model if the host selected a fallback.
   Follow the tag with a blank line, a bold heading, then
   findings ranked most severe first (severity, file:line, scenario, fix).
   Ask it to label each finding **in scope** (a defect in this PR's
   changes) or **out of scope** (pre-existing, new feature, or a
   nice-to-have).
   The orchestrator posts with `pull_request_respond` (in T3 Code) or
   `GITHUB_TOKEN= gh pr comment <n> --body-file <file>` (outside T3 Code).
   The reviewer uses read-only inspection. Report any unavailable review.
2. Check for GitHub Copilot's automated PR review (`copilot-pull-request-reviewer`)
   in the PR conversation (`pull_request_read` with `sections: ["conversation"]`
   in T3 Code, or `gh pr view` / `gh api` outside T3 Code). Include all
   **non-Low** Copilot findings (Medium, High, Critical, or unlabelled) in
   this review cycle alongside the agent review; Low-severity Copilot
   findings may be skipped unless trivial. If Copilot has not posted yet
   when the agent review finishes, check again once CI settles (or on a
   `watch_pull_request` wake) and address any non-Low findings before
   hand-off.
3. Verify each finding from both reviews against the code before acting;
   reviewers can be wrong.
4. **Keep the PR's scope fixed.** Review fixes are only for defects in
   what this PR adds or changes: bugs, crashes, leaks, resource
   regressions, or broken tests in the new code. Anything else is a
   follow-up, not a fix:
   - pre-existing issues;
   - new behaviour or features;
   - hardening of code this PR didn't touch;
   - nice-to-haves.

   File each follow-up as a GitHub Issue (or add it to the matching
   `follow-up` issue) and don't implement it in this PR. Fixes should be
   small. If a
   valid in-scope fix would grow the diff by more than roughly a third, or
   needs a new dependency or a design decision, escalate it in your report
   instead.
5. Push the fixes (batching agent-review and Copilot fixes together when
   both are available), then reply on the PR:
   - For the agent review, post one comment listing each point as one of:
     - ✅ fixed (with commit hash);
     - 📝 documented;
     - ➡️ follow-up (added to the roadmap);
     - declined, with a short reason.
   - For each addressed Copilot inline thread, reply on the thread with
     the same outcome and resolve it (`pull_request_respond` with
     `reviewThreadId` and `resolved: true` in T3 Code).
6. Do at most one fix round. Only run a second, focused review if a
   fix changed behaviour in a non-trivial way, and apply the same scope
   rule to its findings.

## CI and hand-off

- CI runs on macOS, Windows and Linux. In T3 Code, when
  `watch_pull_request` is available (and you are not a delegated child),
  call `watch_pull_request` (`until: "checks"`) and end your turn to wake
  on completion, then call `unwatch_pull_request` before handback.
  Otherwise, wait for the run on your latest commit with a background poll
  of `env -u GITHUB_TOKEN gh run list --branch <branch>` /
  `gh api repos/robhannan/DeskTank/actions/runs/<id>/jobs`. Transient
  `Server Error` responses are noise. Fix any failures, on every platform.
- Before debugging a CI failure in code, rule out infrastructure: check
  https://www.githubstatus.com. Suspect an outage (and re-run, not
  re-code) when a push creates no run, the PR head lags the branch, or a
  job is "Abandoned" with no log. Always bound CI waits and check that
  the PR head is the commit you are waiting on.
- When CI is green on your latest commit and both the agent review and
  any non-Low Copilot findings are addressed, stop.
  Hand off the reviewed PR. Recommend either "merge" or "escalate to the owner" in
  your report. Escalate for anything risky, architectural, hard to reverse,
  security-sensitive, needing new OS permissions or entitlements, or
  touching CI/secrets.

## Report back (≤150 words)

PR URL; CI status of the head commit per platform; review outcome (rounds
run; counts fixed / documented / follow-up / declined, and anything
escalated); recommendation (merge or escalate) and why; what the feature
does; tests added; resource numbers if measured; anything you could not
verify; follow-ups worth adding to the roadmap.
