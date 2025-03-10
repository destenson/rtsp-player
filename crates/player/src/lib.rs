

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video::prelude::VideoOverlayExtManual;
use gstreamer_video as gst_video;
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

#[derive(Debug, Default, Clone, PartialEq)]
struct VideoInfo {
    width: i32,
    height: i32,
    framerate: f64,
    codec: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GuiControls {
    window: Option<HWND>,
    video_window: Option<HWND>,
    play_button: Option<HWND>,
    pause_button: Option<HWND>,
    stop_button: Option<HWND>,
    seekbar: Option<HWND>,
    status_text: Option<HWND>,
    overlay_text: Option<HWND>,
}



enum PlayerMessage {
    EndOfStream,
    Error(String),
    StreamStarted,
    Buffering(i32),
    StateChanged(gst::State),
    VideoInfo(i32, i32, f64, String),
    Reconnecting(u32),
    ConnectionFailed,
    PositionUpdate(u64, u64), // position, duration
}

// Custom error type for better error handling
#[derive(Debug)]
enum PlayerError {
    InitError(String),
    StreamError(String),
    ConnectionError(String),
    WindowsError(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PlayerError::InitError(msg) => write!(f, "Initialization error: {}", msg),
            PlayerError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            PlayerError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            PlayerError::WindowsError(msg) => write!(f, "Windows API error: {}", msg),
        }
    }
}

impl Error for PlayerError {}

const ID_PLAY_BUTTON: u16 = 101;
const ID_PAUSE_BUTTON: u16 = 102;
const ID_STOP_BUTTON: u16 = 103;
const ID_SEEKBAR: u16 = 104;
const ID_STATUS_TEXT: u16 = 105;
const ID_VIDEO_WINDOW: u16 = 106;

#[derive(Debug)]
pub struct RtspPlayer {
    pipeline: gst::Pipeline,
    is_playing: Arc<Mutex<bool>>,
    reconnect_attempts: Arc<Mutex<u32>>,
    url: String,
    video_info: Arc<Mutex<Option<VideoInfo>>>,
    position: Arc<Mutex<u64>>,
    duration: Arc<Mutex<u64>>,
    gui_controls: Arc<Mutex<Option<GuiControls>>>,
    video_window: Arc<Mutex<Option<HWND>>>,
    video_sink_widget: Arc<Mutex<Option<HWND>>>,
    message_sender: Arc<Mutex<Sender<PlayerMessage>>>,
    message_receiver: Receiver<PlayerMessage>,
}

impl RtspPlayer {
    pub fn new(url: &str) -> std::result::Result<Self, Box<dyn Error>> {
        // Initialize GStreamer if not already initialized
        if gst::init().is_err() {
            return Err(Box::new(PlayerError::InitError("Failed to initialize GStreamer".into())));
        }

        // Create a more robust pipeline with better error handling and reconnection
        // Use d3dvideosink for Windows DirectX rendering
        let pipeline_str = format!(
            "rtspsrc location={} latency=100 protocols=tcp+udp+http buffer-mode=auto retry=5 timeout=5000000 ! 
             rtpjitterbuffer ! queue max-size-buffers=3000 max-size-time=0 max-size-bytes=0 ! 
             decodebin ! videoconvert ! d3d11videosink sync=true name=videosink",
            url
        );

        let pipeline = gst::parse::launch(&pipeline_str)?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| PlayerError::InitError("Failed to create pipeline".into()))?;

        let (sender, receiver) = channel::<PlayerMessage>();

        Ok(RtspPlayer {
            pipeline,
            is_playing: Arc::new(Mutex::new(false)),
            reconnect_attempts: Arc::new(Mutex::new(0)),
            url: url.to_string(),
            video_info: Arc::new(Mutex::new(None)),
            position: Arc::new(Mutex::new(0)),
            duration: Arc::new(Mutex::new(0)),
            gui_controls: Arc::new(Mutex::new(None)),
            video_window: Arc::new(Mutex::new(None)),
            video_sink_widget: Arc::new(Mutex::new(None)),
            message_sender: Arc::new(Mutex::new(sender)),
            message_receiver: receiver,
        })
    }

    pub fn create_gui(&self, window_proc: WNDPROC) -> std::result::Result<(), Box<dyn Error>> {
        let instance = unsafe { GetModuleHandleA(None)? };
        
        // Register window class
        let class_name = PCSTR(b"RTSPPlayerWindowClass\0".as_ptr());
        // let hbrBackground = HBRUSH(COLOR_WINDOW.0);
        let hInstance = HINSTANCE(instance.0);
        
        let wc = WNDCLASSA {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: window_proc,
            hInstance,
            lpszClassName: class_name,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            // hbrBackground,
            ..Default::default()
        };
        
        if unsafe { RegisterClassA(&wc) } == 0 {
            return Err(Box::new(PlayerError::WindowsError("Failed to register window class".into())));
        }
        
        // Store self pointer for the window procedure to access
        let player_ptr = Box::into_raw(Box::new(self as *const _));
        
        // Create main window
        let window = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                class_name,
                PCSTR(b"RTSP Player\0".as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT, CW_USEDEFAULT, 800, 600,
                None,
                None,
                Some(hInstance),
                Some(player_ptr as *const _),
            )
        }?;
        
        if window.0.is_null() {
            return Err(Box::new(PlayerError::WindowsError("Failed to create window".into())));
        }

        // let hwndparent = HWND(window.0);
        // let hmenu = HMENU(ID_VIDEO_WINDOW as isize);
        // let hmenu = unsafe {CreateMenu()}?;
        // Menu
        
        // Create video window
        let video_window = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR(b"STATIC\0".as_ptr()),
                PCSTR(b"\0".as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_BORDER,
                0, 0, 800, 500,
                Some(window),
                Some(HMENU(ID_VIDEO_WINDOW as *mut c_void)),
                Some(hInstance),
                None,
            )
        }?;

        const BS_DEFPUSHBUTTON: WINDOW_STYLE = WINDOW_STYLE(windows::Win32::UI::WindowsAndMessaging::BS_DEFPUSHBUTTON as u32);
        
        // Create control buttons
        let play_button = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR(b"BUTTON\0".as_ptr()),
                PCSTR(b"Play\0".as_ptr()),
                WS_TABSTOP | WS_VISIBLE | WS_CHILD | BS_DEFPUSHBUTTON,
                10, 510, 100, 30,
                Some(window),
                None, //Some(HMENU(ID_PLAY_BUTTON as isize)),
                Some(hInstance),
                None,
            )
        }?;
        
        let pause_button = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR(b"BUTTON\0".as_ptr()),
                PCSTR(b"Pause\0".as_ptr()),
                WS_TABSTOP | WS_VISIBLE | WS_CHILD | BS_DEFPUSHBUTTON,
                120, 510, 100, 30,
                Some(window),
                None, //HMENU(ID_PAUSE_BUTTON as isize),
                Some(hInstance),
                None,
            )
        }?;
        
        let stop_button = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR(b"BUTTON\0".as_ptr()),
                PCSTR(b"Stop\0".as_ptr()),
                WS_TABSTOP | WS_VISIBLE | WS_CHILD | BS_DEFPUSHBUTTON,
                230, 510, 100, 30,
                Some(window),
                None, //HMENU(ID_STOP_BUTTON as isize),
                Some(hInstance),
                None,
            )
        }?;
        
        // Create seekbar (trackbar control)
        let seekbar = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR(b"msctls_trackbar32\0".as_ptr()),
                PCSTR(b"\0".as_ptr()),
                WS_CHILD | WS_VISIBLE,// | TBS_HORZ,
                340, 510, 300, 30,
                Some(window),
                None, //HMENU(ID_SEEKBAR as isize),
                Some(hInstance),
                None,
            )
        }?;
        
        // Initialize seekbar range
        unsafe {
            SendMessageA(seekbar, TBM_SETRANGE, WPARAM(0), LPARAM(1000));
        }
        
        // Create status text
        let status_text = unsafe {
            CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR(b"STATIC\0".as_ptr()),
                PCSTR(b"Ready\0".as_ptr()),
                WS_CHILD | WS_VISIBLE,// | SS_LEFT,
                10, 550, 780, 20,
                Some(window),
                None, //HMENU(ID_STATUS_TEXT as isize),
                Some(hInstance),
                None,
            )
        }?;

        let window = Some(window);
        let video_window = Some(video_window);
        let play_button = Some(play_button);
        let pause_button = Some(pause_button);
        let stop_button = Some(stop_button);
        let seekbar = Some(seekbar);
        let status_text = Some(status_text);
        let overlay_text = None;
        
        // Store controls
        *self.gui_controls.lock().unwrap() = Some(GuiControls {
            window,
            video_window,
            play_button,
            pause_button,
            stop_button,
            seekbar,
            status_text,
            overlay_text,
        });
        
        // Make the window visible
        unsafe {
            check_win_err()?;
            let r = ShowWindow(window.unwrap(), SW_SHOW);
            println!("ShowWindow result: {}", r.0);
            check_win_err()?;
            let r = UpdateWindow(window.unwrap());
            println!("UpdateWindow result: {}", r.0);
            check_win_err()?;
        }
        
        // Set up the GStreamer pipeline to render to our window
        // For d3dvideosink, we need to set the window handle
        let video_sink = self.pipeline
            .by_name("videosink")
            .ok_or_else(|| PlayerError::InitError("Could not find video sink".into()))?;

        if let Some(video_window) = video_window {
            // use the set_window_handle() function on the GstOverlay interface
            let video_sink = video_sink.dynamic_cast::<gst_video::VideoSink>().unwrap();
            // Set the window handle on the video sink
            let video_sink = video_sink.dynamic_cast::<gst_video::VideoOverlay>().unwrap();
    
            unsafe { video_sink.set_window_handle(video_window.0 as usize) };
        }
        
        // video_sink.call_async_future(
        //     "set_window_handle",
        //     &[&video_window.0 as &dyn ToValue],
        // )?;
        // // Set the window handle on the video sink
        // video_sink.set_property("window-handle", video_window.0 as u64);
        
        // Store video window
        *self.video_sink_widget.lock().unwrap() = video_window;
        
        Ok(())
    }

    pub fn play(&self) -> std::result::Result<(), Box<dyn Error>> {
        // Start the pipeline
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            if let Some(status_text) = controls.status_text {
                unsafe {
                    SetWindowTextA(status_text, PCSTR(b"Playing\0".as_ptr()));
                }
            }
        }
        
        Ok(())
    }
    
    pub fn pause(&self) -> std::result::Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Paused)?;
        *self.is_playing.lock().unwrap() = false;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            if let Some(status_text) = controls.status_text {
                unsafe {
                    SetWindowTextA(status_text, PCSTR(b"Paused\0".as_ptr()));
                }
            }
        }
        
        Ok(())
    }
    
    pub fn resume(&self) -> std::result::Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            if let Some(status_text) = controls.status_text {
                unsafe {
                    SetWindowTextA(status_text, PCSTR(b"Playing\0".as_ptr()));
                }
            }
        }
        
        Ok(())
    }
    
    pub fn stop(&self) -> std::result::Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Null)?;
        *self.is_playing.lock().unwrap() = false;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            if let Some(status_text) = controls.status_text {
                unsafe {
                    SetWindowTextA(status_text, PCSTR(b"Stopped\0".as_ptr()));
                }
            }
        }
        
        Ok(())
    }
    
    pub fn seek(&self, position_percent: f64) -> std::result::Result<(), Box<dyn Error>> {
        let duration = *self.duration.lock().unwrap();
        if duration > 0 {
            let position = gst::ClockTime::from_nseconds((position_percent * duration as f64) as u64);
            self.pipeline.seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                position,
            )?;
        }
        Ok(())
    }
    
    // fn setup_message_handling(&self) -> std::result::Result<(), Box<dyn Error>> {
    //     let bus = self.pipeline.bus().ok_or_else(|| 
    //         PlayerError::InitError("Failed to get pipeline bus".into())
    //     )?;
    //     
    //     let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
    //     let url_clone = self.url.clone();
    //     let is_playing = Arc::clone(&self.is_playing);
    //     let video_info = Arc::clone(&self.video_info);
    //     let gui_controls = Arc::clone(&self.gui_controls);
    //     let position = Arc::clone(&self.position);
    //     let duration = Arc::clone(&self.duration);
    //     let pipeline_clone = self.pipeline.clone();
    //     
    //     // Set up a timer for updating the position slider
    //     if let Some(controls) = &*gui_controls.lock().unwrap() {
    //         let window = controls.window;
    //         unsafe {
    //             SetTimer(Some(window), 1, 500, None);
    //         }
    //     }
    //     
    //     let _bus_watch = bus.add_watch(|_, msg| {
    //         use gstreamer::MessageView;
    //         // let gui_controls = gui_controls.get_mut().expect("could not get gui controls").expect("GUI controls not initialized").as_ref();
    //         (move || {
    //             match msg.view() {
    //                 MessageView::Eos(..) => {
    //                     println!("End of stream");
    //                     // let controls = gui_controls.clone();
    //                     if let Some(controls) = gui_controls.lock().unwrap().as_ref() {
    //                         unsafe {
    //                             SetWindowTextA(controls.status_text, PCSTR(b"End of stream\0".as_ptr()));
    //                         }
    //                     }
    //                     *is_playing.lock().unwrap() = false;
    //                 }
    //                 // MessageView::Error(err) => {
    //                 //     println!("Error: {} ({:?})", err.error(), err.debug());
    //                 //     
    //                 //     let controls = gui_controls.clone();
    //                 //     // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //         let error_msg = format!("Error: {}\0", err.error());
    //                 //         unsafe {
    //                 //             SetWindowTextA(controls.status_text, PCSTR(error_msg.as_ptr()));
    //                 //         }
    //                 //     // }
    //                 //     
    //                 //     // If currently playing, try to reconnect
    //                 //     if *is_playing.lock().unwrap() {
    //                 //         let mut attempts = reconnect_attempts.lock().unwrap();
    //                 //         if *attempts < 5 {
    //                 //             *attempts += 1;
    //                 //             println!("Attempting to reconnect (attempt {}/5)...", *attempts);
    //                 //             
    //                 //             let controls = gui_controls.clone();
    //                 //             // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //                 let reconnect_msg = format!("Reconnecting ({}/5)...\0", *attempts);
    //                 //                 unsafe {
    //                 //                     SetWindowTextA(controls.status_text, PCSTR(reconnect_msg.as_ptr()));
    //                 //                 }
    //                 //             // }
    //                 //             
    //                 //             // Reset the pipeline
    //                 //             let _ = pipeline_clone.set_state(gst::State::Null);
    //                 //             std::thread::sleep(Duration::from_secs(2));
    //                 //             
    //                 //             // Try to play again
    //                 //             let _ = pipeline_clone.set_state(gst::State::Playing);
    //                 //         } else {
    //                 //             println!("Max reconnection attempts reached, giving up");
    //                 //             let controls = gui_controls.clone();
    //                 //             // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //                 unsafe {
    //                 //                     SetWindowTextA(controls.status_text, PCSTR(b"Connection failed\0".as_ptr()));
    //                 //                 }
    //                 //             // }
    //                 //             *is_playing.lock().unwrap() = false;
    //                 //         }
    //                 //     }
    //                 // }
    //                 // MessageView::StateChanged(state_changed) => {
    //                 //     // Only process messages from the pipeline
    //                 //     if let Some(pipeline) = msg.src().and_then(|s| s.dynamic_cast::<gst::Pipeline>().ok()) {
    //                 //         if pipeline == pipeline_clone && state_changed.current() == gst::State::Playing {
    //                 //             // Reset reconnect counter when we successfully reach playing state
    //                 //             *reconnect_attempts.lock().unwrap() = 0;
    //                 //         }
    //                 //     }
    //                 // }
    //                 // MessageView::StreamStart(_) => {
    //                 //     println!("Stream started successfully");
    //                 //     let controls = gui_controls.clone();
    //                 //     // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //         unsafe {
    //                 //             SetWindowTextA(controls.status_text, PCSTR(b"Stream started\0".as_ptr()));
    //                 //         }
    //                 //     // }
    //                 // }
    //                 // MessageView::Buffering(buffering) => {
    //                 //     let percent = buffering.percent();
    //                 //     println!("Buffering... {}%", percent);
    //                 //     
    //                 //     let controls = gui_controls.clone();
    //                 //     // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //         let buffer_msg = format!("Buffering... {}%\0", percent);
    //                 //         unsafe {
    //                 //             SetWindowTextA(controls.status_text, PCSTR(buffer_msg.as_ptr()));
    //                 //         }
    //                 //     // }
    //                 //     
    //                 //     // Pause the pipeline if buffering and resume when done
    //                 //     if percent < 100 {
    //                 //         let _ = pipeline_clone.set_state(gst::State::Paused);
    //                 //     } else if *is_playing.lock().unwrap() {
    //                 //         let _ = pipeline_clone.set_state(gst::State::Playing);
    //                 //         let controls = gui_controls.clone();
    //                 //         // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //             unsafe {
    //                 //                 SetWindowTextA(controls.status_text, PCSTR(b"Playing\0".as_ptr()));
    //                 //             }
    //                 //         // }
    //                 //     }
    //                 // }
    //                 // MessageView::Element(element) => {
    //                 //     // Extract video information when available
    //                 //     if let Some(structure) = element.structure() {
    //                 //         if structure.name() == "video-info" {
    //                 //             if let (Some(width), Some(height), Some(framerate), Some(codec)) = (
    //                 //                 structure.get::<i32>("width").ok(),
    //                 //                 structure.get::<i32>("height").ok(),
    //                 //                 structure.get::<f64>("framerate").ok(),
    //                 //                 structure.get::<String>("codec").ok(),
    //                 //             ) {
    //                 //                 let mut info = video_info.lock().unwrap();
    //                 //                 *info = Some(VideoInfo {
    //                 //                     width,
    //                 //                     height,
    //                 //                     framerate,
    //                 //                     codec,
    //                 //                 });
    //                 //                 
    //                 //                 println!("Video info: {}x{} @ {:.2} fps, codec: {}", 
    //                 //                     width, height, framerate, codec);
    //                 //                     
    //                 //                 let controls = gui_controls.clone();
    //                 //                 // if let Some(controls) = &*gui_controls.lock().unwrap() {
    //                 //                     let info_text = format!("{}x{} @ {:.2} fps ({})\0", 
    //                 //                         width, height, framerate, codec);
    //                 //                     unsafe {
    //                 //                         SetWindowTextA(controls.status_text, PCSTR(info_text.as_ptr()));
    //                 //                     }
    //                 //                 // }
    //                 //             }
    //                 //         }
    //                 //     }
    //                 // }
    //                 MessageView::Qos(_) => {
    //                     // We could display QoS statistics here if needed
    //                 }
    //                 _ => (),
    //             }
    //             
    //         })();
    //         glib::ControlFlow::Continue
    //     })?;
    //     
    //     Ok(())
    // }
    
    pub fn setup_message_handling(&self) -> std::result::Result<(), Box<dyn Error>> {
        let bus = self.pipeline.bus().ok_or_else(|| 
            PlayerError::InitError("Failed to get pipeline bus".into())
        )?;
        
        // No longer need to share these with the bus watch
        // Just use the sender
        let sender = Arc::clone(&self.message_sender);
        let pipeline_clone = self.pipeline.clone();
        let is_playing_clone = Arc::clone(&self.is_playing);
        let reconnect_attempts_clone = Arc::clone(&self.reconnect_attempts); 
        let url_clone = self.url.clone();
        
        // Create a position update timer using Windows
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            let window = controls.window;
            unsafe {
                SetTimer(window, 1, 500, None); // Check for messages every 500ms
                SetTimer(window, 2, 500, None); // Update position every 500ms
            }
        }
        
        let _bus_watch = bus.add_watch(move |_, msg| {
            use gstreamer::MessageView;
            
            match msg.view() {
                MessageView::Eos(..) => {
                    println!("End of stream");
                    if let Ok(sender) = sender.lock() {
                        let _ = sender.send(PlayerMessage::EndOfStream);
                    }
                    *is_playing_clone.lock().unwrap() = false;
                }
                MessageView::Error(err) => {
                    println!("Error: {} ({:?})", err.error(), err.debug());
                    
                    if let Ok(sender) = sender.lock() {
                        let _ = sender.send(PlayerMessage::Error(err.error().to_string()));
                    }
                    
                    // If currently playing, try to reconnect
                    if *is_playing_clone.lock().unwrap() {
                        let mut attempts = reconnect_attempts_clone.lock().unwrap();
                        if *attempts < 5 {
                            *attempts += 1;
                            println!("Attempting to reconnect (attempt {}/5)...", *attempts);
                            
                            if let Ok(sender) = sender.lock() {
                                let _ = sender.send(PlayerMessage::Reconnecting(*attempts));
                            }
                            
                            // Reset the pipeline
                            let _ = pipeline_clone.set_state(gst::State::Null);
                            std::thread::sleep(Duration::from_secs(2));
                            
                            // Try to play again
                            let _ = pipeline_clone.set_state(gst::State::Playing);
                        } else {
                            println!("Max reconnection attempts reached, giving up");
                            if let Ok(sender) = sender.lock() {
                                let _ = sender.send(PlayerMessage::ConnectionFailed);
                            }
                            *is_playing_clone.lock().unwrap() = false;
                        }
                    }
                }
                MessageView::StateChanged(state_changed) => {
                    // Only process messages from the pipeline
                    if let Some(pipeline) = msg.src().and_then(|s| s.clone().dynamic_cast::<gst::Pipeline>().ok()) {
                        if pipeline == pipeline_clone {
                            if let Ok(sender) = sender.lock() {
                                let _ = sender.send(PlayerMessage::StateChanged(state_changed.current()));
                            }
                            
                            if state_changed.current() == gst::State::Playing {
                                // Reset reconnect counter when we successfully reach playing state
                                *reconnect_attempts_clone.lock().unwrap() = 0;
                            }
                        }
                    }
                }
                MessageView::StreamStart(_) => {
                    println!("Stream started successfully");
                    if let Ok(sender) = sender.lock() {
                        let _ = sender.send(PlayerMessage::StreamStarted);
                    }
                }
                MessageView::Buffering(buffering) => {
                    let percent = buffering.percent();
                    println!("Buffering... {}%", percent);
                    
                    if let Ok(sender) = sender.lock() {
                        let _ = sender.send(PlayerMessage::Buffering(percent));
                    }
                    
                    // Pause the pipeline if buffering and resume when done
                    if percent < 100 {
                        let _ = pipeline_clone.set_state(gst::State::Paused);
                    } else if *is_playing_clone.lock().unwrap() {
                        let _ = pipeline_clone.set_state(gst::State::Playing);
                    }
                }
                MessageView::Element(element) => {
                    // Extract video information when available
                    if let Some(structure) = element.structure() {
                        if structure.name() == "video-info" {
                            if let (Some(width), Some(height), Some(framerate), Some(codec)) = (
                                structure.get::<i32>("width").ok(),
                                structure.get::<i32>("height").ok(),
                                structure.get::<f64>("framerate").ok(),
                                structure.get::<String>("codec").ok(),
                            ) {
                                println!("Video info: {}x{} @ {:.2} fps, codec: {}", 
                                    width, height, framerate, codec);
                                    
                                if let Ok(sender) = sender.lock() {
                                    let _ = sender.send(PlayerMessage::VideoInfo(
                                        width, height, framerate, codec));
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
            
            glib::ControlFlow::Continue
        })?;
        
        Ok(())
    }

    pub fn handle_window_message(&self, hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        fn LOWORD(l: u32) -> u16 {
            (l & 0xffff) as u16
        }
        fn HIWORD(l: u32) -> u16 {
            ((l >> 16) & 0xffff) as u16
        }
        match message {
            WM_COMMAND => {
                let control_id = LOWORD(wparam.0 as u32);
                match control_id {
                    ID_PLAY_BUTTON => {
                        let _ = self.resume();
                        LRESULT(0)
                    },
                    ID_PAUSE_BUTTON => {
                        let _ = self.pause();
                        LRESULT(0)
                    },
                    ID_STOP_BUTTON => {
                        let _ = self.stop();
                        LRESULT(0)
                    },
                    _ => unsafe { DefWindowProcA(hwnd, message, wparam, lparam) }
                }
            },
            WM_HSCROLL => {
                if let Some(controls) = &*self.gui_controls.lock().unwrap() {
                    if let Some(seekbar) = controls.seekbar {
                        if lparam.0 as isize == seekbar.0 as isize {
                            let notify_code = LOWORD(wparam.0 as u32);
                            match notify_code as u32 {
                                TB_THUMBPOSITION | TB_THUMBTRACK => {
                                    let position = HIWORD(wparam.0 as u32) as f64 / 1000.0;
                                    let _ = self.seek(position);
                                },
                                TB_ENDTRACK => {
                                    controls.seekbar.and_then(|x| {
                                        Some(unsafe { SendMessageA(x, TBM_GETTICPOS, WPARAM(0), LPARAM(0)).0 })
                                    }).map(|pos| {
                                        let position = pos as f64 / 1000.0;
                                        let _ = self.seek(position);
                                    }).unwrap_or_default();
                                    // Get the current position from the trackbar
                                    // let position = unsafe { 
                                    //     SendMessageA(controls.seekbar, TBM_GETTICPOS, WPARAM(0), LPARAM(0)).0
                                    // } as f64 / 1000.0;
                                    // let _ = self.seek(position);
                                },
                                _ => {}
                            }
                        }
                    }
                }
                LRESULT(0)
            },
            WM_TIMER => {
                match wparam.0 {
                    1 => {
                        // Timer 1: Process messages from the GStreamer bus thread
                        self.process_player_messages();
                    },
                    2 => {
                        // Timer 2: Update position information
                        if self.is_playing() {
                            if let Some(pos) = self.pipeline.query_position::<gst::ClockTime>() {
                                let pos_secs = pos.seconds();
                                *self.position.lock().unwrap() = pos_secs;
                                
                                // Get duration 
                                if let Some(dur) = self.pipeline.query_duration::<gst::ClockTime>() {
                                    let dur_secs = dur.seconds();
                                    *self.duration.lock().unwrap() = dur_secs;
                                    
                                    if dur_secs > 0 && dur_secs > pos_secs {
                                        // Update position slider
                                        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
                                            if let Some(seekbar) = controls.seekbar {
                                                let slider_value = ((pos_secs as f64 / dur_secs as f64) * 1000.0) as i32;
                                                unsafe {
                                                    SendMessageA(
                                                        seekbar, 
                                                        TBM_SETPOS,
                                                        WPARAM(1), // TRUE to redraw
                                                        LPARAM(slider_value as isize)
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    _ => {}
                }
                LRESULT(0)
            },
            // ... other message handlers remain the same
            WM_SIZE => {
                // Resize video window when main window is resized
                if let Some(controls) = &*self.gui_controls.lock().unwrap() {
                    let width = LOWORD(lparam.0 as u32) as i32;
                    let height = HIWORD(lparam.0 as u32) as i32;
                    
                    // Resize video area
                    unsafe {
                        controls.video_window.and_then(|video_window| {
                            Some(MoveWindow(
                                video_window,
                                0, 0,
                                width,
                                height - 100, // Leave space for controls
                                true
                            ))
                        }).unwrap_or(Ok(()));
                        
                        // Reposition controls
                        let control_y = height - 90;
                        
                        controls.play_button.and_then(|play_button| {
                            Some(MoveWindow(
                                play_button,
                                10, control_y,
                                100, 30,
                                true
                            ))
                        }).unwrap_or(Ok(()));

                        controls.pause_button.and_then(|pause_button| {
                            Some(MoveWindow(
                                pause_button,
                                120, control_y,
                                100, 30,
                                true
                            ))
                        }).unwrap_or(Ok(()));
                        
                        controls.stop_button.and_then(|stop_button| {
                            Some(MoveWindow(
                                stop_button,
                                230, control_y,
                                100, 30,
                                true
                            ))
                        }).unwrap_or(Ok(()));

                        controls.seekbar.and_then(|seekbar| {
                            Some(MoveWindow(
                                seekbar,
                                340, control_y,
                                width - 350, 30,
                                true
                            ))
                        }).unwrap_or(Ok(()));
                        
                        controls.status_text.and_then(|status_text| {
                            Some(MoveWindow(
                                status_text,
                                10, control_y + 40,
                                width - 20, 20,
                                true
                            ))
                        }).unwrap_or(Ok(()));
                    }
                }
                LRESULT(0)
            },
            WM_DESTROY => {
                // Stop playback and quit
                let _ = self.stop();
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            },
            _ => unsafe { DefWindowProcA(hwnd, message, wparam, lparam) }
        }
    }

    // New method to process messages from the channel
    fn process_player_messages(&self) {
        // Try to receive all pending messages without blocking
        while let Ok(msg) = self.message_receiver.try_recv() {
            match msg {
                PlayerMessage::EndOfStream => self.set_status_text("End of stream"),
                PlayerMessage::Error(error_msg) => {
                    let text = format!("Error: {}", error_msg);
                    self.set_status_text(text.as_str());
                },
                PlayerMessage::StreamStarted => self.set_status_text("Stream started"),
                PlayerMessage::Buffering(percent) => {
                    let text = format!("Buffering... {}%\0", percent);
                    self.set_status_text(text.as_str());
                },
                PlayerMessage::StateChanged(state) => {
                    match state {
                        gst::State::Playing => self.set_status_text("Playing"),
                        gst::State::Paused => self.set_status_text("Paused"),
                        gst::State::Ready => self.set_status_text("Ready"),
                        gst::State::Null => self.set_status_text("Stopped"),
                        _ => {}
                    }
                },
                PlayerMessage::VideoInfo(width, height, framerate, codec) => {
                    // Update video information in UI
                    let text = format!("{}x{} @ {:.2} fps ({})", width, height, framerate, codec);
                    self.set_status_text(text.as_str());

                    // Store video info
                    let mut info = self.video_info.lock().unwrap();
                    *info = Some(VideoInfo {
                        width,
                        height,
                        framerate,
                        codec,
                    });
                },
                PlayerMessage::Reconnecting(attempt) => {
                    let text = format!("Reconnecting ({}/5)...", attempt);
                    self.set_status_text(text.as_str());
                },
                PlayerMessage::ConnectionFailed => self.set_status_text("Connection failed"),
                PlayerMessage::PositionUpdate(_pos, _dur) => {
                    // This is handled by the position timer (timer 2)
                },
            }
        }

    }

    fn set_status_text<S: AsRef<str>>(&self, text: S) {
        let text = format!("{}\0", text.as_ref());
        let text = text.as_str();
        // Update status text in the GUI
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            if let Some(status_text) = controls.status_text {
                // let text = CString::new(text).unwrap();
                unsafe {
                    SetWindowTextA(status_text, PCSTR(text.as_ptr()))
                }.expect("Failed to set status text");
            }
        }
    }

    fn get_video_info(&self) -> Option<VideoInfo> {
        self.video_info.lock()
            .ok()
            .map(|x|x.clone().unwrap())
            // .unwrap()//.clone()
    }
    
    fn is_playing(&self) -> bool {
        *self.is_playing.lock().unwrap()
    }
}

fn check_win_err() -> std::result::Result<(), Box<dyn Error>> {
    let last_error = unsafe { GetLastError() };
    if last_error != ERROR_SUCCESS {
        return Err(Box::new(PlayerError::WindowsError(format!("Windows API error: 0x{:08x}", last_error.0))));
    }
    Ok(())
}



