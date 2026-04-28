# rv

> Ruby version & gem manager. uv-grade speed. Single binary. Reproducible.

`rv` is to Ruby what `uv` is to Python and `gv` is to Go: one fast Rust binary
that owns interpreter installs, gem pinning, and reproducible runs — without
the shell-activation overhead and lock-format sprawl of legacy version
managers.

## Why rv

- **Reads `.ruby-version` and `Gemfile`'s `ruby "..."` line** as first-class
  resolution sources. Existing repos work with no migration.
- **Replaces `gem install` for tools.** Pin `rubocop`, `brakeman`, `steep`,
  `sorbet` in `rv.toml`, lock them in `rv.lock`, reproduce on CI.
- **Content-addressed store.** Ruby builds deduped by sha256 across
  projects (no more 200-MB rbenv cellars per machine).
- **Sub-millisecond shim** for IDE compatibility (no `eval rv init` shell hook).
- **Tokio + rustls.** No OpenSSL system dependency.

## Status

🚧 Pre-alpha. Actively building.

## Install

Coming soon (mirrors gv's release infra: `install.sh` + Homebrew tap).

## Quickstart

```bash
rv install 3.3.5                 # install Ruby (shells out to ruby-build)
rv list                          # local installs
rv use-global 3.3.5              # set the default
rv add tool rubocop              # pin a global gem
rv sync --frozen                 # CI: install exactly what gv.lock says
rv tree                          # show what resolves and why
rvx rubocop --version            # ephemeral, no project state touched
```

## Resolution order

1. `RV_VERSION` env var
2. `Gemfile`'s `ruby "..."` directive
3. `.ruby-version`
4. `~/.config/rv/global`
5. Latest installed

## Prerequisites

- `ruby-build` on `$PATH` (`brew install ruby-build`). rv shells out to it
  for the source compile. A future release may build Ruby in-process.

## License

MIT
