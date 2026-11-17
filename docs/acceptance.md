# Acceptance criteria

## Foundation

F1 State is versioned, common-directory scoped, and uncommitted.
F2 State records branch order, bases, remote snapshots, and PR information.
F3 Writes are locked and atomic.
F4 Journals support interruption, continue and abort.
F5 Local commands perform no network operations.
F6 Configuration discovery is local and ambiguous configuration is explicit.

## Local commands

L1 create supports messages, explicit names and empty branches.
L2 Staging preserves the index unless requested; -a includes untracked files.
L3 Generated collisions gain suffixes; explicit collisions and middle reject.
L4 checkout/co provides fuzzy selection and direct local checkout.
L5 up/down follows stack order and handles trunks and boundaries.
L6 Checkout relies on Git protection for uncommitted changes.

## Replay and recovery

R1 Amended parents and multiple commits replay using recorded boundaries.
R2 Replay proposals do not update refs until successful.
R3 Dirty checkouts fail before restacking; restack remains offline.
R4 Conflicts pause in the current checkout; continue and abort work.
R5 Foreign worktrees and required dependent rewrites are skipped.
R6 Ref and metadata interruptions recover without overwriting external edits.

## Sync

S1 Sync processes all locally tracked stacks with a clean checkout.
S2 Fetch once per remote and bound concurrent independent work.
S3 Failed whole-stack replay leaves branch tips and ancestry state unchanged.
S4 Sync never pushes or mutates GitHub state.
S5 Prune eligible merged/closed branches, retaining needed closed ancestors.
S6 Preserve unpublished merged-branch work on a follow-up branch atomically.
S7 Divergence defers an offline reconciliation prompt to restack.
S8 Partial success is summarized and returns nonzero when attention is needed.

## Submission

P1 submit/ss publishes the entire stack; -dn creates drafts without editing.
P2 Existing PR metadata and draft state remain intact.
P3 New PR metadata derives from branch-specific commit messages.
P4 Editor files end in .md with title, blank line, then description.
P5 Finish editing and validation before writes; reject invalid/empty stacks.
P6 Push atomically with explicit leases, without unconditional fallback.
P7 Update bases and native stack grouping while preserving PR identity.
P8 Discover completed actions on retry to prevent duplicates.

## Visualization and browser

V1 ls shows all tracked stacks, connections, current branch and PR status.
V2 Rendering stays local.
V3 pr uses cached URLs and defaults to the current branch.
V4 Chrome reuses PR tabs including subpages and raises their windows.
V5 Create a tab only when no match exists; report missing cache/automation.

## Test mapping

- F1/F2/R1/R2: amended_parent_replays_without_changing_refs_until_apply.
- F4/R4: conflict_abort_restores_all_refs_and_checkout and
  conflict_continue_updates_recorded_boundary.
- F5/L4/L5/V1/V2: navigate_and_render_a_stack_without_a_remote.
- F6/L1/L2: conflict_repo fixtures create staged branches from local main.
- L3: branch_subject_becomes_slug and
  dirty_restack_and_middle_create_preserve_work.
- L6: checkout_preserves_work_and_refuses_overwriting_conflicts.
- R3: dirty_restack_and_middle_create_preserve_work.
- R5: foreign_worktree_blocks_rewrite_without_a_journal.
- R6: external_trunk_change_invalidates_a_replay_plan,
  continue_does_not_erase_work_written_after_interruption, and
  abort_does_not_erase_work_written_after_interruption.
- S6: merged_extra_work_becomes_followup_without_moving_refs.
- S7: divergent_stack_is_reported_without_mutation.
- P1: combined_submit_flags_and_remote_urls.
- P3: metadata_uses_first_subject_and_all_remaining_messages.
- P4/P5: editor_document_has_title_blank_line_and_markdown.
- P8: submission_retry_reuses_pr_created_before_server_error and
  lookup_reuses_existing_pr_and_encodes_branch.
- V1: compact_view_marks_current_pr_and_stale_ancestry.
- V3/V5: picker_without_terminal_and_missing_pr_fail_without_side_effects.
- V4: pr_identity_matches_subpages_but_not_other_numbers_or_hosts.
- Picker matching: fuzzy_matches_noncontiguous_case_insensitive_characters.

## Verification scope

The named tests cover specific behavior, not every branch of each criterion.
Whole-process termination at every journal boundary and external edits between
ref publication and checkout publication require further stress coverage.

Live GitHub smoke created draft PRs #1 and #2 in dbalatero/stack-test-repo,
verified retry reuse without duplicate PRs, and completed local-only sync.
Real terminal smoke verified fuzzy selection, cancellation and restoration.

Chrome script compilation passed. Live smoke exposed a tab delimiter bug;
the code was corrected with a regression test. Further browser interaction
was stopped at the user's request, so the corrected focus behavior has not
been verified live. Two previously opened smoke-test tabs were left alone.

Additional recovery coverage:

- transaction_recovery_before_and_after_refs exercises continue and abort
  from persisted journals both before and after ref publication.
- interrupted_transaction_rejects_external_ref_update preserves changes
  made outside the interrupted operation.
- reconciliation_conflict_can_continue_or_abort_entire_operation exercises
  a conflicting fetched-remote/local reconciliation, on-disk continuation,
  the normal stack cascade afterward, and complete abort restoration.

These deterministic journal fixtures test recovery boundary states. They do
not constitute exhaustive kill-process fault injection at every filesystem
instruction.
