//! Bookkeeping for grid thumbnails: which pictures are ready, which are queued, and which are
//! being built in a dedicated low-concurrency worker pool outside GPUI's shared executor.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::SystemTime;

use futures::channel::oneshot;
use gpui::RenderImage;
use image::{Frame, RgbaImage};
use kwati_core::{Entry, Location, Settings};

/// Ready pictures kept in memory; older ones are evicted first.
pub const MAX_READY_THUMBNAILS: usize = 512;
/// Failed keys kept in memory so broken files are not retried every frame; older ones evict first.
pub const MAX_FAILED_THUMBNAILS: usize = 512;
/// Requests waiting for a worker. Scrolling past many tiles drops the oldest requests, so work
/// goes to what is on screen now.
pub const MAX_QUEUED_THUMBNAILS: usize = 128;
/// Thumbnails built at the same time in the dedicated worker pool.
pub const MAX_THUMBNAIL_JOBS: usize = 2;

type PoolWork = Box<dyn FnOnce() -> Option<Arc<RenderImage>> + Send>;

struct PoolTask {
    generation: Option<(Arc<AtomicU64>, u64)>,
    work: PoolWork,
    reply: oneshot::Sender<Option<Arc<RenderImage>>>,
}

/// Dedicated bounded OS thread pool for CPU-heavy image/SVG/PDF decoding so GPUI's shared
/// `background_executor` stays free for directory listings, file operations, and text previews.
pub struct ThumbnailWorkerPool {
    state: Arc<(Mutex<Vec<PoolTask>>, Condvar)>,
}

impl ThumbnailWorkerPool {
    fn new(workers: usize) -> Self {
        let state = Arc::new((Mutex::new(Vec::<PoolTask>::new()), Condvar::new()));
        for ix in 0..workers.max(1) {
            let shared = Arc::clone(&state);
            let _ = thread::Builder::new()
                .name(format!("kwati-thumbnail-{ix}"))
                .spawn(move || loop {
                    let task = {
                        let (lock, cvar) = &*shared;
                        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while queue.is_empty() {
                            queue = cvar.wait(queue).unwrap_or_else(|e| e.into_inner());
                        }
                        // LIFO: newest visible tiles and active previews run before older ones.
                        queue.pop()
                    };
                    let Some(task) = task else {
                        continue;
                    };
                    let stale = task
                        .generation
                        .as_ref()
                        .is_some_and(|(gen, expected)| gen.load(Ordering::Relaxed) != *expected);
                    let result = if stale { None } else { (task.work)() };
                    let _ = task.reply.send(result);
                });
        }
        Self { state }
    }

    /// Submits a decode job to the dedicated pool. If `generation` mismatches when picked up, the
    /// work closure is skipped and `None` is returned immediately.
    pub fn submit(
        &self,
        generation: Option<(Arc<AtomicU64>, u64)>,
        work: impl FnOnce() -> Option<Arc<RenderImage>> + Send + 'static,
    ) -> oneshot::Receiver<Option<Arc<RenderImage>>> {
        let (tx, rx) = oneshot::channel();
        let (lock, cvar) = &*self.state;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() >= MAX_QUEUED_THUMBNAILS {
            let dropped = queue.remove(0);
            let _ = dropped.reply.send(None);
        }
        queue.push(PoolTask {
            generation,
            work: Box::new(work),
            reply: tx,
        });
        cvar.notify_one();
        rx
    }
}

/// Shared bounded worker pool for thumbnails and preview image decoding.
pub fn global_thumbnail_pool() -> &'static ThumbnailWorkerPool {
    static POOL: OnceLock<ThumbnailWorkerPool> = OnceLock::new();
    POOL.get_or_init(|| ThumbnailWorkerPool::new(MAX_THUMBNAIL_JOBS))
}

/// Identifies one version of one file: a new modification time or size is a different picture.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub location: Location,
    pub modified: Option<SystemTime>,
    pub size: Option<u64>,
}

impl ThumbnailKey {
    pub fn for_entry(location: Location, entry: &Entry) -> Self {
        Self {
            location,
            modified: entry.modified,
            size: entry.size,
        }
    }
}

/// The settings a cached picture depends on; any change invalidates the whole cache.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ThumbnailPrefs {
    enabled: bool,
    extensions: Vec<String>,
    max_file_size_mb: u32,
    remote_enabled: bool,
    max_px: u32,
}

impl ThumbnailPrefs {
    fn matches(&self, settings: &Settings, max_px: u32) -> bool {
        self.enabled == settings.thumbnails_enabled
            && self.extensions == settings.thumbnail_extensions
            && self.max_file_size_mb == settings.thumbnail_max_file_size_mb
            && self.remote_enabled == settings.thumbnail_remote_enabled
            && self.max_px == max_px
    }

    fn from_settings(settings: &Settings, max_px: u32) -> Self {
        Self {
            enabled: settings.thumbnails_enabled,
            extensions: settings.thumbnail_extensions.clone(),
            max_file_size_mb: settings.thumbnail_max_file_size_mb,
            remote_enabled: settings.thumbnail_remote_enabled,
            max_px,
        }
    }
}

/// A picture to build for the file at `key.location`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailJob {
    pub key: ThumbnailKey,
    pub generation: u64,
}

enum Slot {
    Queued,
    Running,
    Ready(Arc<RenderImage>),
    Failed,
}

/// Thumbnail states keyed by file version, with a bounded queue and bounded ready/failure caches.
#[derive(Default)]
pub struct ThumbnailCache {
    slots: HashMap<ThumbnailKey, Slot>,
    queue: VecDeque<ThumbnailJob>,
    ready_order: VecDeque<ThumbnailKey>,
    failed_order: VecDeque<ThumbnailKey>,
    running: usize,
    generation: u64,
    prefs: ThumbnailPrefs,
    evicted: Vec<Arc<RenderImage>>,
}

impl ThumbnailCache {
    /// Adopts the thumbnail settings, dropping every picture and queued request made under
    /// different ones without losing track of still-running jobs.
    pub fn sync_settings(&mut self, settings: &Settings, max_px: u32) {
        if self.prefs.matches(settings, max_px) {
            return;
        }
        self.prefs = ThumbnailPrefs::from_settings(settings, max_px);
        self.generation = self.generation.wrapping_add(1);
        self.queue.clear();
        self.ready_order.clear();
        self.failed_order.clear();
        for (_, slot) in self.slots.drain() {
            if let Slot::Ready(image) = slot {
                self.evicted.push(image);
            }
        }
    }

    /// Cancels pending queued requests and marks active jobs as stale (e.g. on navigation or
    /// leaving Grid mode) while keeping already-decoded thumbnails and failure entries cached.
    pub fn cancel_pending(&mut self) {
        if self.queue.is_empty() && self.running == 0 {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.queue.clear();
        self.slots
            .retain(|_, slot| matches!(slot, Slot::Ready(_) | Slot::Failed));
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn in_flight_count(&self) -> usize {
        self.running
    }

    pub fn max_px(&self) -> u32 {
        self.prefs.max_px
    }

    /// The picture for `key` when it is ready; otherwise queues it once and returns `None`.
    pub fn get_or_request(&mut self, key: &ThumbnailKey) -> Option<Arc<RenderImage>> {
        match self.slots.get(key) {
            Some(Slot::Ready(image)) => return Some(image.clone()),
            Some(_) => return None,
            None => {}
        }
        if self.queue.len() >= MAX_QUEUED_THUMBNAILS {
            if let Some(stale) = self.queue.pop_front() {
                self.slots.remove(&stale.key);
            }
        }
        self.slots.insert(key.clone(), Slot::Queued);
        self.queue.push_back(ThumbnailJob {
            key: key.clone(),
            generation: self.generation,
        });
        None
    }

    /// Hands out queued jobs, newest first, while fewer than [`MAX_THUMBNAIL_JOBS`] run.
    pub fn start_jobs(&mut self) -> Vec<ThumbnailJob> {
        let mut started = Vec::new();
        while self.running < MAX_THUMBNAIL_JOBS {
            let Some(job) = self.queue.pop_back() else {
                break;
            };
            self.slots.insert(job.key.clone(), Slot::Running);
            self.running += 1;
            started.push(job);
        }
        started
    }

    /// Records a job's outcome. Always releases the worker slot; returns `true` when the result
    /// belongs to the current generation and updated the cache.
    pub fn finish(&mut self, job: &ThumbnailJob, image: Option<Arc<RenderImage>>) -> bool {
        self.running = self.running.saturating_sub(1);
        if job.generation != self.generation {
            return false;
        }
        let Some(image) = image else {
            self.slots.insert(job.key.clone(), Slot::Failed);
            self.failed_order.push_back(job.key.clone());
            while self.failed_order.len() > MAX_FAILED_THUMBNAILS {
                let Some(old) = self.failed_order.pop_front() else {
                    break;
                };
                if matches!(self.slots.get(&old), Some(Slot::Failed)) {
                    self.slots.remove(&old);
                }
            }
            return true;
        };
        self.slots.insert(job.key.clone(), Slot::Ready(image));
        self.ready_order.push_back(job.key.clone());
        while self.ready_order.len() > MAX_READY_THUMBNAILS {
            let Some(old) = self.ready_order.pop_front() else {
                break;
            };
            if let Some(Slot::Ready(image)) = self.slots.remove(&old) {
                self.evicted.push(image);
            }
        }
        true
    }

    /// Pictures no longer cached, for the caller to release from the GPU atlas.
    pub fn take_evicted(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.evicted)
    }

    pub fn ready_count(&self) -> usize {
        self.ready_order.len()
    }

    pub fn queued_count(&self) -> usize {
        self.queue.len()
    }
}

/// Wraps an RGBA picture for gpui, which draws raster images from BGRA pixels.
pub fn render_image_from_rgba(mut rgba: RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new([Frame::new(rgba)]))
}
