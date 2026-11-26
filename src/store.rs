use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cloud_key: Option<String>,
    #[serde(skip)]
    deleted_from_account: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CloudNote {
    pub key: String,
    pub title: String,
    pub body: String,
    pub deleted: bool,
    #[serde(deserialize_with = "timestamp_millis")]
    pub updated_at: u64,
    #[serde(
        default,
        deserialize_with = "optional_timestamp_millis",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NoteChange {
    #[serde(flatten)]
    pub note: CloudNote,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

// Convex serializes integral numbers as JSON floats, including millisecond timestamps.
fn timestamp_millis<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    parse_timestamp_millis(f64::deserialize(d)?)
}

fn optional_timestamp_millis<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<u64>, D::Error> {
    Option::<f64>::deserialize(d)?
        .map(parse_timestamp_millis)
        .transpose()
}

fn parse_timestamp_millis<E: serde::de::Error>(millis: f64) -> Result<u64, E> {
    if millis.fract() == 0.0 && (0.0..=(1_u64 << 53) as f64 - 1.0).contains(&millis) {
        Ok(millis as u64)
    } else {
        Err(serde::de::Error::custom("Invalid note timestamp"))
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct AccountNotes {
    entries: Vec<Note>,
    outbox: BTreeMap<String, NoteChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_adoption: Option<String>,
}

impl AccountNotes {
    fn acknowledge(&mut self, sent: &[NoteChange]) {
        for change in sent {
            if self.outbox.get(&change.note.key) == Some(change) {
                self.outbox.remove(&change.note.key);
            }
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Notes {
    next_id: u64,
    entries: Vec<Note>,
    #[serde(default)]
    accounts: BTreeMap<String, AccountNotes>,
}

impl Notes {
    fn entries(&self, account: Option<&str>) -> &[Note] {
        match account {
            Some(account) => &self.accounts[account].entries,
            None => &self.entries,
        }
    }

    fn entries_mut(&mut self, account: Option<&str>) -> &mut Vec<Note> {
        match account {
            Some(account) => &mut self.accounts.get_mut(account).unwrap().entries,
            None => &mut self.entries,
        }
    }

    fn allocate_id(&mut self) -> io::Result<u64> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Note IDs are exhausted"))?;
        Ok(id)
    }

    fn queue_change(&mut self, account: Option<&str>, note: &Note, deleted: bool) {
        if let (Some(account), Some(key)) = (account, &note.cloud_key) {
            self.accounts.get_mut(account).unwrap().outbox.insert(
                key.clone(),
                NoteChange {
                    operation_id: Some(Uuid::new_v4().to_string()),
                    note: CloudNote {
                        key: key.clone(),
                        updated_at: note.updated_at,
                        created_at: note.created_at,
                        title: if deleted {
                            String::new()
                        } else {
                            note.title.clone()
                        },
                        body: if deleted {
                            String::new()
                        } else {
                            note.body.clone()
                        },
                        deleted,
                    },
                },
            );
        }
    }
}

pub struct NoteStore {
    path: PathBuf,
    notes: Notes,
    active_account: Option<String>,
    _lock: File,
}

impl NoteStore {
    pub fn open_default() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Home directory is unavailable")
            })?;
        Self::open(PathBuf::from(home).join("Library/Application Support/HandyNotes/notes.json"))
    }

    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        fs::create_dir_all(parent_directory(&path))?;
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock().map_err(|error| {
            let error = io::Error::from(error);
            if error.kind() == io::ErrorKind::WouldBlock {
                return io::Error::new(error.kind(), "Another instance has the notes open");
            }
            error
        })?;
        let notes = match File::open(&path) {
            Ok(file) => serde_json::from_reader(BufReader::new(file))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Notes {
                next_id: 1,
                entries: Vec::new(),
                accounts: BTreeMap::new(),
            },
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            notes,
            active_account: None,
            _lock: lock,
        })
    }

    pub fn active_account(&self) -> Option<&str> {
        self.active_account.as_deref()
    }

    pub fn has_local_notes(&self) -> bool {
        !self.notes.entries.is_empty()
    }

    pub fn sign_in(&mut self, account: &str, adoption: Option<&str>) -> io::Result<()> {
        let cache = self.notes.accounts.get(account);
        let adoption = adoption
            .filter(|id| cache.and_then(|notes| notes.completed_adoption.as_deref()) != Some(*id));
        let needs_upgrade = cache.is_some_and(|notes| {
            notes
                .outbox
                .values()
                .any(|change| change.operation_id.is_none())
        });
        if cache.is_none() || adoption.is_some() || needs_upgrade {
            let mut notes = self.notes.clone();
            let cache = notes.accounts.entry(account.to_owned()).or_default();
            for change in cache.outbox.values_mut() {
                change
                    .operation_id
                    .get_or_insert_with(|| Uuid::new_v4().to_string());
            }
            if let Some(id) = adoption {
                let mut local_notes = std::mem::take(&mut notes.entries);
                local_notes.sort_unstable_by_key(|note| std::cmp::Reverse(note.id));
                let mut next_created_at = u64::MAX;
                for mut note in local_notes {
                    // Legacy files store only the last edit time. Walk the old
                    // IDs backward to keep their creation order on adoption.
                    let created_at = *note.created_at.get_or_insert_with(|| {
                        note.updated_at.min(next_created_at.saturating_sub(1))
                    });
                    next_created_at = next_created_at.min(created_at);
                    note.cloud_key = Some(Uuid::new_v4().to_string());
                    notes.queue_change(Some(account), &note, false);
                    notes.entries_mut(Some(account)).push(note);
                }
                notes
                    .entries_mut(Some(account))
                    .sort_by_key(|note| std::cmp::Reverse(note.updated_at));
                notes.accounts.get_mut(account).unwrap().completed_adoption = Some(id.to_owned());
            }
            self.save(notes)?;
        }
        self.active_account = Some(account.to_owned());
        Ok(())
    }

    pub fn sign_out(&mut self) -> io::Result<()> {
        self.active_account = None;
        Ok(())
    }

    pub fn pending(&self) -> Vec<NoteChange> {
        self.active_account()
            .map(|account| {
                self.notes.accounts[account]
                    .outbox
                    .values()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn acknowledge(&mut self, account: &str, sent: &[NoteChange]) -> io::Result<()> {
        if self.active_account() == Some(account) && !sent.is_empty() {
            let mut notes = self.notes.clone();
            let cache = notes.accounts.get_mut(account).unwrap();
            cache.acknowledge(sent);
            if cache.outbox != self.notes.accounts[account].outbox {
                self.save(notes)?;
            }
        }
        Ok(())
    }

    pub fn apply_sync(
        &mut self,
        account: &str,
        sent: &[NoteChange],
        remote: Vec<CloudNote>,
    ) -> io::Result<()> {
        if self.active_account() != Some(account) {
            return Ok(());
        }

        let mut snapshot = BTreeMap::new();
        for note in remote {
            if note.key.is_empty() || snapshot.insert(note.key.clone(), note).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid remote note keys",
                ));
            }
        }

        let mut notes = self.notes.clone();
        let cache = notes.accounts.get_mut(account).unwrap();
        cache.acknowledge(sent);

        cache.entries.retain_mut(|note| {
            let key = note.cloud_key.as_ref().unwrap();
            if cache.outbox.contains_key(key) {
                snapshot.remove(key);
                return true;
            }

            let Some(remote) = snapshot.remove(key).filter(|remote| !remote.deleted) else {
                return false;
            };
            note.title = remote.title;
            note.body = remote.body;
            note.updated_at = remote.updated_at;
            note.created_at = remote.created_at.or(note.created_at);
            true
        });

        let mut new_entries = Vec::new();
        for (key, remote) in snapshot {
            if remote.deleted || notes.accounts[account].outbox.contains_key(&key) {
                continue;
            }
            let id = notes.allocate_id()?;
            new_entries.push(Note {
                id,
                title: remote.title,
                body: remote.body,
                updated_at: remote.updated_at,
                created_at: remote.created_at,
                cloud_key: Some(key),
                deleted_from_account: None,
            });
        }
        let entries = notes.entries_mut(Some(account));
        if !new_entries.is_empty() {
            new_entries.append(entries);
            *entries = new_entries;
        }
        entries.sort_by_key(|note| std::cmp::Reverse(note.updated_at));
        if entries != &self.notes.accounts[account].entries
            || notes.accounts[account].outbox != self.notes.accounts[account].outbox
        {
            self.save(notes)?;
        }
        Ok(())
    }

    pub fn get(&self, id: u64) -> Option<&Note> {
        self.entries().iter().find(|note| note.id == id)
    }

    pub fn ids_by_creation(&self) -> Vec<u64> {
        let mut notes: Vec<_> = self.entries().iter().collect();
        if self.active_account.is_some() {
            notes.sort_unstable_by_key(|&note| {
                (
                    note.created_at,
                    note.created_at.is_none().then_some(note.id),
                    note.cloud_key.as_deref(),
                )
            });
        } else {
            notes.sort_unstable_by_key(|note| note.id);
        }
        notes.into_iter().map(|note| note.id).collect()
    }

    pub fn search(&self, query: &str) -> Vec<&Note> {
        let query = query.to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        if terms.is_empty() {
            return self.entries().iter().collect();
        }

        self.entries()
            .iter()
            .filter(|note| {
                let title = note.title.to_lowercase();
                let body = note.body.to_lowercase();
                terms
                    .iter()
                    .all(|term| title.contains(term) || body.contains(term))
            })
            .collect()
    }

    pub fn create_with_body(&mut self, title: &str, body: &str) -> io::Result<u64> {
        let mut notes = self.notes.clone();
        let account = self.active_account();
        let id = notes.allocate_id()?;
        let created_at = now();
        let note = Note {
            id,
            title: title.to_owned(),
            body: body.to_owned(),
            updated_at: created_at,
            created_at: Some(created_at),
            cloud_key: account.map(|_| Uuid::new_v4().to_string()),
            deleted_from_account: None,
        };
        notes.queue_change(account, &note, false);
        notes.entries_mut(account).insert(0, note);
        self.save(notes)?;
        Ok(id)
    }

    pub fn update(&mut self, id: u64, title: &str, body: &str) -> io::Result<()> {
        let index = self.index_of(id)?;
        let current = &self.entries()[index];
        if current.title == title && current.body == body {
            return Ok(());
        }

        let mut notes = self.notes.clone();
        let account = self.active_account();
        let mut note = notes.entries_mut(account).remove(index);
        note.title = title.to_owned();
        note.body = body.to_owned();
        note.updated_at = now();
        notes.queue_change(account, &note, false);
        notes.entries_mut(account).insert(0, note);
        self.save(notes)
    }

    pub fn delete(&mut self, id: u64) -> io::Result<Note> {
        let index = self.index_of(id)?;
        let mut notes = self.notes.clone();
        let account = self.active_account();
        let mut deleted = notes.entries_mut(account).remove(index);
        deleted.deleted_from_account = account.map(str::to_owned);
        notes.queue_change(account, &deleted, true);
        self.save(notes)?;
        Ok(deleted)
    }

    pub fn restore(&mut self, note: &Note) -> io::Result<()> {
        if self.entries().iter().any(|existing| {
            existing.id == note.id
                || (note.cloud_key.is_some() && existing.cloud_key == note.cloud_key)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Note already exists",
            ));
        }
        let mut notes = self.notes.clone();
        let account = self.active_account();
        if account != note.deleted_from_account.as_deref() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Note belongs to another collection",
            ));
        }
        notes.queue_change(account, note, false);
        let entries = notes.entries_mut(account);
        let index = entries.partition_point(|n| n.updated_at > note.updated_at);
        let mut note = note.clone();
        note.deleted_from_account = None;
        entries.insert(index, note);
        self.save(notes)
    }

    fn entries(&self) -> &[Note] {
        self.notes.entries(self.active_account())
    }

    fn index_of(&self, id: u64) -> io::Result<usize> {
        self.entries()
            .iter()
            .position(|note| note.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Note no longer exists"))
    }

    fn save(&mut self, notes: Notes) -> io::Result<()> {
        let mut file = NamedTempFile::new_in(parent_directory(&self.path))?;
        serde_json::to_writer(file.as_file_mut(), &notes)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        self.notes = notes;
        Ok(())
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_convex_timestamp() {
        let raw = r#"{"key":"note","body":"x","title":"x","deleted":false,
            "updated_at":1750000000123.0,"created_at":1750000000000.0}"#;
        let note = serde_json::from_str::<CloudNote>(raw).unwrap();
        assert_eq!(note.updated_at, 1750000000123);
        assert_eq!(note.created_at, Some(1750000000000));
    }

    #[test]
    fn persists_upload_ids_for_legacy_outbox_changes() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        fs::write(
            &path,
            r#"{"next_id":2,"entries":[],"accounts":{"account":{
                "entries":[{"id":1,"title":"Title","body":"Draft","updated_at":1,"cloud_key":"note"}],
                "outbox":{"note":{"key":"note","title":"Title","body":"Draft","deleted":false,"updated_at":1}}
            }}}"#,
        )?;
        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", None)?;
        let pending = store.pending();
        assert!(Uuid::parse_str(pending[0].operation_id.as_deref().unwrap()).is_ok());
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", None)?;
        assert_eq!(store.pending(), pending);
        Ok(())
    }

    #[test]
    fn old_acknowledgement_preserves_a_restored_note_with_identical_content() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = NoteStore::open(directory.path().join("notes.json"))?;
        store.sign_in("account", None)?;
        let id = store.create_with_body("Title", "Draft")?;
        let sent = store.pending();
        let deleted = store.delete(id)?;
        store.restore(&deleted)?;
        let restored = store.pending();

        assert_eq!(restored[0].note, sent[0].note);
        assert_ne!(restored[0].operation_id, sent[0].operation_id);
        store.acknowledge("account", &sent)?;
        assert_eq!(store.pending(), restored);
        Ok(())
    }

    #[test]
    fn adopts_legacy_notes_and_keeps_account_caches_private() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        fs::write(
            &path,
            r#"{"next_id":8,"entries":[{"id":7,"title":"Legacy","body":"Draft","updated_at":1}]}"#,
        )?;
        let mut store = NoteStore::open(&path)?;
        assert!(store.has_local_notes());
        assert_eq!(store.get(7).unwrap().body, "Draft");

        store.sign_in("first", None)?;
        assert!(store.search("").is_empty());
        assert!(store.has_local_notes());
        store.sign_in("first", Some("adoption"))?;
        assert_eq!(store.get(7).unwrap().title, "Legacy");
        assert!(!store.has_local_notes());
        let pending = store.pending();
        assert_eq!(pending.len(), 1);
        assert!(Uuid::parse_str(&pending[0].note.key).is_ok());
        drop(store);

        let mut store = NoteStore::open(&path)?;
        assert_eq!(store.active_account(), None);
        assert!(store.search("").is_empty());
        store.sign_in("second", None)?;
        assert!(store.pending().is_empty());
        assert!(store.get(7).is_none());
        let second = store.create_with_body("Second", "")?;
        assert_eq!(second, 8);
        store.apply_sync("first", &pending, vec![pending[0].note.clone()])?;
        assert_eq!(store.get(second).unwrap().title, "Second");

        store.sign_in("first", None)?;
        assert_eq!(store.pending(), pending);
        assert!(store.get(second).is_none());
        let deleted = store.delete(7)?;
        store.sign_in("second", None)?;
        assert!(store.restore(&deleted).is_err());
        store.sign_out()?;
        assert!(store.search("").is_empty());
        Ok(())
    }

    #[test]
    fn retries_interrupted_adoption_across_restarts_without_moving_notes_twice() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let backup = directory.path().join("notes.backup");
        let mut store = NoteStore::open(&path)?;
        let draft = store.create_with_body("Draft", "Original local note")?;
        fs::rename(&path, &backup)?;
        fs::create_dir(&path)?;

        assert!(store.sign_in("account", Some("adoption")).is_err());
        assert_eq!(store.active_account(), None);
        assert_eq!(store.get(draft).unwrap().body, "Original local note");
        fs::remove_dir(&path)?;
        fs::rename(backup, &path)?;
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", Some("adoption"))?;
        let uploads = store.pending();
        assert_eq!(uploads.len(), 1);
        assert_eq!(store.get(draft).unwrap().body, "Original local note");
        store.sign_out()?;
        let private = store.create_with_body("Private", "New local note")?;
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", Some("adoption"))?;
        assert_eq!(store.pending(), uploads);
        assert!(store.get(private).is_none());
        store.sign_out()?;
        assert_eq!(store.get(private).unwrap().body, "New local note");
        Ok(())
    }

    #[test]
    fn preserves_legacy_local_creation_order_on_adoption() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        fs::write(
            &path,
            r#"{"next_id":3,"entries":[
                {"id":1,"title":"Older","body":"Edited last","updated_at":3000},
                {"id":2,"title":"Newer","body":"Edited first","updated_at":2000}
            ]}"#,
        )?;
        let mut store = NoteStore::open(&path)?;
        assert_eq!(store.ids_by_creation(), vec![1, 2]);

        store.sign_in("account", Some("adoption"))?;
        assert_eq!(store.ids_by_creation(), vec![1, 2]);
        assert!(store.get(1).unwrap().created_at < store.get(2).unwrap().created_at);
        assert!(
            store
                .pending()
                .iter()
                .all(|change| change.note.created_at.is_some())
        );
        Ok(())
    }

    #[test]
    fn synced_creation_order_survives_different_import_orders_and_edits() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let notes = [
            ("z-older", 1000, 3000),
            ("a-same-time", 2000, 2500),
            ("b-newer", 2000, 2000),
        ]
        .map(|(key, created_at, updated_at)| CloudNote {
            key: key.to_owned(),
            title: key.to_owned(),
            body: key.to_owned(),
            deleted: false,
            updated_at,
            created_at: Some(created_at),
        });

        for (file, initial) in [("one.json", &notes[..0]), ("two.json", &notes[2..])] {
            let mut store = NoteStore::open(directory.path().join(file))?;
            store.sign_in("account", None)?;
            store.apply_sync("account", &[], initial.to_vec())?;
            store.apply_sync("account", &[], notes.to_vec())?;
            let ids = store.ids_by_creation();
            let titles = ids
                .iter()
                .map(|&id| store.get(id).unwrap().title.as_str())
                .collect::<Vec<_>>();
            assert_eq!(titles, ["z-older", "a-same-time", "b-newer"]);

            store.update(ids[0], "Older", "Edited locally")?;
            assert_eq!(store.ids_by_creation(), ids);
        }
        Ok(())
    }

    #[test]
    fn snapshot_preserves_edits_and_deletes_made_during_sync() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", Some("adoption"))?;
        let local = store.create_with_body("Local", "First version")?;
        let sent = store.pending();
        let remote = CloudNote {
            key: "remote-note".to_owned(),
            updated_at: 12345,
            created_at: None,
            title: "Remote".to_owned(),
            body: "First version".to_owned(),
            deleted: false,
        };
        store.update(local, "Local", "Newer version")?;
        store.apply_sync("account", &sent, vec![sent[0].note.clone(), remote.clone()])?;
        assert_eq!(store.get(local).unwrap().body, "Newer version");
        let remote_id = store.search("remote")[0].id;
        assert_eq!(store.get(remote_id).unwrap().updated_at, 12345);
        let sent = store.pending();
        let deleted = store.delete(local)?;
        store.apply_sync("account", &sent, vec![sent[0].note.clone(), remote.clone()])?;
        assert!(store.get(local).is_none());
        assert!(store.pending()[0].note.deleted);
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", None)?;
        let sent = store.pending();
        assert!(sent[0].note.deleted);
        store.restore(&deleted)?;
        store.apply_sync("account", &sent, vec![sent[0].note.clone(), remote.clone()])?;
        assert_eq!(store.get(local).unwrap().body, "Newer version");
        let sent = store.pending();
        assert!(!sent[0].note.deleted);
        store.apply_sync("account", &sent, vec![sent[0].note.clone()])?;
        assert!(store.pending().is_empty());
        assert!(store.get(remote_id).is_none());
        assert_eq!(store.get(local).unwrap().body, "Newer version");
        Ok(())
    }

    #[test]
    fn acknowledges_successful_uploads_without_a_snapshot() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", None)?;
        let edited = store.create_with_body("Edited", "First version")?;
        let unchanged = store.create_with_body("Unchanged", "")?;
        let sent = store.pending();
        store.update(edited, "Edited", "Newer version")?;
        store.acknowledge("account", &sent)?;
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", None)?;
        assert_eq!(store.get(unchanged).unwrap().title, "Unchanged");
        let pending = store.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].note.body, "Newer version");
        Ok(())
    }

    #[test]
    fn edits_deletes_and_recency_survive_reopening() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let mut store = NoteStore::open(&path)?;
        let first = store.create_with_body("", "")?;
        let second = store.create_with_body("", "")?;
        store.update(first, "Meeting", "Bring the draft\nConfirm the room")?;
        drop(store);

        let mut store = NoteStore::open(&path)?;
        assert_eq!(store.search("")[0].id, first);
        assert_eq!(
            store.get(first).unwrap().body,
            "Bring the draft\nConfirm the room"
        );
        store.delete(second)?;
        drop(store);

        let mut store = NoteStore::open(&path)?;
        assert!(store.get(second).is_none());
        assert_ne!(store.create_with_body("", "")?, second);
        Ok(())
    }

    #[test]
    fn searches_titles_and_bodies_case_insensitively() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = NoteStore::open(directory.path().join("notes.json"))?;
        let first = store.create_with_body("", "")?;
        store.update(first, "Café", "Fresh PASTA")?;
        let second = store.create_with_body("", "")?;
        store.update(second, "Shopping", "Café supplies")?;

        assert_eq!(
            store
                .search("  CAFÉ pasta ")
                .iter()
                .map(|note| note.id)
                .collect::<Vec<_>>(),
            vec![first]
        );
        assert_eq!(store.search("café").len(), 2);
        assert_eq!(store.search("   ").len(), 2);
        assert!(store.search("missing").is_empty());
        Ok(())
    }

    #[test]
    fn failed_save_leaves_in_memory_notes_intact() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let mut store = NoteStore::open(&path)?;
        let note = store.create_with_body("", "")?;
        fs::remove_file(&path)?;
        fs::create_dir(&path)?;

        assert!(store.update(note, "New title", "New body").is_err());
        assert_eq!(store.get(note).unwrap().title, "");
        assert_eq!(store.search("").len(), 1);
        Ok(())
    }

    #[test]
    fn opening_store_excludes_other_writers_for_its_lifetime() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let mut store = NoteStore::open(&path)?;
        let id = store.create_with_body("Title", "Body")?;
        assert!(matches!(
            NoteStore::open(&path),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
        ));
        drop(store);

        let store = NoteStore::open(&path)?;
        let note = store.get(id).unwrap();
        assert_eq!(note.title, "Title");
        assert_eq!(note.body, "Body");
        Ok(())
    }
}
