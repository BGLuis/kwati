set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

default:
    @just --list

# Build the whole workspace
build:
    cargo build --workspace --locked

# Run the GUI (`just run --release`)
run *args:
    cargo run -p kwati-gui --bin kwati --locked {{args}}

# Run the workspace tests; extra args go to cargo test
test *args:
    cargo test --workspace --locked {{args}}

# SFTP tests against a real SSH server; needs the KWATI_SFTP_TEST_* variables described in crates/kwati-core/tests/sftp.rs
test-remote *args:
    cargo test -p kwati-core --features remote-tests --locked sftp_ {{args}}

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets --locked

# Same steps as the ci workflow
ci: build test

# Build the GPUI spike (separate Cargo project, not a workspace member)
spike *args:
    cd spikes/gpui-list && cargo build --locked {{args}}
