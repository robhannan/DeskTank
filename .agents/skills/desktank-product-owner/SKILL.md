---
name: desktank-product-owner
description: Groom the DeskTank GitHub backlog weekly or on request. Refine thin issues, set issue statuses, plan the next Todo batch and post a session summary.
---

The product owner (PO) is the coordinator. It runs one grooming session, either weekly (Mondays after the owner's Claude usage resets at 08:00 GMT) or when asked. It reads the whole backlog itself and sends issue refinement to lower-cost models. It checks every draft and applies all GitHub changes itself, then reports. Refiners read the repository and the issues. They never write to GitHub. A delegated refiner follows only the Refinement parcel section.

The session runs in the main checkout or any worktree on `main`. It changes GitHub, not files, so it creates no branch or PR. Work out of the scratchpad directory, or `$TMPDIR` if there is none.

## Backlog model

- **Status** is a native issue field, not a Projects board. The values are Backlog, Todo, In progress, In review, Done, Not planned and Duplicate. Read it as `status.name` in REST (`gh api repos/robhannan/DeskTank/issues/<n>`) or `status { name }` in GraphQL. Look up the status IDs each session, because they can change:

  ```sh
  gh api graphql -f query='{repository(owner:"robhannan",name:"DeskTank"){planningStatuses(first:20){nodes{id name}}}}'
  ```

  Set a status with the issue's `node_id`:

  ```sh
  gh api graphql -f query='mutation($i:ID!,$s:ID!){setIssueStatus(input:{issueId:$i,statusId:$s}){issue{number status{name}}}}' -f i=<node_id> -f s=<status_id>
  ```

  Closing an issue sets Done (completed) on its own. Don't set Done, Not planned or Duplicate by hand. Those come from closing, which needs the owner's approval (see Autonomy).
- **Type** is Feature, Bug or Task (`type.name`). Owner ideas are usually Features with no labels or milestone.
- **Milestones** order the roadmap: M1 Living tank → M2 Power, overlay and control → M3 Wallpaper → M4 Screensaver → M5 Lock screen → M6 Depth → M7 Ship it, plus MA Android and MD Devices. `docs/spec/` is the source for scope and priority. The Android TV screensaver leads (`docs/spec/platforms.md` §3.6, D23), so MA work ranks alongside the lowest open core milestone.
- **Labels:**
  - `work-package`: a roadmap package from the spec.
  - `decision`: an open question only the owner can settle.
  - `follow-up`: umbrella issues of review leftovers, one per area (#150–#155).
  - `platform:*`: the target platform.
  - `good first issue`: a small, self-contained change.

## Autonomy

| Directly | Only by proposal in the summary |
| --- | --- |
| Set status on issues with none (Backlog) and move issues between Backlog and Todo | Close an issue (done, duplicate, not planned, stale) |
| Add or correct labels, type and milestone | Change an issue's title or the intent of owner-written text |
| Append a Refinement section to a thin issue | Settle or narrow a `decision` issue |
| Comment with clarifying questions | Change milestone titles, descriptions or due dates |
| File follow-ups, or add them to the matching `follow-up` umbrella issue | Re-scope or split a work package |
| Comment on stale PRs | Anything in In progress or In review, except flagging it |

Never edit the owner's original text. A refinement goes in its own section below a marker (see Apply), and a re-run replaces only that section. Ticked checklist items stay ticked. Write issue and comment bodies without hard wraps.

## Session

### 1. Preflight

1. `gh auth status`. In T3 Code, call `orchestrator_capabilities` once to list refiner models.
2. Find the log discussion, titled **Backlog grooming log**. If Discussions are disabled (`hasDiscussionsEnabled`) or the discussion is missing, create it in the Announcements category with `createDiscussion` and note in the summary that the owner should pin it. Pinning has no API. The **cutoff** is the date of its last comment, or 14 days ago if it has none.

### 2. Snapshot

Save one snapshot to a JSON file and work from it, rather than re-fetching per issue:

- Open issues: number, node_id, title, body, type, labels, milestone, status, created/updated dates, comment count, sub-issue and dependency summaries.
- Open PRs: author, draft state, last update, linked issues.
- PRs merged since the cutoff: titles, bodies (`Closes`/`Part of`) and review threads.
- Milestones: open/closed counts and due dates.

`gh api --paginate 'repos/robhannan/DeskTank/issues?state=open&per_page=100'` returns PRs too; drop entries that have `pull_request`.

### 3. Triage

Put each open issue in one bucket:

- **Ready**: has a goal, acceptance criteria, a type and a milestone or label, and nothing has changed.
- **Needs refinement**: a thin body (no acceptance criteria, or under about 300 characters of prose), or a body out of date against the code or spec.
- **Needs owner**: unclear intent, an open `decision`, or blocked by one.
- **Possibly done**: a merged PR references it, or the code already does what it asks.
- **Possible duplicate**: overlaps another open issue.
- **Stale**: untouched for 90 days or more and not on the roadmap.

Also fix the metadata on every issue as you go:

- Issues with no status get Backlog.
- Owner ideas with no type get Feature.
- Platform-specific issues get their `platform:*` label.

Refine at most **12 issues per session**, newest owner issues and the next milestone first. Carry the rest over and list them in the summary.

### 4. Refine (delegated)

Split the issues that need refinement into parcels of 4–6 related issues each. Send them to lower-cost models, two or three parcels in parallel:

- **In T3 Code:** `delegate_task` with `role: "research"` and a brief that forbids edits, using Sonnet (Claude), Flash (Gemini) or Sol (Codex) from `orchestrator_capabilities`.
- **Elsewhere:** the named `desktank-refiner` agent.

If a usage limit stops a refiner from starting, start a fresh parcel on another provider's model and try each provider once (as in the developer skill's Review section). Each brief contains:

- **Issues:** number, title, the full body and comments, copied in so the refiner needs no GitHub access.
- **Context:** the triage note, likely spec sections and code areas, and related issues.
- **Output:** the Refinement parcel format below.

### 5. Apply

Check each draft against the code and spec before applying it. Refiners can invent files, symbols or constraints, so drop anything you can't confirm.

- Append the refinement with `gh issue edit <n> --body-file <file>`. The file holds the original body unchanged, then this block:

  ```markdown
  <!-- groomed -->
  ## Refinement (groomed YYYY-MM-DD)
  ```

  If the marker is already there, replace everything after it.
- Apply the metadata the draft suggests if it falls under Autonomy's direct column.
- Post the refiner's open questions as one issue comment, headed `**Questions from backlog grooming**`, and list them in the summary too. Don't repeat a question that is still unanswered on the issue. Mention that it is still open instead.

### 6. Hygiene

- **Possibly done:** propose closing it in the summary with the evidence (PR, commit, code path). Don't close it.
- **Unresolved review threads** on PRs merged since the cutoff: if a point isn't already tracked, add it as a checklist item to the matching `follow-up` umbrella issue, or file a new issue when no umbrella fits. Cite the PR and thread.
- **Milestones:** compare each open milestone's issues, description and due date with the spec and what has merged. Propose corrections, for example an empty milestone ready to close, a past due date, or an issue in the wrong milestone.
- **Stale PRs** (no activity in 14 days or more, or failing CI with no follow-up push): comment once asking whether to continue, and flag it in the summary.
- **Status drift:** flag In progress or In review issues with no open PR or recent activity. Don't change them.

### 7. Plan the next Todo batch

Use judgement. The aim is a short, ready queue that the developer workflow can pick up in order.

1. Keep existing Todo items that are still ready. Move an item back to Backlog only with a reason given in the summary.
2. Fill from the **lowest open milestone**, treating MA as the same rank because of the Android TV lead. Respect sub-issues, `blocked by` dependencies and the spec's ordering. Only Ready or just-refined issues qualify. Nothing blocked by an open `decision` qualifies.
3. Add **2–3 easy wins** on top: small, self-contained and low-risk. Good sources are follow-up items, `good first issue` and small owner Features.
4. Aim for **6–10 issues in Todo** in total, ordered: milestone work first, then easy wins.

### 8. Summarise

Write one summary of about 400 words or fewer, with links as `#n`:

```markdown
## Backlog grooming — YYYY-MM-DD

**Todo now** (N): #a milestone work …, #b easy win …
**Refined** (N): #n one-line gist of what was added
**Metadata** : N statuses set, N labels/types/milestones changed (list notable ones)
**Follow-ups filed** : #n …
**Needs you**
- Questions asked: #n …
- Proposed closes: #n reason + evidence
- Decisions blocking the next milestone: #n …
- Milestone corrections: …
- Stale or drifting PRs/issues: …
**Carried over** : issues not refined this session
**Models used** : coordinator + refiner models (and any fallbacks)
```

Post it as a new comment on the **Backlog grooming log** discussion with `addDiscussionComment`, and give the same text as the final reply in the T3 thread, with the discussion comment's link.

## Refinement parcel

You refine backlog issues for DeskTank, a lightweight Rust virtual aquarium (see `AGENTS.md` and `docs/spec/README.md`). Use read-only inspection: don't edit files, run builds, call GitHub or delegate. For each issue, read the spec sections and code it touches, then return:

```markdown
### #<n> <title>
**Goal:** one or two sentences of user-visible outcome, faithful to the owner's text.
**Acceptance criteria:**
- [ ] observable, testable criteria (behaviour, platforms, resource limits where relevant)
**Notes:** spec sections, code paths and related issues you verified, plus constraints (resource use, WebGL2 limits, OS-agnostic core).
**Size:** S / M / L, with a one-line reason.
**Suggested metadata:** type, milestone, labels; only when clearly warranted.
**Questions for the owner:** only what blocks writing the criteria; "none" otherwise.
```

Keep the owner's intent and don't add features. Give only paths, symbols and spec sections you opened. If an issue looks done, duplicated or obsolete, say so with evidence instead of refining it.
