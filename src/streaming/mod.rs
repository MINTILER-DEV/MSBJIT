//! Local-only WebRTC signaling and a bounded render/capture/encode pipeline.
mod encoder;
use crate::render;
use crate::{input::InputEvent, project::Project, runtime::SharedScene};
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Html,
    routing::get,
};
use encoder::EncodedFrame;
use futures_util::{SinkExt, StreamExt};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, watch};
use webrtc::{
    api::{
        APIBuilder,
        media_engine::{MIME_TYPE_H264, MediaEngine},
        setting_engine::SettingEngine,
    },
    ice_transport::ice_candidate::RTCIceCandidateInit,
    media::Sample,
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::track_local_static_sample::TrackLocalStaticSample,
};

pub struct StreamOptions {
    pub address: SocketAddr,
    pub fps: u32,
    pub encoder: String,
    pub vaapi_device: PathBuf,
    pub duration: Option<Duration>,
}
#[derive(Clone)]
struct ServerState {
    scene: Arc<SharedScene>,
    clicks: tokio::sync::mpsc::Sender<(f64, f64)>,
    samples: broadcast::Sender<EncodedFrame>,
    shutdown: watch::Receiver<bool>,
    clients: Arc<AtomicUsize>,
}
pub async fn serve(
    project: Arc<Project>,
    scene: Arc<SharedScene>,
    options: StreamOptions,
) -> Result<()> {
    ensure!(
        (1..=240).contains(&options.fps),
        "streaming fps must be between 1 and 240"
    );
    // Bind before allocating graphics/encoder resources to report address errors.
    let listener = tokio::net::TcpListener::bind(options.address)
        .await
        .context("bind local signaling server")?;
    let address = listener.local_addr()?;
    let mut renderer = render::Renderer::new(&project).await?;
    let mut encoder =
        encoder::Encoder::start(&options.encoder, options.fps, &options.vaapi_device)?;
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (clicks, mut pointer_clicks) = tokio::sync::mpsc::channel(256);
    let state = ServerState {
        scene: scene.clone(),
        clicks,
        samples: encoder.samples.clone(),
        shutdown: shutdown_rx.clone(),
        clients: Arc::new(AtomicUsize::new(0)),
    };
    let router = Router::new()
        .route("/", get(|| async { Html(include_str!("player.html")) }))
        .route("/ws", get(websocket))
        .with_state(state);
    let mut shutdown_server = shutdown_rx;
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_server.changed().await;
            })
            .await
    });
    eprintln!(
        "Browser player: http://{}/ (host-only ICE; no STUN/TURN)",
        address
    );
    eprintln!("Capture: portable GPU readback; DMA/zero-copy interop is not enabled");
    let start = Instant::now();
    let mut interval = tokio::time::interval(Duration::from_secs_f64(1.0 / options.fps as f64));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        interval.tick().await;
        if scene.stopped.load(Ordering::Relaxed)
            || options.duration.is_some_and(|d| start.elapsed() >= d)
        {
            break Ok(());
        }
        if server.is_finished() {
            break Err(anyhow::anyhow!("signaling server stopped unexpectedly"));
        }
        let states = scene.snapshot(&project);
        while let Ok((x, y)) = pointer_clicks.try_recv() {
            if let Some(target) = renderer.hit_test(&states, x, y) {
                scene.input.send(InputEvent::Click { target });
            }
        }
        let result = renderer
            .capture_scene(&scene, &states)
            .and_then(|frame| encoder.submit(frame));
        if let Err(error) = result {
            break Err(error);
        }
    };
    scene.stopped.store(true, Ordering::Relaxed);
    let _ = shutdown.send(true);
    match tokio::time::timeout(Duration::from_secs(3), server).await {
        Ok(joined) => joined.context("signaling task panicked")??,
        Err(_) => eprintln!("Signaling shutdown deadline reached"),
    }
    result
}
async fn websocket(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
) -> axum::response::Response {
    if state.clients.fetch_add(1, Ordering::AcqRel) >= 8 {
        state.clients.fetch_sub(1, Ordering::AcqRel);
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    ws.max_message_size(256 * 1024)
        .on_upgrade(move |socket| async move {
            if let Err(error) = session(socket, state.clone()).await {
                eprintln!("WebRTC session: {error:#}");
            }
            state.clients.fetch_sub(1, Ordering::AcqRel);
        })
}
use axum::response::IntoResponse;
async fn session(socket: WebSocket, state: ServerState) -> Result<()> {
    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let mut settings = SettingEngine::default();
    settings.set_include_loopback_candidate(true);
    // Keep media on UDP. No external discovery or relay servers are configured.
    settings.set_network_types(vec![
        webrtc::ice::network_type::NetworkType::Udp4,
        webrtc::ice::network_type::NetworkType::Udp6,
    ]);
    let registry = webrtc::api::interceptor_registry::register_default_interceptors(
        webrtc::interceptor::registry::Registry::new(),
        &mut media,
    )?;
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_setting_engine(settings)
        .with_interceptor_registry(registry)
        .build();
    let peer = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: vec![],
            ..Default::default()
        })
        .await?,
    );
    let track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.into(),
            clock_rate: 90_000,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .into(),
            ..Default::default()
        },
        "stage".into(),
        "msbjit".into(),
    ));
    let sender = peer.add_track(track.clone()).await?;
    let rtcp = tokio::spawn(async move { while sender.read_rtcp().await.is_ok() {} });
    let mut samples = state.samples.subscribe();
    let media_task = tokio::spawn(async move {
        loop {
            match samples.recv().await {
                Ok(frame) => {
                    let _ = track
                        .write_sample(&Sample {
                            data: frame.data,
                            duration: frame.duration,
                            ..Default::default()
                        })
                        .await;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let (mut output, mut input) = socket.split();
    let mut shutdown = state.shutdown;
    let mut mouse_down = false;
    let result = async {
        loop {
            let message = tokio::select! {
                _=shutdown.changed()=>break,
                message=input.next()=>message,
            };
            let Some(message) = message else {
                break;
            };
            match message? {
                Message::Text(text) => {
                    let message: serde_json::Value = serde_json::from_str(&text)?;
                    match message["type"].as_str() {
                        Some("input") => {
                            let event: InputEvent =
                                serde_json::from_value(message["event"].clone())?;
                            match event {
                                InputEvent::Mouse { x, y, down } => {
                                    if down && !mouse_down && x.is_finite() && y.is_finite() {
                                        let _ = state.clicks.try_send((x, y));
                                    }
                                    mouse_down = down;
                                    state.scene.input.send(InputEvent::Mouse { x, y, down });
                                }
                                InputEvent::Click { .. } => {} // Hit targets are selected by the host.
                                InputEvent::Release => {
                                    mouse_down = false;
                                    state.scene.input.send(InputEvent::Release);
                                }
                                event => state.scene.input.send(event),
                            }
                        }
                        Some("offer") => {
                            let sdp = message["sdp"].as_str().context("missing offer SDP")?;
                            peer.set_remote_description(RTCSessionDescription::offer(sdp.into())?)
                                .await?;
                            let answer = peer.create_answer(None).await?;
                            let mut gathered = peer.gathering_complete_promise().await;
                            peer.set_local_description(answer).await?;
                            tokio::time::timeout(Duration::from_secs(10), gathered.recv())
                                .await
                                .context("host ICE gathering timed out")?;
                            let description = peer
                                .local_description()
                                .await
                                .context("missing local SDP")?;
                            output
                                .send(Message::Text(serde_json::to_string(&description)?.into()))
                                .await?;
                        }
                        Some("candidate") => {
                            let candidate: RTCIceCandidateInit =
                                serde_json::from_value(message["candidate"].clone())?;
                            peer.add_ice_candidate(candidate).await?;
                        }
                        _ => {}
                    }
                }
                Message::Close(_) => break,
                Message::Ping(data) => output.send(Message::Pong(data)).await?,
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    state.scene.input.send(InputEvent::Release);
    media_task.abort();
    rtcp.abort();
    let _ = peer.close().await;
    result
}
