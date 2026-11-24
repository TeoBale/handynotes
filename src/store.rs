use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub updated_at: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct Notes {
    next_id: u64,
    entries: Vec<Note>,
}

pub struct NoteStore {
    path: PathBuf,
    notes: Notes,
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
            },
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            notes,
            _lock: lock,
        })
    }

    pub fn get(&self, id: u64) -> Option<&Note> {
        self.notes.entries.iter().find(|note| note.id == id)
    }

    pub fn search(&self, query: &str) -> Vec<&Note> {
        let query = query.to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        if terms.is_empty() {
            return self.notes.entries.iter().collect();
        }

        self.notes
            .entries
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
        let id = notes.next_id;
        notes.next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Note IDs are exhausted"))?;
        notes.entries.insert(
            0,
            Note {
                id,
                title: title.to_owned(),
                body: body.to_owned(),
                updated_at: now(),
            },
        );
        self.save(notes)?;
        Ok(id)
    }

    pub fn update(&mut self, id: u64, title: &str, body: &str) -> io::Result<()> {
        let index = self.index_of(id)?;
        let current = &self.notes.entries[index];
        if current.title == title && current.body == body {
            return Ok(());
        }

        let mut notes = self.notes.clone();
        let mut note = notes.entries.remove(index);
        note.title = title.to_owned();
        note.body = body.to_owned();
        note.updated_at = now();
        notes.entries.insert(0, note);
        self.save(notes)
    }

    pub fn delete(&mut self, id: u64) -> io::Result<()> {
        let index = self.index_of(id)?;
        let mut notes = self.notes.clone();
        notes.entries.remove(index);
        self.save(notes)
    }

    fn index_of(&self, id: u64) -> io::Result<usize> {
        self.notes
            .entries
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
