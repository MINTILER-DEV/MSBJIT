#![cfg(feature = "streaming")]
use anyhow::{Context, Result, ensure};
use futures_util::{SinkExt, StreamExt};
use msbjit::{
    input::InputEvent,
    project::Project,
    runtime::{self, RuntimeOptions},
    streaming::{self, StreamOptions},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a wgpu adapter and FFmpeg with libx264"]
async fn browser_input_reaches_native_scripts_over_websocket() -> Result<()> {
    let mut project = Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/interactive.sb3"
    ))?;
    project.targets[0].variables.insert(
        "slider".into(),
        ("slider".into(), msbjit::value::ScratchValue::Number(0.0)),
    );
    project.monitors = serde_json::from_value(serde_json::json!([
        {"id":"slider","opcode":"data_variable","mode":"slider","x":10,"y":10,"visible":true,"sliderMin":0,"sliderMax":100}
    ]))?;
    project.targets[0].blocks = serde_json::from_value(serde_json::json!({
        "key":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["s"]},"next":"read"},
        "read":{"opcode":"motion_setx","inputs":{"X":[3,[12,"slider","slider"],[4,"0"]]}},
        "click":{"opcode":"event_whenstageclicked","topLevel":true,"next":"clicked"},
        "clicked":{"opcode":"motion_sety","inputs":{"Y":[1,[4,"99"]]}}
    }))?;
    let project = Arc::new(project);
    let running = runtime::spawn_interactive(project.clone(), RuntimeOptions::default())?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let server = tokio::spawn(streaming::serve(
        project.clone(),
        running.scene.clone(),
        StreamOptions {
            address,
            fps: 30,
            encoder: "software".into(),
            vaapi_device: "/dev/dri/renderD128".into(),
            duration: Some(Duration::from_secs(15)),
        },
    ));
    let test = async {
        let mut connected = None;
        for _ in 0..200 {
            if let Ok((ws, _)) = tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await {
                connected = Some(ws); break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let mut ws = connected.context("input server did not start")?;
        let key = serde_json::json!({"type":"input","event":{"type":"key","key":"ArrowRight","down":true}});
        ws.send(tokio_tungstenite::tungstenite::Message::Text(key.to_string().into())).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while running.scene.snapshot(&project)[1].x != 15.0 {
            ensure!(tokio::time::Instant::now() < deadline, "WebSocket key did not move sprite");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let click = serde_json::json!({"type":"input","event":{"type":"mouse","x":15,"y":0,"down":true}});
        ws.send(tokio_tungstenite::tungstenite::Message::Text(click.to_string().into())).await?;
        while running.scene.snapshot(&project)[1].costume != 1 {
            ensure!(tokio::time::Instant::now() < deadline, "WebSocket click did not change costume");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let flag = serde_json::json!({"type":"input","event":{"type":"flag"}});
        ws.send(tokio_tungstenite::tungstenite::Message::Text(flag.to_string().into())).await?;
        while running.scene.snapshot(&project)[1].x != 0.0 {
            ensure!(tokio::time::Instant::now() < deadline, "WebSocket flag did not restart sprite");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Pointer coordinates cross the same host monitor hit testing used by
        // desktop input. Drag past the track, then read the actual JIT variable.
        for event in [
            serde_json::json!({"type":"release"}),
            serde_json::json!({"type":"mouse","x":-190,"y":139,"down":true}),
            serde_json::json!({"type":"mouse","x":1000,"y":139,"down":true}),
            serde_json::json!({"type":"mouse","x":1000,"y":139,"down":false}),
            serde_json::json!({"type":"key","key":"s","down":true})
        ] {
            let message = serde_json::json!({"type":"input","event":event});
            ws.send(tokio_tungstenite::tungstenite::Message::Text(message.to_string().into())).await?;
        }
        while running.scene.snapshot(&project)[0].x != 100.0 {
            ensure!(tokio::time::Instant::now() < deadline, "WebSocket slider did not update JIT variable");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ensure!(running.scene.snapshot(&project)[0].y == 0.0, "slider click leaked through to stage hat");
        ws.close(None).await?;
        Ok::<_, anyhow::Error>(())
    }.await;
    // Also exercise release through the same public interface used on close.
    running.scene.input.send(InputEvent::Release);
    running.stop();
    server.await??;
    running.join()?;
    test
}
use std::{sync::Arc, time::Duration};
use webrtc::{
    api::{APIBuilder, media_engine::MediaEngine, setting_engine::SettingEngine},
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
    },
    rtp_transceiver::{
        RTCRtpTransceiverInit,
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType},
        rtp_transceiver_direction::RTCRtpTransceiverDirection,
    },
};

/// Exercises real GPU rendering, FFmpeg encoding, WebSocket SDP negotiation,
/// host ICE, DTLS/SRTP and receipt of an H.264 RTP packet. Requires local GPU and
/// FFmpeg; opt in explicitly instead of making ordinary CI hardware-dependent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a wgpu adapter and FFmpeg with libx264"]
async fn local_gpu_to_webrtc_rtp() -> Result<()> {
    let project = Arc::new(Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/demo.sb3"
    ))?);
    let running = runtime::spawn(project.clone(), RuntimeOptions::default())?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let scene = running.scene.clone();
    let server = tokio::spawn(streaming::serve(
        project,
        scene,
        StreamOptions {
            address: ([127, 0, 0, 1], port).into(),
            fps: 30,
            encoder: "software".into(),
            vaapi_device: "/dev/dri/renderD128".into(),
            duration: Some(Duration::from_secs(15)),
        },
    ));
    let test = async {
        let mut socket = None;
        for _ in 0..100 {
            match tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await {
                Ok((ws, _)) => {
                    socket = Some(ws);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        let mut socket = socket.context("local signaling server did not start")?;
        let mut media = MediaEngine::default();
        media.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: "video/H264".into(),
                    clock_rate: 90_000,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                            .into(),
                    ..Default::default()
                },
                payload_type: 96,
                ..Default::default()
            },
            RTPCodecType::Video,
        )?;
        let mut settings = SettingEngine::default();
        settings.set_include_loopback_candidate(true);
        let api = APIBuilder::new()
            .with_media_engine(media)
            .with_setting_engine(settings)
            .build();
        let peer = api.new_peer_connection(RTCConfiguration::default()).await?;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        peer.on_track(Box::new(move |track, _, _| {
            let tx = tx.clone();
            Box::pin(async move {
                if let Ok((packet, _)) = track.read_rtp().await {
                    let _ = tx.send(packet).await;
                }
            })
        }));
        peer.add_transceiver_from_kind(
            RTPCodecType::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: vec![],
            }),
        )
        .await?;
        let offer = peer.create_offer(None).await?;
        let mut gathered = peer.gathering_complete_promise().await;
        peer.set_local_description(offer).await?;
        tokio::time::timeout(Duration::from_secs(5), gathered.recv()).await?;
        let offer = peer.local_description().await.context("missing offer")?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&offer)?.into(),
            ))
            .await?;
        let answer = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await?
            .context("no answer")??;
        let answer: RTCSessionDescription = serde_json::from_str(answer.to_text()?)?;
        ensure!(
            !answer.sdp.contains(" typ srflx") && !answer.sdp.contains(" typ relay"),
            "server advertised external ICE candidates"
        );
        peer.set_remote_description(answer).await?;
        let packet = tokio::time::timeout(Duration::from_secs(8), rx.recv())
            .await?
            .context("no H.264 RTP packet")?;
        ensure!(!packet.payload.is_empty(), "empty RTP payload");
        peer.close().await?;
        socket.close(None).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    running.stop();
    let server_result = server.await?;
    running.join()?;
    server_result?;
    test
}
