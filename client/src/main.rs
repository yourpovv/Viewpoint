mod audio;
mod capture;
mod config;
mod encode;
mod signal;
mod webrtc;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::Settings;
use crate::signal::SignalClient;

const RECOVER_GRACE: Duration = Duration::from_secs(5);
const SESSION_POLL: Duration = Duration::from_secs(2);
const LEAVE_POLL: Duration = Duration::from_secs(2);
const MAX_VIEWERS: usize = 8;
const CAP_WARN_COOLDOWN: Duration = Duration::from_secs(30);

struct Viewer {
    publisher: webrtc::Publisher,
    leave_baseline: u64,
    grace_until: Option<Instant>,
    paused: bool,
}

struct HandshakeRequest {
    link_id: String,
    session_id: String,
    outcome: mpsc::Sender<HandshakeOutcome>,
}

enum HandshakeOutcome {
    Connected {
        session: String,
        publisher: webrtc::Publisher,
        leave_baseline: u64,
    },
    Failed {
        session: String,
        reason: String,
    },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("[ERROR] {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let path = std::env::var("VIEWPOINT_CONFIG").unwrap_or_else(|_| "viewpoint.yaml".into());
    let settings = config::load(&path)?;
    println!(
        "[INFO] Viewpoint client: monitor {} {}x{}@{} {}kbps {}",
        settings.stream.monitor,
        settings.stream.width,
        settings.stream.height,
        settings.stream.fps,
        settings.stream.bitrate_kbps,
        settings.stream.codec,
    );

    let token = std::env::var("VIEWPOINT_JWT_SECRET")
        .map_err(|_| "set VIEWPOINT_JWT_SECRET to the same operator token as the server")?;
    let signal = signal::SignalClient::new(&settings.control_url(), &settings.public_url(), token)?;
    let mut link_id = signal.mint_link().await?;
    println!("[INFO] open {}", signal.ticket_url(&link_id));

    let mut capturer = capture::DesktopCapturer::for_monitor(settings.stream.monitor);
    let mut encoder = encode::Encoder::from_stream(&settings.stream)?;
    let mut audio_capturer = if settings.stream.audio_enabled {
        match audio::AudioCapturer::try_new() {
            Ok(capturer) => {
                println!("[INFO] audio loopback on");
                Some(capturer)
            }
            Err(error) => {
                eprintln!("[WARN] audio off: {error}");
                None
            }
        }
    } else {
        None
    };

    let frame_interval = Duration::from_millis(1000 / settings.stream.fps.max(1) as u64);
    let mut video_ticker = skip_ticker(frame_interval);
    let mut audio_ticker = skip_ticker(Duration::from_millis(20));
    let mut session_ticker = skip_ticker(SESSION_POLL);
    let mut leave_ticker = skip_ticker(LEAVE_POLL);

    let (ready_sender, mut ready_receiver) = mpsc::channel::<HandshakeOutcome>(16);
    let mut last_cap_warn = Instant::now() - CAP_WARN_COOLDOWN;
    let mut viewers: HashMap<String, Viewer> = HashMap::new();
    let mut pending: HashSet<String> = HashSet::new();
    let mut sent = 0u64;

    loop {
        tokio::select! {
            _ = video_ticker.tick() => {
                match capturer.next_frame().await {
                    Ok(frame) => {
                        if let Some(packet) = encoder.encode_frame(frame)? {
                            sent += 1;
                            if sent.is_multiple_of(300) {
                                println!("[INFO] streaming: {sent} frames to {} viewers", viewers.len());
                            }
                            fan_out(&mut viewers, MediaKind::Video(packet)).await;
                        }
                    }
                    Err(error) => {
                        eprintln!("[WARN] capture failed: {error}; retrying");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                for session in drain_viewer_events(&mut viewers) {
                    remove_viewer(&mut viewers, &session).await;
                }
            }
            _ = audio_ticker.tick() => {
                if let Some(capturer) = audio_capturer.as_mut() {
                    match capturer.next_packet().await {
                        Ok(Some(packet)) => {
                            fan_out(
                                &mut viewers,
                                MediaKind::Audio(bytes::Bytes::from(packet.bytes)),
                            )
                            .await;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            eprintln!("[WARN] audio capture failed: {error}");
                        }
                    }
                }
                for session in drain_viewer_events(&mut viewers) {
                    remove_viewer(&mut viewers, &session).await;
                }
            }
            _ = session_ticker.tick() => {
                match signal.list_sessions(&link_id).await {
                    Ok(sessions) => {
                        for session in sessions {
                            if viewers.contains_key(&session) || !pending.insert(session.clone()) {
                                continue;
                            }
                            if viewers.len() >= MAX_VIEWERS {
                                pending.remove(&session);
                                if last_cap_warn.elapsed() >= CAP_WARN_COOLDOWN {
                                    eprintln!("[WARN] at {MAX_VIEWERS} viewers, ignoring {}", short_id(&session));
                                    last_cap_warn = Instant::now();
                                }
                                continue;
                            }
                            spawn_handshake(&settings, &signal, HandshakeRequest {
                                link_id: link_id.clone(),
                                session_id: session,
                                outcome: ready_sender.clone(),
                            });
                        }
                    }
                    Err(error) if error.not_found() => {
                        link_id = signal.mint_link().await?;
                        println!("[INFO] link expired, new link: {}", signal.ticket_url(&link_id));
                        viewers.clear();
                        pending.clear();
                    }
                    Err(error) => {
                        eprintln!("[WARN] session poll failed: {error}");
                    }
                }
            }
            outcome = ready_receiver.recv() => {
                let Some(outcome) = outcome else {
                    return Ok(());
                };
                match outcome {
                    HandshakeOutcome::Connected { session, publisher, leave_baseline } => {
                        pending.remove(&session);
                        if viewers.contains_key(&session) {
                            publisher.close().await;
                            continue;
                        }
                        println!("[INFO] viewer {} connected ({} total)", short_id(&session), viewers.len() + 1);
                        viewers.insert(session, Viewer {
                            publisher,
                            leave_baseline,
                            grace_until: None,
                            paused: false,
                        });
                    }
                    HandshakeOutcome::Failed { session, reason } => {
                        pending.remove(&session);
                        eprintln!("[WARN] handshake for {} failed: {reason}; will re-offer", short_id(&session));
                    }
                }
            }
            _ = leave_ticker.tick() => {
                let mut gone = Vec::new();
                for (session, viewer) in viewers.iter() {
                    match signal.leave_count(&link_id, session).await {
                        Ok(count) if count > viewer.leave_baseline => gone.push(session.clone()),
                        Ok(_) => {}
                        Err(error) if error.not_found() => gone.push(session.clone()),
                        Err(_) => {}
                    }
                }
                for session in gone {
                    remove_viewer(&mut viewers, &session).await;
                }
            }
        }
    }
}

enum MediaKind {
    Video(encode::Packet),
    Audio(bytes::Bytes),
}

async fn fan_out(viewers: &mut HashMap<String, Viewer>, media: MediaKind) {
    for viewer in viewers.values_mut() {
        if viewer.paused {
            continue;
        }
        let result = match &media {
            MediaKind::Video(packet) => viewer.publisher.publish(packet.clone()).await,
            MediaKind::Audio(bytes) => viewer.publisher.publish_audio(bytes.clone()).await,
        };
        if let Err(error) = result {
            eprintln!("[WARN] send failed: {error}");
        }
    }
}

fn spawn_handshake(settings: &Settings, signal: &SignalClient, request: HandshakeRequest) {
    let settings = settings.clone();
    let signal = signal.clone();
    tokio::spawn(async move {
        let outcome = match webrtc::Publisher::connect(
            &settings,
            &signal,
            &request.link_id,
            &request.session_id,
        )
        .await
        {
            Ok(publisher) => {
                let leave_baseline = signal
                    .leave_count(&request.link_id, &request.session_id)
                    .await
                    .unwrap_or(0);
                HandshakeOutcome::Connected {
                    session: request.session_id,
                    publisher,
                    leave_baseline,
                }
            }
            Err(error) => HandshakeOutcome::Failed {
                session: request.session_id,
                reason: error.to_string(),
            },
        };
        let _ = request.outcome.send(outcome).await;
    });
}

fn drain_viewer_events(viewers: &mut HashMap<String, Viewer>) -> Vec<String> {
    let mut gone = Vec::new();
    for (session, viewer) in viewers.iter_mut() {
        if viewer.publisher.is_left() {
            gone.push(session.clone());
            continue;
        }
        match viewer.publisher.latest_event() {
            Some(webrtc::SessionEvent::Failed) => gone.push(session.clone()),
            Some(webrtc::SessionEvent::Disconnected) => {
                if viewer.grace_until.is_none() {
                    viewer.grace_until = Some(Instant::now() + RECOVER_GRACE);
                }
            }
            Some(webrtc::SessionEvent::Connected) if viewer.grace_until.take().is_some() => {
                println!("[INFO] viewer {} reconnected", short_id(session));
            }
            _ => {}
        }
        if let Some(deadline) = viewer.grace_until {
            if Instant::now() >= deadline {
                gone.push(session.clone());
            }
        }
        let paused = viewer.publisher.is_paused();
        if paused && !viewer.paused {
            println!("[INFO] viewer {} paused", short_id(session));
        } else if !paused && viewer.paused {
            println!("[INFO] viewer {} resumed", short_id(session));
        }
        viewer.paused = paused;
    }
    gone
}

async fn remove_viewer(viewers: &mut HashMap<String, Viewer>, session: &str) {
    if let Some(viewer) = viewers.remove(session) {
        viewer.publisher.close().await;
        println!(
            "[INFO] viewer {} left ({} left)",
            short_id(session),
            viewers.len()
        );
    }
}

fn short_id(session: &str) -> &str {
    &session[..session.len().min(8)]
}

fn skip_ticker(period: Duration) -> tokio::time::Interval {
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker
}
