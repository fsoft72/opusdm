# Git hooks

Version-controlled hooks for this repo. Activated per clone by pointing git at
this directory:

```sh
git config core.hooksPath .githooks
```

The root `package.json` `prepare` script runs that command automatically on
`pnpm install`, so a normal install is enough. Run it by hand after cloning if
you skip the install step.

## Hooks

- **pre-commit** - formats the staged files with [dprint](https://dprint.dev)
  (config: `dprint.json` at the repo root) and re-stages them, so every commit
  contains dprint-formatted code. Needs `dprint` on `PATH` (or installed as a
  dev dependency); the commit is blocked with an install hint if it is missing.
  Files staged as a partial hunk are re-added whole - stage the rest or commit
  the file separately if that matters.
- **prepare-commit-msg** - `core.hooksPath` makes git ignore any globally
  configured hooks, so this re-invokes a global `prepare-commit-msg` (from
  `git config --global core.hooksPath`, else `~/.git-templates/hooks`) if one
  exists.

## Bypass

`git commit --no-verify` skips all hooks. Do not use it to dodge formatting.

## Format the whole tree

```sh
pnpm format         # dprint fmt
pnpm format:check   # dprint check (CI-friendly, no writes)
```
