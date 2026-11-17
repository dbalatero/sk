# Implementation plan

Every behavior uses red, green, refactor. Use 80-column Rust and documentation.

1. Record design and numbered acceptance criteria.
2. Establish shared repository/state/process interfaces and test fixtures.
3. Run three independent workstreams: core Git and recovery; GitHub sync and
   submit; CLI presentation, navigation, editor and Chrome integration.
4. Coordinator integrates shared contracts and dispatch, then cross-reviews.
5. Run formatting, strict Clippy, unit/integration tests and live smoke checks.

## Ownership and dependencies

Core owns Cargo dependencies, domain/state, Git primitives, create, replay,
and recovery. Remote owns API/authentication, sync and submission. UI owns
rendering, fuzzy checkout/navigation and Chrome. Coordinator owns command
entrypoint, README, and final integration. Each fixture gets a unique repo.

All workers depend on Repository, State, Stack, Branch and PullRequest.
Sync integration additionally depends on the core replay transaction.
Shared interface changes must be communicated before dependent edits.

## Completion gates

- Every acceptance item has a named test or explicit remaining limitation.
- cargo fmt --check
- cargo clippy --all-targets -- -D warnings
- cargo test --all-targets
- Integration smoke against the supplied stack-test-repo.
