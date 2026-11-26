//! Bookkeeping for grid thumbnails: which pictures are ready, which are queued, and which are
//! being built in the background. Building them is the list view's job.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::SystemTime;

use gpui::RenderImage;
use image::{Frame, RgbaImage};
use kwati_core::{Entry, Location, Settings};

/// Ready pictures kept in memory; older ones are evicted first.
pub const MAX_READY_THUMBNAILS: usize = 512;
/// Requests waiting for a worker. Scrolling past many tiles drops the oldest requests, so work
/// goes to what is on screen now.
pub const MAX_QUEUED_THUMBNAILS: usize = 128;
/// Thumbnails built at the same time.
pub const MAX_THUMBNAIL_JOBS: usize = 4;

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

/// Thumbnail states keyed by file version, with a bounded queue and a bounded set of pictures.
#[derive(Default)]
pub struct ThumbnailCache {
    slots: HashMap<ThumbnailKey, Slot>,
    queue: VecDeque<ThumbnailJob>,
    ready_order: VecDeque<ThumbnailKey>,
    running: usize,
    generation: u64,
    prefs: ThumbnailPrefs,
    evicted: Vec<Arc<RenderImage>>,
}

impl ThumbnailCache {
    /// Adopts the thumbnail settings, dropping every picture and request made under different
    /// ones. `max_px` is the side of the square a picture must fit in, in device pixels.
    pub fn sync_settings(&mut self, settings: &Settings, max_px: u32) {
        if self.prefs.matches(settings, max_px) {
            return;
        }
        self.prefs = ThumbnailPrefs::from_settings(settings, max_px);
        self.generation += 1;
        self.queue.clear();
        self.ready_order.clear();
        self.running = 0;
        for (_, slot) in self.slots.drain() {
            if let Slot::Ready(image) = slot {
                self.evicted.push(image);
            }
        }
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

    /// Records a job's outcome. A result from before the last preference change is discarded.
    pub fn finish(&mut self, job: &ThumbnailJob, image: Option<Arc<RenderImage>>) {
        if job.generation != self.generation {
            return;
        }
        self.running = self.running.saturating_sub(1);
        let Some(image) = image else {
            self.slots.insert(job.key.clone(), Slot::Failed);
            return;
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
