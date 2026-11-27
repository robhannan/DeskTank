//! GPU frame time from timestamp queries, for `desktank bench`.

use core::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Gpu;

/// Two timestamps around the render pass (or the first and last of a
/// frame's passes), resolved and read back. Reading
/// blocks on the GPU, so only benchmarks use it.
#[derive(Debug)]
pub struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
}

const BYTES: u64 = 16;

impl GpuTimer {
    /// A timer, if the device has timestamp queries.
    #[must_use]
    pub fn new(gpu: &Gpu) -> Option<Self> {
        if !gpu.has_timestamps() {
            return None;
        }
        let d = &gpu.device;
        Some(Self {
            queries: d.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("frame timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp resolve"),
                size: BYTES,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp readback"),
                size: BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            period_ns: f64::from(gpu.queue.get_timestamp_period()),
        })
    }

    /// Timestamp writes for the render pass.
    #[must_use]
    pub fn pass_writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        }
    }

    /// Timestamp writes for the first of several passes: the start.
    #[must_use]
    pub fn begin_writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: None,
        }
    }

    /// Timestamp writes for the last of several passes: the end.
    #[must_use]
    pub fn end_writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some(1),
        }
    }

    /// Resolves the pass's timestamps into the readback buffer.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, BYTES);
    }

    /// Waits for `submission` and returns the pass's GPU time. `None` if the
    /// readback fails (or the platform can't block, as on the web).
    pub fn read(&self, gpu: &Gpu, submission: wgpu::SubmissionIndex) -> Option<Duration> {
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        self.readback.map_async(wgpu::MapMode::Read, .., move |r| {
            flag.store(r.is_ok(), Ordering::Release);
        });
        let waited = gpu.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(1)),
        });
        let ticks = (waited.is_ok() && done.load(Ordering::Acquire))
            .then(|| self.readback.get_mapped_range(..).ok())
            .flatten()
            .map(|view| {
                let word = |i: usize| {
                    let mut b = [0; 8];
                    b.copy_from_slice(&view[i * 8..i * 8 + 8]);
                    u64::from_le_bytes(b)
                };
                word(1).saturating_sub(word(0))
            });
        // Always unmap (or cancel a pending map), so the next frame's copy
        // into the buffer stays valid even after a failed read.
        self.readback.unmap();
        let ticks = ticks?;
        #[allow(clippy::cast_precision_loss)] // GPU ticks over one frame
        let ns = ticks as f64 * self.period_ns;
        Some(Duration::from_secs_f64(ns / 1e9))
    }
}
