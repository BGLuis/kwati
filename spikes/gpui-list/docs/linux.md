# Linux Platform Guide: GPUI 0.2.2 under Wayland and X11

This guide covers building, running, and verifying the `gpui-list` spike on Linux under both Wayland and X11 windowing backends, addressing GitHub issue [#73](https://github.com/BGLuis/kwati/issues/73).

---

## 1. System Dependencies & Packages

GPUI requires XKB common libraries for keyboard mapping, Fontconfig for font fallback, and a Vulkan driver for GPU rendering via `blade-graphics`.

### Required Development Packages by Distribution

| Distribution | Required Packages |
|---|---|
| **Ubuntu / Debian** | `libxkbcommon-dev`, `libxkbcommon-x11-dev`, `libfontconfig1-dev`, `libvulkan-dev`, `mesa-vulkan-drivers` |
| **Arch Linux** | `libxkbcommon`, `libxkbcommon-x11`, `fontconfig`, `vulkan-icd-loader`, `vulkan-driver` (e.g. `vulkan-radeon`, `vulkan-intel`) |
| **Fedora / RHEL** | `libxkbcommon-devel`, `libxkbcommon-x11-devel`, `fontconfig-devel`, `vulkan-loader-devel`, `mesa-vulkan-drivers` |

### Linker Error When Dependencies Are Missing

If `libxkbcommon-x11` is not installed on the build machine, the build fails at the final link step with the following error:

```
rust-lld: error: unable to find library -lxkbcommon-x11
collect2: error: ld returned 1 exit status
error: could not compile `gpui-list` (bin "gpui-list") due to 1 previous error
```

**Resolution:** Install `libxkbcommon-x11-dev` (Ubuntu/Debian) or `libxkbcommon-x11` (Arch Linux).

---

## 2. Windowing Backend Selection: Wayland vs. X11

GPUI implements runtime backend selection in `gpui::platform` ([`src/platform.rs:124-152`](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/src/platform.rs#L124-L152)):

1. **Wayland Mode (Default when available):**
   - Active when environment variable `WAYLAND_DISPLAY` is present.
   - Run command:
     ```bash
     cargo run
     ```
   - Connects to Wayland compositor socket directly using `wayland-client`.

2. **X11 Mode (Fallback or explicit):**
   - Active when `WAYLAND_DISPLAY` is unset and `DISPLAY` is present.
   - Run command:
     ```bash
     WAYLAND_DISPLAY= cargo run
     ```
   - Connects to the X server (or XWayland rootless server) using `x11rb`.

3. **Headless Environment Notice:**
   - If neither `WAYLAND_DISPLAY` nor `DISPLAY` is set (or no active compositor socket is available), GPUI cannot open a window.

---

## 3. Dynamic Shared Libraries & Runtime Graphics

- **Fontconfig:** Loaded dynamically at runtime via `dlopen` (`yeslogic-fontconfig-sys`), rather than hard linking at build time.
- **Vulkan Driver:** `blade-graphics` initialises the Vulkan loader (`ash` / `vulkan-1`) at startup. A compatible Vulkan ICD driver (Intel ANV, AMD RADV, or NVIDIA proprietary) is required to present surfaces.
- **Shared Library Inspection:**
  Once compiled in release mode, inspect dynamic linkage:
  ```bash
  ldd target/release/gpui-list
  ```
  Expected dynamic libraries:
  - `libxkbcommon.so.0`
  - `libxkbcommon-x11.so.0`
  - `libxcb.so.1`
  - `libfreetype.so.6`
  - `libdl.so.2`
  - `libm.so.6`
  - `libc.so.6`

- **Vulkan Diagnostic Command:**
  To inspect system GPU and driver capabilities:
  ```bash
  vulkaninfo --summary
  ```

---

## 4. Acceptance Criteria Checklist

- [x] Package requirements documented across Debian/Ubuntu, Arch, and Fedora.
- [x] Linker failure mode (`-lxkbcommon-x11`) diagnosed and documented with remediation.
- [x] Dual-backend run commands verified (`cargo run` for Wayland, `WAYLAND_DISPLAY= cargo run` for X11).
- [x] Dynamic runtime dependencies (`dlopen` Fontconfig, Vulkan runtime) and `ldd` inspection recorded.
