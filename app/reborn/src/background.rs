//! Freshness and quiescence for read-only observations on the existing watchdog.
use reborn_core::Source;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
struct State {
    generation: AtomicU64,
    active: AtomicBool,
}
#[derive(Clone)]
pub struct Authority(Arc<State>);
pub struct Gate {
    authority: Authority,
    music: PathBuf,
    sources: Vec<Source>,
    prepared: bool,
}
#[derive(Clone)]
pub struct Request {
    pub music: PathBuf,
    generation: u64,
}
impl Gate {
    pub fn new(music: &Path, sources: &[Source]) -> Self {
        Self {
            authority: Authority(Arc::new(State {
                generation: AtomicU64::new(1),
                active: AtomicBool::new(false),
            })),
            music: music.into(),
            sources: sources.to_vec(),
            prepared: false,
        }
    }
    pub fn authority(&self) -> Authority {
        self.authority.clone()
    }
    pub fn update(&mut self, music: &Path, sources: &[Source], prepared: bool) -> bool {
        if self.music == music && self.sources == sources && self.prepared == prepared {
            return false;
        }
        self.music = music.into();
        self.sources = sources.to_vec();
        self.prepared = prepared;
        if prepared {
            self.quiesce();
        } else {
            self.authority.0.generation.fetch_add(1, Ordering::AcqRel);
        }
        true
    }
    pub fn request(&self) -> Option<Request> {
        (!self.prepared).then(|| Request {
            music: self.music.clone(),
            generation: self.authority.0.generation.load(Ordering::Acquire),
        })
    }
    pub fn accepts(&self, request: &Request) -> bool {
        !self.prepared && self.authority.current(request)
    }
    /// Invalidate queued and in-flight results before acknowledging sleep.
    /// An active read must finish before the caller can acknowledge preparation.
    pub fn quiesce(&mut self) -> bool {
        self.prepared = true;
        self.authority.0.generation.fetch_add(1, Ordering::AcqRel);
        !self.authority.0.active.load(Ordering::Acquire)
    }
}
impl Authority {
    fn current(&self, request: &Request) -> bool {
        request.generation == self.0.generation.load(Ordering::Acquire)
    }
    pub fn run<T>(&self, request: &Request, work: impl FnOnce() -> T) -> Option<T> {
        if !self.current(request) || self.0.active.swap(true, Ordering::AcqRel) {
            return None;
        }
        struct Active<'a>(&'a AtomicBool);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _active = Active(&self.0.active);
        if !self.current(request) {
            return None;
        }
        let result = work();
        self.current(request).then_some(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    #[test]
    fn source_music_and_resume_generations_reject_old_work_even_after_reinsertion() {
        let mut gate = Gate::new(Path::new("/music"), &[]);
        let old = gate.request().unwrap();
        assert!(gate.update(Path::new("/other"), &[], false));
        assert!(gate.update(Path::new("/music"), &[], false));
        assert!(!gate.accepts(&old));
        assert!(gate
            .authority()
            .run(&old, || panic!("stale queued I/O ran"))
            .is_none());
        let source = Source {
            id: "card".into(),
            mount_id: Some(1),
            kind: reborn_core::MediaSource::Internal,
            root: "/music".into(),
            online: true,
            mount: "fixture".into(),
        };
        assert!(gate.update(Path::new("/music"), std::slice::from_ref(&source), false));
        let old = gate.request().unwrap();
        let mut replaced = source;
        replaced.mount_id = Some(2);
        gate.update(Path::new("/music"), &[replaced], false);
        assert!(!gate.accepts(&old));
        let current = gate.request().unwrap();
        assert!(gate.quiesce());
        assert!(gate.request().is_none());
        assert!(!gate.accepts(&current));
        gate.update(Path::new("/music"), &[], false);
        assert!(!gate.accepts(&current));
        assert!(gate.request().is_some());
    }
    #[test]
    fn slow_inflight_read_refuses_sleep_ack_and_cannot_publish_after_invalidation() {
        let mut gate = Gate::new(Path::new("/music"), &[]);
        let request = gate.request().unwrap();
        let authority = gate.authority();
        let (started, ready) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        let worker = std::thread::spawn(move || {
            authority.run(&request, || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                42
            })
        });
        ready.recv().unwrap();
        assert!(!gate.quiesce());
        release.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), None);
        assert!(gate.quiesce());
    }
}
