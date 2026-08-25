# Publishing Trox to crates.io

Releases are made from a clean, reviewed commit on `master`. Publishing is
permanent: never publish from a working tree with uncommitted changes or from a
commit that has not passed CI.

## One-time setup

1. Verify the crates.io account email and create a narrowly scoped API token.
2. For GitHub publishing, create a protected `crates-io` environment and add the
   token as its `CARGO_REGISTRY_TOKEN` secret. Require an environment reviewer
   so the manual workflow cannot publish accidentally.
3. After the first publication, add at least one backup owner for both crates:

   ```sh
   cargo owner --add GITHUB_LOGIN trox
   cargo owner --add GITHUB_LOGIN trox-cli
   ```

Both package names are claimed by the first successful release. Confirm their
availability immediately before publishing.

## Prepare a release

1. Set the same version in the workspace and in `packages/trox/package.json`.
2. Confirm that the `trox-cli` dependency pins that exact `trox` version.
3. Run the checks used by CI:

   ```sh
   cargo fmt --all -- --check
   cargo test --workspace --all-targets
   cargo clippy --workspace --all-targets -- -D warnings
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
   npm ci
   npm run typecheck
   npm test
   npm run build
   cargo package --workspace
   ```

4. Test both extracted packages. `cargo package` verifies compilation but does
   not run the packaged tests:

   ```sh
   cargo test --manifest-path target/package/trox-VERSION/Cargo.toml --all-targets
   cargo test \
     --manifest-path target/package/trox-cli-VERSION/Cargo.toml \
     --config "patch.crates-io.trox.path='$PWD/target/package/trox-VERSION'" \
     --all-targets
   ```

   The temporary patch tests the CLI archive against the library archive that
   will be published first; it does not change either package.

5. Inspect each archive with `cargo package --list -p PACKAGE`. Confirm that the
   README, Apache-2.0 license, required fixtures, and only intended source files
   are present.

## Publish

The crates must be published in dependency order. The manual **Publish crates**
GitHub workflow enforces the same sequence:

```sh
cargo publish -p trox
cargo info trox@VERSION
cargo publish -p trox-cli
```

Wait until `cargo info` resolves the new `trox` version before publishing
`trox-cli`. The CLI archive replaces its local path dependency with the exact
registry dependency during packaging.

## Verify and tag

In fresh temporary projects, run:

```sh
cargo add trox@VERSION
cargo install trox-cli --version VERSION
trox --version
trox --help
```

After both crates pass the smoke test, tag the exact published commit as
`vVERSION` and create the GitHub release notes.
