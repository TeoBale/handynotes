//! Serializes authentication and cloud I/O on a dedicated thread.
use crate::{cloud::CloudClient, store::CloudNote};
use async_channel::{Receiver, Sender};

pub enum Request {
    Login {
        email: String,
        password: String,
        register: bool,
    },
    Logout,
    Sync {
        account: String,
        pending: Vec<CloudNote>,
        version: Option<f64>,
    },
}

pub enum Event {
    Session {
        account: Option<String>,
        adopt: bool,
    },
    Current,
    Snapshot {
        version: f64,
        account: String,
        sent: Vec<CloudNote>,
        notes: Vec<CloudNote>,
        upload_error: Option<String>,
    },
    PullFailed {
        account: String,
        sent: Vec<CloudNote>,
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
        let restored = client.restore().map(|_| Event::Session {
            account: account(&client),
            adopt: false,
        });
        let _ = events.send_blocking(restored.unwrap_or_else(Event::Error));
        let mut failed_upload: Option<String> = None;
        while let Ok(request) = requests.recv_blocking() {
            let previous_account = account(&client);
            let result = match request {
                Request::Login {
                    email,
                    password,
                    register,
                } => client
                    .sign_in(&email, &password, register)
                    .map(|_| Event::Session {
                        account: account(&client),
                        adopt: true,
                    }),
                Request::Logout => client.sign_out().map(|_| Event::Session {
                    account: None,
                    adopt: false,
                }),
                Request::Sync {
                    account: owner,
                    mut pending,
                    version,
                } => {
                    if account(&client).as_deref() != Some(owner.as_str()) {
                        Err("Please sign in again to sync.".into())
                    } else {
                        let mut sent = Vec::new();
                        let mut upload_error = None;
                        // A note rejected by the server must not indefinitely block
                        // unrelated offline edits behind it in the outbox.
                        if let Some(key) = failed_upload.take()
                            && let Some(index) = pending.iter().position(|n| n.key == key)
                        {
                            pending.rotate_left(index + 1);
                        }
                        for note in pending {
                            match client.put(&note) {
                                Ok(()) => sent.push(note),
                                Err(error) => {
                                    failed_upload = Some(note.key);
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
                && !matches!(&event, Event::Session { .. })
                && events
                    .send_blocking(Event::Session {
                        account: None,
                        adopt: false,
                    })
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
