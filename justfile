# proxenos task runner

# Format, lint, and test — the full local gate
default: check

# Everything CI enforces
check: fmt-check lint test

# Run the test suite. All features, so the estimator comparison in
# docs/proxy-behavior.md §6.3 is part of the gate rather than an aside.
test:
    cargo nextest run --locked --all-features --status-level fail --final-status-level fail --failure-output final --success-output never

# Run one test filter, e.g. `just test-one incremental`
test-one filter:
    cargo nextest run --locked "{{filter}}" --status-level fail --final-status-level fail --failure-output final

# Lint with warnings as errors
lint:
    cargo clippy --all-targets --all-features --locked -- -D warnings

# Verify formatting
fmt-check:
    cargo fmt --all --check

# Apply formatting
fmt:
    cargo fmt --all

# Review pending snapshot changes
snapshots:
    cargo insta review

# Run the daemon locally with verbose logging
run *ARGS:
    RUST_LOG=proxenos=debug cargo run -p proxenos -- run {{ARGS}}

# Capture upstream exchanges as test fixtures
record *ARGS:
    cargo run -p proxenos -- record {{ARGS}}

# Probe backend capabilities from the fixture corpus; --live spends real inference quota
doctor *ARGS:
    cargo run -p proxenos -- doctor {{ARGS}}

# Install development tooling
setup:
    mise install
    # `.tool-versions` pins the version but cannot carry components, and
    # `just check` is mostly rustfmt and clippy.
    rustup component add rustfmt clippy
    cargo install cargo-nextest --locked
    cargo install cargo-insta --locked

# Build optimized binaries
build:
    cargo build --release --locked

# macOS builds natively and Linux through cargo-zigbuild. Windows needs MSVC,
# so only the release workflow builds it.
# Package the release archives into target/dist, as the release workflow does
dist:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = Darwin ] || { echo "dist builds the macOS targets natively, so it runs on macOS" >&2; exit 1; }
    command -v cargo-zigbuild >/dev/null || { echo "dist needs cargo-zigbuild: cargo install cargo-zigbuild --locked" >&2; exit 1; }
    targets=(aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)
    rustup target add "${targets[@]}"
    cargo build --release --locked --target aarch64-apple-darwin --target x86_64-apple-darwin
    # The oldest glibc the Linux binaries have been run against.
    cargo zigbuild --release --locked --target x86_64-unknown-linux-gnu.2.28 --target aarch64-unknown-linux-gnu.2.28
    rm -rf target/dist
    mkdir -p target/dist
    for target in "${targets[@]}"; do
        name="proxenos-$target"
        mkdir "target/dist/$name"
        cp "target/$target/release/proxenos" README.md LICENSE "target/dist/$name/"
        # Files copied on macOS carry xattrs such as com.apple.provenance, which
        # bsdtar writes as pax headers that GNU tar warns about on every file.
        (cd target/dist && COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$name.tar.gz" "$name")
    done
    (cd target/dist && shasum -a 256 *.tar.gz > SHA256SUMS)
    ls -l target/dist

# Refuse anything but a clean checkout of a pushed tag matching Cargo.toml
release-ready:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
    [ -z "$(git status --porcelain)" ] || { echo "the working tree has changes; a release is built from its tag alone" >&2; exit 1; }
    tag="$(git describe --exact-match --tags HEAD 2>/dev/null)" || { echo "HEAD is not tagged; tag it v$version and push the tag first" >&2; exit 1; }
    [ "$tag" = "v$version" ] || { echo "HEAD is tagged $tag, but Cargo.toml says $version" >&2; exit 1; }
    git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null || { echo "$tag is not on origin; push the tag first" >&2; exit 1; }

# Publish the tagged release from here: gate, archives, then the GitHub release
release: release-ready check dist
    #!/usr/bin/env bash
    set -euo pipefail
    tag="$(git describe --exact-match --tags HEAD)"
    gh release create "$tag" target/dist/*.tar.gz target/dist/SHA256SUMS --verify-tag --title "$tag" --notes-file .github/release-notes.md --generate-notes
