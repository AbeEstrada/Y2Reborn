//! The playlist worker: every playlist file is found, read and written on
//! this thread, never on the UI thread (an SD card can stall for seconds).
//! Each edit reads the file fresh and checks the entry it changes is still
//! there, so a change made on a computer meanwhile is never overwritten
//! blindly.

use reborn_core::playlist::{Playlist, PlaylistEdit, PlaylistEntry};
use reborn_observability::isolated;
use reborn_platform::playlists as files;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

pub enum Job {
    Refresh(Vec<PathBuf>),
    Create {
        dir: PathBuf,
        name: String,
        entries: Vec<PlaylistEntry>,
    },
    Edit {
        path: PathBuf,
        edit: PlaylistEdit,
        create: bool,
    },
    Rename {
        path: PathBuf,
        name: String,
    },
    Duplicate(PathBuf),
    Delete(PathBuf),
}

pub enum Reply {
    Listed(Vec<Playlist>),
    /// A playlist was written; `created` when it is new (the UI opens it).
    Saved {
        playlist: Playlist,
        created: bool,
    },
    Renamed {
        old: PathBuf,
        playlist: Playlist,
    },
    Deleted(PathBuf),
    Failed(String),
}

pub struct Worker {
    jobs: Sender<Job>,
    pub replies: Receiver<Reply>,
}

impl Worker {
    pub fn spawn() -> Option<Self> {
        let (jobs, queue) = channel::<Job>();
        let (replies_tx, replies) = channel();
        std::thread::Builder::new()
            .name("playlists".into())
            .spawn(move || {
                while let Ok(job) = queue.recv() {
                    // Y2Linux 8.27: the SD card is read-only between writes;
                    // a change to a playlist on it runs inside a write window.
                    let window = on_card(&job)
                        && reborn_platform::client::sd_write_window(true).is_ok();
                    // A bug on one odd file costs that job, not the player.
                    let reply = isolated(|| run(job)).unwrap_or_else(Reply::Failed);
                    if window {
                        let _ = reborn_platform::client::sd_write_window(false);
                    }
                    let reply = match reply {
                        Reply::Failed(error) if error.contains("os error 30") => Reply::Failed(
                            "The SD card can't be changed right now. If it has errors, repair it in Settings > Storage."
                                .into(),
                        ),
                        reply => reply,
                    };
                    if replies_tx.send(reply).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Self { jobs, replies })
    }
    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

/// The job writes on the SD card (the platform's mount point).
fn on_card(job: &Job) -> bool {
    let path = match job {
        Job::Refresh(_) => return false,
        Job::Create { dir, .. } => dir,
        Job::Edit { path, .. } | Job::Rename { path, .. } => path,
        Job::Duplicate(path) | Job::Delete(path) => path,
    };
    path.starts_with("/media/sd")
}

fn run(job: Job) -> Reply {
    match job {
        Job::Refresh(roots) => Reply::Listed(
            files::discover(&roots)
                .iter()
                .filter_map(|path| isolated(|| files::read(path)).ok()?.ok())
                .collect(),
        ),
        Job::Create { dir, name, entries } => (|| {
            let mut playlist = files::create(&dir, &name)?;
            if !entries.is_empty() {
                playlist.apply(&PlaylistEdit::Append(entries))?;
                save(&mut playlist)?;
            }
            Ok(Reply::Saved {
                playlist,
                created: true,
            })
        })()
        .unwrap_or_else(Reply::Failed),
        Job::Edit { path, edit, create } => (|| {
            let (mut playlist, created) = match files::read(&path) {
                Ok(playlist) => (playlist, false),
                Err(_) if create && !path.exists() => {
                    let dir = path.parent().ok_or("no folder")?;
                    (files::create(dir, &files::name_of(&path))?, true)
                }
                Err(e) => return Err(e),
            };
            playlist.apply(&edit)?;
            save(&mut playlist)?;
            Ok(Reply::Saved { playlist, created })
        })()
        .unwrap_or_else(Reply::Failed),
        Job::Rename { path, name } => (|| {
            let target = files::rename(&path, &name)?;
            Ok(Reply::Renamed {
                old: path,
                playlist: files::read(&target)?,
            })
        })()
        .unwrap_or_else(Reply::Failed),
        Job::Duplicate(path) => (|| {
            let source = files::read(&path)?;
            let dir = path.parent().ok_or("no folder")?;
            let name = copy_name(dir, &source.name);
            let mut copy = files::create(dir, &name)?;
            copy.entries = source.entries;
            copy.trailer = source.trailer;
            save(&mut copy)?;
            Ok(Reply::Saved {
                playlist: copy,
                created: true,
            })
        })()
        .unwrap_or_else(Reply::Failed),
        Job::Delete(path) => match std::fs::remove_file(&path) {
            Ok(()) => Reply::Deleted(path),
            Err(e) => Reply::Failed(format!("Couldn't delete the playlist: {e}")),
        },
    }
}

fn save(playlist: &mut Playlist) -> Result<(), String> {
    let (stamp, encoding) = files::write(playlist)?;
    playlist.stamp = stamp;
    playlist.encoding = encoding;
    Ok(())
}

/// "Road Trip Copy", then "Road Trip Copy 2", … whichever is free.
fn copy_name(dir: &Path, name: &str) -> String {
    let free = |candidate: &str| {
        ["m3u8", "m3u"]
            .iter()
            .all(|e| !dir.join(format!("{candidate}.{e}")).exists())
    };
    let base = format!("{name} Copy");
    if free(&base) {
        return base;
    }
    (2..1000)
        .map(|n| format!("{base} {n}"))
        .find(|c| free(c))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_writes_on_the_sd_card_take_a_write_window() {
        let sd = PathBuf::from("/media/sd/Playlists/Road.m3u8");
        let internal = PathBuf::from("/media/internal/music/Playlists/Road.m3u8");
        assert!(on_card(&Job::Delete(sd.clone())));
        assert!(on_card(&Job::Rename {
            path: sd.clone(),
            name: "x".into()
        }));
        assert!(on_card(&Job::Duplicate(sd)));
        assert!(!on_card(&Job::Delete(internal.clone())));
        assert!(!on_card(&Job::Create {
            dir: internal,
            name: "x".into(),
            entries: vec![]
        }));
        assert!(!on_card(&Job::Refresh(vec![PathBuf::from("/media/sd")])));
    }

    #[test]
    fn edits_create_favorites_duplicate_rename_and_delete_through_the_worker() {
        let dir = std::env::temp_dir().join(format!("reborn-plw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lists = dir.join("Playlists");
        let entry = |l: &str| PlaylistEntry {
            location: l.into(),
            info: None,
            extra: vec![],
        };
        // The heart on a player without Favorites.m3u8 makes it.
        let favorites = lists.join("Favorites.m3u8");
        let Reply::Saved { playlist, created } = run(Job::Edit {
            path: favorites.clone(),
            edit: PlaylistEdit::Append(vec![entry("../a.mp3")]),
            create: true,
        }) else {
            panic!("favorites not saved")
        };
        assert!(created);
        assert_eq!(playlist.entries.len(), 1);
        // A change made on a computer meanwhile is read before the edit.
        std::fs::write(&favorites, "#EXTM3U\n../b.mp3\n../a.mp3\n").unwrap();
        let Reply::Saved { playlist, .. } = run(Job::Edit {
            path: favorites.clone(),
            edit: PlaylistEdit::Remove {
                index: 1,
                location: "../a.mp3".into(),
            },
            create: false,
        }) else {
            panic!("edit failed")
        };
        assert_eq!(playlist.entries, vec![entry("../b.mp3")]);
        assert!(matches!(
            run(Job::Edit {
                path: favorites.clone(),
                edit: PlaylistEdit::Remove {
                    index: 0,
                    location: "../a.mp3".into()
                },
                create: false,
            }),
            Reply::Failed(_)
        ));
        // Create with songs, duplicate, rename, delete.
        let Reply::Saved { playlist, created } = run(Job::Create {
            dir: lists.clone(),
            name: "Road Trip".into(),
            entries: vec![entry("../x.flac")],
        }) else {
            panic!("create failed")
        };
        assert!(created && playlist.path == lists.join("Road Trip.m3u8"));
        let Reply::Saved { playlist: copy, .. } = run(Job::Duplicate(playlist.path.clone())) else {
            panic!("duplicate failed")
        };
        assert_eq!(copy.name, "Road Trip Copy");
        assert_eq!(copy.entries, playlist.entries);
        let Reply::Renamed {
            old,
            playlist: renamed,
        } = run(Job::Rename {
            path: copy.path.clone(),
            name: "Summer".into(),
        })
        else {
            panic!("rename failed")
        };
        assert_eq!((old, renamed.name.as_str()), (copy.path, "Summer"));
        assert!(matches!(
            run(Job::Delete(renamed.path.clone())),
            Reply::Deleted(_)
        ));
        let Reply::Listed(found) = run(Job::Refresh(vec![dir.clone()])) else {
            panic!("refresh failed")
        };
        let mut names: Vec<_> = found.iter().map(|p| p.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["Favorites", "Road Trip"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
