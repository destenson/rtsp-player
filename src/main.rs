use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;
use std::env;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Custom error type for better error handling
#[derive(Debug)]
enum PlayerError {
    InitError(String),
    StreamError(String),
    ConnectionError(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PlayerError::InitError(msg) => write!(f, "Initialization error: {}", msg),
            PlayerError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            PlayerError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
        }
    }
}

impl Error for PlayerError {}

struct RtspPlayer {
    pipeline: gst::Pipeline,
    main_loop: glib::MainLoop,
    is_playing: Arc<Mutex<bool>>,
    reconnect_attempts: Arc<Mutex<u32>>,
    url: String,
    video_info: Arc<Mutex<Option<VideoInfo>>>,
}

#[derive(Debug, Clone)]
struct VideoInfo {
    width: i32,
    height: i32,
    framerate: f64,
    codec: String,
}

impl RtspPlayer {
    fn new(url: &str) -> Result<Self, Box<dyn Error>> {
        // Initialize GStreamer if not already initialized
        if gst::init().is_err() {
            return Err(Box::new(PlayerError::InitError("Failed to initialize GStreamer".into())));
        }

        // Create a more robust pipeline with better error handling and reconnection
        let pipeline_str = format!(
            "rtspsrc location={} latency=100 protocols=tcp+udp+http buffer-mode=auto retry=5 timeout=5000000 ! 
             rtpjitterbuffer ! queue max-size-buffers=3000 max-size-time=0 max-size-bytes=0 ! 
             decodebin ! videoconvert ! autovideosink sync=false",
            url
        );

        let pipeline = gst::parse::launch(&pipeline_str)?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| PlayerError::InitError("Failed to create pipeline".into()))?;

        let main_loop = glib::MainLoop::new(None, false);

        Ok(RtspPlayer {
            pipeline,
            main_loop,
            is_playing: Arc::new(Mutex::new(false)),
            reconnect_attempts: Arc::new(Mutex::new(0)),
            url: url.to_string(),
            video_info: Arc::new(Mutex::new(None)),
        })
    }

    fn play(&self) -> Result<(), Box<dyn Error>> {
        // Start the pipeline
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        
        println!("Stream started. Playing from {}", self.url);
        println!("Press Ctrl+C to stop playback.");
        
        // Set up message handling
        self.setup_message_handling()?;
        
        // Run the main loop
        self.main_loop.run();
        
        Ok(())
    }
    
    fn pause(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Paused)?;
        *self.is_playing.lock().unwrap() = false;
        println!("Playback paused.");
        Ok(())
    }
    
    fn resume(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        println!("Playback resumed.");
        Ok(())
    }
    
    fn stop(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Null)?;
        *self.is_playing.lock().unwrap() = false;
        println!("Playback stopped.");
        self.main_loop.quit();
        Ok(())
    }
    
    fn setup_message_handling(&self) -> Result<(), Box<dyn Error>> {
        let bus = self.pipeline.bus().ok_or_else(|| 
            PlayerError::InitError("Failed to get pipeline bus".into())
        )?;
        
        let main_loop_clone = self.main_loop.clone();
        let pipeline_clone = self.pipeline.clone();
        let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
        let url_clone = self.url.clone();
        let is_playing = Arc::clone(&self.is_playing);
        let video_info = Arc::clone(&self.video_info);
        
        let _bus_watch = bus.add_watch(move |_, msg| {
            use gstreamer::MessageView;
            
            match msg.view() {
                MessageView::Eos(..) => {
                    println!("End of stream");
                    main_loop_clone.quit();
                }
                MessageView::Error(err) => {
                    println!("Error: {} ({:?})", err.error(), err.debug());
                    
                    // If currently playing, try to reconnect
                    if *is_playing.lock().unwrap() {
                        let mut attempts = reconnect_attempts.lock().unwrap();
                        if *attempts < 5 {
                            *attempts += 1;
                            println!("Attempting to reconnect (attempt {}/5)...", *attempts);
                            
                            // Reset the pipeline
                            let _ = pipeline_clone.set_state(gst::State::Null);
                            std::thread::sleep(Duration::from_secs(2));
                            
                            // Create a new source element
                            let src_str = format!(
                                "rtspsrc location={} latency=100 protocols=tcp+udp+http buffer-mode=auto retry=5 timeout=5000000",
                                url_clone
                            );
                            
                            match gst::parse::launch(&src_str) {
                                Ok(elem) => {
                                    println!("Reconnection attempt successful");
                                    let _ = pipeline_clone.set_state(gst::State::Playing);
                                }
                                Err(e) => {
                                    println!("Failed to reconnect: {}", e);
                                    if *attempts >= 5 {
                                        println!("Max reconnection attempts reached, giving up");
                                        main_loop_clone.quit();
                                    }
                                }
                            }
                        } else {
                            println!("Max reconnection attempts reached, giving up");
                            main_loop_clone.quit();
                        }
                    } else {
                        main_loop_clone.quit();
                    }
                }
                MessageView::StateChanged(state_changed) => {
                    // Only process messages from the pipeline
                    if let Some(pipeline) = msg.src().and_then(|s| s.clone().dynamic_cast::<gst::Pipeline>().ok()) {
                        if pipeline == pipeline_clone && state_changed.current() == gst::State::Playing {
                            // Reset reconnect counter when we successfully reach playing state
                            *reconnect_attempts.lock().unwrap() = 0;
                        }
                    }
                }
                MessageView::StreamStart(_) => {
                    println!("Stream started successfully");
                }
                MessageView::Buffering(buffering) => {
                    let percent = buffering.percent();
                    println!("Buffering... {}%", percent);
                    
                    // Pause the pipeline if buffering and resume when done
                    if percent < 100 {
                        let _ = pipeline_clone.set_state(gst::State::Paused);
                    } else if *is_playing.lock().unwrap() {
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
                                let mut info = video_info.lock().unwrap();
                                *info = Some(VideoInfo {
                                    width,
                                    height,
                                    framerate,
                                    codec: codec.clone(),
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
    
    // Set up Ctrl+C handler
    let player_clone = Arc::new(player);
    let player_for_signal = Arc::clone(&player_clone);
    
    ctrlc::set_handler(move || {
        println!("\nReceived Ctrl+C, shutting down...");
        let _ = player_for_signal.stop();
    })?;
    
    // Start playback
    player_clone.play()?;
    
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
    
    // More tests could be added here
}
