# Contributing

## Development

Build from source with Rust and Cargo:

```bash
cargo build --release --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Unit tests live in the test-only module at the bottom of `src/main.rs`.
`tests/lazy_start.rs` exercises the binary with a fake Docker socket and fake
systemctl. It checks inactive Docker stays asleep, idle Docker is not polled,
queued events share a refresh, and a broken event stream recovers. Real Docker
is never queried by the tests. The process test requires `127.0.0.1:5354` to be
free; stop an installed plop-dns instance before running it.

## Releases

The GitHub workflow builds and tests both architectures on native runners, using
musl for static binaries. Branch pushes, pull requests, and manual workflow runs
produce downloadable Actions artifacts. No local ARM compiler is needed.

To release, set the version in `Cargo.toml`, update `Cargo.lock` with `cargo check`,
commit the changes, and push the matching tag:

```bash
git tag 0.1.0
git push origin 0.1.0
```

Only tags matching the package version create a draft GitHub release. After both
builds and tests pass, it contains both archives and `SHA256SUMS`. Review the draft
and publish it from GitHub Releases. No personal access token is required by the
workflow; it uses GitHub's repository token.

To package an already built static binary locally:

```bash
cargo build --release --locked --target x86_64-unknown-linux-musl
./scripts/package.sh
```

This requires the target's Rust standard library and a suitable musl linker.
Packaging defaults to the current machine's architecture (`uname -m`), or accepts
`x86_64` or `aarch64` as an argument. Archives are written under `target/dist/`. The ARM64 target is
`aarch64-unknown-linux-musl`; package it with `./scripts/package.sh aarch64`.
