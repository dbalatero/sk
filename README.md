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

## Nix

Build or run from this checkout:

```sh
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

Until the flake is pushed to GitHub, use
`git+file:///Users/dbalatero/code/sk` as the input URL. Git inputs pin a
commit, so commit local changes before updating your configuration's lock:

```sh
cd ~/.config/nixpkgs
nix flake update sk
./bin/switch
```

The package includes the tools needed for authentication; sign in separately
with `gh auth login` if needed. It does not embed credentials.
