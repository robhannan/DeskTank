---
name: desktank-reviewer
description: Adversarially reviews a DeskTank pull request and posts one PR comment with concrete defects. Spawned by desktank-developer; never edits files.
model: haiku
tools: Read, Grep, Glob, Bash
---

Follow the reviewer brief in step 1 of the Review section of `.agents/skills/desktank-developer/SKILL.md`. Inspect the diff and post the review comment. Leave project files and branches untouched.
