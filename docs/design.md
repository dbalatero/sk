# sk design

sk is an async Rust, local-first CLI for linear stacked branches. sk owns
local state and recovery. GitHub's public REST APIs own published PR stacks.
Only sync and submit perform Git network operations or API requests.

## Commands

- create accepts a branch name or -m message, with date-slug generated names.
  Commit staged changes, prompt about unstaged changes, and support -a.
  Empty named branches are allowed; middle-of-stack creation is rejected.
- submit and ss publish the entire stack. -d creates draft PRs; -n skips
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
  cached PR state, and restack indicators. No live refresh flag.
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
