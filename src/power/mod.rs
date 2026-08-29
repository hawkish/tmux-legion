//! System sleep and wake notifications.
//!
//! The keyboard keeps whatever colours it was last written, and macOS leaves
//! the port powered while it sleeps, so a lid closed on a blocked agent leaves
//! a red key glowing all night. Fixing that needs a callback that runs *before*
//! the machine suspends — a wake-side clock-jump heuristic can repaint after
//! the fact but can never blank the keys in time.
//!
//! Only macOS is wired up. Everything else gets `watch` returning `None` and
//! behaves exactly as it did before.

#[cfg(target_os = "macos")]
mod macos;

use std::sync::mpsc::Receiver;

/// What the system is about to do, or just did.
///
/// Off macOS nothing ever constructs these, since `watch` returns `None` — but
/// the sidebar still matches on them, so the enum has to exist and dead_code
/// has to be told as much on exactly those platforms.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    /// Sleep is imminent and has been deferred until `ack` runs. Whoever
    /// handles this must finish quickly: the system waits, and macOS gives up
    /// on us after about 30 seconds.
    Sleeping,
    /// Awake again. Anything cached about the outside world is now suspect.
    Woke,
}

/// Start listening. `None` means no notifications on this platform, or that
/// registering failed — in both cases the caller carries on without them,
/// because losing sleep handling is never worth losing the sidebar.
///
/// The returned channel stays open for the life of the process; dropping the
/// receiver leaves the watcher thread parked harmlessly on its run loop.
pub fn watch() -> Option<Receiver<Power>> {
    #[cfg(target_os = "macos")]
    {
        macos::watch()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Tell the system it may now sleep.
///
/// Call this once per `Power::Sleeping`, after the keys are dark. Skipping it
/// stalls sleep until the timeout; calling it twice is ignored.
pub fn allow_sleep() {
    #[cfg(target_os = "macos")]
    macos::allow_sleep();
}
