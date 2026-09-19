export async function checkHealth() {
  try {
    const response = await fetch("/healthz", { cache: "no-store" });
    return response.ok;
  } catch (error) {
    console.error(error);
    return false;
  }
}

export async function fetchIceServers(linkId) {
  const fallback = [{ urls: ["stun:stun.l.google.com:19302"] }];
  try {
    const response = await fetch(`/api/session/${linkId}/ice`, {
      cache: "no-store",
    });
    if (!response.ok) {
      return fallback;
    }
    const body = await response.json();
    const servers = [];
    if (Array.isArray(body.stun_urls) && body.stun_urls.length > 0) {
      servers.push({ urls: body.stun_urls });
    } else {
      servers.push(fallback[0]);
    }
    if (body.turn_url && body.turn_username && body.turn_password) {
      servers.push({
        urls: [body.turn_url],
        username: body.turn_username,
        credential: body.turn_password,
      });
    }
    return servers;
  } catch (error) {
    console.error(error);
    return fallback;
  }
}

export async function createSession(linkId) {
  try {
    const response = await fetch(`/api/links/${linkId}/sessions`, {
      method: "POST",
    });
    if (!response.ok) {
      return { status: "dead" };
    }
    const body = await response.json();
    return { status: "ok", sessionId: body.session_id };
  } catch (error) {
    console.error(error);
    return { status: "unreachable" };
  }
}

export async function pollOfferOnce(linkId, sessionId) {
  try {
    const response = await fetch(`/api/session/${linkId}/${sessionId}/offer`, {
      cache: "no-store",
    });
    if (response.status === 204) {
      return { state: "empty" };
    }
    if (!response.ok) {
      return { state: "dead" };
    }
    const body = await response.json();
    return { state: "ready", sdp: body.sdp };
  } catch (error) {
    console.error(error);
    return { state: "unreachable" };
  }
}

export async function postAnswer(linkId, sessionId, sdp) {
  try {
    const response = await fetch(`/api/session/${linkId}/${sessionId}/answer`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ sdp }),
    });
    return response.ok;
  } catch (error) {
    console.error(error);
    return false;
  }
}

export function sendLeaveBeacon(linkId, sessionId) {
  try {
    navigator.sendBeacon(`/api/session/${linkId}/${sessionId}/leave`);
  } catch (error) {
    console.error(error);
  }
}
