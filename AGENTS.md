# Ai Agent Guidelines

## Agent rules

- Keep development and test artifacts in gitignored `target/` directories or `/tmp`, never in the application's runtime state directory. Treat real dictation history as read-only during analysis; save annotations and benchmark results with the development artifacts.
- Make sure that we've got proper test coverage giving us confidence in changes we're making. We want to be sure there will not be any unforeseen regressions on other platforms etc.
- Comments explain why, not what. Do not write comments that restate what can be read from the code; they get out of sync quickly.
- Legacy code and compatibility fallbacks are not accepted in this codebase. Replace old paths instead of preserving them.
- Every commit message must include a meaningful body that explains the change's intent and motivation-not merely what the diff does—so future readers tracing code through `git log` or `git blame` can understand why the change was introduced.
- Never commit without running all CI checks beforehand. Everything must pass, no matter if CI fails because of your changes or not.
- When rebasing newer version prefer origin/ variant and always use git rebase.
