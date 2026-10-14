//! Touch ID through LocalAuthentication (`LAPolicyDeviceOwnerAuthentication`: Touch ID, or the
//! macOS account password when Touch ID is unavailable or declined, as TablePlus's "Touch ID /
//! computer password"), and the system notifications that lock the app (sleep, screen lock).
//!
//! `authenticate` blocks until the user answers the system dialog: call it from a blocking task,
//! never from the main thread.

use std::time::Duration;

use crate::error::AppError;

/// Emitted to every window when the app must lock now: the Mac or its screens go to sleep, the
/// screen locks, the user session is switched away, or "Lock Tabler" was chosen in a window.
/// Payload: the reason (`sleep`, `screens`, `screen-locked`, `session`, `user`).
pub const EVENT_SYSTEM_LOCK: &str = "lock:system";

/// How long the system dialog may wait for an answer.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest reason shown in the system dialog ("Tabler is trying to <reason>.").
pub const MAX_REASON_CHARS: usize = 200;

/// The reason as the system dialog shows it: trimmed, a fallback when empty, capped.
pub fn reason_text(reason: &str) -> String {
    let trimmed = reason.trim();
    let text = if trimmed.is_empty() {
        "unlock Tabler"
    } else {
        trimmed
    };
    text.chars().take(MAX_REASON_CHARS).collect()
}

/// Whether this Mac can authenticate the owner (Touch ID or the account password).
pub fn available() -> bool {
    imp::available()
}

/// Shows the system authentication dialog with `reason` and waits for the answer: true when the
/// owner authenticated, false when they cancelled. Blocking.
pub fn authenticate(reason: &str) -> Result<bool, AppError> {
    imp::authenticate(&reason_text(reason))
}

#[cfg(target_os = "macos")]
pub use imp::watch_system_lock;

/// Other platforms have no system lock notifications to watch.
#[cfg(not(target_os = "macos"))]
pub fn watch_system_lock<R: tauri::Runtime>(_app: tauri::AppHandle<R>) {}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CString, c_char};
    use std::ptr::NonNull;
    use std::sync::Mutex;
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use tauri::{AppHandle, Emitter, Runtime};

    use super::{AUTH_TIMEOUT, EVENT_SYSTEM_LOCK};
    use crate::error::AppError;

    #[link(name = "LocalAuthentication", kind = "framework")]
    unsafe extern "C" {}

    /// `LAPolicyDeviceOwnerAuthentication`: biometrics, a watch, or the account password.
    const LA_POLICY_DEVICE_OWNER_AUTHENTICATION: isize = 2;

    fn la_context() -> Result<Retained<AnyObject>, AppError> {
        let class = AnyClass::get(c"LAContext")
            .ok_or_else(|| AppError::Unsupported("LocalAuthentication is not available".into()))?;
        // SAFETY: `+[LAContext new]` takes no argument and returns a new (+1) object or nil.
        let ctx: Option<Retained<AnyObject>> = unsafe { msg_send![class, new] };
        ctx.ok_or_else(|| AppError::internal("could not create an LAContext"))
    }

    fn can_evaluate(ctx: &AnyObject) -> bool {
        let no_error: *mut *mut AnyObject = std::ptr::null_mut();
        // SAFETY: `-canEvaluatePolicy:error:` takes an NSInteger policy and a nullable NSError**
        // (NULL: the error is not wanted) and returns BOOL.
        let ok: Bool = unsafe {
            msg_send![ctx, canEvaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION, error: no_error]
        };
        ok.as_bool()
    }

    fn ns_string(text: &str) -> Result<Retained<AnyObject>, AppError> {
        let c = CString::new(text.replace('\0', " "))
            .map_err(|_| AppError::InvalidInput("invalid text".into()))?;
        let class = AnyClass::get(c"NSString").ok_or_else(|| AppError::internal("no NSString"))?;
        let ptr: NonNull<c_char> =
            NonNull::new(c.as_ptr().cast_mut()).ok_or_else(|| AppError::internal("null text"))?;
        // SAFETY: `+stringWithUTF8String:` copies the NUL-terminated UTF-8 bytes, which outlive
        // the call, and returns an autoreleased NSString (retained here) or nil.
        let s: Option<Retained<AnyObject>> = unsafe { msg_send![class, stringWithUTF8String: ptr] };
        s.ok_or_else(|| AppError::internal("could not create an NSString"))
    }

    pub fn available() -> bool {
        autoreleasepool(|_| la_context().map(|ctx| can_evaluate(&ctx)).unwrap_or(false))
    }

    pub fn authenticate(reason: &str) -> Result<bool, AppError> {
        let (tx, rx) = mpsc::channel::<bool>();
        // The context must outlive the dialog: it is kept until the reply arrived.
        let ctx = autoreleasepool(|_| -> Result<Retained<AnyObject>, AppError> {
            let ctx = la_context()?;
            if !can_evaluate(&ctx) {
                return Err(AppError::Unsupported(
                    "Touch ID and the account password are not available on this Mac".into(),
                ));
            }
            let reason = ns_string(reason)?;
            // Called once, on a private queue; the sender is taken so a second call is a no-op.
            let sender = Mutex::new(Some(tx));
            let reply = RcBlock::new(move |success: Bool, _error: *mut AnyObject| {
                let tx = sender.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some(tx) = tx {
                    let _ = tx.send(success.as_bool());
                }
            });
            // SAFETY: `-evaluatePolicy:localizedReason:reply:` takes an NSInteger, an NSString
            // and a `void (^)(BOOL, NSError *)` block, which it copies; it returns void and
            // calls the block once when the user answered.
            unsafe {
                let _: () = msg_send![
                    &*ctx,
                    evaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION,
                    localizedReason: &*reason,
                    reply: &*reply
                ];
            }
            Ok(ctx)
        })?;
        let answer = rx.recv_timeout(AUTH_TIMEOUT);
        drop(ctx);
        match answer {
            Ok(ok) => Ok(ok),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(AppError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(false),
        }
    }

    /// Observes `name` on `center` for the app's lifetime; each post emits `EVENT_SYSTEM_LOCK`.
    fn observe<R: Runtime>(
        center: &AnyObject,
        name: &str,
        reason: &'static str,
        app: AppHandle<R>,
    ) -> Result<(), AppError> {
        let name = ns_string(name)?;
        let block = RcBlock::new(move |_note: NonNull<AnyObject>| {
            log::info!("app lock: system event {reason}");
            let _ = app.emit(EVENT_SYSTEM_LOCK, reason);
        });
        let none: Option<&AnyObject> = None;
        // SAFETY: `-addObserverForName:object:queue:usingBlock:` takes the name (NSString), a
        // nullable sender and queue (nil: any sender, the posting thread) and a
        // `void (^)(NSNotification *)` block it copies; it returns the observer token.
        let token: Option<Retained<AnyObject>> = unsafe {
            msg_send![
                center,
                addObserverForName: &*name,
                object: none,
                queue: none,
                usingBlock: &*block
            ]
        };
        // Observed until the app quits: the token is never removed.
        std::mem::forget(token);
        Ok(())
    }

    fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), AppError> {
        let workspace_class =
            AnyClass::get(c"NSWorkspace").ok_or_else(|| AppError::internal("no NSWorkspace"))?;
        // SAFETY: `+sharedWorkspace` and `-notificationCenter` take no argument and return
        // objects that live for the whole process.
        let workspace: Retained<AnyObject> = unsafe { msg_send![workspace_class, sharedWorkspace] };
        let center: Retained<AnyObject> = unsafe { msg_send![&*workspace, notificationCenter] };
        observe(
            &center,
            "NSWorkspaceWillSleepNotification",
            "sleep",
            app.clone(),
        )?;
        observe(
            &center,
            "NSWorkspaceScreensDidSleepNotification",
            "screens",
            app.clone(),
        )?;
        observe(
            &center,
            "NSWorkspaceSessionDidResignActiveNotification",
            "session",
            app.clone(),
        )?;
        let distributed = AnyClass::get(c"NSDistributedNotificationCenter")
            .ok_or_else(|| AppError::internal("no NSDistributedNotificationCenter"))?;
        // SAFETY: `+defaultCenter` takes no argument and returns the process-wide center.
        let center: Retained<AnyObject> = unsafe { msg_send![distributed, defaultCenter] };
        observe(
            &center,
            "com.apple.screenIsLocked",
            "screen-locked",
            app.clone(),
        )
    }

    /// Starts watching the system's sleep / screen-lock notifications. Call once, from `setup`
    /// (the main thread).
    pub fn watch_system_lock<R: Runtime>(app: AppHandle<R>) {
        match autoreleasepool(|_| install(&app)) {
            Ok(()) => log::debug!("app lock: watching sleep and screen lock"),
            Err(e) => log::warn!("app lock: cannot watch sleep / screen lock: {e}"),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use crate::error::AppError;

    pub fn available() -> bool {
        false
    }

    pub fn authenticate(_reason: &str) -> Result<bool, AppError> {
        Err(AppError::Unsupported(
            "Touch ID is only available on macOS".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_text_is_trimmed_capped_and_never_empty() {
        assert_eq!(reason_text("  unlock Safe Mode "), "unlock Safe Mode");
        assert_eq!(reason_text("   "), "unlock Tabler");
        assert_eq!(
            reason_text(&"x".repeat(MAX_REASON_CHARS + 50))
                .chars()
                .count(),
            MAX_REASON_CHARS
        );
    }
}
