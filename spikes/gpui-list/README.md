# GPUI 0.2.2 List Spike (`spikes/gpui-list`)

This standalone spike crate evaluates whether `gpui = "=0.2.2"` can back the cross-platform file explorer for `kwati` across Linux, Windows, and macOS.

This spike addresses parent issue [#2](https://github.com/BGLuis/kwati/issues/2) and sub-issues [#72](https://github.com/BGLuis/kwati/issues/72), [#73](https://github.com/BGLuis/kwati/issues/73), [#74](https://github.com/BGLuis/kwati/issues/74), and [#75](https://github.com/BGLuis/kwati/issues/75).

---

## 1. Structure

- **Standalone Workspace:** Located in `spikes/gpui-list/`, outside the main workspace root (`Cargo.toml`). It defines its own `[workspace]` and maintains an isolated, committed `Cargo.lock`. This guarantees that root operations (`cargo check --workspace`, `cargo test --workspace`) remain fast and headless.
- **Example Implementation:** `src/main.rs` is adapted from upstream Zed's [`uniform_list.rs`](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/examples/uniform_list.rs) under Apache-2.0. It renders a 50-item scrollable list inside a window, printing `clicked Item <id>` on click.
- **CI Matrix:** `.github/workflows/spike-gpui.yml` builds debug and release binaries across `ubuntu-latest`, `windows-latest`, and `macos-latest` runners with `--locked`, uploading the release binaries as artifacts.

---

## 2. Dependency Notes & Lockfile Resolution

- **`libc` Pinning (`0.2.186`):**
  `zed-async-tar v0.5.0-zed` pulls `xattr v0.2.3`. When `libc >= 0.2.191` is resolved, `xattr` fails to compile with:
  ```
  error[E0425]: cannot find value `ENOATTR` in crate `::libc`
  ```
  `spikes/gpui-list/Cargo.lock` pins `libc` to `0.2.186`, ensuring clean compilation across all environments.

---

## 3. Platform Guides

Detailed platform guides, requirements, and findings are documented in:
- [Linux (Wayland & X11)](docs/linux.md) - Issue #73
- [Windows (DirectX & HLSL)](docs/windows.md) - Issue #74
- [macOS (Metal Shaders & Fallback)](docs/macos.md) - Issue #75

---

## 4. Building Locally

From within `spikes/gpui-list/`:

```bash
# Debug build
cargo build --locked

# Release build
cargo build --release --locked

# Run (Wayland on Linux)
cargo run

# Run (X11 on Linux)
WAYLAND_DISPLAY= cargo run
```
