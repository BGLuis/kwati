use std::io::{self, Read, Seek, SeekFrom};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::ReadSeek;

/// Default block size for sequential read-ahead (4 MiB).
pub const DEFAULT_PREFETCH_BLOCK_SIZE: usize = 4 * 1024 * 1024;
/// Smaller initial block fetched on a non-sequential seek miss (1 MiB).
pub const DEFAULT_SEEK_BLOCK_SIZE: usize = 1024 * 1024;

/// A random-access byte source backed by a network or local file handle.
pub trait RangeSource: Send {
    /// Reads up to `buf.len()` bytes starting at `offset`.
    fn read_range(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// Total byte length of the source, if known.
    fn len(&self) -> Option<u64>;

    /// Whether the source is known to be empty.
    fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }
}

/// Adapts any [`ReadSeek`] stream into a [`RangeSource`], avoiding redundant `seek` calls when
/// reads arrive sequentially.
pub struct ReadSeekRangeSource {
    inner: Box<dyn ReadSeek>,
    pos: u64,
    len: Option<u64>,
}

impl ReadSeekRangeSource {
    pub fn new(inner: Box<dyn ReadSeek>, len: Option<u64>) -> Self {
        Self { inner, pos: 0, len }
    }
}

impl RangeSource for ReadSeekRangeSource {
    fn read_range(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let Some(total) = self.len {
            if offset >= total {
                return Ok(0);
            }
        }
        if self.pos != offset {
            self.inner.seek(SeekFrom::Start(offset))?;
            self.pos = offset;
        }
        let mut total_read = 0usize;
        while total_read < buf.len() {
            let n = self.inner.read(&mut buf[total_read..])?;
            if n == 0 {
                break;
            }
            total_read += n;
            self.pos = self.pos.saturating_add(n as u64);
        }
        Ok(total_read)
    }

    fn len(&self) -> Option<u64> {
        self.len
    }
}

#[derive(Clone)]
struct Block {
    start: u64,
    data: Vec<u8>,
}

impl Block {
    fn end(&self) -> u64 {
        self.start.saturating_add(self.data.len() as u64)
    }

    fn contains(&self, offset: u64) -> bool {
        offset >= self.start && offset < self.end()
    }
}

enum WorkerCmd {
    Fetch { offset: u64, size: usize },
    Shutdown,
}

enum PrefetchSlot {
    Idle,
    InFlight { start: u64 },
    Ready(Block),
}

/// Asynchronous double-buffering reader over a [`RangeSource`].
///
/// A dedicated worker thread owns the underlying [`RangeSource`] and pre-fetches the next
/// sequential block while the current block is being read. After a non-sequential seek miss, it
/// fetches a smaller `seek_block_size` and defers speculative read-ahead until the following read
/// confirms sequential access (avoiding wasted bandwidth during rapid seek probes).
pub struct PrefetchReader {
    pos: u64,
    known_len: Option<u64>,
    block_size: usize,
    seek_block_size: usize,
    current: Option<Block>,
    next: PrefetchSlot,
    awaiting_sequential_confirm: bool,
    eof_reached: bool,
    cmd_tx: Option<Sender<WorkerCmd>>,
    resp_rx: Receiver<io::Result<Block>>,
    worker: Option<JoinHandle<()>>,
}

impl PrefetchReader {
    pub fn new(source: Box<dyn RangeSource>) -> Self {
        Self::with_sizes(source, DEFAULT_PREFETCH_BLOCK_SIZE, DEFAULT_SEEK_BLOCK_SIZE)
    }

    pub fn with_sizes(
        mut source: Box<dyn RangeSource>,
        block_size: usize,
        seek_block_size: usize,
    ) -> Self {
        let block_size = block_size.max(1);
        let seek_block_size = seek_block_size.max(1).min(block_size);
        let known_len = source.len();

        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let (resp_tx, resp_rx) = mpsc::channel::<io::Result<Block>>();

        let worker = thread::Builder::new()
            .name("kwati-prefetch-io".to_string())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        WorkerCmd::Fetch { offset, size } => {
                            let mut buf = vec![0u8; size];
                            let res = source.read_range(offset, &mut buf).map(|n| {
                                buf.truncate(n);
                                Block {
                                    start: offset,
                                    data: buf,
                                }
                            });
                            if resp_tx.send(res).is_err() {
                                break;
                            }
                        }
                        WorkerCmd::Shutdown => break,
                    }
                }
            })
            .expect("failed to spawn prefetch worker");

        let mut reader = Self {
            pos: 0,
            known_len,
            block_size,
            seek_block_size,
            current: None,
            next: PrefetchSlot::Idle,
            awaiting_sequential_confirm: false,
            eof_reached: false,
            cmd_tx: Some(cmd_tx),
            resp_rx,
            worker: Some(worker),
        };
        reader.kick_prefetch(0, block_size);
        reader
    }

    pub fn known_len(&self) -> Option<u64> {
        self.known_len
    }

    fn at_known_eof(&self, offset: u64) -> bool {
        self.known_len.is_some_and(|len| offset >= len)
    }

    fn kick_prefetch(&mut self, offset: u64, size: usize) {
        if !matches!(self.next, PrefetchSlot::Idle) || self.at_known_eof(offset) {
            return;
        }
        if let Some(tx) = &self.cmd_tx {
            if tx.send(WorkerCmd::Fetch { offset, size }).is_ok() {
                self.next = PrefetchSlot::InFlight { start: offset };
            }
        }
    }

    fn wait_in_flight(&mut self) -> io::Result<Block> {
        let result = self
            .resp_rx
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "prefetch worker stopped"))?;
        self.next = PrefetchSlot::Idle;
        result
    }

    fn drain_stale_prefetch(&mut self) {
        if matches!(self.next, PrefetchSlot::InFlight { .. }) {
            let _ = self.wait_in_flight();
        }
        self.next = PrefetchSlot::Idle;
    }

    fn poll_ready_prefetch(&mut self) -> io::Result<()> {
        if matches!(self.next, PrefetchSlot::InFlight { .. }) {
            if let Ok(res) = self.resp_rx.try_recv() {
                let block = res?;
                self.next = PrefetchSlot::Ready(block);
            }
        }
        Ok(())
    }

    fn ensure_cache(&mut self) -> io::Result<()> {
        if self.at_known_eof(self.pos) {
            self.eof_reached = true;
            return Ok(());
        }

        self.poll_ready_prefetch()?;

        if let Some(ref cur) = self.current {
            if cur.contains(self.pos) {
                if self.awaiting_sequential_confirm {
                    // Second access within the seek block confirms sequential streaming.
                    self.awaiting_sequential_confirm = false;
                    let next_start = cur.end();
                    self.kick_prefetch(next_start, self.block_size);
                }
                return Ok(());
            }
        }

        // Check whether the next slot holds or is fetching the block covering `self.pos`.
        let next_covers = match &self.next {
            PrefetchSlot::Ready(block) => block.contains(self.pos),
            PrefetchSlot::InFlight { start } => {
                self.pos >= *start && self.pos < start.saturating_add(self.block_size as u64)
            }
            PrefetchSlot::Idle => false,
        };

        if next_covers {
            let block = match std::mem::replace(&mut self.next, PrefetchSlot::Idle) {
                PrefetchSlot::Ready(block) => block,
                PrefetchSlot::InFlight { .. } => {
                    self.next = PrefetchSlot::InFlight { start: 0 };
                    self.wait_in_flight()?
                }
                PrefetchSlot::Idle => unreachable!(),
            };
            if block.data.is_empty() {
                self.eof_reached = true;
                return Ok(());
            }
            let block_end = block.end();
            self.awaiting_sequential_confirm = false;
            self.current = Some(block);
            self.kick_prefetch(block_end, self.block_size);
            return Ok(());
        }

        // Non-sequential seek miss: discard stale prefetch, fetch `seek_block_size` synchronously,
        // and defer speculative prefetch until sequential access is confirmed.
        self.drain_stale_prefetch();
        let tx = self
            .cmd_tx
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "prefetch closed"))?;
        tx.send(WorkerCmd::Fetch {
            offset: self.pos,
            size: self.seek_block_size,
        })
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "prefetch worker stopped"))?;
        self.next = PrefetchSlot::InFlight { start: self.pos };
        let block = self.wait_in_flight()?;
        if block.data.is_empty() {
            self.eof_reached = true;
            return Ok(());
        }
        self.current = Some(block);
        self.awaiting_sequential_confirm = true;
        Ok(())
    }
}

impl Read for PrefetchReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.at_known_eof(self.pos) {
            return Ok(0);
        }

        self.ensure_cache()?;
        if self.eof_reached {
            return Ok(0);
        }

        let Some(ref cur) = self.current else {
            return Ok(0);
        };
        if !cur.contains(self.pos) {
            return Ok(0);
        }

        let inner_off = (self.pos - cur.start) as usize;
        let available = cur.data.len() - inner_off;
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&cur.data[inner_off..inner_off + to_copy]);
        self.pos = self.pos.saturating_add(to_copy as u64);
        Ok(to_copy)
    }
}

impl Seek for PrefetchReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next_pos: i128 = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.pos) + i128::from(offset),
            SeekFrom::End(offset) => {
                let len = self.known_len.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Unsupported, "unknown stream length")
                })?;
                i128::from(len) + i128::from(offset)
            }
        };
        let next = u64::try_from(next_pos)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid seek position"))?;
        self.pos = next;
        self.eof_reached = self.at_known_eof(next);
        Ok(self.pos)
    }
}

impl Drop for PrefetchReader {
    fn drop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(WorkerCmd::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct CountingSource {
        data: Vec<u8>,
        reads: Arc<AtomicUsize>,
        offsets: Arc<Mutex<Vec<(u64, usize)>>>,
    }

    impl RangeSource for CountingSource {
        fn read_range(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.offsets.lock().unwrap().push((offset, buf.len()));
            let start = offset as usize;
            if start >= self.data.len() {
                return Ok(0);
            }
            let n = (self.data.len() - start).min(buf.len());
            buf[..n].copy_from_slice(&self.data[start..start + n]);
            Ok(n)
        }

        fn len(&self) -> Option<u64> {
            Some(self.data.len() as u64)
        }
    }

    #[test]
    fn sequential_read_and_seek_use_prefetch_and_deferred_speculation() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let reads = Arc::new(AtomicUsize::new(0));
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let source = Box::new(CountingSource {
            data: data.clone(),
            reads: Arc::clone(&reads),
            offsets: Arc::clone(&offsets),
        });

        // block_size=200, seek_block_size=100
        let mut reader = PrefetchReader::with_sizes(source, 200, 100);

        // Sequential read of first 350 bytes
        let mut head = vec![0u8; 350];
        reader.read_exact(&mut head).unwrap();
        assert_eq!(head, &data[..350]);

        // Seek far forward (non-sequential miss): uses seek_block_size=100 and does NOT
        // immediately trigger speculative prefetch until a second read confirms sequentiality.
        reader.seek(SeekFrom::Start(700)).unwrap();
        let mut one = [0u8; 20];
        reader.read_exact(&mut one).unwrap();
        assert_eq!(&one, &data[700..720]);

        // Give worker a moment if any speculative fetch had been sent (none should have!).
        std::thread::sleep(std::time::Duration::from_millis(20));
        let recorded = offsets.lock().unwrap().clone();
        assert_eq!(recorded.last().copied(), Some((700, 100)));

        // Second read inside seek block confirms sequential access -> triggers prefetch at 800
        let mut two = [0u8; 80];
        reader.read_exact(&mut two).unwrap();
        assert_eq!(&two, &data[720..800]);

        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, &data[800..]);
    }
}
