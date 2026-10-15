//! Library search (Beta 5): artists, albums and songs whose words begin with
//! every typed word. Case, accents and punctuation are ignored as in A to Z
//! order (`reborn_core::sort::fold`): "beat" finds The Beatles, "sigur ros"
//! finds Sigur Rós, "beatles abbey" finds Abbey Road.
use reborn_core::{
    sort::{fold, sort_key, SortAs},
    Track,
};
use std::collections::BTreeMap;

/// Folded names per track, rebuilt only when the library changes.
#[derive(Default)]
pub struct Index {
    stamp: Option<(usize, usize)>,
    /// title, artist, album, album artist (folded) per track.
    folded: Vec<[String; 4]>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Results {
    /// Artist name and a track standing for it (its cover).
    pub artists: Vec<(String, usize)>,
    /// A track of each album (album and album artist are taken from it).
    pub albums: Vec<usize>,
    pub songs: Vec<usize>,
}

impl Results {
    /// "2 artists · 1 album · 14 songs", or "No matches".
    pub fn summary(&self) -> String {
        let part = |n: usize, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        let parts: Vec<String> = [
            part(self.artists.len(), "artist", "artists"),
            part(self.albums.len(), "album", "albums"),
            part(self.songs.len(), "song", "songs"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            "No matches".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// Every query word begins a word of `name`.
fn matches(query: &[&str], name: &str) -> bool {
    query
        .iter()
        .all(|q| name.split(' ').any(|word| word.starts_with(q)))
}

impl Index {
    fn ensure(&mut self, tracks: &[Track]) {
        let stamp = (tracks.as_ptr() as usize, tracks.len());
        if self.stamp == Some(stamp) {
            return;
        }
        self.stamp = Some(stamp);
        self.folded = tracks
            .iter()
            .map(|t| {
                [
                    fold(if t.title.is_empty() {
                        &t.filename
                    } else {
                        &t.title
                    }),
                    fold(&t.artist),
                    fold(&t.album),
                    fold(&t.album_artist),
                ]
            })
            .collect();
    }

    pub fn search(&mut self, tracks: &[Track], query: &str) -> Results {
        self.ensure(tracks);
        let folded = fold(query);
        let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
        let mut results = Results::default();
        if words.is_empty() {
            return results;
        }
        let mut artists: BTreeMap<&str, usize> = BTreeMap::new();
        let mut albums: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for (i, (t, [title, artist, album, album_artist])) in
            tracks.iter().zip(&self.folded).enumerate()
        {
            if !t.online {
                continue;
            }
            if !t.artist.is_empty() && matches(&words, artist) {
                // The first track with a cover stands for the artist.
                let entry = artists.entry(t.artist.as_str()).or_insert(i);
                if !tracks[*entry].artwork && t.artwork {
                    *entry = i;
                }
            }
            let owner = if album_artist.is_empty() {
                artist
            } else {
                album_artist
            };
            if !t.album.is_empty() && matches(&words, &format!("{album} {owner}")) {
                albums
                    .entry((t.album.as_str(), t.album_artist.as_str()))
                    .or_insert(i);
            }
            if matches(&words, &format!("{title} {artist} {album}")) {
                results.songs.push(i);
            }
        }
        results.artists = artists
            .into_iter()
            .map(|(name, i)| (name.to_owned(), i))
            .collect();
        results
            .artists
            .sort_by_cached_key(|(name, _)| sort_key(name, SortAs::Artist));
        results.albums = albums.into_values().collect();
        results
            .albums
            .sort_by_cached_key(|&i| sort_key(&tracks[i].album, SortAs::Title));
        results
            .songs
            .sort_by_cached_key(|&i| sort_key(crate::track_title(&tracks[i]), SortAs::Title));
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, album: &str) -> Track {
        Track {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            online: true,
            ..Default::default()
        }
    }

    #[test]
    fn word_beginnings_ignore_case_accents_and_articles() {
        let tracks = vec![
            track("Come Together", "The Beatles", "Abbey Road"),
            track("Something", "The Beatles", "Abbey Road"),
            track("Hoppípolla", "Sigur Rós", "Takk..."),
            track("Beat It", "Michael Jackson", "Thriller"),
        ];
        let mut index = Index::default();
        let r = index.search(&tracks, "beat");
        assert_eq!(r.artists, vec![("The Beatles".to_owned(), 0)]);
        assert_eq!(
            r.songs,
            vec![3, 0, 1],
            "Beat It, then the Beatles songs by title"
        );
        // An album is found by its artist's name too.
        assert_eq!(r.albums, vec![0]);
        assert_eq!(r.summary(), "1 artist · 1 album · 3 songs");
        let r = index.search(&tracks, "SIGUR ros");
        assert_eq!(r.artists.len(), 1);
        assert_eq!(r.songs, vec![2]);
        let r = index.search(&tracks, "beatles abbey");
        assert_eq!(r.albums, vec![0]);
        assert_eq!(r.songs.len(), 2);
        // Inside a word is not a beginning.
        assert_eq!(index.search(&tracks, "eatles").summary(), "No matches");
        assert_eq!(index.search(&tracks, "   "), Results::default());
    }

    #[test]
    fn offline_tracks_are_left_out() {
        let mut tracks = vec![track("Yesterday", "The Beatles", "Help!")];
        tracks[0].online = false;
        assert_eq!(
            Index::default().search(&tracks, "yes").summary(),
            "No matches"
        );
    }
}
