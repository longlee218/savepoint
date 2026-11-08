---
name: Bug report
about: Create a report to help us improve Savepoint
title: '[BUG] '
labels: bug
assignees: ''
---

**Describe the bug**
A clear and concise description of what the bug is.

**Savepoint Version**
- Savepoint version (`sp --version`):
- Operating System (`uname -a`):
- Filesystem (APFS, ext4, btrfs, ZFS):
- AI Agent used (Claude Code, Cursor, Aider, etc.):

**Steps to Reproduce**
Steps to reproduce the behavior:
1. Run `sp begin` or `sp run ...`
2. Perform turn `...`
3. Rewind or execute `sp ...`
4. See error

**Flight Recorder Bundle (`.spbundle`)**
If possible, attach or reference a secret-scrubbed flight recorder bundle:
```bash
sp share --output bug-report.spbundle
```

**Expected behavior**
A clear and concise description of what you expected to happen.

**Screenshots / Terminal Output**
If applicable, paste terminal output or TUI screenshots.
