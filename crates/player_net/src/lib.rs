// Rust side: Create a C-compatible FFI layer (lib.rs)

use std::ffi::{c_char, CStr, CString};
use std::os::raw::c_void;
use std::ptr;
use std::sync::{Arc, Mutex};

// Import the RtspPlayer implementation
// ... (existing RtspPlayer code would be here)

// FFI-safe player handle
pub struct PlayerHandle {
    player: Arc<RtspPlayer>,
}

// Exported C interface
#[no_mangle]
pub extern "C" fn rtsp_player_create(url: *const c_char) -> *mut PlayerHandle {
    if url.is_null() {
        return ptr::null_mut();
    }
    
    let c_url = unsafe { CStr::from_ptr(url) };
    let url_str = match c_url.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };
    
    match RtspPlayer::new(url_str) {
        Ok(player) => {
            let handle = Box::new(PlayerHandle {
                player: Arc::new(player),
            });
            Box::into_raw(handle)
        },
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn rtsp_player_destroy(handle: *mut PlayerHandle) {
    if !handle.is_null() {
        unsafe {
            let _ = Box::from_raw(handle);
        }
    }
}

#[no_mangle]
pub extern "C" fn rtsp_player_play(handle: *mut PlayerHandle) -> bool {
    if handle.is_null() {
        return false;
    }
    
    let handle = unsafe { &*handle };
    match handle.player.play() {
        Ok(_) => true,
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn rtsp_player_pause(handle: *mut PlayerHandle) -> bool {
    if handle.is_null() {
        return false;
    }
    
    let handle = unsafe { &*handle };
    match handle.player.pause() {
        Ok(_) => true,
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn rtsp_player_stop(handle: *mut PlayerHandle) -> bool {
    if handle.is_null() {
        return false;
    }
    
    let handle = unsafe { &*handle };
    match handle.player.stop() {
        Ok(_) => true,
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn rtsp_player_set_hwnd(handle: *mut PlayerHandle, hwnd: *mut c_void) -> bool {
    if handle.is_null() || hwnd.is_null() {
        return false;
    }
    
    let handle = unsafe { &*handle };
    
    // Get the video sink from the pipeline
    match handle.player.pipeline.by_name("videosink") {
        Some(sink) => {
            // Set the window handle on the video sink
            sink.set_property("window-handle", hwnd as u64);
            true
        },
        None => false,
    }
}

#[no_mangle]
pub extern "C" fn rtsp_player_get_last_error() -> *mut c_char {
    // Implementation to return last error message
    // For a real implementation, you would maintain a thread-local error message
    let error = CString::new("No error").unwrap();
    error.into_raw()
}

#[no_mangle]
pub extern "C" fn rtsp_player_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

