use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};
use wgpu::{Device, Queue, Surface, TextureFormat};
use std::env;
use raw_window_handle::HasRawWindowHandle;

// Custom error type for better error handling
#[derive(Debug)]
enum PlayerError {
    InitError(String),
    StreamError(String),
    ConnectionError(String),
    WindowError(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PlayerError::InitError(msg) => write!(f, "Initialization error: {}", msg),
            PlayerError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            PlayerError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            PlayerError::WindowError(msg) => write!(f, "Window error: {}", msg),
        }
    }
}

impl Error for PlayerError {}

struct VideoInfo {
    width: i32,
    height: i32,
    framerate: f64,
    codec: String,
}

struct RtspPlayer {
    pipeline: gst::Pipeline,
    is_playing: Arc<Mutex<bool>>,
    reconnect_attempts: Arc<Mutex<u32>>,
    url: String,
    video_info: Arc<Mutex<Option<VideoInfo>>>,
    position: Arc<Mutex<u64>>,
    duration: Arc<Mutex<u64>>,
    status_text: Arc<Mutex<String>>,
}

struct GuiState {
    device: Device,
    queue: Queue,
    surface: Surface,
    surface_format: TextureFormat,
    size: winit::dpi::PhysicalSize<u32>,
    play_button_rect: (f32, f32, f32, f32), // x, y, width, height
    pause_button_rect: (f32, f32, f32, f32),
    is_seeking: bool,
    seek_position: f32,
}

impl RtspPlayer {
    fn new(url: &str) -> Result<Self, Box<dyn Error>> {
        // Initialize GStreamer if not already initialized
        if gst::init().is_err() {
            return Err(Box::new(PlayerError::InitError("Failed to initialize GStreamer".into())));
        }

        // Create a more robust pipeline with better error handling and reconnection
        // Use d3d11videosink for Windows DirectX rendering
        let pipeline_str = format!(
            "rtspsrc location={} latency=100 protocols=tcp+udp+http buffer-mode=auto retry=5 timeout=5000000 ! 
             rtpjitterbuffer ! queue max-size-buffers=3000 max-size-time=0 max-size-bytes=0 ! 
             decodebin ! videoconvert ! d3d11videosink sync=true name=videosink",
            url
        );

        let pipeline = gst::parse::launch(&pipeline_str)?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| PlayerError::InitError("Failed to create pipeline".into()))?;

        Ok(RtspPlayer {
            pipeline,
            is_playing: Arc::new(Mutex::new(false)),
            reconnect_attempts: Arc::new(Mutex::new(0)),
            url: url.to_string(),
            video_info: Arc::new(Mutex::new(None)),
            position: Arc::new(Mutex::new(0)),
            duration: Arc::new(Mutex::new(0)),
            status_text: Arc::new(Mutex::new(String::from("Initializing..."))),
        })
    }

    fn play(&self) -> Result<(), Box<dyn Error>> {
        // Start the pipeline
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        *self.status_text.lock().unwrap() = String::from("Playing");
        
        println!("Stream started. Playing from {}", self.url);
        
        Ok(())
    }
    
    fn pause(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Paused)?;
        *self.is_playing.lock().unwrap() = false;
        *self.status_text.lock().unwrap() = String::from("Paused");
        
        println!("Playback paused.");
        Ok(())
    }
    
    fn resume(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        *self.status_text.lock().unwrap() = String::from("Playing");
        
        println!("Playback resumed.");
        Ok(())
    }
    
    fn stop(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Null)?;
        *self.is_playing.lock().unwrap() = false;
        *self.status_text.lock().unwrap() = String::from("Stopped");
        
        println!("Playback stopped.");
        Ok(())
    }
    
    fn seek(&self, position_percent: f64) -> Result<(), Box<dyn Error>> {
        let duration = *self.duration.lock().unwrap();
        if duration > 0 {
            let position = (position_percent / 100.0) * (duration as f64);
            self.pipeline.seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                position * gst::ClockTime::SECOND.nseconds() as f64,
            )?;
            
            println!("Seeking to {}%", position_percent);
        }
        
        Ok(())
    }
    
    fn setup_message_handling(&self) -> Result<(), Box<dyn Error>> {
        let bus = self.pipeline.bus().ok_or_else(|| 
            PlayerError::InitError("Failed to get pipeline bus".into())
        )?;
        
        let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
        let url_clone = self.url.clone();
        let is_playing = Arc::clone(&self.is_playing);
        let video_info = Arc::clone(&self.video_info);
        let status_text = Arc::clone(&self.status_text);
        let pipeline_clone = self.pipeline.clone();
        
        let _bus_watch = bus.add_watch(move |_, msg| {
            use gstreamer::MessageView;
            
            match msg.view() {
                MessageView::Eos(..) => {
                    println!("End of stream");
                    *is_playing.lock().unwrap() = false;
                    *status_text.lock().unwrap() = String::from("End of stream");
                }
                MessageView::Error(err) => {
                    println!("Error: {} ({:?})", err.error(), err.debug());
                    *status_text.lock().unwrap() = format!("Error: {}", err.error());
                    
                    // If currently playing, try to reconnect
                    if *is_playing.lock().unwrap() {
                        let mut attempts = reconnect_attempts.lock().unwrap();
                        if *attempts < 5 {
                            *attempts += 1;
                            println!("Attempting to reconnect (attempt {}/5)...", *attempts);
                            *status_text.lock().unwrap() = format!("Reconnecting ({}/5)...", *attempts);
                            
                            // Reset the pipeline
                            let _ = pipeline_clone.set_state(gst::State::Null);
                            std::thread::sleep(Duration::from_secs(2));
                            
                            // Create a new source element
                            let src_str = format!(
                                "rtspsrc location={} latency=100 protocols=tcp+udp+http buffer-mode=auto retry=5 timeout=5000000",
                                url_clone
                            );
                            
                            match gst::parse::launch(&src_str) {
                                Ok(_) => {
                                    println!("Reconnection attempt successful");
                                    let _ = pipeline_clone.set_state(gst::State::Playing);
                                }
                                Err(e) => {
                                    println!("Failed to reconnect: {}", e);
                                    if *attempts >= 5 {
                                        println!("Max reconnection attempts reached, giving up");
                                        *status_text.lock().unwrap() = String::from("Connection failed");
                                    }
                                }
                            }
                        } else {
                            println!("Max reconnection attempts reached, giving up");
                            *status_text.lock().unwrap() = String::from("Connection failed");
                        }
                    }
                }
                MessageView::StateChanged(state_changed) => {
                    // Only process messages from the pipeline
                    if let Some(pipeline) = msg.src().and_then(|s| s.dynamic_cast::<gst::Pipeline>().ok()) {
                        if pipeline == pipeline_clone && state_changed.current() == gst::State::Playing {
                            // Reset reconnect counter when we successfully reach playing state
                            *reconnect_attempts.lock().unwrap() = 0;
                        }
                    }
                }
                MessageView::StreamStart(_) => {
                    println!("Stream started successfully");
                    *status_text.lock().unwrap() = String::from("Stream started");
                }
                MessageView::Buffering(buffering) => {
                    let percent = buffering.percent();
                    println!("Buffering... {}%", percent);
                    *status_text.lock().unwrap() = format!("Buffering... {}%", percent);
                    
                    // Pause the pipeline if buffering and resume when done
                    if percent < 100 {
                        let _ = pipeline_clone.set_state(gst::State::Paused);
                    } else if *is_playing.lock().unwrap() {
                        let _ = pipeline_clone.set_state(gst::State::Playing);
                        *status_text.lock().unwrap() = String::from("Playing");
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
                                let mut info = video_info.lock().unwrap();
                                *info = Some(VideoInfo {
                                    width,
                                    height,
                                    framerate,
                                    codec,
                                });
                                
                                println!("Video info: {}x{} @ {:.2} fps, codec: {}", 
                                    width, height, framerate, codec);
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
    
    fn get_video_info(&self) -> Option<VideoInfo> {
        self.video_info.lock().unwrap().clone()
    }
    
    fn is_playing(&self) -> bool {
        *self.is_playing.lock().unwrap()
    }
    
    fn get_position_percent(&self) -> f64 {
        let position = *self.position.lock().unwrap();
        let duration = *self.duration.lock().unwrap();
        
        if duration > 0 {
            (position as f64 / duration as f64) * 100.0
        } else {
            0.0
        }
    }
    
    fn get_status_text(&self) -> String {
        self.status_text.lock().unwrap().clone()
    }
    
    fn update_position(&self) {
        if let Ok(Some(pos)) = self.pipeline.query_position::<gst::ClockTime>() {
            let pos_secs = pos.seconds();
            *self.position.lock().unwrap() = pos_secs;
            
            // Get duration 
            if let Ok(Some(dur)) = self.pipeline.query_duration::<gst::ClockTime>() {
                let dur_secs = dur.seconds();
                *self.duration.lock().unwrap() = dur_secs;
            }
        }
    }
}

fn create_window_and_device() -> Result<(EventLoop<()>, Window, Device, Queue, Surface, TextureFormat), Box<dyn Error>> {
    let event_loop = EventLoop::new();
    
    let window = WindowBuilder::new()
        .with_title("RTSP Player")
        .with_inner_size(LogicalSize::new(800.0, 600.0))
        .build(&event_loop)?;
    
    // Set up wgpu instance
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        dx12_shader_compiler: Default::default(),
    });
    
    // Create surface
    let surface = unsafe { instance.create_surface(&window) }?;
    
    // Request adapter
    let adapter = pollster::block_on(instance.request_adapter(
        &wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        },
    )).ok_or_else(|| PlayerError::WindowError("Failed to find an appropriate adapter".into()))?;
    
    // Create device and queue
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            features: wgpu::Features::empty(),
            limits: wgpu::Limits::default(),
            label: None,
        },
        None,
    ))?;
    
    // Configure surface
    let size = window.inner_size();
    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps.formats.iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(surface_caps.formats[0]);
    
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width,
        height: size.height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
    };
    
    surface.configure(&device, &config);
    
    Ok((event_loop, window, device, queue, surface, surface_format))
}

fn run_gui_loop(
    event_loop: EventLoop<()>,
    window: Window,
    player: Arc<RtspPlayer>
) -> Result<(), Box<dyn Error>> {
    // Start the GStreamer pipeline
    player.play()?;
    
    // Create position update thread
    let player_clone = Arc::clone(&player);
    std::thread::spawn(move || {
        loop {
            if player_clone.is_playing() {
                player_clone.update_position();
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    
    // Set up GUI state
    let mut gui_state = {
        let size = window.inner_size();
        
        // Define button positions and sizes
        let play_button_rect = (20.0, size.height as f32 - 60.0, 80.0, 40.0);
        let pause_button_rect = (110.0, size.height as f32 - 60.0, 80.0, 40.0);
        
        GuiState {
            device: wgpu::Device::new(),
            queue: wgpu::Queue::new(),
            surface: wgpu::Surface::new(),
            surface_format: wgpu::TextureFormat::Rgba8Unorm,
            size,
            play_button_rect,
            pause_button_rect,
            is_seeking: false,
            seek_position: 0.0,
        }
    };
    
    // Run the event loop
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;
        
        match event {
            Event::WindowEvent { window_id, event } if window_id == window.id() => {
                match event {
                    WindowEvent::CloseRequested => {
                        let _ = player.stop();
                        *control_flow = ControlFlow::Exit;
                    }
                    WindowEvent::Resized(new_size) => {
                        gui_state.size = new_size;
                        // Update seek bar and button positions based on new size
                        gui_state.play_button_rect.1 = new_size.height as f32 - 60.0;
                        gui_state.pause_button_rect.1 = new_size.height as f32 - 60.0;
                    }
                    WindowEvent::MouseInput { state: winit::event::ElementState::Pressed, .. } => {
                        // Check if buttons were clicked
                        if let Some(cursor_position) = window.cursor_position() {
                            // Check play button
                            if point_in_rect(cursor_position, gui_state.play_button_rect) {
                                let _ = player.resume();
                            }
                            // Check pause button
                            else if point_in_rect(cursor_position, gui_state.pause_button_rect) {
                                let _ = player.pause();
                            }
                            // Check seek bar (positioned at the bottom of the window)
                            else {
                                let seek_bar_y = gui_state.size.height as f32 - 100.0;
                                let seek_bar_height = 20.0;
                                
                                if cursor_position.y >= seek_bar_y && cursor_position.y <= seek_bar_y + seek_bar_height {
                                    let seek_percent = (cursor_position.x as f32 / gui_state.size.width as f32) * 100.0;
                                    gui_state.is_seeking = true;
                                    gui_state.seek_position = seek_percent;
                                    
                                    // Perform the seek
                                    let _ = player.seek(seek_percent as f64);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::RedrawRequested(window_id) if window_id == window.id() => {
                // This would be where we'd redraw our UI if needed
                // Since we're using GStreamer's d3d11videosink, it will handle the video rendering
                // We would add UI elements on top if needed
            }
            Event::MainEventsCleared => {
                // Request a redraw
                window.request_redraw();
            }
            _ => {}
        }
    });
}

fn point_in_rect(point: PhysicalPosition<f64>, rect: (f32, f32, f32, f32)) -> bool {
    let (x, y, width, height) = rect;
    point.x >= x as f64 && point.x <= (x + width) as f64 && 
    point.y >= y as f64 && point.y <= (y + height) as f64
}

fn main() -> Result<(), Box<dyn Error>> {
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
    
    // Set up message handling
    player.setup_message_handling()?;
    
    // Set up Ctrl+C handler
    let player_for_signal = Arc::new(player);
    let player_clone = Arc::clone(&player_for_signal);
    
    ctrlc::set_handler(move || {
        println!("\nReceived Ctrl+C, shutting down...");
        let _ = player_clone.stop();
        std::process::exit(0);
    })?;

    // Create window and GUI
    let (event_loop, window, device, queue, surface, surface_format) = create_window_and_device()?;
    
    // Run the main event loop
    run_gui_loop(event_loop, window, player_for_signal)?;
    
    Ok(())
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
