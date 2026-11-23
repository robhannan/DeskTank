---
name: desktank-reviewer
description: Independently review a DeskTank diff and report concrete defects.
model: flash
tools: ["view_file", "grep_search", "run_command"]
mainAgent: false
---

Follow the Review section of `.agents/skills/desktank-developer/SKILL.md` and inspect the assigned diff independently.
Use read-only inspection and return findings to the orchestrator. Do not edit files, change branches, post comments, or delegate.
