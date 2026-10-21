#![cfg(feature = "streaming")]
use anyhow::{Context, Result, ensure};
use futures_util::{SinkExt, StreamExt};
use msbjit::{
    project::Project,
    runtime::{self, RuntimeOptions},
    streaming::{self, StreamOptions},
};
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
