# Kwatí Engineering & Performance Guardrails (`AGENTS.md`)

> **Precedence:** This file is the canonical agent instruction file for the Kwatí repository. Keep changes minimal, follow [DESIGN.md](DESIGN.md) for visual/UX rules, and use `justfile` commands (`just build`, `just test`, `just clippy`, `just fmt-check`, `just run --release`).

## Project Context

- **Workspace:** `crates/kwati-core` (filesystem, remote SFTP/SMB/FTP backends, preview, thumbnailing, settings, undo) and `crates/kwati-gui` (GPUI `=0.2.2` UI with Blade GPU backend on Linux/Windows).
- **Primary Architecture Rule:** Never block the GPUI UI thread, never saturate GPUI's shared `background_executor` with unbounded CPU/GPU work, and never introduce O(N) or O(K) per-frame work in virtualized views.

---

## 1. Image, SVG, and PDF Thumbnails (Pitfalls Fixed After `4d98297` — Never Regress)

When modifying or extending grid thumbnails (`crates/kwati-core/src/thumbnail.rs`, `crates/kwati-gui/src/components/thumbnails.rs`, `crates/kwati-gui/src/views/list_view.rs`), **preserve these invariants and avoid every pitfall below**:

1. **Unoptimized CPU PDF Rasterizer (`hayro 0.9.0` + `vello_cpu`) Without Bounds**
   - *Cause:* `hayro` explicitly states it has no performance optimizations yet. Unbounded `fs::read` + `hayro::render` on complex vectors, shadings, or giant page streams can pin CPU cores for seconds to minutes per file.
   - *Implemented Guardrail & Rule:* `pdf_thumbnail` (`crates/kwati-core/src/thumbnail.rs`) enforces `MAX_PDF_BYTES = 32 MiB` before `fs::read`, positive finite page dimensions, and `MAX_PDF_PAGE_STREAM_BYTES = 4 MiB` before calling `hayro::render`. Never decode or rasterize untrusted PDFs without strict memory, dimension, and complexity caps on the dedicated low-concurrency pool.

2. **Unbounded SVG CPU Rasterization & Missing Zero-Size Guard**
   - *Cause:* Unbounded `fs::read` + `usvg`/`resvg` without node/filter caps, and dividing `max_px / size.width().max(size.height())` without verifying `width > 0.0 && height > 0.0`.
   - *Implemented Guardrail & Rule:* `svg_thumbnail` (`crates/kwati-core/src/thumbnail.rs`) validates `MAX_SVG_BYTES = 4 MiB` before reading, positive finite SVG dimensions before scaling, and `validate_svg_complexity` (`MAX_SVG_NODES = 10_000`, `MAX_SVG_FILTERS = 32`), clamping output dimensions to `1..=max_px`. Always preserve positive finite checks and complexity caps.

3. **Full-Resolution Raster Decoding & Concurrent RAM Spikes**
   - *Cause:* Decoding full-res rasters with `MAX_DECODE_ALLOC = 256 MiB` across `4` workers spiked RAM to **1 GiB+**, causing allocator contention and swapping.
   - *Implemented Guardrail & Rule:* `raster_image_bounded` enforces `MAX_DECODE_ALLOC = 96 MiB` and `MAX_DECODE_DIMENSION = 16_384`, and `MAX_THUMBNAIL_JOBS = 2` keeps total concurrent decode memory bounded (`<= 192 MiB`). Keep decode concurrency conservatively bounded and always set `Limits::max_alloc` and dimension caps.

4. **Starving GPUI's Shared `background_executor` & Uncancelled In-Flight Jobs**
   - *Cause:* GPUI's `LinuxDispatcher` uses one shared thread pool (`available_parallelism()` threads) for directory listings (`state.rs`), previews (`preview_view.rs`), icon probes (`list_view.rs`), volume polling (`sidebar_view.rs`), and file operations (`state.rs`). Running heavy decode tasks on `background_executor` starved navigation and previews; additionally, `ThumbnailCache::sync_settings` previously reset `self.running = 0` without cancelling active tasks.
   - *Implemented Guardrail & Rule:* Heavy image/SVG/PDF decoding now runs on `ThumbnailWorkerPool` (`global_thumbnail_pool()` in `crates/kwati-gui/src/components/thumbnails.rs`) with `MAX_THUMBNAIL_JOBS = 2` dedicated LIFO worker threads and generation-token stale-skip checks. `ThumbnailCache::finish` always decrements `running` (even for stale generations), and `sync_settings` never resets `running` to zero. Never move heavy decoding back onto `background_executor` or reset `running` without real worker tracking/cancellation.

5. **Per-Thumbnail `cx.notify()` Render Storms, Stale Queues, & Unbounded Failure Map**
   - *Cause:* Firing `cx.notify()` on every completed thumbnail triggered repeated full `ListView::render` + GPU uploads + `wait_for_gpu()`; navigating away or leaving Grid mode left `thumbnails.queue` running; and `Slot::Failed` entries grew unboundedly.
   - *Implemented Guardrail & Rule:* `ListView` batches completion notifications with `THUMBNAIL_NOTIFY_COALESCE` (`16 ms`, notifying immediately only when the queue and in-flight count hit zero), calls `cancel_pending_thumbnails()` on directory changes (`dir_changed`) and when switching away from `ViewMode::Grid`, and `ThumbnailCache` evicts oldest `Slot::Failed` entries past `MAX_FAILED_THUMBNAILS = 512`.

6. **Unoptimized `dev` Profile Builds for Pure-Rust Rasterizers & Text/Layout Crates**
   - *Cause:* `opt-level = 0` makes pure-Rust decoders (`hayro*`, `vello_cpu`, `resvg`, `tiny-skia`, `image`) and GPUI's text/layout/crypto crates (`cosmic-text`, `rustybuzz`, `ttf-parser`, `swash`, `taffy`, `unicode-bidi`, `unicode-linebreak`, `num-bigint*`) 10×–50× slower during `just run`.
   - *Implemented Guardrail & Rule:* `Cargo.toml` sets `opt-level = 2` for `[profile.dev.package.*]` across these pure-Rust image/SVG/PDF, text shaping/layout, and startup crypto crates. Always benchmark heavy media flows under `--release` (`just run --release`) as well.

7. **`DirWatcher` (`notify 8`) `EventKind::Access(_)` Infinite Reload Loop**
   - *Cause:* `notify::recommended_watcher(...)` enables Linux `inotify` `WatchMask::OPEN`, emitting `EventKind::Access(_)` whenever a directory or file is opened for reading. Every `fs::read_dir` in `AppState::begin_load` and every file read by `kwati-thumbnail` / `PreviewView` / `probe_entry_icon` emitted `EventKind::Access(_)`, triggering a continuous 50 ms reload loop -> `set_entries` -> `listing_revision` bump -> cache wipe (`grid_label_cache`, `row_text_cache`, `entry_icon_cache`).
   - *Implemented Guardrail & Rule:* `DirWatcher` (`crates/kwati-core/src/watcher.rs`) explicitly ignores `EventKind::Access(_)` events (`is_relevant_event`). Additionally, `AppState::finish_load` (`crates/kwati-gui/src/state.rs`) skips `set_entries` (keeping `listing_revision` and `ListView` caches intact) on no-op `WatcherOrPoll` reloads where `entries == *self.entries`.

8. **`fit_two_line_label` Shaping Storm in Grid Mode (`crates/kwati-gui/src/views/list_view.rs`)**
   - *Cause:* Calling `shape_line` + `shape_text` + binary-search `shape_line` per long filename ran ~8–10 full `cosmic_text`/`rustybuzz` OpenType shaping passes per visible tile on the UI thread during initial paint and scroll.
   - *Implemented Guardrail & Rule:* `fit_two_line_label` must use GPUI's cached `window.text_system().line_wrapper(font, font_size)` (`wrap_line` + `truncate_line` with a 1 px safety margin) instead of repeated `shape_line`/`shape_text` calls.

---

## 2. GPUI `img(...)`, Global Asset Cache, and `BladeAtlas` GPU Traps (Preview & VRAM Freezes)

1. **Never Pass Raw User Paths to GPUI `img(path)` in `PreviewView`**
   - *Cause:* GPUI `0.2.2`'s `ImageAssetLoader` decodes raw paths with **no `Limits::max_alloc`**, decodes **all frames** of animated GIF/WebP files, renders SVGs at `2×` while calling `fontdb::Database::load_system_fonts()`, uploads full-resolution BGRA textures via `BladeAtlas::upload_texture` + `wait_for_gpu()`, and retains every loaded file indefinitely in global `App::loading_assets`.
   - *Implemented Guardrail & Rule:* `PreviewView` (`crates/kwati-gui/src/views/preview_view.rs`) decodes and downscales preview images off-thread via `global_thumbnail_pool()` + `generate_preview_image(path, MAX_PREVIEW_IMAGE_PX = 1024)` (first frame only, strict `Limits`, no upscaling of small images), passes `Arc<RenderImage>` to `img(image)`, and explicitly calls `cx.drop_image(old, None)` whenever the preview is replaced, cancelled, or closed. Never pass arbitrary user file paths directly to `img(path)`.

2. **GPUI `0.2.2` `BladeAtlas::remove` GPU Texture Fragmentation**
   - *Cause (`gpui-0.2.2/src/platform/blade/blade_atlas.rs:104-126`):* `BladeAtlas::remove` deletes the key and decrements `live_atlas_keys`, but **never calls `deallocate` on `etagere::BucketedAtlasAllocator`**. A `1024×1024` GPU atlas texture is only destroyed when `live_atlas_keys == 0`—meaning a single long-lived icon or preview sharing an atlas page pins the entire page forever while evicted thumbnails allocate new `1024×1024` GPU textures.
   - *Rule:* Keep cached `RenderImage` dimensions small and bounded, avoid rapid eviction/re-creation churn of `RenderImage` IDs, and do not assume `window.drop_image` / `cx.drop_image` immediately reclaims partial atlas pages in GPUI `0.2.2`.

---

## 3. Synchronous I/O & Subprocesses on the UI Thread

### Fixed UI-Thread Blockers (Keep Asynchronous / Cached)
- **`ContextMenuView::open_at` (`crates/kwati-gui/src/views/context_menu.rs`):** Opens immediately using listing metadata (`entry.is_dir()`) and resolves `menu_applications_for` (XDG MIME / `.desktop` lookups) and `volume_kind` asynchronously on `background_executor` with generation-based cancellation.
- **`Assets::load` (`crates/kwati-gui/src/components/icon.rs`):** Caches `fs-symbolic:` SVG file reads in a bounded `SYMBOLIC_SVG_CACHE` (`256` entries) instead of calling `std::fs::read` on every `paint_svg`.
- **`AppState::update_settings` / `update_icon_config` (`crates/kwati-gui/src/state.rs`):** When attached to the UI (`foreground.is_some()`), commits in-memory state immediately and coalesces TOML writes onto `background_executor`.
- **`ListView::sync_listing_transitions` (`crates/kwati-gui/src/views/list_view.rs`):** Uses cached `self.special_folders` and pure-memory `fast_icon_for_entry` for deleted `exiting_rows` instead of `SpecialFolders::from_system()` and blocking `icon_for_entry`.
- **`PreviewView::render` (`crates/kwati-gui/src/views/preview_view.rs`):** Caches `SpecialFolders` once in `PreviewView::new` and calls `fast_icon_for_entry` instead of `icon_for` (`SpecialFolders::from_system()` + `stat` on every frame).
- **One-time Startup Subprocesses (`tokens.rs:94` `fc-match`, `motion.rs:205, 220` `gsettings`/`defaults`):** Must remain cached once (`OnceLock` / `Fonts` global) and never be invoked per-frame or per-render.

### Remaining Low-Priority Edge Case
- **`PreviewView::render` (`crates/kwati-gui/src/views/preview_view.rs`):** Retains a synchronous `prepare_preview_lines` fallback only if `text_lines_path` is somehow out of sync with `preview_state`; keep `spawn_preview_load` precomputing lines off-thread so the fallback is never hit in normal browsing.

### Historical Freeze Regressions Already Fixed (Never Reintroduce)
- **Sync directory reads on UI thread (`5e9d797`):** Always load directories via `background_executor` with `list_dir_bounded` timeout, and drop/cancel superseded `load_task`s.
- **Sync preview loading & unvirtualized 64 KiB text shaping (`0593e68`):** Always debounce preview loads (`50 ms`), cancel on selection change, skip when preview panel is closed, and render text previews via `uniform_list` split into bounded `MAX_PREVIEW_LINE_CHARS` (`200`) lines.
- **Per-row disk probes & uncached text/icon shaping during scroll (`4b31372`):** Use pure-memory `fast_icon_for_entry` during `render` + batched async `probe_entry_icon`, and cache row strings, wrapped grid labels (`fit_two_line_label`), and resolved icons per `listing_revision`.
- **Per-frame syscalls in `.drag_over(...)` (`ca2578f`):** Use purely lexical `validate_*_drop_fast` and `resolve_drop_operation_for_drag` against cached `AppState::volume_mounts`. Never call `canonicalize()`, `symlink_metadata()`, or OS volume queries inside `drag_over`.
- **Full XDG `.desktop` directory tree scans on right-click (`ca2578f`):** Query `mimeapps.list` + `mimeinfo.cache` rather than walking all application directories, and run `open_at` resolution off the UI thread.

---

## 4. O(N) / O(K) Render & Selection Bottlenecks + GPUI Platform Gotchas

- **O(N) / O(K) traps in large directories (`6fedcee`):**
  - Always use `AppState::entry_index` (`entry_index_of` / `entry_by_path`) for O(1) lookups; never scan `entries.iter().position(...)` per item or per selection operation.
  - Keep selection/cut/drag checks O(1) via `HashSet<Location>` and resolve `DragEntries` lazily on drag start (`OnceLock`) rather than cloning all `K` selected entries for every visible row on every frame.
  - Cache derived status-bar summaries in `MainView` keyed by `(listing_revision, selection_revision, ...)`.
- **GPUI Wayland re-borrow panic (`crates/kwati-gui/src/views/mod.rs:182-186`):** Inside `cx.observe_window_appearance`, always read `window.appearance()` — calling `cx.window_appearance()` re-borrows GPUI's Wayland client and panics.

---

## 5. Pre-Change Checklist for AI Agents

Before merging any GUI, thumbnail, or preview change, verify every item:
- [ ] Zero filesystem `stat`/`read`/`canonicalize`, subprocesses, or network calls on the GPUI UI thread (especially in `render`, `uniform_list`, `on_hover`, `drag_over`, `on_mouse_move`).
- [ ] Heavy CPU work (PDF, SVG, image, video/audio decoding) runs on a dedicated bounded low-priority pool (`global_thumbnail_pool()`), **not** GPUI's shared `background_executor`.
- [ ] Every decoder enforces strict input size, pixel/allocation (`Limits`), dimension (`> 0`), complexity, and cancellation bounds.
- [ ] No raw user file paths passed directly to GPUI `img(path)`; decoded `RenderImage` textures are bounded in size and explicitly dropped (`window.drop_image` / `cx.drop_image`).
- [ ] Pending background queues/tasks are cancelled on directory navigation, view-mode switch, panel close, and settings change; in-flight counters are never reset without real completion/cancellation.
- [ ] Background completion notifications (`cx.notify()`) are batched/coalesced rather than firing once per item.
- [ ] Render paths remain `O(visible)` with cached text shaping, icons, and `O(1)` `HashMap`/`HashSet` lookups.
- [ ] Performance-sensitive changes are tested in `--release` (`just run --release`) on large directories and complex PDF/SVG/image files.
