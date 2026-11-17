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

Only `sync`, `submit`, and `merge` perform Git/API network operations.
Local commands use cached PR information. `sync` and `restack` require a clean checkout.
Use `sk continue` after resolving and staging rebase conflicts, or `sk abort`
to restore the original stack.

New PR editing uses `$EDITOR` and a temporary `.md` file: first line title,
second line blank, remaining lines description. `-n` skips editing; `-d`
creates drafts. Chrome reuses an existing tab for a cached PR URL on macOS.

See [design](docs/design.md), [implementation plan](docs/implementation-plan.md),
and [acceptance criteria](docs/acceptance.md) for scope and verification.

## Nix

With Nix installed and the `nix-command` and `flakes` experimental features
enabled, install `sk` into your user profile:

```sh
nix profile install github:dbalatero/sk
sk --help
```

Or run it without installing:

```sh
nix run github:dbalatero/sk -- --help
```

To install, build, or run from this checkout:

```sh
nix profile install .
nix build
nix run . -- --help
```

The flake exposes `packages.<system>.sk` and `packages.<system>.default`
for Apple Silicon/Intel macOS and ARM64/x86-64 Linux. Builds run the Rust
tests and wrap `sk` with Git and GitHub CLI on its PATH.

To install through a Home Manager flake, add this input:

```nix
inputs.sk = {
  url = "github:dbalatero/sk";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Pass `inputs` through `extraSpecialArgs`, then add to your home module:

```nix
{ inputs, pkgs, ... }:
{
  home.packages = [ inputs.sk.packages.${pkgs.stdenv.hostPlatform.system}.sk ];
}
```

To update the Home Manager input, run this from your configuration's flake
directory, then rebuild your Home Manager configuration:

```sh
nix flake update sk
```

The package includes the tools needed for authentication; sign in separately
with `gh auth login` if needed. It does not embed credentials.

## Aliases

Run `sk aliases` to create and print the path to `~/.config/sk/aliases`
(or `$XDG_CONFIG_HOME/sk/aliases`). Edit that file to customize commands:

```text
# my aliases:
ss stack --submit
fdsa create
```

Then `sk fdsa -m "a change"` runs `sk create -m "a change"`. Alias arguments
come before caller arguments. Comments, quoted arguments, and chained
aliases are supported; shell expressions are not executed.

Built-in aliases work before creating the file: `c` → `create`, `s` →
`submit`, `ss` → `submit --stack`, `ls` → `log short`, `co` → `checkout`.
User entries override these defaults. `s` submits ancestors through the
current branch; `ss` also submits descendants, matching Graphite's submit
scope. Use `sk <alias> --help` for command help.

## Reorder

Run `sk reorder` to edit branch order from trunk through the current branch,
or `sk reorder --stack` for the whole stack. The editor lists the tip first;
move lines while keeping every branch exactly once. Descendants are restacked
automatically, and your original branch remains checked out. Configure the
editor using `GIT_EDITOR`, `git config core.editor`, or `EDITOR`.

On conflicts, resolve and stage changes, then run `sk continue`. Use
`sk abort` to restore the original stack. Reorder is offline and requires a
clean checkout. Adding/removing lines is unsupported by sk's linear model.

## Merge

`sk merge` merges open PRs below the current branch into trunk, excluding the
current branch. Already merged PRs are skipped. `--include-current` includes
the current branch; `--stack` selects the whole stack. Use `--dry-run` to
fetch and preview the exact selection without submitting a merge request.

GitHub chooses the default merge method, or specify one of `--squash`,
`--rebase`, or `--merge`. The async stack merge API honors repository rules
and routes to a merge queue when configured. A queued result is reported as
queued, not merged. Draft or closed PRs, unpublished work, and mismatched
local/remote stack order block the request. The target head SHA is pinned.

The command polls for completion for up to 60 attempts, one second apart
(plus request time). If still pending, rerunning resumes an existing request
when GitHub returns its ID. An ambiguous network error reports uncertainty;
check GitHub before retrying. Local refs and cached PR state are unchanged:
run `sk sync` afterward to refresh and prune merged branches.
