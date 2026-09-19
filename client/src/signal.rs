use std::fmt;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct SignalError {
    pub status: Option<u16>,
    detail: String,
}

impl SignalError {
    fn transport(source: reqwest::Error, action: &str) -> Self {
        Self {
            status: source.status().map(|code| code.as_u16()),
            detail: format!("{action}: {source}"),
        }
    }

    fn refused(status: reqwest::StatusCode, action: &str) -> Self {
        Self {
            status: Some(status.as_u16()),
            detail: format!("{action}: server said {status}"),
        }
    }

    pub fn failed(action: String) -> Self {
        Self {
            status: None,
            detail: action,
        }
    }

    pub fn not_found(&self) -> bool {
        self.status == Some(404)
    }
}

impl fmt::Display for SignalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

impl std::error::Error for SignalError {}

impl From<String> for SignalError {
    fn from(detail: String) -> Self {
        SignalError::failed(detail)
    }
}

impl From<&str> for SignalError {
    fn from(detail: &str) -> Self {
        SignalError::failed(detail.to_owned())
    }
}

#[derive(serde::Deserialize)]
struct LinkResponse {
    id: String,
}

#[derive(serde::Deserialize)]
struct AnswerResponse {
    sdp: String,
}

#[derive(serde::Deserialize)]
struct LeaveCountResponse {
    leaves: u64,
}

#[derive(serde::Deserialize)]
struct SessionListResponse {
    sessions: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct IceConfig {
    #[serde(default)]
    pub stun_urls: Vec<String>,
    #[serde(default)]
    pub turn_url: String,
    #[serde(default)]
    pub turn_username: String,
    #[serde(default)]
    pub turn_password: String,
}

#[derive(Clone)]
pub struct SignalClient {
    control_url: String,
    public_url: String,
    token: String,
    http: reqwest::Client,
}

impl SignalClient {
    pub fn new(control_url: &str, public_url: &str, token: String) -> Result<Self, SignalError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|source| SignalError::failed(format!("build http client: {source}")))?;
        Ok(Self {
            control_url: control_url.trim_end_matches('/').to_owned(),
            public_url: public_url.trim_end_matches('/').to_owned(),
            token,
            http,
        })
    }

    pub fn ticket_url(&self, link_id: &str) -> String {
        format!("{}/v/{link_id}", self.public_url)
    }

    fn session_path(&self, link_id: &str, session_id: &str, action: &str) -> String {
        format!(
            "{}/api/session/{link_id}/{session_id}/{action}",
            self.control_url
        )
    }

    pub async fn mint_link(&self) -> Result<String, SignalError> {
        let response = self
            .http
            .post(format!("{}/api/links", self.control_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|source| SignalError::transport(source, "mint link"))?;
        if !response.status().is_success() {
            return Err(SignalError::refused(response.status(), "mint link"));
        }
        let link: LinkResponse = response
            .json()
            .await
            .map_err(|source| SignalError::failed(format!("read link: {source}")))?;
        Ok(link.id)
    }

    pub async fn list_sessions(&self, link_id: &str) -> Result<Vec<String>, SignalError> {
        let response = self
            .http
            .get(format!("{}/api/links/{link_id}/sessions", self.control_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|source| SignalError::transport(source, "list sessions"))?;
        if !response.status().is_success() {
            return Err(SignalError::refused(response.status(), "list sessions"));
        }
        let body: SessionListResponse = response
            .json()
            .await
            .map_err(|source| SignalError::failed(format!("read sessions: {source}")))?;
        Ok(body.sessions)
    }

    pub async fn publish_offer(
        &self,
        link_id: &str,
        session_id: &str,
        sdp: &str,
    ) -> Result<(), SignalError> {
        let response = self
            .http
            .post(self.session_path(link_id, session_id, "offer"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "sdp": sdp }))
            .send()
            .await
            .map_err(|source| SignalError::transport(source, "publish offer"))?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(());
        }
        Err(SignalError::refused(response.status(), "publish offer"))
    }

    pub async fn fetch_ice(&self, link_id: &str) -> Result<IceConfig, SignalError> {
        let response = self
            .http
            .get(format!("{}/api/session/{link_id}/ice", self.control_url))
            .send()
            .await
            .map_err(|source| SignalError::transport(source, "fetch ice"))?;
        if !response.status().is_success() {
            return Err(SignalError::refused(response.status(), "fetch ice"));
        }
        response
            .json()
            .await
            .map_err(|source| SignalError::failed(format!("read ice: {source}")))
    }

    pub async fn leave_count(&self, link_id: &str, session_id: &str) -> Result<u64, SignalError> {
        let response = self
            .http
            .get(self.session_path(link_id, session_id, "leave"))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|source| SignalError::transport(source, "leave count"))?;
        if !response.status().is_success() {
            return Err(SignalError::refused(response.status(), "leave count"));
        }
        let body: LeaveCountResponse = response
            .json()
            .await
            .map_err(|source| SignalError::failed(format!("read leaves: {source}")))?;
        Ok(body.leaves)
    }

    pub async fn poll_answer(
        &self,
        link_id: &str,
        session_id: &str,
        wait: Duration,
    ) -> Result<String, SignalError> {
        let deadline = Instant::now() + wait;
        loop {
            let response = self
                .http
                .get(self.session_path(link_id, session_id, "answer"))
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(|source| SignalError::transport(source, "poll answer"))?;
            match response.status() {
                reqwest::StatusCode::NO_CONTENT => {
                    if Instant::now() >= deadline {
                        return Err(SignalError::failed(
                            "no viewer answered in time; open the link and retry".into(),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                reqwest::StatusCode::OK => {
                    let answer: AnswerResponse = response
                        .json()
                        .await
                        .map_err(|source| SignalError::failed(format!("read answer: {source}")))?;
                    return Ok(answer.sdp);
                }
                status => return Err(SignalError::refused(status, "poll answer")),
            }
        }
    }
}
