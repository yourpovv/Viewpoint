import {
  checkHealth,
  createSession,
  fetchIceServers,
  pollOfferOnce,
  postAnswer,
  sendLeaveBeacon,
} from "./signal.js";

const video = document.getElementById("screen");
const status = document.getElementById("status");
const statusText = document.getElementById("statusText");
const pauseButton = document.getElementById("pause");
const pauseLabel = document.getElementById("pauseLabel");
const pauseIcon = document.getElementById("pauseIcon");
const muteButton = document.getElementById("mute");
const muteLabel = document.getElementById("muteLabel");
const retryButton = document.getElementById("retry");
const retryDivider = document.getElementById("retryDivider");
const controls = document.getElementById("controls");

const OFFER_POLL_MS = 500;
const GATHER_TIMEOUT_MS = 10000;
const TRACK_TIMEOUT_MS = 20000;
const MAX_ATTEMPTS = 5;
const RETRY_WAIT_MS = 2000;
const IDLE_HIDE_MS = 3000;

const linkId = window.location.pathname.split("/v/")[1] ?? "";
let sessionId = "";
let controlChannel = null;
let paused = false;
let idleTimer = null;

const ICON_PAUSE =
  '<rect x="2.5" y="2" width="4" height="12" rx="1"/><rect x="9.5" y="2" width="4" height="12" rx="1"/>';
const ICON_PLAY = '<path d="M4 2.3v11.4c0 .8.9 1.3 1.6.9l8.6-5.7c.6-.4.6-1.4 0-1.8L5.6 1.4c-.7-.4-1.6.1-1.6.9z"/>';

function setStatus(state, text) {
  status.dataset.state = state;
  statusText.textContent = text;
}

function showRetry() {
  controls.hidden = false;
  retryButton.hidden = false;
  retryDivider.hidden = false;
}

function fail(text) {
  setStatus("failed", text);
  showRetry();
}

function pokeActivity() {
  document.body.classList.remove("idle");
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => document.body.classList.add("idle"), IDLE_HIDE_MS);
}

async function takeOffer() {
  while (true) {
    const result = await pollOfferOnce(linkId, sessionId);
    if (result.state === "empty") {
      setStatus("connecting", "Host starting, waiting for offer");
      await sleep(OFFER_POLL_MS);
      continue;
    }
    if (result.state === "dead") {
      return null;
    }
    return result.sdp;
  }
}

function gatherComplete(peer) {
  if (peer.iceGatheringState === "complete") {
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    const timeout = setTimeout(() => {
      console.warn("ice gathering timed out, continuing with partial candidates");
      resolve();
    }, GATHER_TIMEOUT_MS);
    peer.addEventListener("icegatheringstatechange", function check() {
      if (peer.iceGatheringState === "complete") {
        clearTimeout(timeout);
        peer.removeEventListener("icegatheringstatechange", check);
        resolve();
      }
    });
  });
}

function setPaused(next) {
  paused = next;
  pauseLabel.textContent = paused ? "Resume" : "Pause";
  pauseIcon.innerHTML = paused ? ICON_PLAY : ICON_PAUSE;
  if (paused) {
    video.pause();
    setStatus("paused", "Paused");
  } else {
    video.play().catch(console.error);
    if (video.srcObject) {
      setStatus("live", "Live");
    }
  }
  if (controlChannel && controlChannel.readyState === "open") {
    controlChannel.send(paused ? "pause" : "resume");
  }
  pokeActivity();
}

function sendLeave() {
  try {
    if (controlChannel && controlChannel.readyState === "open") {
      controlChannel.send("bye");
    }
  } catch (error) {
    console.error(error);
  }
  sendLeaveBeacon(linkId, sessionId);
}

function waitForTrack(peer, pendingTrack) {
  if (pendingTrack.received) {
    return Promise.resolve(pendingTrack.event);
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("no track in time")), TRACK_TIMEOUT_MS);
    peer.addEventListener("track", function onTrack(event) {
      clearTimeout(timer);
      peer.removeEventListener("track", onTrack);
      resolve(event);
    });
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function setupControls() {
  pauseButton.addEventListener("click", () => setPaused(!paused));
  muteButton.addEventListener("click", () => {
    video.muted = !video.muted;
    muteLabel.textContent = video.muted ? "Unmute" : "Mute";
    pokeActivity();
  });
  retryButton.addEventListener("click", () => {
    window.location.reload();
  });
  for (const event of ["mousemove", "touchstart", "click"]) {
    document.addEventListener(event, pokeActivity, { passive: true });
  }
  pokeActivity();
}

async function attemptHandshake(offer, iceServers) {
  const peer = new RTCPeerConnection({ iceServers });
  const pendingTrack = { received: false, event: null };
  peer.addEventListener("track", function earlyTrack(event) {
    pendingTrack.received = true;
    pendingTrack.event = event;
    peer.removeEventListener("track", earlyTrack);
  });
  peer.ondatachannel = function (event) {
    controlChannel = event.channel;
  };
  try {
    await peer.setRemoteDescription({ type: "offer", sdp: offer });
    const answer = await peer.createAnswer();
    await peer.setLocalDescription(answer);
    await gatherComplete(peer);
    const posted = await postAnswer(linkId, sessionId, peer.localDescription.sdp);
    if (!posted) {
      await peer.close();
      return null;
    }
    const trackEvent = await waitForTrack(peer, pendingTrack);
    return { peer, stream: trackEvent.streams[0] };
  } catch (error) {
    console.error(error);
    await peer.close();
    return null;
  }
}

function watchConnection(peer) {
  peer.onconnectionstatechange = function () {
    if (peer.connectionState === "failed") {
      setStatus("failed", "Connection failed, retrying");
      try {
        peer.close();
      } catch (error) {
        console.error(error);
      }
      startViewer();
    } else if (peer.connectionState === "disconnected") {
      setStatus("connecting", "Reconnecting");
    } else if (peer.connectionState === "closed") {
      fail("Ended");
    } else if (peer.connectionState === "connected") {
      setStatus(paused ? "paused" : "live", paused ? "Paused" : "Live");
    }
  };
  peer.oniceconnectionstatechange = function () {
    if (peer.iceConnectionState === "failed") {
      setStatus("failed", "Connection failed, retrying");
      try {
        peer.close();
      } catch (error) {
        console.error(error);
      }
      startViewer();
    }
  };
}

let viewing = false;

async function startViewer() {
  if (viewing) {
    return;
  }
  viewing = true;
  try {
    await runViewer();
  } finally {
    viewing = false;
  }
}

async function runViewer() {
  if (!linkId) {
    fail("Missing private link");
    return;
  }
  if (!(await checkHealth())) {
    fail("Server unreachable");
    return;
  }
  const session = await createSession(linkId);
  if (session.status !== "ok") {
    fail(session.status === "dead" ? "Link invalid or expired" : "Server unreachable");
    return;
  }
  sessionId = session.sessionId;
  const iceServers = await fetchIceServers(linkId);
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    setStatus("connecting", attempt === 1 ? "Connecting" : `Retrying (${attempt}/${MAX_ATTEMPTS})`);
    const offer = await takeOffer();
    if (!offer) {
      fail("Link invalid or expired");
      return;
    }
    const live = await attemptHandshake(offer, iceServers);
    if (live) {
      video.srcObject = live.stream;
      video.play().catch(console.error);
      retryButton.hidden = true;
      retryDivider.hidden = true;
      setStatus("live", "Live");
      controls.hidden = false;
      pokeActivity();
      watchConnection(live.peer);
      return;
    }
    await sleep(RETRY_WAIT_MS);
  }
  fail("Connection failed");
}

window.addEventListener("pagehide", sendLeave);
setupControls();
startViewer();
