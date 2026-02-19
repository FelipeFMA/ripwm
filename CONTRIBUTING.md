# Contributing

Thanks for your interest in improving `ripwm`.

## Ground rules

- Keep changes focused and minimal.
- Prefer explicit error handling over panic paths.
- Avoid behavior changes unless the PR clearly documents them.
- Match existing style and file organization.

## Development workflow

1. Create a branch from `main`.
2. Implement your change with clear commits.
3. Run checks locally:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features
cargo check
```

4. Open a PR with:
   - What changed
   - Why it changed
   - How it was validated

## PR checklist

- [ ] Builds successfully
- [ ] Passes formatting and clippy checks
- [ ] No new panic paths introduced in runtime code
- [ ] Error paths are logged with actionable messages
- [ ] Docs updated if behavior changed
