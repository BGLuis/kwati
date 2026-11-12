# Windows Platform Guide: GPUI 0.2.2 DirectX and Shader Compiler Requirements

This guide documents building, running, and configuring the `gpui-list` spike on Windows, addressing GitHub issue [#74](https://github.com/BGLuis/kwati/issues/74).

---

## 1. Overview of Windows Support

While the upstream GPUI README highlights macOS and Linux, GPUI contains a native Windows backend (`src/platform/windows.rs`) targeting DirectX 11/12 via `blade-graphics` and DirectComposition.

---

## 2. Debug vs. Release Shader Compilation

The compilation pipeline for Windows differs substantially between build profiles:

| Profile | Shader Compilation Mechanism | Source Reference |
|---|---|---|
| **Debug (`cargo build`)** | Shaders are compiled **at runtime** via D3DCompile when creating DirectX pipeline state. | `src/platform/windows/directx_renderer.rs:1491` |
| **Release (`cargo build --release`)** | HLSL shaders are compiled **at build time** using the Microsoft DirectX Shader Compiler (`fxc.exe`). | `build.rs:257-264` |

### `fxc.exe` Search Precedence in GPUI `build.rs`

When building in release mode, GPUI searches for `fxc.exe` in the following order (`build.rs:329-356`):
1. **Environment Variable `GPUI_FXC_PATH`**: Explicit path to `fxc.exe`.
2. **System `PATH`**: Discovered via `which::which("fxc.exe")`.
3. **Hardcoded Windows SDK Path**: `C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe`.

If none of these locations contain `fxc.exe`, `build.rs` panics with:
```
panic!("Failed to find fxc.exe! Set GPUI_FXC_PATH or install the Windows SDK.")
```

---

## 3. Toolchain & Prerequisite Setup

To build and run on Windows 10 or 11:
1. **Visual Studio C++ Build Tools (MSVC):**
   Install the "Desktop development with C++" workload, ensuring the **Windows 10/11 SDK** is checked.
2. **Locating `fxc.exe`:**
   In PowerShell or Command Prompt, check if `fxc.exe` is in `PATH`:
   ```powershell
   where.exe fxc.exe
   ```
   If not found in `PATH`, locate your installed Windows SDK:
   ```powershell
   Get-ChildItem -Path "C:\Program Files (x86)\Windows Kits\10\bin" -Filter "fxc.exe" -Recurse | Where-Object { $_.FullName -like "*x64*" }
   ```
3. **Setting `GPUI_FXC_PATH`:**
   ```powershell
   $env:GPUI_FXC_PATH = "C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64\fxc.exe"
   ```

---

## 4. Resource File (`windows-manifest`)

GPUI includes a default Cargo feature `windows-manifest` (`build.rs:268-275`), which compiles Windows resources (`gpui.manifest`, application icons, and high-DPI awareness declarations) using `embed_resource`. This ensures correct high-DPI scaling on Windows displays without blurry text.

---

## 5. CI Workflow Integration

The GitHub Actions workflow (`.github/workflows/spike-gpui.yml`) on `windows-latest` automatically discovers `fxc.exe` from the installed Windows Kits directory and sets `GPUI_FXC_PATH` in `$env:GITHUB_ENV`, allowing both `cargo build --locked` and `cargo build --release --locked` to pass out-of-the-box.

---

## 6. Verification Steps on Windows

```powershell
# 1. Verify Debug
cargo run

# 2. Verify Release
cargo run --release

# 3. Check Windows build and GPU info
winver
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion
```
