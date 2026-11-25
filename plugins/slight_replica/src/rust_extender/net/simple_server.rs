//! Jorge rust_extender TCP simple_server — :7878, `<TCP_MESSAGE>` framing.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::thread;

use parking_lot::Mutex;

static LISTEN_FD: AtomicI32 = AtomicI32::new(-1);
static CLIENT_FD: AtomicI32 = AtomicI32::new(-1);
// Atomic (not Mutex): read by the game thread every frame, written on the server thread —
// a contended parking_lot lock can park a waiter forever in this environment.
static CLIENT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static OUTBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INBOUND: Mutex<Vec<String>> = Mutex::new(Vec::new());
static mut SOCKET_POOL: [u8; 0x40000] = [0; 0x40000];

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SOL_SOCKET: i32 = 0xffff;
const SO_REUSEADDR: i32 = 4;
// Switch BSD message flags (libnx sys/socket.h), not Linux host flags.
// Nintendo's BSD service uses Linux errno values (Switchbrew Sockets services).
const MSG_DONTWAIT: i32 = 0x80;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
// The Ryujinx BSD service recognizes 0x800. The SDK may translate the
// FreeBSD O_NONBLOCK (0x4), so verify the mode by reading it back.
const O_NONBLOCK_BSD: i32 = 0x4;
const O_NONBLOCK_SERVICE: i32 = 0x800;
const EINTR: u32 = 4;
const EAGAIN: u32 = 11;
const SEND_STALL_MS: f64 = 10_000.0;
const CAPTURE_HIGH_WATER: usize = 512;
const OUTBOX_CAP: usize = 8192;
const SEND_BATCH_CAP: usize = 512;
const SEND_BATCH_BYTES: usize = 128 * 1024;
const FRAME_OPEN: &str = "<TCP_MESSAGE>";
const FRAME_CLOSE: &str = "</TCP_MESSAGE>";
const PONG_INNER: &str = r#"{"header":"Pong","body":"{}"}"#;

// Cumulative counters retained for low-frequency health diagnostics.
static IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static COALESCED: AtomicU64 = AtomicU64::new(0);
static REQUEUED: AtomicU64 = AtomicU64::new(0);
static SEND_ERRORS: AtomicU64 = AtomicU64::new(0);

fn retryable_socket_error(errno: u32) -> bool {
    matches!(errno, EINTR | EAGAIN)
}

fn tick_now() -> u64 {
    unsafe { skyline::nn::os::GetSystemTick() as u64 }
}

fn elapsed_ms(ticks: u64) -> f64 {
    ticks as f64 / 19_200.0
}

pub fn note_network_stats(frame: u64) {
    let load = |v: &AtomicU64| v.load(Ordering::Relaxed);
    let outbox = OUTBOX.try_lock().map(|q| q.len());
    let inbound = INBOUND.try_lock().map(|q| q.len());
    crate::slight::diag::note(format!(
        "TCP frame={frame} client={} outbox={outbox:?} inbound={inbound:?} in_flight={} dropped={} coalesced={} requeued={} send_errors={}",
        client_id(), load(&IN_FLIGHT), load(&DROPPED), load(&COALESCED),
        load(&REQUEUED), load(&SEND_ERRORS),
    ));
}

extern "C" {
    // Exact variadic SDK import confirmed in Smash 13.0.5's main.
    #[link_name = "\u{1}_ZN2nn6socket5FcntlEiiz"]
    fn Fcntl(socket: i32, command: i32, ...) -> i32;
    #[link_name = "\u{1}_ZN2nn6socket4RecvEiPvmi"]
    fn Recv(socket: i32, buffer: *mut u8, bufferLength: u64, flags: i32) -> i64;
    #[link_name = "\u{1}_ZN2nn6socket8ShutdownEii"]
    fn Shutdown(socket: i32, how: i32) -> i32;
    #[link_name = "\u{1}_ZN2nn6socket5CloseEi"]
    fn Close(socket: i32) -> i32;
}

pub fn start(port: u16) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::slight::diag::note(format!("SRV start(:{port}) — spawning server_loop"));
    thread::spawn(move || server_loop(port));
}

/// Sleep via nn::os directly — std::thread::sleep never wakes under skyline on Ryujinx
/// (observed: both server threads went permanently silent after their first sleep; the
/// skyline panic handler also uses nn::os::SleepThread, not std).
fn sleep_ms(ms: u64) {
    unsafe {
        nnsdk::nn::os::SleepThread(nnsdk::nn::TimeSpan {
            nanoseconds: ms * 1_000_000,
        });
    }
}

/// Messages queued and not yet sent (diagnostic — growth = sender thread dead/stuck).
pub fn outbox_depth() -> usize {
    OUTBOX.try_lock().map(|out| out.len()).unwrap_or_default()
}

/// Budget only for streaming captures. Other events keep their existing queue capacity.
/// On lock contention, leave records in their source rather than draining them.
pub fn capture_budget(max: usize) -> usize {
    if !has_client() {
        return max;
    }
    OUTBOX.try_lock().map_or(0, |out| {
        let queued = out.len().saturating_add(IN_FLIGHT.load(Ordering::Relaxed) as usize);
        max.min(CAPTURE_HIGH_WATER.saturating_sub(queued))
    })
}

fn server_loop(port: u16) {
    unsafe {
        crate::slight::diag::note("SRV thread entered — waiting for first game frame");
        crate::slight::diag::note(format!("SRV single worker max={SEND_BATCH_BYTES} bytes/{SEND_BATCH_CAP} frames"));
        // Touching nn::socket BEFORE it is initialized crashes/hangs this thread, and whether
        // skyline's logger has initialized it yet is a boot-order race (lost on some boots).
        // The per-frame driver ticking means boot init — including skyline's socket init — is
        // done, so gate on that before the first socket call.
        while !crate::slight::agent_extender::driver_has_ticked() {
            sleep_ms(250);
        }
        crate::slight::diag::note("SRV game frames running — probing sockets");
        // Do NOT call nn::socket::Initialize unconditionally: skyline's own TCP logger (:6969)
        // already initializes nn::socket in this process, and the nn SDK ABORTS on double-init
        // (on Ryujinx this silently killed the server thread before the socket was ever
        // created — :7878 never bound). Probe with Socket() first; only Initialize if sockets
        // aren't up yet (e.g. environments where the skyline logger is disabled).
        let mut listen = nnsdk::nn::socket::Socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        crate::slight::diag::note(format!("SRV socket probe -> {listen}"));
        if listen < 0 {
            let rc = nnsdk::nn::socket::Initialize(
                std::ptr::addr_of_mut!(SOCKET_POOL).cast::<u8>(),
                0x40000,
                0x20000,
                0x20,
            );
            crate::slight::diag::note(format!("SRV nn::socket::Initialize rc={rc}"));
            if rc != 0 {
                skyline::println!("[SLight] nn::socket::Initialize failed: {rc}");
                return;
            }
            listen = nnsdk::nn::socket::Socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
            crate::slight::diag::note(format!("SRV socket retry -> {listen}"));
        }
        if listen < 0 {
            skyline::println!("[SLight] socket create failed ({listen}) — no debuggable server");
            return;
        }
        LISTEN_FD.store(listen, Ordering::Release);

        let reuse: i32 = 1;
        nnsdk::nn::socket::SetSockOpt(
            listen,
            SOL_SOCKET,
            SO_REUSEADDR,
            &reuse as *const i32 as *const u8,
            4,
        );

        let mut addr: nnsdk::nn::socket::SockAddrIn = std::mem::zeroed();
        addr.sin_len = 16;
        addr.sin_family = AF_INET as u8;
        addr.sin_port = nnsdk::nn::socket::InetHtons(port);
        addr.sin_addr = [0, 0, 0, 0];

        let bind_rc = nnsdk::nn::socket::Bind(
            listen,
            &addr as *const _ as *const nnsdk::root::sockaddr,
            16,
        );
        crate::slight::diag::note(format!("SRV bind :{port} rc={bind_rc}"));
        if bind_rc != 0 {
            report_bind_failure(port, bind_rc);
            return;
        }

        nnsdk::nn::socket::Listen(listen, 2);
        crate::slight::diag::note(format!("SRV listening on :{port}"));
        skyline::println!("[SLight] Accepting clients from debbugable server on :{port}");

        loop {
            let mut len: u32 = 16;
            let mut peer: nnsdk::nn::socket::SockAddrIn = std::mem::zeroed();
            let client = nnsdk::nn::socket::Accept(
                listen,
                &mut peer as *mut _ as *mut nnsdk::root::sockaddr,
                &mut len,
            ) as i32;
            if client < 0 {
                sleep_ms(20);
                continue;
            }
            // The same worker owns accept, handshake, receive, send and close.
            let flags = configure_client(client);
            let Some(cid) = handshake(client, flags) else {
                crate::slight::diag::note(format!(
                    "SRV handshake failed fd={client} — waiting for the next client"
                ));
                close_client(client);
                continue;
            };
            CLIENT_FD.store(client, Ordering::Release);
            crate::rust_extender::debuggable_server::on_rpm_client_connected(cid);
            serve_client(client, flags);
            close_client(client);
        }
    }
}

/// The one line a reader should be able to grep `diag.txt` for. Kept short and unmistakable so
/// it survives being pasted out of a chat log; the deploy script prints the same phrase.
pub const DOUBLE_PLUGIN_BANNER: &str = "!!!! SLIGHT: PORT ALREADY BOUND — A SECOND COPY OF THIS \
                                        PLUGIN IS PROBABLY LOADED";

/// Say out loud what a failed bind almost always means, because the silent version of this cost
/// about six rounds of misdiagnosis.
///
/// Skyline loads **every file** in `romfs:/skyline/plugins/` as a plugin, extension ignored, so a
/// `lib_effect_viewer.nro.bak` left beside the real one runs a second full copy of us: two sets of
/// ACMD hooks, two per-frame drivers, and two servers racing for this port. The visible symptom is
/// a hard 60→30 fps drop on entering training mode, which looks like a performance regression in
/// whatever was last changed and is not one.
///
/// The bind failure was already logged — as `SRV bind :7878 rc=-1`, one lowercase line among
/// thousands, which nobody reads as "you have two plugins installed". What makes this loud is not
/// the flush; it is naming the cause, the remedy, and a second check the reader can run.
///
/// **The second check is the honest part.** A bind can fail for other reasons (a port set in
/// `gateway.txt` that something else owns), so the banner does not assert the diagnosis, it points
/// at the corroborating evidence: each instance runs its own server thread and its own diag flush,
/// so `SRV thread entered` appears **once per loaded copy**. Two of them is the proof.
///
/// It also explains the tell that misled us the first time. [`diag::start_session`] truncates and
/// rewrites the header, and both copies do it during plugin init, so whichever loads *second* wins
/// the `build=` line — a reader who just deployed build X sees build Y at the top of the file and
/// concludes their deploy did not take.
///
/// [`diag::start_session`]: crate::slight::diag::start_session
fn report_bind_failure(port: u16, rc: u32) {
    use crate::slight::diag;
    let build = crate::slight::effect_viewer::live_eff::BUILD_TAG;
    diag::note(DOUBLE_PLUGIN_BANNER);
    diag::note(format!(
        "     bind :{port} failed rc={rc}; this copy is build={build}"
    ));
    diag::note("     check: `SRV thread entered` appears once per loaded copy — two = two plugins");
    diag::note("     note:  the build= header is written by whichever copy loads LAST, not yours");
    diag::note("     fix:   romfs:/skyline/plugins/ must hold ONE lib_effect_viewer.nro and no");
    diag::note("            copies of it under any other name — Skyline loads every file there,");
    diag::note("            .bak and .old included. Re-run the deploy script; it now refuses.");
    diag::note("     without this server there is no live preview; the game is otherwise fine.");
    // Flush here rather than leaving it to the per-frame driver. This runs once, on the server
    // thread, and the buffer it would otherwise sit in is capped at 40000 lines and shared with
    // two chatty instances — the one message that must not be dropped should not queue behind
    // 30 frames of SPAWN lines from a plugin the reader does not know is running.
    diag::flush();
    skyline::println!("[SLight] bind :{port} failed rc={rc} — second plugin copy? build={build}");
}

// Only touch the accepted Visionary socket. If the SDK/service does not
// support persistent nonblocking, fall back to per-call flags.
unsafe fn configure_client(client: i32) -> i32 {
    let before = Fcntl(client, F_GETFL);
    if before < 0 {
        let errno = nnsdk::nn::socket::GetLastError();
        crate::slight::diag::note(format!("TCP mode=per_call_nonblocking fallback fd={client} errno={errno}"));
        return MSG_DONTWAIT;
    }
    // Try the native flag first, then the value accepted by Ryujinx's BSD service.
    for bit in [O_NONBLOCK_BSD, O_NONBLOCK_SERVICE] {
        let rc = Fcntl(client, F_SETFL, before | bit);
        let after = if rc == 0 { Fcntl(client, F_GETFL) } else { -1 };
        if rc == 0 && after >= 0 && after & bit != 0 {
            crate::slight::diag::note(format!("TCP mode=persistent_nonblocking fd={client} flags={after:#x}"));
            return 0;
        }
    }
    crate::slight::diag::note("TCP mode=per_call_nonblocking fallback");
    MSG_DONTWAIT
}

unsafe fn handshake(client: i32, flags: i32) -> Option<u64> {
    let cid = CLIENT_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let frames = vec![
        r#"{"header":"RemoveAll","body":"{}"}"#.to_owned(),
        format!(
            r#"{{"header":"GiveClientId","body":"{{\"GiveClientId\":{{\"client_id\":{cid},\"scoped_cancels\":true,\"scoped_acmd\":true,\"scoped_aliases\":true}}}}"}}"#
        ),
    ];
    let mut batch = TxBatch::new(frames);
    loop {
        match batch.write_step(client, false, flags) {
            Ok(true) => return Some(cid),
            Ok(false) => sleep_ms(4),
            Err(()) => return None,
        }
    }
}

fn frame_len(inner: &str) -> usize {
    FRAME_OPEN.len() + inner.len() + FRAME_CLOSE.len()
}

// Keep the queue lock independent of socket writes. A single large frame may exceed
// the byte budget, but must not block the rest of the queue permanently.
fn batch_count(messages: &[String]) -> usize {
    let mut bytes = 0usize;
    let mut count = 0usize;
    for message in messages.iter().take(SEND_BATCH_CAP) {
        let next = bytes.saturating_add(frame_len(message));
        if count > 0 && next > SEND_BATCH_BYTES {
            break;
        }
        bytes = next;
        count += 1;
    }
    count
}

fn encode_batch(messages: &[String]) -> (Vec<u8>, Vec<usize>) {
    let mut bytes = Vec::new();
    let mut ends = Vec::with_capacity(messages.len());
    for message in messages {
        bytes.extend_from_slice(FRAME_OPEN.as_bytes());
        bytes.extend_from_slice(message.as_bytes());
        bytes.extend_from_slice(FRAME_CLOSE.as_bytes());
        ends.push(bytes.len());
    }
    (bytes, ends)
}

// A partially written batch stays owned by this connection across receive turns.
struct TxBatch {
    messages: Vec<String>,
    bytes: Vec<u8>,
    ends: Vec<usize>,
    sent: usize,
    completed: usize,
    last_progress: u64,
}

impl TxBatch {
    fn new(messages: Vec<String>) -> Self {
        let (bytes, ends) = encode_batch(&messages);
        Self { messages, bytes, ends, sent: 0, completed: 0, last_progress: tick_now() }
    }

    /// At most one SDK Send per turn. Keep offsets on short writes and EAGAIN.
    unsafe fn write_step(&mut self, client: i32, track: bool, flags: i32) -> Result<bool, ()> {
        let remaining = self.bytes.len() - self.sent;
        if remaining == 0 {
            return Ok(true);
        }
        let n = nnsdk::nn::socket::Send(
            client, self.bytes.as_ptr().add(self.sent), remaining as u64, flags
        ) as i64;
        let errno = if n < 0 { nnsdk::nn::socket::GetLastError() } else { 0 };
        if n < 0 && retryable_socket_error(errno) {
            if elapsed_ms(tick_now().saturating_sub(self.last_progress)) < SEND_STALL_MS {
                return Ok(false);
            }
        }
        if n <= 0 || n as usize > remaining {
            SEND_ERRORS.fetch_add(1, Ordering::Relaxed);
            crate::slight::diag::note(format!(
                "TCP send rc={n} errno={errno} remaining={remaining} completed={}/{}",
                self.completed, self.messages.len()
            ));
            return Err(());
        }
        self.last_progress = tick_now();
        self.sent += n as usize;
        self.completed = self.ends.partition_point(|&end| end <= self.sent);
        if track {
            IN_FLIGHT.store((self.messages.len() - self.completed) as u64, Ordering::Relaxed);
        }
        Ok(self.sent == self.bytes.len())
    }

    fn requeue(self) {
        requeue_front(self.messages.into_iter().skip(self.completed).collect());
        IN_FLIGHT.store(0, Ordering::Relaxed);
    }
}

unsafe fn serve_client(client: i32, flags: i32) {
    let mut batch: Option<TxBatch> = None;
    let mut buf = String::new();
    let mut utf8_tail = Vec::new();
    loop {
        // Alternate one receive and one send. No lock is held during SDK calls.
        let received = match recv_once(client, &mut buf, &mut utf8_tail, flags) {
            Ok(received) => received,
            Err(()) => break,
        };
        if batch.is_none() {
            batch = take_batch();
        }
        let mut sent = false;
        if let Some(tx) = batch.as_mut() {
            match tx.write_step(client, true, flags) {
                Ok(true) => {
                    batch = None;
                    sent = true;
                }
                Ok(false) => {}
                Err(()) => break,
            }
        }

        if !received && !sent {
            sleep_ms(4);
        }
    }
    // Stop publishing before restoring unsent frames. The next client receives
    // its own handshake before any queued application data.
    CLIENT_FD.store(-1, Ordering::Release);
    if let Some(tx) = batch {
        tx.requeue();
    }
}

unsafe fn recv_once(client: i32, buf: &mut String, tail: &mut Vec<u8>, flags: i32) -> Result<bool, ()> {
    let mut chunk = [0u8; 4096];
    let n = Recv(client, chunk.as_mut_ptr(), chunk.len() as u64, flags);
    let errno = if n < 0 { nnsdk::nn::socket::GetLastError() } else { 0 };
    if n < 0 && retryable_socket_error(errno) {
        return Ok(false);
    }
    if n <= 0 || n as usize > chunk.len() {
        crate::slight::diag::note(format!("SRV recv fd={client} rc={n} errno={errno} — client disconnected"));
        return Err(());
    }
    append_tcp_utf8(buf, tail, &chunk[..n as usize]);
    extract_inbound(buf);
    Ok(true)
}

// TCP can split UTF-8 code points anywhere, including across SDK reads.
fn append_tcp_utf8(buf: &mut String, tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(chunk);
    let mut rest = tail.as_slice();
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                buf.push_str(valid);
                rest = &[];
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                buf.push_str(std::str::from_utf8(&rest[..valid]).unwrap());
                rest = &rest[valid..];
                if let Some(invalid) = error.error_len() {
                    buf.push('\u{fffd}');
                    rest = &rest[invalid..];
                } else {
                    break;
                }
            }
        }
    }
    let consumed = tail.len() - rest.len();
    tail.drain(..consumed);
}

fn extract_inbound(buf: &mut String) {
    for payload in extract_payloads(buf) {
        inbound_push(payload);
    }
}

/// Extract framed or legacy newline-delimited payloads without touching plugin state.  Keeping
/// this pure makes the stream recovery rules testable on the host and avoids hiding parser bugs
/// behind the game-only inbound queue.
fn extract_payloads(buf: &mut String) -> Vec<String> {
    let mut payloads = Vec::new();
    loop {
        let start = buf.find(FRAME_OPEN);
        let end = buf.find(FRAME_CLOSE);
        if let (Some(s), Some(e)) = (start, end) {
            if e >= s {
                let payload = buf[s + FRAME_OPEN.len()..e].trim().to_string();
                *buf = buf[e + FRAME_CLOSE.len()..].to_string();
                if !payload.is_empty() {
                    payloads.push(payload);
                }
                continue;
            }
            // A stale close tag before the next opening tag would otherwise remain forever and
            // poison every subsequent frame on this connection.
            *buf = buf[e + FRAME_CLOSE.len()..].to_string();
            continue;
        }

        if start.is_none() {
            // Drop an orphan closing tag even when there is no opening tag yet. This is the same
            // recovery policy as the desktop parser and handles a torn previous frame.
            if let Some(e) = end {
                *buf = buf[e + FRAME_CLOSE.len()..].to_string();
                continue;
            }
        }

        if let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            *buf = buf[nl + 1..].to_string();
            if !line.is_empty() {
                payloads.push(line);
            }
            continue;
        }

        // Runaway guard: only discard when we're NOT mid-message. A large frame in flight
        // (e.g. a ~1.3 MB base64 donor eff) legitimately exceeds any small cap before its
        // closing tag arrives — clearing it here silently dropped donor_bytes. Keep
        // accumulating while an opening tag is present; only clear genuine garbage.
        if buf.len() > 64 * 1024 * 1024 && !buf.contains("<TCP_MESSAGE>") {
            buf.clear();
        }
        break;
    }
    payloads
}

/// Server thread → INBOUND. try_lock + SleepThread retry — never parks (parked waiters
/// never wake in this environment).
fn inbound_push(payload: String) {
    // Heartbeats belong to the transport rather than the game-frame pump. Answer here so a
    // paused or heavily loaded game still proves that its socket thread is alive.
    if is_ping(&payload) {
        queue_pong();
        return;
    }
    // Timing rules are pure shared state. Install them on receipt so the next ACMD coroutine
    // boundary can see a frame-0 edit; the general parser below remains responsible for all
    // commands and game-thread-only updates.
    if crate::rust_extender::debuggable_server::apply_timing_rules_from_network(&payload) {
        return;
    }
    loop {
        if let Some(mut q) = INBOUND.try_lock() {
            q.push(payload);
            return;
        }
        sleep_ms(1);
    }
}

fn is_ping(payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .is_some_and(|value| {
            value.get("command").and_then(|command| command.as_str()) == Some("ping")
        })
}

// Only identical pending snapshots are redundant. Preserve transitions, errors,
// and all capture lines/end markers; never replace an earlier distinct state.
fn redundant_snapshot(out: &[String], inner: &str) -> bool {
    let Some(header) = inner.strip_prefix("{\"header\":\"")
        .and_then(|rest| rest.split_once('"')).map(|(header, _)| header) else {
        return false;
    };
    let (prefix, barrier) = match header {
        "Multipliers" => ("{\"header\":\"Multipliers\"", ""),
        "CarrierStatus" => ("{\"header\":\"CarrierStatus\"", "{\"header\":\"CarrierError\""),
        "AssetBundleStatus" => ("{\"header\":\"AssetBundleStatus\"", "{\"header\":\"AssetBundleError\""),
        _ => return false,
    };
    for previous in out.iter().rev() {
        if previous.starts_with("{\"header\":\"RemoveAll\"")
            || (!barrier.is_empty() && previous.starts_with(barrier)) {
            return false;
        }
        if previous.starts_with(prefix) {
            return previous == inner;
        }
    }
    false
}

pub fn queue(inner: String) {
    // Push only: the network worker owns all socket calls. Never send from the game thread.
    // try_lock + short spin — NEVER park the game thread (parked waiters never wake here).
    // Worst case the message is dropped; RPM gets a full re-sync on reconnect anyway.
    for _ in 0..1000 {
        if let Some(mut out) = OUTBOX.try_lock() {
            if redundant_snapshot(&out, &inner) {
                COALESCED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            // Bound growth if RPM is stuck; drop newest.
            if out.len() < OUTBOX_CAP {
                out.push(inner);
            } else {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        core::hint::spin_loop();
    }
    DROPPED.fetch_add(1, Ordering::Relaxed);
}

fn take_batch() -> Option<TxBatch> {
    let messages = {
        let mut out = OUTBOX.try_lock()?;
        let count = batch_count(&out);
        if count == 0 {
            return None;
        }
        IN_FLIGHT.store(count as u64, Ordering::Relaxed);
        out.drain(..count).collect::<Vec<_>>()
    };
    Some(TxBatch::new(messages))
}

fn requeue_front(unsent: Vec<String>) {
    if unsent.is_empty() {
        return;
    }
    REQUEUED.fetch_add(unsent.len() as u64, Ordering::Relaxed);
    loop {
        if let Some(mut out) = OUTBOX.try_lock() {
            let mut combined = unsent;
            combined.extend(out.drain(..));
            // Keep the same oldest-first/drop-newest policy as `queue` while preserving all
            // frames that were already in flight ahead of newer edits.
            DROPPED.fetch_add(combined.len().saturating_sub(OUTBOX_CAP) as u64, Ordering::Relaxed);
            combined.truncate(OUTBOX_CAP);
            *out = combined;
            return;
        }
        sleep_ms(1);
    }
}

fn close_client(client: i32) {
    unsafe {
        let _ = Shutdown(client, 2);
        let _ = Close(client);
    }
}

/// Queue the heartbeat response expected by the desktop `GameLink` ping probe.
fn queue_pong() {
    loop {
        if let Some(mut out) = OUTBOX.try_lock() {
            if !out.iter().any(|message| message == PONG_INNER) {
                out.insert(0, PONG_INNER.to_owned());
                DROPPED.fetch_add(out.len().saturating_sub(OUTBOX_CAP) as u64, Ordering::Relaxed);
                out.truncate(OUTBOX_CAP);
            }
            return;
        }
        sleep_ms(1);
    }
}

pub fn has_client() -> bool {
    CLIENT_FD.load(Ordering::Acquire) >= 0
}

pub fn client_id() -> u64 {
    CLIENT_ID.load(Ordering::Acquire)
}

/// Game thread. try_lock — under contention with the server thread's push, skip a frame
/// instead of parking (parked waiters never wake here).
pub fn take_inbound() -> Vec<String> {
    match INBOUND.try_lock() {
        Some(mut q) => std::mem::take(&mut *q),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pong_is_a_desktop_compatible_framed_envelope() {
        let frame = format!("{FRAME_OPEN}{PONG_INNER}{FRAME_CLOSE}");
        let payload = &frame[FRAME_OPEN.len()..frame.len() - FRAME_CLOSE.len()];
        let value: serde_json::Value = serde_json::from_str(payload).expect("valid Pong JSON");
        assert_eq!(value["header"], "Pong");
        assert_eq!(value["body"], "{}");
    }

    #[test]
    fn extractor_reassembles_torn_frames_and_recovers_from_orphan_close() {
        let mut buf = format!("stale{FRAME_CLOSE}{FRAME_OPEN}{{\"command\":\"ping\"}}");
        assert!(extract_payloads(&mut buf).is_empty());
        buf.push_str(FRAME_CLOSE);
        assert_eq!(
            extract_payloads(&mut buf),
            vec![r#"{"command":"ping"}"#.to_owned()]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn extractor_handles_multiple_frames_in_one_read() {
        let mut buf = format!("{FRAME_OPEN}one{FRAME_CLOSE}{FRAME_OPEN}two{FRAME_CLOSE}");
        assert_eq!(extract_payloads(&mut buf), vec!["one", "two"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn batch_preserves_frame_order_and_boundaries() {
        let messages = vec!["one".to_owned(), "deux é".to_owned(), "three".to_owned()];
        let (bytes, ends) = encode_batch(&messages);
        let mut buf = String::from_utf8(bytes).unwrap();
        assert_eq!(extract_payloads(&mut buf), messages);
        assert!(buf.is_empty());
        assert_eq!(ends[0], frame_len("one"));
        assert_eq!(ends.partition_point(|&end| end <= ends[1] - 1), 1);
        assert_eq!(ends.partition_point(|&end| end <= ends[1]), 2);
        assert_eq!(ends.partition_point(|&end| end <= ends[2]), 3);
    }

    #[test]
    fn batch_limits_count_and_bytes_but_allows_large_first_frame() {
        let small = vec!["x".to_owned(); SEND_BATCH_CAP + 1];
        assert_eq!(batch_count(&small), SEND_BATCH_CAP);
        let half = "x".repeat(SEND_BATCH_BYTES / 2);
        assert_eq!(batch_count(&[half.clone(), half]), 1);
        let large = "x".repeat(SEND_BATCH_BYTES + 1);
        assert_eq!(batch_count(&[large, "next".to_owned()]), 1);
        assert_eq!(batch_count(&[]), 0);
    }

    #[test]
    fn only_temporary_socket_errors_are_retried() {
        assert!(retryable_socket_error(4));
        assert!(retryable_socket_error(11));
        assert!(!retryable_socket_error(0));
        assert!(!retryable_socket_error(104));
    }

    #[test]
    fn duplicate_snapshots_do_not_erase_transitions_or_errors() {
        let a = r#"{"header":"CarrierStatus","body":"a"}"#.to_owned();
        let b = r#"{"header":"CarrierStatus","body":"b"}"#.to_owned();
        let err = r#"{"header":"CarrierError","body":"error"}"#.to_owned();
        let line = r#"{"header":"AcmdCapture","body":"line"}"#.to_owned();
        assert!(redundant_snapshot(&[a.clone(), line.clone()], &a));
        assert!(!redundant_snapshot(&[a.clone(), b], &a));
        assert!(!redundant_snapshot(&[a.clone(), err], &a));
        assert!(!redundant_snapshot(&[line.clone()], &line));
    }

    #[test]
    fn receive_utf8_survives_every_split() {
        let wire = format!("{FRAME_OPEN}é漢🙂{FRAME_CLOSE}{FRAME_OPEN}second{FRAME_CLOSE}");
        for split in 0..=wire.len() {
            let mut buf = String::new();
            let mut tail = Vec::new();
            append_tcp_utf8(&mut buf, &mut tail, &wire.as_bytes()[..split]);
            let mut frames = extract_payloads(&mut buf);
            append_tcp_utf8(&mut buf, &mut tail, &wire.as_bytes()[split..]);
            frames.extend(extract_payloads(&mut buf));
            assert_eq!(frames, vec!["é漢🙂", "second"]);
            assert!(tail.is_empty());
        }
    }

    #[test]
    fn ping_detection_is_exact() {
        assert!(is_ping(r#"{"command":"ping","echo":4}"#));
        assert!(!is_ping(r#"{"command":"live_eff_ping"}"#));
        assert!(!is_ping("not json"));
    }
}
