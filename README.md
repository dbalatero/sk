# sk

A local-first stacked-branch CLI written in async Rust. GitHub's public
Stacks API manages published PR stacks; local branches and recovery belong
to `sk`.

## Development

Requires Rust, Git 2.56 or newer, and existing GitHub CLI authentication for
remote commands. The browser integration targets Google Chrome on macOS.

```sh
cargo build
cargo install --path .
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## Workflow

```sh
sk create -m "feat: authentication"
sk create -m "feat: API"
sk ls
sk ss -dn
sk sync
sk restack
sk co
sk down
sk up
sk pr
```

Only `sync` and `submit` perform Git/API network operations. Local commands
use cached PR information. `sync` and `restack` require a clean checkout.
Use `sk continue` after resolving and staging rebase conflicts, or `sk abort`
to restore the original stack.

New PR editing uses `$EDITOR` and a temporary `.md` file: first line title,
second line blank, remaining lines description. `-n` skips editing; `-d`
creates drafts. Chrome reuses an existing tab for a cached PR URL on macOS.

See [design](docs/design.md), [implementation plan](docs/implementation-plan.md),
and [acceptance criteria](docs/acceptance.md) for scope and verification.
