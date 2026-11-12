//! Keeping the heap from holding on to memory the compositor has finished with.
//!
//! glibc gives each thread that allocates its own arena, up to eight per core, and frees memory
//! back to the system only from the top of each. With dozens of threads, most of them idle bus
//! and helper threads, that leaves many mostly empty arenas resident. Two arenas are plenty for
//! threads that allocate little, and a trim at quiet moments hands freed pages back.

use std::{cell::RefCell, time::Duration};

use smithay::reexports::calloop::{
    LoopHandle,
    timer::{TimeoutAction, Timer},
};

use crate::Slipstream;

/// How long after the moment a trim was asked for it runs: long enough for a closing panel to
/// finish going and let its pictures go.
const SETTLE: Duration = Duration::from_secs(3);

thread_local! {
    /// The event loop, on its own thread only, for timers set from code with no state to hand.
    static LOOP: RefCell<Option<LoopHandle<'static, Slipstream>>> = const { RefCell::new(None) };
    /// Whether a trim is already waiting to run.
    static PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Caps glibc's arenas. Called first thing, before any thread is spawned.
pub fn limit_arenas() {
    #[cfg(target_env = "gnu")]
    // SAFETY: mallopt only sets an allocator parameter, and nothing else is running yet.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
}

/// Hands memory the heap has free back to the system now.
pub fn trim() {
    #[cfg(target_env = "gnu")]
    // SAFETY: malloc_trim takes the allocator's own locks and touches nothing of ours.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// How long after start-up the heap is first trimmed: the apps, icons and home folder have been
/// read by then, and what reading them took has been freed.
const STARTED: Duration = Duration::from_secs(20);

/// Lets the heap set timers on `handle`, the loop of the thread this is called on, and trims it
/// once start-up has settled.
pub fn set_loop(handle: LoopHandle<'static, Slipstream>) {
    let _ = handle.insert_source(Timer::from_duration(STARTED), |_, _, _| {
        trim();
        TimeoutAction::Drop
    });
    LOOP.with_borrow_mut(|slot| *slot = Some(handle));
}

/// Runs `f` with the event loop, if this is its thread.
pub fn with_loop(f: impl FnOnce(&LoopHandle<'static, Slipstream>)) {
    // Thread-locals are torn down at exit, and what the loop owned can still ask on the way out.
    let _ = LOOP.try_with(|slot| {
        if let Ok(slot) = slot.try_borrow()
            && let Some(handle) = slot.as_ref()
        {
            f(handle);
        }
    });
}

/// Lets go of the event loop's handle, so the loop and everything it owns are dropped with it
/// rather than while this thread's locals are being torn down.
pub fn forget_loop() {
    let handle = LOOP
        .try_with(|slot| slot.borrow_mut().take())
        .ok()
        .flatten();
    drop(handle);
}

/// Trims the heap a little later, once whatever is closing has gone. Asking again while one
/// waits changes nothing.
pub fn trim_soon() {
    if PENDING.get() {
        return;
    }
    with_loop(|handle| {
        let inserted = handle.insert_source(Timer::from_duration(SETTLE), |_, _, _| {
            PENDING.set(false);
            trim();
            TimeoutAction::Drop
        });
        PENDING.set(inserted.is_ok());
    });
}
