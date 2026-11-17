//! What the cover worker is asked to decode for a song: a list thumbnail, Cover Flow's centre cover.
use super::*;

/// The worker's job for `track`'s picture, filed under the renderer's `thumb` key.
pub(super) fn job(model: &AppModel, track: &Track, thumb: u64, cache: PathBuf) -> artwork::Job {
    let mount = model
        .sources
        .iter()
        .find(|s| s.id == track.source_id)
        .and_then(|s| s.mount_id);
    artwork::Job {
        thumb,
        art: artwork::Key::new(track, mount, &model.settings.music_directory).into(),
        cache,
    }
}
