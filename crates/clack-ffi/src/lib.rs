//! C ABI over `clack-core`. The header is `include/clack.h`.
//! Strings this library returns are owned by the caller and freed with
//! `clack_string_free`. The access token is not logged.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use clack_core::{input_devices, normalize_model_ref, Clack, SpeechActivity, TranscriptUpdate, LEVEL_BARS};

pub struct ClackSession {
    inner: Clack,
    pending: VecDeque<TranscriptUpdate>,
}

fn c_string(text: impl AsRef<str>) -> *mut c_char {
    CString::new(text.as_ref().replace('\0', ""))
        .unwrap_or_else(|_| CString::new("").expect("empty string"))
        .into_raw()
}

fn borrow<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(ptr).to_str().ok() }
    }
}

fn set_error(slot: *mut *mut c_char, message: &str) {
    if !slot.is_null() {
        unsafe { *slot = c_string(message) };
    }
}

fn ok_or(result: clack_core::ClackResult<()>, error: *mut *mut c_char) -> i32 {
    match result {
        Ok(()) => 1,
        Err(err) => {
            set_error(error, &err.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn clack_normalize_model_ref(input: *const c_char, error: *mut *mut c_char) -> *mut c_char {
    let Some(input) = borrow(input) else {
        set_error(error, "model string is null");
        return ptr::null_mut();
    };
    match normalize_model_ref(input) {
        Ok(repo) => c_string(repo),
        Err(err) => {
            set_error(error, &err.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn clack_create(
    model: *const c_char,
    language: *const c_char,
    token: *const c_char,
    error: *mut *mut c_char,
) -> *mut ClackSession {
    let (Some(model), Some(language)) = (borrow(model), borrow(language)) else {
        set_error(error, "model and language are required");
        return ptr::null_mut();
    };
    let token = borrow(token).filter(|token| !token.is_empty());
    match Clack::new_with_token(model, language, token) {
        Ok(inner) => Box::into_raw(Box::new(ClackSession {
            inner,
            pending: VecDeque::new(),
        })),
        Err(err) => {
            set_error(error, &err.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn clack_free(session: *mut ClackSession) {
    if !session.is_null() {
        unsafe { drop(Box::from_raw(session)) };
    }
}

#[no_mangle]
pub extern "C" fn clack_set_language(
    session: *mut ClackSession,
    language: *const c_char,
    error: *mut *mut c_char,
) -> i32 {
    let (Some(session), Some(language)) = (session_mut(session), borrow(language)) else {
        set_error(error, "session or language is null");
        return 0;
    };
    ok_or(session.inner.set_language(language), error)
}

#[no_mangle]
pub extern "C" fn clack_set_input_device(
    session: *mut ClackSession,
    device_id: *const c_char,
    error: *mut *mut c_char,
) -> i32 {
    let Some(session) = session_mut(session) else {
        set_error(error, "session is null");
        return 0;
    };
    let device = borrow(device_id).filter(|id| !id.is_empty());
    ok_or(session.inner.set_input_device(device), error)
}

#[no_mangle]
pub extern "C" fn clack_start_listening(session: *mut ClackSession, error: *mut *mut c_char) -> i32 {
    call(session, error, |session| session.inner.start_listening())
}

#[no_mangle]
pub extern "C" fn clack_stop_listening(session: *mut ClackSession, error: *mut *mut c_char) -> i32 {
    call(session, error, |session| session.inner.stop_listening())
}

#[no_mangle]
pub extern "C" fn clack_cancel_listening(session: *mut ClackSession, error: *mut *mut c_char) -> i32 {
    call(session, error, |session| session.inner.cancel_listening())
}

#[no_mangle]
pub extern "C" fn clack_push_audio(
    session: *mut ClackSession,
    samples: *const f32,
    count: usize,
    sample_rate: u32,
    error: *mut *mut c_char,
) -> i32 {
    let Some(session) = session_mut(session) else {
        set_error(error, "session is null");
        return 0;
    };
    if samples.is_null() && count > 0 {
        set_error(error, "samples pointer is null");
        return 0;
    }
    let audio = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(samples, count) }
    };
    ok_or(session.inner.push_audio(audio, sample_rate), error)
}

#[no_mangle]
pub extern "C" fn clack_is_listening(session: *const ClackSession) -> i32 {
    session_ref(session).is_some_and(|session| session.inner.is_listening()) as i32
}

#[no_mangle]
pub extern "C" fn clack_transcript(session: *const ClackSession) -> *mut c_char {
    session_ref(session)
        .map(|session| c_string(session.inner.transcript()))
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn clack_last_error(session: *const ClackSession) -> *mut c_char {
    session_ref(session)
        .and_then(|session| session.inner.last_error().map(c_string))
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn clack_repo_id(session: *const ClackSession) -> *mut c_char {
    session_ref(session)
        .map(|session| c_string(session.inner.repo_id()))
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn clack_poll(
    session: *mut ClackSession,
    is_final: *mut i32,
    cancelled: *mut i32,
    text: *mut *mut c_char,
) -> i32 {
    let Some(session) = session_mut(session) else {
        return 0;
    };
    if session.pending.is_empty() {
        session.pending.extend(session.inner.poll());
    }
    let Some(update) = session.pending.pop_front() else {
        return 0;
    };
    if !is_final.is_null() {
        unsafe { *is_final = update.is_final as i32 };
    }
    if !cancelled.is_null() {
        unsafe { *cancelled = update.cancelled as i32 };
    }
    if !text.is_null() {
        unsafe { *text = c_string(update.text) };
    }
    1
}

#[no_mangle]
pub extern "C" fn clack_speech_activity(session: *const ClackSession, out: *mut ClackActivity) {
    if out.is_null() {
        return;
    }
    let activity = session_ref(session)
        .map(|session| session.inner.speech_activity())
        .unwrap_or_default();
    unsafe { write_activity(out, &activity) };
}

#[repr(C)]
pub struct ClackActivity {
    pub speaking: i32,
    pub levels: [f32; LEVEL_BARS],
}

fn write_activity(out: *mut ClackActivity, activity: &SpeechActivity) {
    let mut levels = [0.0_f32; LEVEL_BARS];
    for (index, level) in activity.levels.iter().take(LEVEL_BARS).enumerate() {
        levels[index] = *level;
    }
    unsafe {
        *out = ClackActivity {
            speaking: activity.speaking as i32,
            levels,
        };
    }
}

#[no_mangle]
pub extern "C" fn clack_input_device_count(error: *mut *mut c_char) -> i32 {
    match input_devices() {
        Ok(devices) => devices.len() as i32,
        Err(err) => {
            set_error(error, &err.to_string());
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn clack_input_device_id(index: i32, error: *mut *mut c_char) -> *mut c_char {
    device_field(index, error, |device| device.id)
}

#[no_mangle]
pub extern "C" fn clack_input_device_name(index: i32, error: *mut *mut c_char) -> *mut c_char {
    device_field(index, error, |device| device.name)
}

#[no_mangle]
pub extern "C" fn clack_input_device_is_default(index: i32) -> i32 {
    input_devices()
        .ok()
        .and_then(|devices| devices.into_iter().nth(index as usize))
        .is_some_and(|device| device.is_default) as i32
}

#[no_mangle]
pub extern "C" fn clack_string_free(text: *mut c_char) {
    if !text.is_null() {
        unsafe { drop(CString::from_raw(text)) };
    }
}

fn device_field(index: i32, error: *mut *mut c_char, field: impl Fn(clack_core::InputDevice) -> String) -> *mut c_char {
    match input_devices() {
        Ok(devices) => match devices.into_iter().nth(index as usize) {
            Some(device) => c_string(field(device)),
            None => {
                set_error(error, "input device index is out of range");
                ptr::null_mut()
            }
        },
        Err(err) => {
            set_error(error, &err.to_string());
            ptr::null_mut()
        }
    }
}

fn session_mut<'a>(session: *mut ClackSession) -> Option<&'a mut ClackSession> {
    if session.is_null() {
        None
    } else {
        Some(unsafe { &mut *session })
    }
}

fn session_ref<'a>(session: *const ClackSession) -> Option<&'a ClackSession> {
    if session.is_null() {
        None
    } else {
        Some(unsafe { &*session })
    }
}

fn call(
    session: *mut ClackSession,
    error: *mut *mut c_char,
    body: impl FnOnce(&mut ClackSession) -> clack_core::ClackResult<()>,
) -> i32 {
    let Some(session) = session_mut(session) else {
        set_error(error, "session is null");
        return 0;
    };
    ok_or(body(session), error)
}
