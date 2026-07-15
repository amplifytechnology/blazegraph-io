# Git hooks

Secret-scanning hooks for this repo. Because `blazegraph-io` is public, a secret
committed even once — and later removed — stays exposed in history. These hooks
stop that at the door.

- **`pre-commit`** — scans the *staged diff* with [gitleaks](https://github.com/gitleaks/gitleaks)
  plus a filename guard (`.env`, `*.pem`, `*.key`, …). Fast; runs on every commit.
- **`pre-push`** — full-history gitleaks scan as a backstop before anything leaves
  the machine.

## Enable

Hooks are not active until you point git at this directory:

```sh
make hooks
# or, equivalently:
git config core.hooksPath .githooks
```

Install the scanner: `brew install gitleaks` (or see the gitleaks releases page).

## Override

If a finding is a genuine false positive, `git commit --no-verify` /
`git push --no-verify`. Prefer fixing the pattern or adding a `.gitleaks.toml`
allowlist entry over routinely bypassing the hook.
