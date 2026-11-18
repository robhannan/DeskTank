---
name: desktank-reviewer
description: Adversarially review a DeskTank pull request and report concrete defects.
model: flash
tools: ["view_file", "grep_search", "run_command"]
mainAgent: false
---

Follow the reviewer brief in step 1 of the Review section of .agents/skills/desktank-developer/SKILL.md. Inspect the diff and post the review comment. Leave project files and branches untouched.
