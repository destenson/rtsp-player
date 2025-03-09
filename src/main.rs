use gstreamer as gst;
use gstreamer::prelude::*;
use std::env;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    // Initialize GStreamer
    gst::init()?;
    
    // Get the RTSP URL from command line or use a default
    let args: Vec<String> = env::args().collect();
    let rtsp_url = if args.len() > 1 {
        &args[1]
    } else {
        "rtsp://127.0.0.1:8554/live" // Default URL
    };
    
    println!("Playing RTSP stream from: {}", rtsp_url);
    
    // Create a GStreamer pipeline
    let pipeline_str = format!(
        "rtspsrc location={} latency=100 ! queue ! decodebin ! videoconvert ! autovideosink",
        rtsp_url
    );
    
    let pipeline = gst::parse::launch(&pipeline_str)?;
    
    // Start playing
    pipeline.set_state(gst::State::Playing)?;
    
    println!("Stream started. Press Ctrl+C to stop.");
    
    // Create a main loop to listen for events
    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_clone = main_loop.clone();
    
    // Set up a bus watch to handle messages from the pipeline
    let bus = pipeline.bus().unwrap();
    let _bus_watch = bus.add_watch(move |_, msg| {
        use gstreamer::MessageView;
        // println!("Received message: {:?}", msg.view());
        
        match msg.view() {
            MessageView::Eos(..) => {
                println!("End of stream");
                main_loop_clone.quit();
            }
            MessageView::Error(err) => {
                println!(
                    "Error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                main_loop_clone.quit();
            }
            _ => (),
        }
        
        glib::ControlFlow::Continue
    })?;
    
    // Catch Ctrl+C to quit gracefully
    let pipeline_clone = pipeline.clone();
    ctrlc::set_handler(move || {
        println!("Received Ctrl+C, shutting down...");
        pipeline_clone.set_state(gst::State::Null).unwrap();
        std::process::exit(0);
    })?;
    
    // Run the main loop
    main_loop.run();
    
    // Clean up
    pipeline.set_state(gst::State::Null)?;
    
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtsp_stream() {
        // The following command can be used to test the RTSP stream:
        // ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 -c:v libx264 -preset ultrafast -tune zerolatency -f rtsp rtsp://localhost:8554/live



    }
}

