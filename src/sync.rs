//! Serializes authentication and cloud I/O on a dedicated thread.
use crate::{
    cloud::CloudClient,
    oauth::Loopback,
    store::{CloudNote, NoteChange},
};
use async_channel::{Receiver, Sender};

pub enum Request {
    Login,
    Logout,
    Sync {
        account: String,
        pending: Vec<NoteChange>,
        version: Option<f64>,
    },
}

pub struct SessionChange {
    pub account: Option<String>,
    pub adoption: Option<String>,
}

impl SessionChange {
    fn from_client(client: &CloudClient) -> Self {
        Self {
            account: account(client),
            adoption: client.pending_adoption().map(str::to_owned),
        }
    }
}

pub enum Event {
    OpenBrowser(String),
    Session(SessionChange),
    Current,
    Snapshot {
        version: f64,
        account: String,
        sent: Vec<NoteChange>,
        notes: Vec<CloudNote>,
        upload_error: Option<String>,
    },
    PullFailed {
        account: String,
        sent: Vec<NoteChange>,
        error: String,
    },
    Error(String),
}

fn account(client: &CloudClient) -> Option<String> {
    client
        .account_id()
        .map(|id| format!("{}|{id}", client.deployment_url()))
}

pub fn connect(url: String) -> (Sender<Request>, Receiver<Event>) {
    let (tx, requests) = async_channel::unbounded();
    let (events, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let mut client = match CloudClient::new(&url) {
            Ok(client) => client,
            Err(error) => {
                let _ = events.send_blocking(Event::Error(error));
                return;
            }
        };
        let restored = client
            .restore()
            .map(|_| Event::Session(SessionChange::from_client(&client)));
        let _ = events.send_blocking(restored.unwrap_or_else(Event::Error));
        let mut failed_upload: Option<String> = None;
        while let Ok(request) = requests.recv_blocking() {
            let previous_account = account(&client);
            let result = match request {
                Request::Login => sign_in(&mut client, &events),
                Request::Logout => client
                    .sign_out()
                    .map(|_| Event::Session(SessionChange::from_client(&client))),
                Request::Sync {
                    account: owner,
                    mut pending,
                    version,
                } => {
                    if account(&client).as_deref() != Some(owner.as_str()) {
                        Err("Please sign in again to sync.".into())
                    } else {
                        // The UI only syncs this session after the adoption is saved.
                        client.confirm_adoption();
                        let mut sent = Vec::new();
                        let mut upload_error = None;
                        // A note rejected by the server must not indefinitely block
                        // unrelated offline edits behind it in the outbox.
                        if let Some(key) = failed_upload.take()
                            && let Some(index) =
                                pending.iter().position(|change| change.note.key == key)
                        {
                            pending.rotate_left(index + 1);
                        }
                        for change in pending {
                            match client.put(&change) {
                                Ok(()) => sent.push(change),
                                Err(error) => {
                                    failed_upload = Some(change.note.key);
                                    upload_error = Some(error);
                                    break;
                                }
                            }
                        }
                        // Read the version before the pages so a concurrent write
                        // always triggers another pull on the next poll.
                        let pull = client.version().and_then(|latest| {
                            if version == Some(latest) && sent.is_empty() && upload_error.is_none()
                            {
                                Ok(None)
                            } else {
                                client.list().map(|notes| Some((latest, notes)))
                            }
                        });
                        Ok(match pull {
                            Ok(None) => Event::Current,
                            Ok(Some((version, notes))) => Event::Snapshot {
                                version,
                                account: owner,
                                sent,
                                notes,
                                upload_error,
                            },
                            Err(error) => Event::PullFailed {
                                account: owner,
                                sent,
                                error,
                            },
                        })
                    }
                }
            };
            let event = result.unwrap_or_else(Event::Error);
            if previous_account.is_some()
                && account(&client).is_none()
                && !matches!(&event, Event::Session(_))
                && events
                    .send_blocking(Event::Session(SessionChange::from_client(&client)))
                    .is_err()
            {
                break;
            }
            if events.send_blocking(event).is_err() {
                break;
            }
        }
    });
    (tx, rx)
}

fn sign_in(client: &mut CloudClient, events: &Sender<Event>) -> Result<Event, String> {
    let callback = Loopback::bind()?;
    let start = client.start_github_sign_in(callback.url().as_str())?;
    events
        .send_blocking(Event::OpenBrowser(start.browser_url()?.into()))
        .map_err(|_| "The sign-in window was closed.")?;
    let code = callback.wait_for_code()?;
    client.complete_github_sign_in(&start.verifier, &code)?;
    Ok(Event::Session(SessionChange::from_client(client)))
}
