# macOS Platform Guide: GPUI 0.2.2 Metal Shaders and Runtime Fallback

This guide documents building, running, and configuring the `gpui-list` spike on macOS with Metal shaders, addressing GitHub issue [#75](https://github.com/BGLuis/kwati/issues/75).

---

## 1. Metal Shader Toolchain Requirements

On macOS, GPUI uses Apple's Metal graphics framework for accelerated rendering.

### Build-Time Metal Compilation (`build.rs:195-246`)

By default, GPUI compiles its Metal shading language sources (`.metal`) at build time:
1. `xcrun -sdk macosx metal -mmacosx-version-min=10.15.7 ...` compiles shader sources into Metal air modules.
2. `xcrun -sdk macosx metallib ...` links them into a pre-compiled `.metallib` binary archive embedded in the application.

### Full Xcode vs. Command Line Tools

Standard Command Line Tools (`xcode-select --install`) often omit the full Metal toolchain SDK. Building without full Xcode configured causes `build.rs` to fail with:

```
metal shader compilation failed: xcrun: error: unable to find utility "metal"
```
or
```
metallib compilation failed: xcrun: error: unable to find utility "metallib"
```

**Resolution:**
1. Install Xcode from the Mac App Store or Apple Developer downloads.
2. Select the Xcode developer directory:
   ```bash
   sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
   ```
3. Verify that the Metal compiler is active:
   ```bash
   xcode-select -p
   xcrun -sdk macosx -f metal
   ```

---

## 2. Fallback: `runtime_shaders` Feature

If a machine does not have the full Xcode IDE installed (e.g. CI environments with minimal tools or lightweight developer machines), GPUI provides a fallback feature flag:

- **Feature Flag:** `gpui/runtime_shaders` (`Cargo.toml:32`, `build.rs:177-178`).
- When this feature is active, build-time Metal compilation is skipped entirely. Instead, GPUI compiles the Metal shader source strings into the Metal pipeline at runtime using macOS Metal framework APIs.
- **Run command with fallback:**
  ```bash
  cargo run --features gpui/runtime_shaders
  ```

---

## 3. GitHub Actions CI Configuration

In `.github/workflows/spike-gpui.yml`, macOS jobs run on `macos-latest` (Apple Silicon M-series runners). The workflow selects the default Xcode developer path before invoking `cargo build --locked` and `cargo build --release --locked`.

---

## 4. Local Verification Commands

On a macOS system:

```bash
# 1. System and Xcode Info
sw_vers -productVersion
uname -m
xcodebuild -version
xcode-select -p
xcrun -sdk macosx -f metal

# 2. Build and Run (Default build-time Metal shaders)
cargo run

# 3. Build and Run (Fallback runtime Metal shaders)
cargo run --features gpui/runtime_shaders

# 4. Release Build
cargo build --release --locked
```
