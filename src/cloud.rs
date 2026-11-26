use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::store::{CloudNote, NoteChange};

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "com.handynotes.cloud";
const REFRESH_BUFFER_SECONDS: i64 = 60;
const NOTES_PAGE_SIZE: u32 = 100;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Tokens {
    token: String,
    refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_adoption: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct OAuthStart {
    redirect: String,
    pub verifier: String,
}

impl OAuthStart {
    pub(crate) fn browser_url(&self) -> Result<url::Url, String> {
        let url = url::Url::parse(&self.redirect)
            .map_err(|_| "Convex returned an invalid GitHub sign-in URL.")?;
        if !is_secure_url(&url)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/api/auth/signin/github"
        {
            return Err("Convex returned an invalid GitHub sign-in URL.".into());
        }
        Ok(url)
    }
}

#[derive(Deserialize)]
struct SignInResult {
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesPage {
    page: Vec<CloudNote>,
    is_done: bool,
    continue_cursor: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
}

struct Session {
    tokens: Tokens,
    account_id: String,
    expires_at: i64,
}

impl Session {
    fn new(tokens: Tokens) -> Result<Self, String> {
        // Convex validates the JWT on authenticated requests. These claims are only
        // used to partition the local sync state and schedule access-token refresh.
        let claims = tokens
            .token
            .split('.')
            .nth(1)
            .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
            .and_then(|payload| serde_json::from_slice::<Claims>(&payload).ok())
            .ok_or("Convex returned an invalid access token.")?;
        let (account_id, _) = claims
            .sub
            .split_once('|')
            .filter(|(user, session)| !user.is_empty() && !session.is_empty())
            .ok_or("Convex returned an invalid account ID.")?;
        Ok(Self {
            tokens,
            account_id: account_id.to_owned(),
            expires_at: claims.exp,
        })
    }

    fn needs_refresh(&self) -> bool {
        self.expires_at <= chrono::Utc::now().timestamp() + REFRESH_BUFFER_SECONDS
    }
}

pub(crate) struct CloudClient {
    deployment_url: String,
    agent: ureq::Agent,
    session: Option<Session>,
}

impl CloudClient {
    pub(crate) fn new(deployment_url: &str) -> Result<Self, String> {
        let url = url::Url::parse(deployment_url.trim())
            .map_err(|_| "Enter a valid Convex deployment URL.")?;
        if !is_secure_url(&url)
            || url.host_str().is_none()
            || url.path() != "/"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Enter a Convex deployment origin using HTTPS.".into());
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .new_agent();
        Ok(Self {
            deployment_url: url.origin().ascii_serialization(),
            agent,
            session: None,
        })
    }

    pub(crate) fn deployment_url(&self) -> &str {
        &self.deployment_url
    }

    pub(crate) fn account_id(&self) -> Option<&str> {
        self.session
            .as_ref()
            .map(|session| session.account_id.as_str())
    }

    pub(crate) fn pending_adoption(&self) -> Option<&str> {
        self.session.as_ref()?.tokens.pending_adoption.as_deref()
    }

    pub(crate) fn confirm_adoption(&mut self) {
        if let Some(session) = &mut self.session
            && let Some(adoption) = session.tokens.pending_adoption.take()
            && save_tokens(&self.deployment_url, &session.tokens).is_err()
        {
            session.tokens.pending_adoption = Some(adoption);
        }
    }

    pub(crate) fn start_github_sign_in(&self, redirect_to: &str) -> Result<OAuthStart, String> {
        self.call(
            "action",
            "auth:signIn",
            json!({ "provider": "github", "params": { "redirectTo": redirect_to } }),
            None,
        )
    }

    pub(crate) fn complete_github_sign_in(
        &mut self,
        verifier: &str,
        code: &str,
    ) -> Result<(), String> {
        let response: SignInResult = self.call(
            "action",
            "auth:signIn",
            json!({ "params": { "code": code }, "verifier": verifier }),
            None,
        )?;
        let mut tokens = response.tokens.ok_or("Convex did not return a session.")?;
        tokens.pending_adoption = Some(Uuid::new_v4().to_string());
        let session = Session::new(tokens)?;
        save_tokens(&self.deployment_url, &session.tokens)?;
        self.session = Some(session);
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> Result<bool, String> {
        self.session = load_tokens(&self.deployment_url)?
            .map(Session::new)
            .transpose()?;
        Ok(self.session.is_some())
    }

    pub(crate) fn sign_out(&mut self) -> Result<(), String> {
        delete_tokens(&self.deployment_url)?;
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        let tokens = if session.needs_refresh() {
            let response: SignInResult = self.call(
                "action",
                "auth:signIn",
                json!({ "refreshToken": session.tokens.refresh_token }),
                None,
            )?;
            let Some(tokens) = response.tokens else {
                return Ok(());
            };
            tokens
        } else {
            session.tokens
        };
        let _: Value = self.call("action", "auth:signOut", json!({}), Some(&tokens.token))?;
        Ok(())
    }

    pub(crate) fn version(&mut self) -> Result<f64, String> {
        self.refresh_if_needed()?;
        self.call(
            "query",
            "notes:version",
            json!({}),
            Some(self.access_token()?),
        )
    }

    pub(crate) fn list(&mut self) -> Result<Vec<CloudNote>, String> {
        let mut notes = Vec::new();
        let mut cursor = None;
        loop {
            self.refresh_if_needed()?;
            let result: NotesPage = self.call(
                "query",
                "notes:listPage",
                json!({ "paginationOpts": { "numItems": NOTES_PAGE_SIZE, "cursor": cursor } }),
                Some(self.access_token()?),
            )?;
            notes.extend(result.page);
            if result.is_done {
                return Ok(notes);
            }
            if cursor.as_ref() == Some(&result.continue_cursor) {
                return Err("Convex did not advance the notes cursor.".into());
            }
            cursor = Some(result.continue_cursor);
        }
    }

    pub(crate) fn put(&mut self, change: &NoteChange) -> Result<(), String> {
        self.refresh_if_needed()?;
        let args = serde_json::to_value(change).map_err(|_| "Could not encode the note.")?;
        let _: Value = self.call("mutation", "notes:put", args, Some(self.access_token()?))?;
        Ok(())
    }

    fn access_token(&self) -> Result<&str, String> {
        self.session
            .as_ref()
            .map(|session| session.tokens.token.as_str())
            .ok_or_else(|| "The cloud session has ended. Sign in again.".into())
    }

    fn refresh_if_needed(&mut self) -> Result<(), String> {
        let Some(session) = self
            .session
            .as_ref()
            .filter(|session| session.needs_refresh())
        else {
            return Ok(());
        };
        let response: SignInResult = self.call(
            "action",
            "auth:signIn",
            json!({ "refreshToken": session.tokens.refresh_token }),
            None,
        )?;
        match response.tokens {
            Some(mut tokens) => {
                tokens.pending_adoption = session.tokens.pending_adoption.clone();
                let refreshed = Session::new(tokens)?;
                if refreshed.account_id != session.account_id {
                    return Err("Convex returned a session for a different account.".into());
                }
                let persist = save_tokens(&self.deployment_url, &refreshed.tokens);
                // Keep the rotated token in memory even if updating Keychain fails.
                self.session = Some(refreshed);
                persist
            }
            None => {
                self.session = None;
                delete_tokens(&self.deployment_url)
            }
        }
    }

    fn call<T: DeserializeOwned>(
        &self,
        kind: &str,
        path: &str,
        args: Value,
        token: Option<&str>,
    ) -> Result<T, String> {
        let mut request = self
            .agent
            .post(format!("{}/api/{kind}", self.deployment_url));
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let mut response = request
            .send_json(json!({ "path": path, "args": args, "format": "json" }))
            .map_err(|_| "Could not connect to Convex.")?;
        let status = response.status();
        // Convex also uses HTTP 560 for function errors.
        if !status.is_success() && status.as_u16() != 560 {
            return Err(format!("Convex returned HTTP {}.", status.as_u16()));
        }
        let body: Value = response
            .body_mut()
            .read_json()
            .map_err(|_| "Convex returned an invalid response.")?;
        if body.get("status").and_then(Value::as_str) == Some("success") {
            let value = body.get("value").cloned().unwrap_or(Value::Null);
            return serde_json::from_value(value)
                .map_err(|_| "Convex returned an unexpected response.".into());
        }
        if path == "auth:signIn" {
            return Err("Convex could not complete authentication.".into());
        }
        Err(format!("Convex could not run {path}."))
    }
}

fn is_secure_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
}

#[cfg(target_os = "macos")]
fn load_tokens(deployment: &str) -> Result<Option<Tokens>, String> {
    use security_framework::passwords::get_generic_password;

    match get_generic_password(KEYCHAIN_SERVICE, deployment) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "The saved cloud session is invalid.".into()),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(error) => Err(format!(
            "Could not read the cloud session from Keychain ({error})."
        )),
    }
}

#[cfg(target_os = "macos")]
fn save_tokens(deployment: &str, tokens: &Tokens) -> Result<(), String> {
    use security_framework::passwords::set_generic_password;

    let bytes = serde_json::to_vec(tokens).map_err(|_| "Could not encode the cloud session.")?;
    set_generic_password(KEYCHAIN_SERVICE, deployment, &bytes)
        .map_err(|error| format!("Could not save the cloud session to Keychain ({error})."))
}

#[cfg(target_os = "macos")]
fn delete_tokens(deployment: &str) -> Result<(), String> {
    use security_framework::passwords::delete_generic_password;

    match delete_generic_password(KEYCHAIN_SERVICE, deployment) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(error) => Err(format!(
            "Could not remove the cloud session from Keychain ({error})."
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn load_tokens(_: &str) -> Result<Option<Tokens>, String> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn save_tokens(_: &str, _: &Tokens) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn delete_tokens(_: &str) -> Result<(), String> {
    Ok(())
}
