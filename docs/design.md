# sk design

sk is an async Rust, local-first CLI for linear stacked branches. sk owns
local state and recovery. GitHub's public REST APIs own published PR stacks.
Only sync, submit, and merge perform Git network operations or API requests.

## Commands

- create accepts a branch name or -m message, with date-slug generated names.
  Commit staged changes, prompt about unstaged changes, and support -a.
  Empty named branches are allowed. --insert/-i permits middle-of-stack
  creation; descendants keep their refs until explicit sk restack.
- submit/s publish ancestors through the current branch; ss includes
  descendants (--stack). -d creates draft PRs; -n skips
  editing. Existing PR metadata and draft state are preserved.
- New PR titles use the first unique commit subject. Descriptions use its
  body followed by subsequent commit messages. Each editor file ends in .md:
  title on line 1, blank line 2, description thereafter. Use $EDITOR.
- sync refreshes all tracked stacks with bounded parallel work. Require a
  clean checkout. Never push. Apply a stack only when its whole replay works.
  Skip conflicts and divergence, continue independent stacks, and report
  restack instructions. Prune merged and eligible closed local branches.
- Unpublished work beyond a merged PR becomes an unpublished follow-up
  branch, with descendants moved onto it in the same transaction. Ambiguous
  boundaries leave the stack unchanged. Retain closed ancestors still needed.
- restack stays offline. Conflicts use the current checkout and Git rebase.
  continue resumes; abort restores the original stack and checkout. Remote
  divergence choices use snapshots fetched by sync, without new requests.
- log short and ls show all stacks locally with connections, current branch,
  and restack indicators. Short output omits PR details. No live refresh flag.
- checkout and co offer a built-in fuzzy picker when no branch is supplied.
  up/down accept a step count. From trunk, up selects among multiple stacks.
- pr opens the cached PR in Chrome. macOS AppleScript focuses an existing
  tab for the PR, including subpages, before creating a new tab.

## Storage and recovery

Versioned JSON lives in Git's common directory under sk, shared by worktrees.
Record ordered branches, trunk, base commits, remote snapshots, and cached PRs.
Git refs are authoritative for current tips. Serialize writes with a lock;
replace metadata atomically. Journal original state, refs, checkout and steps.

Compute replays with git replay --ref-action=print. Accept only success.
Publish stack refs using compare-and-swap transactions. Keep affected stacks
unchanged on sync conflicts. Skip branches owned by other worktrees and
required dependent rewrites. Never automatically stash dirty changes.

## Boundaries

V1 supports new stacks, macOS, GitHub.com, and Git 2.56 or newer. Reuse gh
credentials without invoking gh stack. No import workflow, sibling branches,
or background update checks. Preserve PR identity when regrouping remote
stacks; respect queued PR restrictions. Retry partial submissions by discovery.

## Offline divergence reconciliation

Restack offers local, fetched remote, replay, or cancel choices. Replay uses
cached commit IDs only. The last published remote commit must remain an
ancestor of the local tip, and the recorded branch base must be an ancestor
of the fetched tip. When those boundaries cannot be proven, choose local or
remote explicitly rather than guessing which changes should survive.

## Nix distribution

The repository provides a flake with default and named `sk` packages for
macOS and Linux on ARM64 and x86-64. Home Manager consumers can follow their
own nixpkgs input. `package.nix` remains usable through `pkgs.callPackage`.
Cargo.lock pins Rust dependencies, package builds run tests, and a wrapper
provides Git and GitHub CLI. Authentication stays with the user's existing
Git and `gh` configuration. A local Git flake input supports development;
consumers update their lock after committing changes. Activation is manual.

## Command aliases

`sk aliases` creates `~/.config/sk/aliases` with defaults if absent and prints
its path. It honors XDG_CONFIG_HOME and preserves existing contents. Each
non-comment line contains an alias followed by command words; shell-style
quoting groups arguments, without executing shell expressions. User entries
override defaults, aliases may chain, and cycles report an error.

Defaults: `c create`, `s submit`, `ss submit --stack`, `ls log short`, and
`co checkout`. Expansion prepends configured arguments to caller arguments
and happens before help handling. `stack --submit` is also accepted as
`submit --stack`, supporting aliases such as `ss stack --submit`.

Submission follows Graphite's documented scope: default submit includes
ancestors and the current branch; --stack adds descendants. --no-stack
explicitly selects the default scope. Narrow submission preserves local
state for excluded descendants and leaves existing remote stack members
in place. It does not prompt to broaden scope to published descendants.

## Reorder

`sk reorder` opens the Git-selected editor (GIT_EDITOR, core.editor, VISUAL,
EDITOR, or Git's fallback). The document lists ancestors through the current
branch, tip first, with trunk as a comment below. `--stack` includes the full
linear stack. Saving a new order replays each branch's original commit range
onto its new parent and restacks descendants, preserving branch names, PR
cache, and the original checkout. The command never fetches or publishes.

Only permutations of the selected branches are accepted. Graphite allows
removing lines to introduce sibling branches; that falls outside sk's linear
stack model. Missing, duplicate, added, or trunk entries fail before mutation.
An unchanged order is a no-op. Editor failure, dirty worktrees, and affected
branches checked out elsewhere prevent applying changes. Ref snapshots also
detect edits made externally while the editor is open.

Successful planning updates refs transactionally. Conflicts enter the same
journaled rebase flow as restack: stage resolutions and run `sk continue`, or
run `sk abort` to restore original refs, metadata, and checkout.

Reference inspection: @withgraphite/graphite-cli 1.8.6 and its darwin-arm64
package were downloaded to `/tmp/sk-graphite-inspect`. The npm JS is a native
binary launcher; no implementation source was copied into this repository.
Native `reorder --help` and a disposable repository probe confirmed the editor
layout, scope, and sibling behavior when a line is removed. See also
https://graphite.com/docs/command-reference and
https://graphite.com/docs/edit-branch-order.

## Merge

Merge is an explicit addition to the network-command allowlist. Default scope
is all branches strictly below current, excluding current. --include-current
adds current; --stack includes descendants; --dry-run performs read-only API
validation and prints the selection. No browser or interactive confirmation
is involved. A bottom branch with default scope exits without authentication.

Fresh PR data determines eligibility. Skip already merged PRs; closed/draft
or missing PRs block. Selected open heads must match local commits, and the
remote stack's unmerged prefix must exactly match the selection so GitHub
cannot implicitly include unexpected PRs. A standalone PR is supported only
when it targets the configured trunk. Never automatically submit local work.

Use PUT pulls/{number}/merge-async with the target SHA, merge_action=default,
and bypass_rules=false. Optional method flags choose merge/squash/rebase;
otherwise omit the method for GitHub's default. Poll the returned UUID using
GET pulls/{number}/merge-async/{uuid}. Treat an existing pending request (409)
as resumable, not permission to submit again. Distinguish merged, enqueued,
failed, and pending. Polling is bounded; failed/unknown outcomes are errors.
No local refs or PR cache are changed by merge; sync owns refresh and pruning.

Sources: GitHub's public async merge API documentation and github/gh-stack's
internal/github/merge_async.go, inspected in /tmp/sk-gh-stack-merge-inspect.
https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request-asynchronously

## Modify, track, split, and boundary navigation

- `modify`/`m` amends the current tracked branch's last commit, preserving its
  message by default. -c creates a commit; -a stages all files; -m supplies a
  message; -e uses Git's editor. Combined short flags are supported. Empty
  branches create commits rather than amending their parents. New commits
  require -m or -e. Unstaged work remains untouched without -a. Descendants
  remain unchanged and the command directs the user to explicit restack.
  Patch staging, --into, and interactive rebase are outside this initial scope.
- `track`/`tr` adopts a named local branch or current branch, recursively
  discovering parents along first-parent history. --parent overrides the first
  parent choice. Ambiguous names at a commit require explicit selection. It
  records existing boundaries without rewriting refs or changing checkout,
  preserving dirty work. Existing tracked parents must be stack tips. Trunk
  comes from sk.trunk, cached origin/HEAD, or an unambiguous main/master.
- `split`/`sp` supports commit boundaries only. The editor lists full SHAs and
  generated branch names oldest first. Keeping a line creates a boundary;
  deleting it groups commits with the next boundary. The original branch and
  tip must remain last, retaining its PR metadata. --each skips the editor
  and selects every boundary. Generated names derive from branch and commit
  subjects with numeric suffixes for collisions. No commits are rewritten.
  Ref additions and metadata use the existing transactional recovery path.
  Dirty worktrees, non-linear history, invalid edits, and occupied worktrees
  are rejected. Original checkout and descendant tips are preserved.
- `top`/`t` selects the tip; `bottom`/`b` selects the first branch above trunk,
  not trunk itself. From a trunk shared by stacks, reuse the interactive stack
  selector. Checkout uses Git's normal protection for uncommitted work.

All commands and their aliases expose offline help. Existing alias files
remain unchanged; the default alias map supplies the new aliases unless
explicitly overridden. These commands introduce no network access.

## Compact short log layout

The short renderer follows the packaged Graphite renderer: one row per branch,
no connector-only rows, names aligned after the widest graph, and sibling
stacks joined on the trunk row. Stack lanes sort by their bottom branch name.
Use ◯ for ordinary branches and ◉ for current, including when trunk is current.
Each lane has a terminal color; current is also bold. No redundant current
label, PR number, or PR status appears. Keep the compact (needs restack)
annotation. Multiple trunks have separate graphs and a (trunk) annotation.

Inspected the readable minified JS renderer from Graphite npm 1.7.0 under
/tmp/sk-graphite-inspect/1.7.0/package/graphite.js. Verified current Graphite
1.8.6 using a fresh repository initialized by gt and five stacks created with
gt create (heights 3, 1, 2, 4, 1). sk track adopted those existing branches
only after the gt fixture was complete. Plain-text outputs matched exactly
with gamma-1, delta-4, and main checked out. Fixture and captures are under
/tmp/sk-gt-log-w9jaxmsl; no remote or browser was involved.


## Submit action output

Use Graphite-style per-branch rows: `▸ branch (Action)` followed by the PR URL.
Labels are Create, Update code, New parent, Update code, new parent, and No-op.
These are completed branch actions, not predictions: emit only after the PR
operation and its state save succeed. The later native stack grouping can
still fail, in which case the command reports that error.

Creation requires a successful create-PR call. Rediscovering an existing PR
on retry is not creation. Code updates compare observed remote refs with the
snapshotted local heads; only differing refs enter the atomic leased push.
If none differ, skip the push entirely. PR base changes come from successful
base updates rather than assumptions based on local metadata. No-op means
no code push, PR creation, or PR base change for that branch; it does not
promise zero API reads or zero native-stack regrouping.

Reference: submit status labels in the inspected Graphite 1.7.0 npm bundle,
/tmp/sk-graphite-inspect/1.7.0/package/graphite.js. No Graphite service calls
are needed for sk's output.

## Insert a branch

`sk create <name> --insert` (or `-i`) inserts directly above the current
branch, preserving descendant PR associations and their original commit
boundaries. It supports the usual staged commit, `-a`, and `-m` behavior.
The new branch becomes current. Existing descendant refs stay untouched;
the command prints `sk restack` instructions. This follows our explicit
restack workflow rather than automatically rebasing during creation.

At the tip it behaves like ordinary create. At trunk it inserts below the
only existing stack, or starts a stack when none exists. Multiple stacks
at trunk are ambiguous and fail before staging or creating anything; use
create at the intended stack tip followed by `sk reorder --stack` instead.
Ordinary create from trunk still starts a separate stack. No network is used.
