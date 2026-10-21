use anyhow::{Result, ensure};
use clap::Parser;
use msbjit::{
    project::Project,
    runtime::{self, RuntimeOptions},
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

#[derive(Parser)]
#[command(
    version,
    about = "Run Scratch 3 scripts as native Cranelift machine code"
)]
struct Args {
    /// Scratch .sb3 archive or standalone project.json
    project: PathBuf,
    /// Remove normal loop frame boundaries (explicit waits still wait)
    #[arg(long)]
    turbo: bool,
    /// Stop execution after this many seconds
    #[arg(long)]
    seconds: Option<f64>,
    /// Stop execution after this many compiled instructions
    #[arg(long)]
    max_steps: Option<u64>,
    /// Normal-mode scheduler tick rate
    #[arg(long, default_value_t = 30.0)]
    tick_hz: f64,
    /// Maximum uninterrupted warp execution, at most 500 ms
    #[arg(long, default_value_t = 500)]
    watchdog_ms: u64,
    /// Emit the final state and execution statistics as JSON
    #[arg(long)]
    json: bool,
    /// Open an interactive native stage window (enabled by default for .sb3)
    #[arg(long, conflicts_with_all = ["headless", "serve"])]
    display: bool,
    /// Run without the native stage window
    #[arg(long)]
    headless: bool,
    /// Serve a local browser player and WebSocket signaling (requires streaming feature)
    #[arg(long)]
    serve: Option<std::net::SocketAddr>,
    /// Render/capture rate, independent of execution ticks
    #[arg(long, default_value_t = 120)]
    fps: u32,
    /// FFmpeg H.264 encoder: auto, nvenc, vaapi, software
    #[arg(long, default_value = "auto")]
    encoder: String,
    #[arg(long, default_value = "/dev/dri/renderD128")]
    vaapi_device: PathBuf,
}
fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(seconds) = args.seconds {
        ensure!(
            seconds.is_finite() && seconds > 0.0 && seconds <= 86400.0 * 365.0,
            "seconds must be positive and at most one year"
        );
    }
    ensure!(
        (1..=500).contains(&args.watchdog_ms),
        "watchdog-ms must be between 1 and 500"
    );
    ensure!(
        (1..=240).contains(&args.fps),
        "fps must be between 1 and 240"
    );
    #[cfg(not(feature = "streaming"))]
    ensure!(
        args.serve.is_none(),
        "browser streaming requires: cargo run --features streaming -- <project> --serve 127.0.0.1:8080"
    );
    let project = Arc::new(Project::load(&args.project)?);
    let display = args.display
        || (cfg!(feature = "desktop")
            && !args.headless
            && args.serve.is_none()
            && args
                .project
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sb3")));
    ensure!(
        !display || cfg!(feature = "desktop"),
        "native display requires the desktop feature (enabled by default)"
    );
    let spawn = if display || args.serve.is_some() {
        runtime::spawn_interactive
    } else {
        runtime::spawn
    };
    let running = spawn(
        project.clone(),
        RuntimeOptions {
            turbo: args.turbo,
            tick_hz: args.tick_hz,
            warp_timeout: Duration::from_millis(args.watchdog_ms),
            time_limit: args.seconds.map(Duration::from_secs_f64),
            max_steps: args.max_steps,
        },
    )?;
    let scene = running.scene.clone();
    ctrlc::set_handler(move || scene.stopped.store(true, Ordering::Relaxed))?;
    #[cfg(feature = "desktop")]
    if display {
        msbjit::desktop::show(
            &project,
            running.scene.clone(),
            args.fps,
            args.seconds.map(Duration::from_secs_f64),
        )?;
    }
    #[cfg(feature = "streaming")]
    if let Some(address) = args.serve {
        let executor = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        // block_on keeps rendering on the calling/main thread; signaling and
        // network tasks run on Tokio workers, encoding on its own OS threads.
        executor.block_on(msbjit::streaming::serve(
            project,
            running.scene.clone(),
            msbjit::streaming::StreamOptions {
                address,
                fps: args.fps,
                encoder: args.encoder,
                vaapi_device: args.vaapi_device,
                duration: args.seconds.map(Duration::from_secs_f64),
            },
        ))?;
    }
    let report = running.join()?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for message in &report.messages {
            println!("{message}");
        }
        for variable in &report.variables {
            println!(
                "{}.{} = {}",
                variable.target,
                variable.name,
                variable.value.text()
            );
        }
        eprintln!(
            "{} native steps, {} ticks, {} yields in {:.3}s{}",
            report.steps,
            report.ticks,
            report.yields,
            report.elapsed_seconds,
            if report.interrupted { " (stopped)" } else { "" }
        );
    }
    Ok(())
}
