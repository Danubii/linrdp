//! A bounded snapshot handoff: decoding continues while presentation is busy.
use super::{Display, Mutex, Phase, Session};

impl Display {
    pub(super) fn take_pixels(&mut self, pixels: &mut linrdp_proto::desktop::Snapshot) {
        std::mem::swap(pixels, &mut self.pixels);
        self.pending = false;
    }
}

pub(super) fn publish(
    state: &Session,
    shared: &Mutex<Display>,
    staging: &mut linrdp_proto::desktop::Snapshot,
    updates: &mut u64,
    resize_ready: bool,
) -> bool {
    // Every delta is decoded into Session. Only snapshot production is deferred;
    // no protocol update, cursor state or input transition is discarded.
    if state.revision == *updates
        || state.framebuffer.updates == 0
        || shared.lock().unwrap().pending
    {
        return false;
    }
    // The worker is the sole producer. The UI only consumes, so the empty slot
    // stays available while this full-screen copy runs outside the mutex.
    state.update_snapshot(staging);
    let mut frame = shared.lock().unwrap();
    frame.width = usize::from(state.framebuffer.width);
    frame.height = usize::from(state.framebuffer.height);
    std::mem::swap(&mut frame.pixels, staging);
    frame.revision += 1;
    frame.pending = true;
    frame.active = state.phase == Phase::Active && resize_ready;
    *updates = state.revision;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use linrdp_proto::desktop::Framebuffer;

    #[test]
    fn slow_consumer_preserves_all_deltas_and_transfers_buffer_ownership() {
        let mut state = Session::new(1002, 1003).unwrap();
        state.framebuffer = Framebuffer::new(32, 32).unwrap();
        state.phase = Phase::Active;
        let shared = Mutex::new(Display::default());
        let mut staging = linrdp_proto::desktop::Snapshot::default();
        let mut updates = 0;
        let mut pixels = linrdp_proto::desktop::Snapshot::default();
        pixels.pixels.resize(1024, 0);
        assert!(!publish(&state, &shared, &mut staging, &mut updates, true));
        for index in 0..1024 {
            state.framebuffer.damage.mark(
                index / state.framebuffer.width as usize,
                index / state.framebuffer.width as usize + 1,
            );
            state.framebuffer.pixels[index] = index as u32 + 1;
            state.framebuffer.updates += 1;
            state.revision += 1;
            // A blocked UI causes exactly one snapshot, not 1024 full copies.
            assert_eq!(
                publish(&state, &shared, &mut staging, &mut updates, true),
                index == 0
            );
        }
        {
            let mut frame = shared.lock().unwrap();
            let allocation = frame.pixels.pixels.as_ptr();
            let recycled = pixels.pixels.as_ptr();
            frame.take_pixels(&mut pixels);
            assert_eq!(pixels.pixels.as_ptr(), allocation);
            assert_eq!(frame.pixels.pixels.as_ptr(), recycled);
        }
        assert!(publish(&state, &shared, &mut staging, &mut updates, true));
        shared.lock().unwrap().take_pixels(&mut pixels);
        assert_eq!(pixels.pixels, state.framebuffer.pixels);
        assert!(!publish(&state, &shared, &mut staging, &mut updates, true));
        assert_eq!(shared.lock().unwrap().revision, 2);

        // A new activation can change both dimensions and input readiness.
        state.framebuffer = Framebuffer::new(20, 30).unwrap();
        state.framebuffer.pixels.fill(0xabcdef);
        state.framebuffer.updates = 1;
        state.revision += 1;
        assert!(publish(&state, &shared, &mut staging, &mut updates, false));
        let mut frame = shared.lock().unwrap();
        assert!(!frame.active);
        assert_eq!((frame.width, frame.height), (20, 30));
        frame.take_pixels(&mut pixels);
        assert_eq!(pixels.pixels, vec![0xabcdef; 600]);
    }

    #[test]
    #[ignore = "release microbenchmark; run with --ignored --nocapture"]
    fn benchmark_snapshot_burst() {
        use std::{hint::black_box, time::Instant};
        let mut state = Session::new(1002, 1003).unwrap();
        state.framebuffer = Framebuffer::new(1920, 1080).unwrap();
        state.phase = Phase::Active;
        let mut output = Vec::new();
        let start = Instant::now();
        for index in 0..1024 {
            state.framebuffer.damage.mark(
                index / state.framebuffer.width as usize,
                index / state.framebuffer.width as usize + 1,
            );
            state.framebuffer.pixels[index] = index as u32;
            state.copy_display(black_box(&mut output));
        }
        let old = start.elapsed();
        let mut output = linrdp_proto::desktop::Snapshot::default();
        let shared = Mutex::new(Display::default());
        let mut staging = linrdp_proto::desktop::Snapshot::default();
        let mut updates = 0;
        let start = Instant::now();
        let mut snapshots = 0;
        for index in 0..1024 {
            state.framebuffer.damage.mark(
                index / state.framebuffer.width as usize,
                index / state.framebuffer.width as usize + 1,
            );
            state.framebuffer.pixels[index] = index as u32;
            state.framebuffer.updates += 1;
            state.revision += 1;
            snapshots += usize::from(publish(
                black_box(&state),
                &shared,
                &mut staging,
                &mut updates,
                true,
            ));
            if index % 64 == 63 {
                shared.lock().unwrap().take_pixels(&mut output);
            }
        }
        snapshots += usize::from(publish(&state, &shared, &mut staging, &mut updates, true));
        shared.lock().unwrap().take_pixels(&mut output);
        assert_eq!(output.pixels, state.framebuffer.pixels);
        println!(
            "1080p / 1024 deltas / UI consumes every 64 deltas: per-packet copy {old:?}; demand snapshots {:?}, {snapshots} copies",
            start.elapsed()
        );
    }
}
