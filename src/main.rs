use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;
use gtk::prelude::*;
use gtk::{Button, Box as GtkBox, Label, LevelBar, Scale, Window, WindowType, Orientation};
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
    gui_controls: Arc<Mutex<Option<GuiControls>>>,
}

struct GuiControls {
    play_button: Button,
    position_scale: Scale,
    buffer_level: LevelBar,
    status_label: Label,
    info_label: Label,
}

impl RtspPlayer {
    fn new(url: &str) -> Result<Self, Box<dyn Error>> {
        // Initialize GStreamer if not already initialized
        if gst::init().is_err() {
            return Err(Box::new(PlayerError::InitError("Failed to initialize GStreamer".into())));
        }

        // Create an overlay sink for video rendering that can be embedded in GTK
        let pipeline_str = format!(
            "rtspsrc location={} latency=100 protocols=tcp+udp+http buffer-mode=auto retry=5 timeout=5000000 ! 
             rtpjitterbuffer ! queue max-size-buffers=3000 max-size-time=0 max-size-bytes=0 ! 
             decodebin ! videoconvert ! gtksink name=videosink",
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
            gui_controls: Arc::new(Mutex::new(None)),
        })
    }

    fn setup_gui(&self) -> Result<Window, Box<dyn Error>> {
        // Create the main window
        let window = Window::new(WindowType::Toplevel);
        window.set_title(&format!("RTSP Player - {}", self.url));
        window.set_default_size(800, 600);
        window.connect_delete_event(|_, _| {
            gtk::main_quit();
            Inhibit(false)
        });

        // Create main container
        let main_box = GtkBox::new(Orientation::Vertical, 0);
        window.add(&main_box);

        // Get the video widget from the pipeline
        let video_sink = self.pipeline
            .by_name("videosink")
            .ok_or_else(|| PlayerError::InitError("Could not find video sink".into()))?;
        
        let video_widget = video_sink
            .property::<gtk::Widget>("widget")
            .ok_or_else(|| PlayerError::InitError("Could not get video widget".into()))?;
        
        // Add the video widget to the container
        main_box.pack_start(&video_widget, true, true, 0);

        // Create controls
        let controls_box = GtkBox::new(Orientation::Horizontal, 5);
        controls_box.set_margin_top(10);
        controls_box.set_margin_bottom(10);
        controls_box.set_margin_start(10);
        controls_box.set_margin_end(10);
        main_box.pack_start(&controls_box, false, false, 0);

        // Play/Pause button
        let play_button = Button::with_label("Pause");
        controls_box.pack_start(&play_button, false, false, 0);

        // Position slider
        let position_scale = Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
        position_scale.set_draw_value(false);
        position_scale.set_hexpand(true);
        controls_box.pack_start(&position_scale, true, true, 0);

        // Create status area
        let status_box = GtkBox::new(Orientation::Horizontal, 5);
        status_box.set_margin_start(10);
        status_box.set_margin_end(10);
        status_box.set_margin_bottom(10);
        main_box.pack_start(&status_box, false, false, 0);

        // Buffer level indicator
        let buffer_label = Label::new(Some("Buffer:"));
        status_box.pack_start(&buffer_label, false, false, 0);
        
        let buffer_level = LevelBar::new();
        buffer_level.set_min_value(0.0);
        buffer_level.set_max_value(100.0);
        buffer_level.set_value(0.0);
        buffer_level.set_size_request(100, -1);
        status_box.pack_start(&buffer_level, false, false, 0);
        
        // Status label
        let status_label = Label::new(Some("Initializing..."));
        status_box.pack_start(&status_label, false, false, 10);
        
        // Video info label
        let info_label = Label::new(None);
        status_box.pack_end(&info_label, false, false, 0);

        // Store controls for later access
        *self.gui_controls.lock().unwrap() = Some(GuiControls {
            play_button: play_button.clone(),
            position_scale: position_scale.clone(),
            buffer_level,
            status_label,
            info_label,
        });

        // Connect button signals
        let player_ref = self.clone();
        play_button.connect_clicked(move |button| {
            if player_ref.is_playing() {
                let _ = player_ref.pause();
                button.set_label("Play");
            } else {
                let _ = player_ref.resume();
                button.set_label("Pause");
            }
        });

        // Connect position slider
        let player_ref = self.clone();
        position_scale.connect_value_changed(move |scale| {
            let value = scale.value() as u64;
            let duration = *player_ref.duration.lock().unwrap();
            if duration > 0 {
                let position = (value as f64 / 100.0) * (duration as f64);
                let _ = player_ref.pipeline.seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                    position * gst::ClockTime::SECOND.nseconds() as f64,
                );
            }
        });

        // Show all widgets
        window.show_all();

        Ok(window)
    }

    fn play(&self) -> Result<(), Box<dyn Error>> {
        // Start the pipeline
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            controls.status_label.set_text("Playing");
            controls.play_button.set_label("Pause");
        }
        
        Ok(())
    }
    
    fn pause(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Paused)?;
        *self.is_playing.lock().unwrap() = false;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            controls.status_label.set_text("Paused");
        }
        
        Ok(())
    }
    
    fn resume(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Playing)?;
        *self.is_playing.lock().unwrap() = true;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            controls.status_label.set_text("Playing");
        }
        
        Ok(())
    }
    
    fn stop(&self) -> Result<(), Box<dyn Error>> {
        self.pipeline.set_state(gst::State::Null)?;
        *self.is_playing.lock().unwrap() = false;
        
        // Update status
        if let Some(controls) = &*self.gui_controls.lock().unwrap() {
            controls.status_label.set_text("Stopped");
            controls.play_button.set_label("Play");
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
        let gui_controls = Arc::clone(&self.gui_controls);
        let position = Arc::clone(&self.position);
        let duration = Arc::clone(&self.duration);
        let pipeline_clone = self.pipeline.clone();
        
        // Set up a timeout for updating the position slider
        let timeout_player = self.clone();
        gtk::timeout_add(500, move || {
            if timeout_player.is_playing() {
                if let Some(controls) = &*timeout_player.gui_controls.lock().unwrap() {
                    // Query position
                    if let Ok(Some(pos)) = timeout_player.pipeline.query_position::<gst::ClockTime>() {
                        let pos_secs = pos.seconds();
                        *timeout_player.position.lock().unwrap() = pos_secs;
                        
                        // Get duration 
                        if let Ok(Some(dur)) = timeout_player.pipeline.query_duration::<gst::ClockTime>() {
                            let dur_secs = dur.seconds();
                            *timeout_player.duration.lock().unwrap() = dur_secs;
                            
                            if dur_secs > 0 {
                                // Update position slider without triggering the value-changed signal
                                let slider_value = (pos_secs as f64 / dur_secs as f64) * 100.0;
                                controls.position_scale.block_signal_handlers();
                                controls.position_scale.set_value(slider_value);
                                controls.position_scale.unblock_signal_handlers();
                            }
                        }
                    }
                }
            }
            
            Continue(true)
        });
        
        let _bus_watch = bus.add_watch(move |_, msg| {
            use gstreamer::MessageView;
            
            match msg.view() {
                MessageView::Eos(..) => {
                    println!("End of stream");
                    if let Some(controls) = &*gui_controls.lock().unwrap() {
                        controls.status_label.set_text("End of stream");
                        controls.play_button.set_label("Play");
                    }
                    *is_playing.lock().unwrap() = false;
                }
                MessageView::Error(err) => {
                    println!("Error: {} ({:?})", err.error(), err.debug());
                    
                    if let Some(controls) = &*gui_controls.lock().unwrap() {
                        controls.status_label.set_text(&format!("Error: {}", err.error()));
                    }
                    
                    // If currently playing, try to reconnect
                    if *is_playing.lock().unwrap() {
                        let mut attempts = reconnect_attempts.lock().unwrap();
                        if *attempts < 5 {
                            *attempts += 1;
                            println!("Attempting to reconnect (attempt {}/5)...", *attempts);
                            
                            if let Some(controls) = &*gui_controls.lock().unwrap() {
                                controls.status_label.set_text(&format!("Reconnecting ({}/5)...", *attempts));
                            }
                            
                            // Reset the pipeline
                            let _ = pipeline_clone.set_state(gst::State::Null);
                            std::thread::sleep(Duration::from_secs(2));
                            
                            // Try to play again
                            let _ = pipeline_clone.set_state(gst::State::Playing);
                        } else {
                            println!("Max reconnection attempts reached, giving up");
                            if let Some(controls) = &*gui_controls.lock().unwrap() {
                                controls.status_label.set_text("Connection failed");
                            }
                            *is_playing.lock().unwrap() = false;
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
                    if let Some(controls) = &*gui_controls.lock().unwrap() {
                        controls.status_label.set_text("Stream started");
                    }
                }
                MessageView::Buffering(buffering) => {
                    let percent = buffering.percent();
                    println!("Buffering... {}%", percent);
                    
                    if let Some(controls) = &*gui_controls.lock().unwrap() {
                        controls.buffer_level.set_value(percent as f64);
                        controls.status_label.set_text(&format!("Buffering... {}%", percent));
                    }
                    
                    // Pause the pipeline if buffering and resume when done
                    if percent < 100 {
                        let _ = pipeline_clone.set_state(gst::State::Paused);
                    } else if *is_playing.lock().unwrap() {
                        let _ = pipeline_clone.set_state(gst::State::Playing);
                        if let Some(controls) = &*gui_controls.lock().unwrap() {
                            controls.status_label.set_text("Playing");
                        }
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
                                    
                                if let Some(controls) = &*gui_controls.lock().unwrap() {
                                    controls.info_label.set_text(&format!("{}x{} @ {:.2} fps ({})", 
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
    
    fn get_video_info(&self) -> Option<VideoInfo> {
        self.video_info.lock().unwrap().clone()
    }
    
    fn is_playing(&self) -> bool {
        *self.is_playing.lock().unwrap()
    }
    
    fn clone(&self) -> Self {
        RtspPlayer {
            pipeline: self.pipeline.clone(),
            is_playing: Arc::clone(&self.is_playing),
            reconnect_attempts: Arc::clone(&self.reconnect_attempts),
            url: self.url.clone(),
            video_info: Arc::clone(&self.video_info),
            position: Arc::clone(&self.position),
            duration: Arc::clone(&self.duration),
            gui_controls: Arc::clone(&self.gui_controls),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Initialize GTK
    gtk::init()?;
    
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
    let _window = player.setup_gui()?;
    
    // Set up message handling
    player.setup_message_handling()?;
    
    // Start playback
    player.play()?;
    
    // Start the GTK main loop
    gtk::main();
    
    // Clean up
    player.stop()?;
    
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
