//! IOKit system power notifications.
//!
//! `IORegisterForSystemPower` is the only way to run code between "the user
//! closed the lid" and the machine actually suspending. It hands us a token per
//! sleep request and waits — for about 30 seconds — until we pass that token
//! back to `IOAllowPowerChange`. That pause is what lets the keys go dark
//! before the display does.
//!
//! The notifications arrive on a CFRunLoop, which needs a thread of its own:
//! the sidebar's own loop belongs to crossterm.

use super::Power;
use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

type IONotificationPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;
type IoConnect = u32;
type IoObject = u32;

type IOServiceInterestCallback =
    extern "C" fn(refcon: *mut c_void, service: IoObject, message_type: u32, argument: *mut c_void);

// From IOKit/IOMessage.h, via iokit_common_msg(): sys_iokit (0xE0000000) with
// sub_iokit_common (0), so the message number is the low half.
const CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
const SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
const SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

#[allow(non_snake_case)]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        port: *mut IONotificationPortRef,
        callback: IOServiceInterestCallback,
        notifier: *mut IoObject,
    ) -> IoConnect;
    fn IONotificationPortGetRunLoopSource(port: IONotificationPortRef) -> CFRunLoopSourceRef;
    fn IOAllowPowerChange(root_port: IoConnect, notification_id: isize) -> i32;
}

#[allow(non_snake_case, non_upper_case_globals)]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(loop_: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRun();
    static kCFRunLoopCommonModes: CFStringRef;
}

/// The connection `IOAllowPowerChange` needs. Written once, by the watcher
/// thread, before it starts delivering anything.
static ROOT_PORT: Mutex<IoConnect> = Mutex::new(0);

/// The token for a sleep that is waiting on us, taken by `allow_sleep`. At most
/// one sleep is ever outstanding, and taking it is what makes a second
/// `allow_sleep` a no-op.
static PENDING: Mutex<Option<isize>> = Mutex::new(None);

pub fn watch() -> Option<Receiver<Power>> {
    let (tx, rx) = mpsc::channel();
    // Registration has to happen on the thread that runs the loop, so the
    // caller waits here to learn whether it worked.
    let (ready_tx, ready_rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("legion-power".into())
        .spawn(move || {
            // Handed to every callback as refcon, and deliberately never freed:
            // the run loop below owns this thread until the process exits.
            let sender = Box::into_raw(Box::new(tx));
            let mut port: IONotificationPortRef = ptr::null_mut();
            let mut notifier: IoObject = 0;

            // SAFETY: `port` and `notifier` are live for the call, and `sender`
            // outlives every callback because nothing frees it.
            let root = unsafe {
                IORegisterForSystemPower(
                    sender as *mut c_void,
                    &mut port,
                    on_power_change,
                    &mut notifier,
                )
            };
            if root == 0 || port.is_null() {
                // SAFETY: registration failed, so no callback can hold this.
                drop(unsafe { Box::from_raw(sender) });
                let _ = ready_tx.send(false);
                return;
            }
            if let Ok(mut slot) = ROOT_PORT.lock() {
                *slot = root;
            }

            // SAFETY: all three are live; the source belongs to `port`, which
            // is never released.
            unsafe {
                CFRunLoopAddSource(
                    CFRunLoopGetCurrent(),
                    IONotificationPortGetRunLoopSource(port),
                    kCFRunLoopCommonModes,
                );
            }
            let _ = ready_tx.send(true);

            // Parks this thread forever, waking only to run the callback.
            unsafe { CFRunLoopRun() };
        })
        .ok()?;

    match ready_rx.recv() {
        Ok(true) => Some(rx),
        _ => None,
    }
}

pub fn allow_sleep() {
    let Some(id) = PENDING.lock().ok().and_then(|mut p| p.take()) else {
        return; // nothing waiting, or a previous call already answered
    };
    let port = ROOT_PORT.lock().map(|p| *p).unwrap_or(0);
    if port != 0 {
        // SAFETY: `port` came from a successful registration and stays valid
        // for the life of the process.
        unsafe { IOAllowPowerChange(port, id) };
    }
}

/// Runs on the watcher thread's run loop.
///
/// Must not panic: unwinding through the C frame above it is undefined
/// behaviour, so every fallible step here is swallowed rather than unwrapped.
extern "C" fn on_power_change(
    refcon: *mut c_void,
    _service: IoObject,
    message_type: u32,
    argument: *mut c_void,
) {
    // SAFETY: refcon is the leaked Sender from `watch`, still alive.
    let tx = unsafe { &*(refcon as *const Sender<Power>) };

    match message_type {
        // An idle sleep we are allowed to veto. We never want to — answer at
        // once and leave the real work to SYSTEM_WILL_SLEEP, which follows.
        CAN_SYSTEM_SLEEP => {
            let port = ROOT_PORT.lock().map(|p| *p).unwrap_or(0);
            if port != 0 {
                // SAFETY: as in `allow_sleep`.
                unsafe { IOAllowPowerChange(port, argument as isize) };
            }
        }
        // Sleep is happening. The system now waits on our ack, so hold it and
        // let the sidebar blank the keys first.
        SYSTEM_WILL_SLEEP => {
            if let Ok(mut pending) = PENDING.lock() {
                *pending = Some(argument as isize);
            }
            if tx.send(Power::Sleeping).is_err() {
                // The sidebar is gone. Never stall the machine over it.
                allow_sleep();
            }
        }
        SYSTEM_HAS_POWERED_ON => {
            let _ = tx.send(Power::Woke);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one part of this file a test can actually execute: that the IOKit
    /// and CoreFoundation symbols link, that the kernel accepts the
    /// registration, and that the run loop thread comes up.
    ///
    /// What happens *after* a notification arrives is unreachable from here —
    /// nothing in a test can make the machine sleep.
    #[test]
    fn registering_for_system_power_succeeds() {
        assert!(
            watch().is_some(),
            "IORegisterForSystemPower failed; the sidebar would run without \
             sleep handling"
        );
    }

    /// The constants are hand-transcribed from IOMessage.h, where they are
    /// built by macro. Pin the arithmetic so a typo can't quietly mean "some
    /// message we never match".
    #[test]
    fn message_constants_match_iokit_common_msg() {
        // sys_iokit = err_system(0x38) = 0x38 << 26; sub_iokit_common = 0.
        let iokit_common_msg = |m: u32| (0x38u32 << 26) | m;
        assert_eq!(CAN_SYSTEM_SLEEP, iokit_common_msg(0x270));
        assert_eq!(SYSTEM_WILL_SLEEP, iokit_common_msg(0x280));
        assert_eq!(SYSTEM_HAS_POWERED_ON, iokit_common_msg(0x300));
    }

    /// With nothing pending, `allow_sleep` must be inert — it runs on every
    /// wake path and after a failed registration.
    #[test]
    fn allow_sleep_without_a_pending_request_does_nothing() {
        if let Ok(mut p) = PENDING.lock() {
            *p = None;
        }
        allow_sleep();
        allow_sleep();
        assert!(PENDING.lock().unwrap().is_none());
    }

    /// The token is taken, not merely read: a second ack for the same sleep
    /// would be answering a request that no longer exists.
    #[test]
    fn allow_sleep_consumes_the_token() {
        if let Ok(mut p) = PENDING.lock() {
            *p = Some(42);
        }
        // ROOT_PORT is 0 under test, so no syscall happens — but the token
        // must still be cleared.
        allow_sleep();
        assert!(PENDING.lock().unwrap().is_none(), "token must be taken");
    }
}
