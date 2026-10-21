//! The entire C ABI boundary. Accessed exclusively by one native worker thread.
use crate::TelegramError;
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_int},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroize;

static RECEIVER_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) struct ReceiverPermit;
impl ReceiverPermit {
    pub fn acquire() -> Result<Self, TelegramError> {
        RECEIVER_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| TelegramError::AlreadyRunning)?;
        Ok(Self)
    }
}
impl Drop for ReceiverPermit {
    fn drop(&mut self) {
        RECEIVER_ACTIVE.store(false, Ordering::Release);
    }
}

pub(crate) struct Native {
    // TDLib's background threads may outlive authorizationStateClosed briefly.
    // Keep the shared library loaded for the process lifetime (never dlclose).
    _library: &'static Library,
    create: unsafe extern "C" fn() -> c_int,
    send: unsafe extern "C" fn(c_int, *const c_char),
    receive: unsafe extern "C" fn(f64) -> *const c_char,
    execute: unsafe extern "C" fn(*const c_char) -> *const c_char,
}
impl Native {
    pub fn load(path: &Path) -> Result<Self, TelegramError> {
        // SAFETY: user/config selects an installed trusted TDLib library. Loading a
        // native library necessarily trusts its initialization code and C ABI.
        let library = unsafe { Library::new(path) }.map_err(|_| TelegramError::NativeLibrary)?;
        // SAFETY: symbol signatures exactly match the pinned td_json_client.h C ABI.
        // Function pointers are copied and the library remains loaded permanently.
        let (create, send, receive, execute) = unsafe {
            (
                *library
                    .get::<unsafe extern "C" fn() -> c_int>(b"td_create_client_id\0")
                    .map_err(|_| TelegramError::NativeLibrary)?,
                *library
                    .get::<unsafe extern "C" fn(c_int, *const c_char)>(b"td_send\0")
                    .map_err(|_| TelegramError::NativeLibrary)?,
                *library
                    .get::<unsafe extern "C" fn(f64) -> *const c_char>(b"td_receive\0")
                    .map_err(|_| TelegramError::NativeLibrary)?,
                *library
                    .get::<unsafe extern "C" fn(*const c_char) -> *const c_char>(b"td_execute\0")
                    .map_err(|_| TelegramError::NativeLibrary)?,
            )
        };
        Ok(Self {
            _library: Box::leak(Box::new(library)),
            create,
            send,
            receive,
            execute,
        })
    }
    pub fn initialize(&self) -> Result<i32, TelegramError> {
        let request = c"{\"@type\":\"setLogStream\",\"log_stream\":{\"@type\":\"logStreamEmpty\"}}";
        // SAFETY: static C string; execute is called only on this worker, and its
        // result is copied before any subsequent execute/receive invalidates it.
        let result = unsafe { (self.execute)(request.as_ptr()) };
        let response = self
            .copy_response(result)?
            .ok_or(TelegramError::InvalidResponse)?;
        crate::check_response(response)?;
        // SAFETY: valid function pointer, no arguments, worker holds the receiver permit.
        let id = unsafe { (self.create)() };
        if id <= 0 {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(id)
    }
    pub fn send(&self, id: i32, value: &serde_json::Value) -> Result<(), TelegramError> {
        let bytes = serde_json::to_vec(value).map_err(|_| TelegramError::InvalidInput)?;
        let request = CString::new(bytes).map_err(|_| TelegramError::InvalidInput)?;
        // SAFETY: live client id, valid NUL-terminated request, TDLib copies the
        // request before returning. All native calls remain on this worker.
        unsafe {
            (self.send)(id, request.as_ptr());
        }
        let mut bytes = request.into_bytes_with_nul();
        bytes.zeroize();
        Ok(())
    }
    pub fn receive(&self) -> Result<Option<serde_json::Value>, TelegramError> {
        // SAFETY: exactly one worker holds ReceiverPermit. TDLib bounds the wait
        // and returns a borrowed NUL-terminated pointer valid until the next call.
        let pointer = unsafe { (self.receive)(0.05) };
        self.copy_response(pointer)
    }
    fn copy_response(
        &self,
        pointer: *const c_char,
    ) -> Result<Option<serde_json::Value>, TelegramError> {
        if pointer.is_null() {
            return Ok(None);
        }
        // SAFETY: only called with TDLib's documented live C string, immediately
        // after execute/receive, before the next native call. Never free this pointer.
        let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(TelegramError::InvalidResponse);
        }
        serde_json::from_slice(bytes)
            .map(Some)
            .map_err(|_| TelegramError::InvalidResponse)
    }
}
