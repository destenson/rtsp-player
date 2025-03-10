use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video::prelude::VideoOverlayExtManual;
use gstreamer_video as gst_video;
use player::RtspPlayer;
use std::env;
use std::error::Error;
use std::os::raw::c_void;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::time::Duration;
use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::UI::Controls::*,
    Win32::UI::WindowsAndMessaging::*,
    Win32::Graphics::Gdi::*,
    Win32::System::LibraryLoader::GetModuleHandleA,
};


extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_CREATE {
        // Store the RtspPlayer instance pointer in the window's user data
        let create_struct = unsafe { &*(lparam.0 as *const CREATESTRUCTA) };
        let player_ptr_ptr = create_struct.lpCreateParams as *const *const RtspPlayer;
        let player_ptr = unsafe { *player_ptr_ptr };
        
        unsafe {
            SetWindowLongPtrA(hwnd, GWLP_USERDATA, player_ptr as isize);
        }
        
        return LRESULT(0);
    }
    
    // Get the RtspPlayer instance from the window's user data
    let player_ptr = unsafe { GetWindowLongPtrA(hwnd, GWLP_USERDATA) } as *const RtspPlayer;
    
    if !player_ptr.is_null() {
        let player = unsafe { &*player_ptr };
        return player.handle_window_message(hwnd, message, wparam, lparam);
    }
    
    unsafe { DefWindowProcA(hwnd, message, wparam, lparam) }
}

fn main() -> std::result::Result<(), Box<dyn Error>> {
    // Get the RTSP URL from command line or use a default
    let args: Vec<String> = env::args().collect();
    let rtsp_url = if args.len() > 1 {
        args[1].clone()
    } else {
        String::from("rtsp://127.0.0.1:8554/live.sdp") // Default URL
    };
    
    println!("Initializing RTSP player for: {}", rtsp_url);
    
    // Create the RTSP player
    let player = RtspPlayer::new(&rtsp_url)?;
    
    // Set up GUI
    player.create_gui(Some(window_proc))?;
    
    // Set up message handling
    player.setup_message_handling()?;
    
    // Start playback
    player.play()?;
    
    // Windows message loop
    unsafe {
        let mut msg = MSG::default();
        while GetMessageA(&mut msg, None, 0, 0).into() {
            let r = TranslateMessage(&msg);
            if r.0 != 0 {
                println!("TranslateMessage returned {}", r.0);
            }
            DispatchMessageA(&msg);
        }
    }
    
    // Clean up
    player.stop()?;
    
    Ok(())
}

// Helper function to get LOWORD
fn LOWORD(dword: u32) -> u16 {
    (dword & 0xFFFF) as u16
}

// Helper function to get HIWORD
fn HIWORD(dword: u32) -> u16 {
    ((dword >> 16) & 0xFFFF) as u16
}

// Helper function for MAKELONG
fn MAKELONG(low: u16, high: u16) -> u32 {
    ((high as u32) << 16) | (low as u32)
}

// For testing
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_player() {
        let test_url = "rtsp://127.0.0.1:8554/test.sdp";
        let player = RtspPlayer::new(test_url);
        assert!(player.is_ok(), "Should be able to create a player instance");
    }
}
