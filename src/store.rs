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
}

// Convex serializes integral numbers as JSON floats, including millisecond timestamps.
fn timestamp_millis<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let millis = f64::deserialize(d)?;
    if millis.fract() == 0.0 && (0.0..=(1_u64 << 53) as f64 - 1.0).contains(&millis) {
        Ok(millis as u64)
    } else {
        Err(serde::de::Error::custom("Invalid note timestamp"))
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct AccountNotes {
    entries: Vec<Note>,
    outbox: BTreeMap<String, CloudNote>,
}

impl AccountNotes {
    fn acknowledge(&mut self, sent: &[CloudNote]) {
        for note in sent {
            if self.outbox.get(&note.key) == Some(note) {
                self.outbox.remove(&note.key);
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
                CloudNote {
                    key: key.clone(),
                    updated_at: note.updated_at,
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

    pub fn sign_in(&mut self, account: &str, adopt_local: bool) -> io::Result<()> {
        if !self.notes.accounts.contains_key(account) || (adopt_local && self.has_local_notes()) {
            let mut notes = self.notes.clone();
            notes.accounts.entry(account.to_owned()).or_default();
            if adopt_local {
                for mut note in std::mem::take(&mut notes.entries) {
                    note.cloud_key = Some(Uuid::new_v4().to_string());
                    notes.queue_change(Some(account), &note, false);
                    notes.entries_mut(Some(account)).push(note);
                }
                notes
                    .entries_mut(Some(account))
                    .sort_by_key(|note| std::cmp::Reverse(note.updated_at));
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

    pub fn pending(&self) -> Vec<CloudNote> {
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

    pub fn acknowledge(&mut self, account: &str, sent: &[CloudNote]) -> io::Result<()> {
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
        sent: &[CloudNote],
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
        let note = Note {
            id,
            title: title.to_owned(),
            body: body.to_owned(),
            updated_at: now(),
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
        let raw =
            r#"{"key":"note","body":"x","title":"x","deleted":false,"updated_at":1750000000123.0}"#;
        assert_eq!(
            serde_json::from_str::<CloudNote>(raw).unwrap().updated_at,
            1750000000123
        );
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

        store.sign_in("first", false)?;
        assert!(store.search("").is_empty());
        assert!(store.has_local_notes());
        store.sign_in("first", true)?;
        assert_eq!(store.get(7).unwrap().title, "Legacy");
        assert!(!store.has_local_notes());
        let pending = store.pending();
        assert_eq!(pending.len(), 1);
        assert!(Uuid::parse_str(&pending[0].key).is_ok());
        drop(store);

        let mut store = NoteStore::open(&path)?;
        assert_eq!(store.active_account(), None);
        assert!(store.search("").is_empty());
        store.sign_in("second", false)?;
        assert!(store.pending().is_empty());
        assert!(store.get(7).is_none());
        let second = store.create_with_body("Second", "")?;
        assert_eq!(second, 8);
        store.apply_sync("first", &pending, pending.clone())?;
        assert_eq!(store.get(second).unwrap().title, "Second");

        store.sign_in("first", false)?;
        assert_eq!(store.pending(), pending);
        assert!(store.get(second).is_none());
        let deleted = store.delete(7)?;
        store.sign_in("second", false)?;
        assert!(store.restore(&deleted).is_err());
        store.sign_out()?;
        assert!(store.search("").is_empty());
        Ok(())
    }

    #[test]
    fn snapshot_preserves_edits_and_deletes_made_during_sync() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("notes.json");
        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", true)?;
        let local = store.create_with_body("Local", "First version")?;
        let sent = store.pending();
        let remote = CloudNote {
            key: "remote-note".to_owned(),
            updated_at: 12345,
            title: "Remote".to_owned(),
            body: "First version".to_owned(),
            deleted: false,
        };
        store.update(local, "Local", "Newer version")?;
        store.apply_sync("account", &sent, vec![sent[0].clone(), remote.clone()])?;
        assert_eq!(store.get(local).unwrap().body, "Newer version");
        let remote_id = store.search("remote")[0].id;
        assert_eq!(store.get(remote_id).unwrap().updated_at, 12345);
        let sent = store.pending();
        let deleted = store.delete(local)?;
        store.apply_sync("account", &sent, vec![sent[0].clone(), remote.clone()])?;
        assert!(store.get(local).is_none());
        assert!(store.pending()[0].deleted);
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", false)?;
        let sent = store.pending();
        assert!(sent[0].deleted);
        store.restore(&deleted)?;
        store.apply_sync("account", &sent, vec![sent[0].clone(), remote.clone()])?;
        assert_eq!(store.get(local).unwrap().body, "Newer version");
        let sent = store.pending();
        assert!(!sent[0].deleted);
        store.apply_sync("account", &sent, vec![sent[0].clone()])?;
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
        store.sign_in("account", false)?;
        let edited = store.create_with_body("Edited", "First version")?;
        let unchanged = store.create_with_body("Unchanged", "")?;
        let sent = store.pending();
        store.update(edited, "Edited", "Newer version")?;
        store.acknowledge("account", &sent)?;
        drop(store);

        let mut store = NoteStore::open(&path)?;
        store.sign_in("account", false)?;
        assert_eq!(store.get(unchanged).unwrap().title, "Unchanged");
        let pending = store.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].body, "Newer version");
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
