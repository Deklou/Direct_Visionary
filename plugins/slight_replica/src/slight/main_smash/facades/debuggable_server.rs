use super::Facade;

use std::sync::atomic::AtomicU64;

/// Live-update cadence for dirty kind tabs (frames). 10 ≈ 6 Hz — feels live in RPM without
/// flooding the socket when rainbows animate every frame.
const NOTIFY_EVERY: u64 = 10;
const MAX_NOTIFY_PER_TICK: usize = 32;
const CAPTURE_DEBUG_EVERY: u64 = 60;

static FRAME: AtomicU64 = AtomicU64::new(0);

pub struct DebuggableServerFacade;

impl Facade for DebuggableServerFacade {
    fn name(&self) -> &'static str {
        "Debuggable Server"
    }
    fn install(&mut self) {
        skyline::println!("[SLight] Installing facade Debuggable Server");
        crate::rust_extender::debuggable_server::init();
    }
    fn global_only(&self) -> bool {
        true
    }
    fn on_frame(&mut self) {
        // Game-thread half of the RPM connect handshake (server thread only sets a flag —
        // it must never take the shared locks; see debuggable_server::on_rpm_client_connected).
        crate::rust_extender::debuggable_server::resync_if_requested();

        // Kind-tab pipeline: enforce pinned values on every live instance, then (throttled)
        // notify RPM of dirty tabs and drain the pending queue.
        //
        // NOTE: no per-frame rainbow animation on kind data — animating the displayed color
        // made the user's form snapshot always differ from last_sent, so ANY edit spuriously
        // pinned color/movement_state at a random rainbow frame ("editing size messes up
        // the color"). Kind tabs show stable spawn params; color changes only via edits.
        crate::slight::effect_viewer::apply::enforce_pinned();

        let n = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::slight::fighter_traits::tick();
        if n % NOTIFY_EVERY == 0 {
            for (eff_hash, _name, _data) in
                crate::slight::effect_viewer::kinds::take_dirty(MAX_NOTIFY_PER_TICK)
            {
                crate::slight::effect_viewer::show::queue_show(eff_hash);
            }
            // Live-ACMD capture stream. The first battle object to execute each fighter-kind +
            // motion owns its one immutable snapshot until the editor explicitly clears it.
            // Leave captures in their original buffers when the transport is backed up.
            // Keep a shared budget across replay, live lines and completion markers.
            let budget = crate::rust_extender::net::simple_server::capture_budget(3 * MAX_NOTIFY_PER_TICK);
            let replay = crate::slight::hitbox_viewer::take_archive_replay(budget.min(MAX_NOTIFY_PER_TICK));
            let remaining = budget.saturating_sub(replay.len());
            for record in replay {
                crate::rust_extender::debuggable_server::notify_acmd_capture_archive_record(&record);
            }
            // Call even with zero budget: this also services clear-capture requests.
            let lines = crate::slight::hitbox_viewer::take_pending(remaining.min(MAX_NOTIFY_PER_TICK));
            let remaining = remaining.saturating_sub(lines.len());
            for line in lines {
                crate::rust_extender::debuggable_server::notify_acmd_capture(&line);
            }
            for end in crate::slight::hitbox_viewer::take_pending_ends(remaining.min(MAX_NOTIFY_PER_TICK)) {
                crate::rust_extender::debuggable_server::notify_acmd_capture_end(&end);
            }
            // Diagnostic rows are cumulative. Do not mark them sent while the queue
            // is congested; the next snapshot will contain the newer counters.
            if n % CAPTURE_DEBUG_EVERY == 0 {
                let available = crate::rust_extender::net::simple_server::capture_budget(MAX_NOTIFY_PER_TICK);
                if available == MAX_NOTIFY_PER_TICK {
                    for snapshot in crate::slight::hitbox_viewer::take_capture_debug(available) {
                        crate::rust_extender::debuggable_server::notify_acmd_capture_debug(&snapshot);
                    }
                }
                crate::slight::hitbox_viewer::note_capture_backlog(n);
            }
        }
        crate::slight::pending::process();

        // RPM edits target kind tabs (id == eff_hash). TCP edits are consumed on receipt (no
        // retry medium); transaction files are only marked .done when the edit applied.
        for edit in crate::rust_extender::debuggable_server::poll_tcp_edits() {
            let _ = crate::slight::effect_viewer::apply::apply_kind_edit(&edit);
        }
        // Throttled with the rest: this is a `read_dir` of a directory that is empty except in
        // the seconds after someone drops a file into it by hand, which is the exact shape
        // `slight::sd_poll` exists to keep off the frame path. It ran every frame until R2.
        //
        // It stays here rather than moving into `sd_poll::tick` because it returns edits to
        // apply and that tick returns nothing — but it is on a throttle now, and the retry
        // budget below is expressed in frames so the two cannot drift apart silently.
        if n % crate::rust_extender::debuggable_server::TRANSACTION_POLL_EVERY == 0 {
            for (edit, path) in crate::rust_extender::debuggable_server::poll_transactions() {
                let applied = crate::slight::effect_viewer::apply::apply_kind_edit(&edit);
                crate::rust_extender::debuggable_server::finish_transaction(&path, applied);
            }
        }
    }
}
