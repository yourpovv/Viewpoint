use crate::config::Settings;
use crate::encode::Packet;
use crate::signal::{SignalClient, SignalError};
use rtc::interceptor::Registry;
use rtc::media::Sample;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_H264};
use rtc::peer_connection::configuration::setting_engine::SettingEngine;
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RtpCodecKind,
};
use rtc::shared::time::SystemInstant;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use webrtc::data_channel::DataChannelEvent;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::{MediaStreamTrack, Track};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState,
};
use webrtc::rtp_transceiver::RtpSender;
use webrtc::runtime::{channel, Receiver, Sender, TokioRuntime};

const VIDEO_CLOCK_RATE: u32 = 90000;
const VIDEO_PAYLOAD_TYPE: u8 = 102;
const VIDEO_FMTP: &str = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f";
const CONTROL_LABEL: &str = "viewpoint-control";
const EVENT_CAPACITY: usize = 16;
const HOST_BIND: &str = "0.0.0.0:0";
const GATHER_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const ANSWER_WAIT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionEvent {
    Gathered,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Clone)]
struct HostEvents {
    events: Sender<SessionEvent>,
}

impl HostEvents {
    fn emit(&self, event: SessionEvent) {
        let _ = self.events.try_send(event);
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for HostEvents {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.emit(SessionEvent::Gathered);
        }
    }

    async fn on_connection_state_change(
        &self,
        state: webrtc::peer_connection::RTCPeerConnectionState,
    ) {
        use webrtc::peer_connection::RTCPeerConnectionState as ConnectionState;
        match state {
            ConnectionState::Connected => self.emit(SessionEvent::Connected),
            ConnectionState::Failed | ConnectionState::Closed => self.emit(SessionEvent::Failed),
            ConnectionState::Disconnected => self.emit(SessionEvent::Disconnected),
            _ => {}
        }
    }

    async fn on_ice_connection_state_change(
        &self,
        state: webrtc::peer_connection::RTCIceConnectionState,
    ) {
        use webrtc::peer_connection::RTCIceConnectionState as IceState;
        match state {
            IceState::Connected | IceState::Completed => self.emit(SessionEvent::Connected),
            IceState::Failed | IceState::Closed => self.emit(SessionEvent::Failed),
            IceState::Disconnected => self.emit(SessionEvent::Disconnected),
            _ => {}
        }
    }
}

pub struct Publisher {
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    audio_track: Arc<TrackLocalStaticSample>,
    peer: Arc<dyn PeerConnection>,
    event_receiver: Receiver<SessionEvent>,
    paused: Arc<AtomicBool>,
    left: Arc<AtomicBool>,
    rtp_clock: u32,
    rtp_step: u32,
    audio_clock: u32,
    frame_interval: Duration,
}

impl Publisher {
    pub async fn connect(
        settings: &Settings,
        signal: &SignalClient,
        link_id: &str,
        session_id: &str,
    ) -> Result<Self, SignalError> {
        let ice_servers = resolve_ice_servers(settings, signal, link_id).await;
        let paused = Arc::new(AtomicBool::new(false));
        let left = Arc::new(AtomicBool::new(false));
        let (event_sender, pending_events) = channel::<SessionEvent>(EVENT_CAPACITY);
        let events = HostEvents {
            events: event_sender,
        };

        let video_codec = build_video_codec();
        let mut engine = MediaEngine::default();
        engine
            .register_codec(video_codec.clone(), RtpCodecKind::Video)
            .map_err(|source| format!("register codec: {source}"))?;
        engine
            .register_codec(build_audio_codec(), RtpCodecKind::Audio)
            .map_err(|source| format!("register audio codec: {source}"))?;
        let registry = register_default_interceptors(Registry::new(), &mut engine)
            .map_err(|source| format!("register interceptors: {source}"))?;
        let configuration = RTCConfigurationBuilder::new()
            .with_ice_servers(ice_servers)
            .build();

        let mut setting_engine = SettingEngine::default();
        setting_engine.set_multicast_dns_mode(rtc::ice::mdns::MulticastDnsMode::Disabled);
        let built = PeerConnectionBuilder::new()
            .with_configuration(configuration)
            .with_media_engine(engine)
            .with_setting_engine(setting_engine)
            .with_interceptor_registry(registry)
            .with_handler(Arc::new(events))
            .with_runtime(Arc::new(TokioRuntime))
            .with_udp_addrs(vec![HOST_BIND.to_owned()])
            .build()
            .await
            .map_err(|source| format!("build peer: {source}"))?;
        let peer: Arc<dyn PeerConnection> = Arc::new(built);

        let track = build_video_track(video_codec)?;
        let sender = peer
            .add_track(track.clone() as Arc<dyn TrackLocal>)
            .await
            .map_err(|source| format!("add track: {source}"))?;
        let audio_track = build_audio_track()?;
        peer.add_track(audio_track.clone() as Arc<dyn TrackLocal>)
            .await
            .map_err(|source| format!("add audio track: {source}"))?;

        spawn_control_channel(&peer, paused.clone(), left.clone());

        let offer = peer
            .create_offer(None)
            .await
            .map_err(|source| format!("create offer: {source}"))?;
        peer.set_local_description(offer)
            .await
            .map_err(|source| format!("set local offer: {source}"))?;
        let pending_events = wait_gathering(pending_events).await?;
        let local = peer
            .local_description()
            .await
            .ok_or("missing local offer after gathering")?;
        signal
            .publish_offer(link_id, session_id, &local.sdp)
            .await?;
        let answer_sdp = signal.poll_answer(link_id, session_id, ANSWER_WAIT).await?;
        peer.set_remote_description(
            RTCSessionDescription::answer(answer_sdp)
                .map_err(|source| format!("read answer: {source}"))?,
        )
        .await
        .map_err(|source| format!("set remote answer: {source}"))?;
        let pending_events = wait_connected(pending_events).await?;

        let fps = settings.stream.fps.max(1) as u64;
        Ok(Self {
            track,
            sender,
            audio_track,
            peer,
            event_receiver: pending_events,
            paused,
            left,
            rtp_clock: 0,
            rtp_step: (VIDEO_CLOCK_RATE / fps as u32),
            audio_clock: 0,
            frame_interval: Duration::from_millis(1000 / fps),
        })
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn is_left(&self) -> bool {
        self.left.load(Ordering::Acquire)
    }

    pub fn latest_event(&mut self) -> Option<SessionEvent> {
        let mut last = None;
        while let Ok(event) = self.event_receiver.try_recv() {
            last = Some(event);
        }
        last
    }

    pub async fn close(&self) {
        let _ = self.peer.close().await;
    }

    pub async fn publish(&mut self, packet: Packet) -> Result<(), SignalError> {
        let payload_type = self
            .sender
            .get_parameters()
            .await
            .map_err(|source| format!("read sender: {source}"))?
            .rtp_parameters
            .codecs
            .first()
            .map(|codec| codec.payload_type)
            .ok_or("sender has no negotiated codec")?;
        let ssrc = self
            .track
            .ssrcs()
            .await
            .first()
            .copied()
            .ok_or("track has no ssrc")?;
        self.rtp_clock = self.rtp_clock.wrapping_add(self.rtp_step);
        self.track
            .write_sample(
                ssrc,
                payload_type,
                &Sample {
                    data: packet.bytes,
                    timestamp: SystemInstant::now(),
                    duration: self.frame_interval,
                    packet_timestamp: self.rtp_clock,
                    prev_dropped_packets: 0,
                    prev_padding_packets: 0,
                },
                &[],
            )
            .await
            .map_err(|source| SignalError::failed(format!("send packet: {source}")))
    }

    pub async fn publish_audio(&mut self, bytes: bytes::Bytes) -> Result<(), SignalError> {
        let ssrc = self
            .audio_track
            .ssrcs()
            .await
            .first()
            .copied()
            .ok_or("audio track has no ssrc")?;
        self.audio_clock = self
            .audio_clock
            .wrapping_add(crate::audio::SAMPLES_PER_PACKET as u32);
        self.audio_track
            .write_sample(
                ssrc,
                0,
                &Sample {
                    data: bytes,
                    timestamp: SystemInstant::now(),
                    duration: Duration::from_millis(20),
                    packet_timestamp: self.audio_clock,
                    prev_dropped_packets: 0,
                    prev_padding_packets: 0,
                },
                &[],
            )
            .await
            .map_err(|source| SignalError::failed(format!("send audio: {source}")))
    }
}

fn build_video_codec() -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: VIDEO_CLOCK_RATE,
            channels: 0,
            sdp_fmtp_line: VIDEO_FMTP.to_owned(),
            rtcp_feedback: vec![],
        },
        payload_type: VIDEO_PAYLOAD_TYPE,
    }
}

fn build_audio_codec() -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: "audio/PCMU".to_owned(),
            clock_rate: 8000,
            channels: 1,
            sdp_fmtp_line: String::new(),
            rtcp_feedback: vec![],
        },
        payload_type: 0,
    }
}

fn build_audio_track() -> Result<Arc<TrackLocalStaticSample>, SignalError> {
    let ssrc = rand::random::<u32>();
    let track = TrackLocalStaticSample::new(MediaStreamTrack::new(
        "viewpoint-audio-stream".to_owned(),
        "viewpoint-audio".to_owned(),
        "viewpoint audio".to_owned(),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec: build_audio_codec().rtp_codec,
            ..Default::default()
        }],
    ))
    .map_err(|source| format!("build audio track: {source}"))?;
    Ok(Arc::new(track))
}

fn build_video_track(
    video_codec: RTCRtpCodecParameters,
) -> Result<Arc<TrackLocalStaticSample>, SignalError> {
    let ssrc = rand::random::<u32>();
    let track = TrackLocalStaticSample::new(MediaStreamTrack::new(
        "viewpoint-stream".to_owned(),
        "viewpoint-video".to_owned(),
        "viewpoint video".to_owned(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec: video_codec.rtp_codec,
            ..Default::default()
        }],
    ))
    .map_err(|source| format!("build track: {source}"))?;
    Ok(Arc::new(track))
}

async fn resolve_ice_servers(
    settings: &Settings,
    signal: &SignalClient,
    link_id: &str,
) -> Vec<RTCIceServer> {
    if let Ok(live) = signal.fetch_ice(link_id).await {
        let mut servers = Vec::new();
        if !live.stun_urls.is_empty() {
            servers.push(RTCIceServer {
                urls: live.stun_urls,
                username: String::new(),
                credential: String::new(),
            });
        }
        if !live.turn_url.is_empty()
            && !live.turn_username.is_empty()
            && !live.turn_password.is_empty()
        {
            servers.push(RTCIceServer {
                urls: vec![live.turn_url],
                username: live.turn_username,
                credential: live.turn_password,
            });
        }
        if !servers.is_empty() {
            return servers;
        }
    }
    vec![RTCIceServer {
        urls: settings.webrtc.stun_urls.clone(),
        username: String::new(),
        credential: String::new(),
    }]
}

async fn wait_gathering(
    events: Receiver<SessionEvent>,
) -> Result<Receiver<SessionEvent>, SignalError> {
    match tokio::time::timeout(
        GATHER_TIMEOUT,
        wait_for_event(events, SessionEvent::Gathered),
    )
    .await
    {
        Ok(Ok(receiver)) => Ok(receiver),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(SignalError::failed("ice gathering timed out".into())),
    }
}

async fn wait_for_event(
    mut events: Receiver<SessionEvent>,
    want: SessionEvent,
) -> Result<Receiver<SessionEvent>, SignalError> {
    loop {
        match events.recv().await {
            Some(event) if event == want => return Ok(events),
            Some(SessionEvent::Failed) => {
                return Err(SignalError::failed("viewer connection failed".into()));
            }
            Some(_) => continue,
            None => return Err(SignalError::failed("viewer connection lost".into())),
        }
    }
}

async fn wait_connected(
    events: Receiver<SessionEvent>,
) -> Result<Receiver<SessionEvent>, SignalError> {
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        wait_for_event(events, SessionEvent::Connected),
    )
    .await
    {
        Ok(Ok(receiver)) => Ok(receiver),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(SignalError::failed("viewer never connected in time".into())),
    }
}

fn spawn_control_channel(
    peer: &Arc<dyn PeerConnection>,
    paused: Arc<AtomicBool>,
    left: Arc<AtomicBool>,
) {
    let peer = peer.clone();
    tokio::spawn(async move {
        let channel = match peer.create_data_channel(CONTROL_LABEL, None).await {
            Ok(channel) => channel,
            Err(_) => return,
        };
        loop {
            match channel.poll().await {
                Some(DataChannelEvent::OnMessage(message)) => {
                    if let Ok(text) = String::from_utf8(message.data.to_vec()) {
                        match text.trim() {
                            "pause" => paused.store(true, Ordering::Release),
                            "resume" => paused.store(false, Ordering::Release),
                            "bye" => {
                                left.store(true, Ordering::Release);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Some(DataChannelEvent::OnClose) | None => break,
                _ => {}
            }
        }
    });
}
