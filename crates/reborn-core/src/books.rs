//! Audiobooks: which files are books, how they form books and in what order
//! their parts play.
//!
//! A file is a book when a folder named Audiobooks or Hörbücher (any case,
//! anywhere in its path) holds it, when it is an `.m4b`, or when its genre
//! says Audiobook or Hörbuch. Nothing here is stored: it follows from the
//! path and the tags, so a rule change needs no rescan.
//!
//! A book is the file's folder. Folders such as `CD 2` or `Part 1` fold into
//! the book above them, never past the first folder below the Audiobooks
//! folder (`Audiobooks/Part 1/` stays a book). `.m4b` files are whole books:
//! in one folder they are separate books unless they share an album tag
//! (`Book Part 1.m4b`, `Book Part 2.m4b`). A file lying directly in the
//! Audiobooks folder is a book of its own.
use crate::{sort::fold, Track};
use std::{cmp::Ordering, collections::HashMap, path::Path};

/// Folder names (folded) that hold audiobooks.
const FOLDERS: [&str; 7] = [
    "audiobooks",
    "audiobook",
    "audio books",
    "horbucher",
    "horbuch",
    "hoerbuecher",
    "hoerbuch",
];
/// Genres (folded) that mark a file as a book.
const GENRES: [&str; 6] = [
    "audiobook",
    "audiobooks",
    "audio book",
    "horbuch",
    "horbucher",
    "hoerbuch",
];

fn is_m4b(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("m4b"))
}

/// Where the Audiobooks folder is among `path`'s folders (component index).
fn marker(path: &Path) -> Option<usize> {
    let parent = path.parent()?;
    parent
        .components()
        .position(|c| FOLDERS.contains(&fold(&c.as_os_str().to_string_lossy()).as_str()))
}

/// `Disc 2`, `CD1`, `Part 3`, `Vol. 1`, `Side A` and the like.
fn subdivision(name: &str) -> bool {
    let folded = fold(name);
    let mut words = folded.split(' ');
    let (Some(word), rest) = (words.next(), words.collect::<Vec<_>>()) else {
        return false;
    };
    const NAMES: [&str; 11] = [
        "disc", "disk", "cd", "part", "pt", "vol", "volume", "tape", "side", "teil", "tl",
    ];
    // "cd1" or "cd 1": a name, then a number (or a side letter).
    let (name, number) = match word.find(|c: char| c.is_ascii_digit()) {
        Some(at) if rest.is_empty() => (&word[..at], word[at..].to_string()),
        _ if rest.len() == 1 => (word, rest[0].to_string()),
        _ => return false,
    };
    NAMES.contains(&name)
        && (number.chars().all(|c| c.is_ascii_digit()) && !number.is_empty()
            || (name == "side" && number.len() == 1))
}

/// Whether `track` is a book (computed once when the library loads).
pub fn is_book(track: &Track) -> bool {
    !track.is_radio()
        && (marker(&track.path).is_some()
            || is_m4b(&track.path)
            || GENRES.contains(&fold(&track.genre).as_str()))
}

/// The book a file belongs to: its stable key, the folder it is named after
/// and whether the file alone is the book (then its title names it).
fn identity(track: &Track) -> (String, std::path::PathBuf, bool) {
    let parent = track.path.parent().unwrap_or(Path::new("")).to_path_buf();
    let mark = marker(&track.path);
    let depth = parent.components().count();
    // Directly in the Audiobooks folder: a book of its own.
    if mark.is_some_and(|m| m + 1 == depth) && !is_m4b(&track.path) {
        return (
            format!("{}|{}", track.source_id, track.path.display()),
            parent,
            true,
        );
    }
    // Fold disc folders into the book, never above the first folder below
    // the Audiobooks folder.
    let floor = mark.map_or(1, |m| m + 2);
    let mut dir = parent;
    while dir.components().count() > floor
        && dir
            .file_name()
            .is_some_and(|n| subdivision(&n.to_string_lossy()))
    {
        dir.pop();
    }
    let mut key = format!("{}|{}", track.source_id, dir.display());
    let mut alone = false;
    if is_m4b(&track.path) {
        let album = fold(&track.album);
        if album.is_empty() {
            key.push('|');
            key.push_str(&track.filename);
            alone = true;
        } else {
            key.push_str("|album:");
            key.push_str(&album);
        }
    }
    (key, dir, alone)
}

/// One audiobook: its parts in play order, as indexes into the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    /// Stable identity (source, folder, and for `.m4b` the album or file).
    pub key: String,
    pub title: String,
    /// The album artist or artist all parts share, else an author folder.
    pub author: String,
    pub parts: Vec<usize>,
    /// Sum of the parts; 0 when any part's length is unknown.
    pub duration_ms: u64,
    /// A part with a cover, for the book's picture.
    pub art: Option<usize>,
}

/// The value every item shares, if there is one and it is not empty.
fn shared<'a>(mut values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let first = values.next()?.trim();
    (!first.is_empty() && values.all(|v| v.trim() == first)).then_some(first)
}

/// Every book in `tracks` (only online books), A to Z by title.
pub fn books(tracks: &[Track]) -> Vec<Book> {
    let mut groups: HashMap<String, (std::path::PathBuf, bool, Vec<usize>)> = HashMap::new();
    for (i, t) in tracks.iter().enumerate() {
        if t.book && t.online {
            let (key, dir, alone) = identity(t);
            groups
                .entry(key)
                .or_insert_with(|| (dir, alone, vec![]))
                .2
                .push(i);
        }
    }
    let mut books: Vec<Book> = groups
        .into_iter()
        .map(|(key, (dir, alone, mut parts))| {
            order(tracks, &mut parts);
            let all = || parts.iter().map(|&i| &tracks[i]);
            let title = shared(all().map(|t| t.album.as_str()))
                .map(str::to_owned)
                .or_else(|| alone.then(|| tracks[parts[0]].title.clone()))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| {
                    dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            let author = shared(all().map(|t| t.album_artist.as_str()))
                .or_else(|| shared(all().map(|t| t.artist.as_str())))
                .map(str::to_owned)
                .unwrap_or_else(|| author_folder(&tracks[parts[0]].path, &dir));
            let duration_ms = if all().any(|t| t.duration_ms == 0) {
                0
            } else {
                all().map(|t| t.duration_ms).sum()
            };
            let art = parts.iter().copied().find(|&i| tracks[i].artwork);
            Book {
                key,
                title,
                author,
                parts,
                duration_ms,
                art,
            }
        })
        .collect();
    books.sort_by_cached_key(|b| (fold(&b.title), fold(&b.author), b.key.clone()));
    books
}

/// `Audiobooks/<Author>/<Book>`: the folder between, when there is one.
fn author_folder(path: &Path, book: &Path) -> String {
    let Some(m) = marker(path) else {
        return String::new();
    };
    let names: Vec<_> = book.components().collect();
    if names.len() == m + 3 {
        names[m + 1].as_os_str().to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

/// Play order: disc, folder, track number (only when every part has one),
/// then file name with numbers compared as numbers.
pub fn order(tracks: &[Track], parts: &mut [usize]) {
    let numbered = parts.iter().all(|&i| tracks[i].track > 0);
    parts.sort_by(|&a, &b| {
        let (x, y) = (&tracks[a], &tracks[b]);
        x.disc
            .cmp(&y.disc)
            .then_with(|| {
                natural(
                    &x.path.parent().unwrap_or(Path::new("")).to_string_lossy(),
                    &y.path.parent().unwrap_or(Path::new("")).to_string_lossy(),
                )
            })
            .then_with(|| {
                if numbered {
                    x.track.cmp(&y.track)
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| natural(&x.filename, &y.filename))
            .then_with(|| x.id.cmp(&y.id))
    });
}

/// Compare with runs of digits as numbers ("2" before "10"), ignoring case.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |s: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = s.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        s.next();
                    }
                    n
                };
                let (n, m) = (take(&mut a), take(&mut b));
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let ord = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> Track {
        let mut t = Track {
            id: path.len() as i64,
            source_id: "internal".into(),
            path: path.into(),
            filename: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            title: Path::new(path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            duration_ms: 60_000,
            online: true,
            ..Default::default()
        };
        t.book = is_book(&t);
        t
    }
    fn keys(tracks: &[Track]) -> Vec<(String, Vec<String>)> {
        books(tracks)
            .into_iter()
            .map(|b| {
                (
                    b.title,
                    b.parts
                        .iter()
                        .map(|&i| tracks[i].filename.clone())
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn folders_m4b_and_genre_make_books_and_music_stays_music() {
        assert!(file("/m/Audiobooks/Dune/01.mp3").book);
        assert!(
            file("/m/Media/HÖRBÜCHER/Momo/01.mp3").book,
            "any case, umlauts"
        );
        assert!(file("/m/Hoerbuecher/Momo/01.mp3").book);
        assert!(file("/m/Music/Book.M4B").book);
        assert!(!file("/m/Music/Song.m4a").book);
        assert!(
            !file("/m/MyAudiobooks/x.mp3").book,
            "a word inside a name is not the folder"
        );
        assert!(
            !file("/m/Music/Audiobooks.mp3").book,
            "a file name is not the folder"
        );
        let mut tagged = file("/m/Music/Talk/01.mp3");
        tagged.genre = "Hörbuch".into();
        assert!(is_book(&tagged));
        let mut radio = Track::radio(&Default::default());
        radio.path = "/Audiobooks/stream".into();
        assert!(!is_book(&radio));
    }

    #[test]
    fn disc_folders_fold_into_the_book_and_author_folders_stay() {
        let tracks = [
            file("/m/Audiobooks/Frank Herbert/Dune/CD 2/01.mp3"),
            file("/m/Audiobooks/Frank Herbert/Dune/CD 1/02.mp3"),
            file("/m/Audiobooks/Frank Herbert/Dune/CD 1/01.mp3"),
            file("/m/Audiobooks/Frank Herbert/Dune/CD 10/01.mp3"),
            file("/m/Audiobooks/Part 1/a.mp3"),
            file("/m/Audiobooks/Loose.mp3"),
        ];
        let books = books(&tracks);
        let dune = books.iter().find(|b| b.title == "Dune").unwrap();
        assert_eq!(
            dune.author, "Frank Herbert",
            "the author folder names the author"
        );
        let order: Vec<_> = dune.parts.iter().map(|&i| tracks[i].path.clone()).collect();
        assert_eq!(
            order,
            [
                "/m/Audiobooks/Frank Herbert/Dune/CD 1/01.mp3",
                "/m/Audiobooks/Frank Herbert/Dune/CD 1/02.mp3",
                "/m/Audiobooks/Frank Herbert/Dune/CD 2/01.mp3",
                "/m/Audiobooks/Frank Herbert/Dune/CD 10/01.mp3",
            ]
            .map(std::path::PathBuf::from)
        );
        assert_eq!(dune.duration_ms, 4 * 60_000);
        assert!(
            books.iter().any(|b| b.title == "Part 1"),
            "a book named like a part"
        );
        assert!(
            books.iter().any(|b| b.title == "Loose"),
            "a loose file is a book"
        );
        assert_eq!(books.len(), 3);
    }

    #[test]
    fn m4b_files_are_whole_books_unless_they_share_an_album() {
        let mut tracks = vec![
            file("/m/Audiobooks/Tolkien/Hobbit.m4b"),
            file("/m/Audiobooks/Tolkien/Return of the King.m4b"),
            file("/m/Music/Book Part 2.m4b"),
            file("/m/Music/Book Part 1.m4b"),
        ];
        tracks[2].album = "The Book".into();
        tracks[3].album = "The Book".into();
        assert_eq!(
            keys(&tracks),
            [
                ("Hobbit".to_string(), vec!["Hobbit.m4b".to_string()]),
                (
                    "Return of the King".to_string(),
                    vec!["Return of the King.m4b".to_string()]
                ),
                (
                    "The Book".to_string(),
                    vec!["Book Part 1.m4b".to_string(), "Book Part 2.m4b".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn tags_name_the_book_and_order_follows_disc_then_track_then_name() {
        let mut tracks = vec![
            file("/m/Audiobooks/x/b.mp3"),
            file("/m/Audiobooks/x/a.mp3"),
            file("/m/Audiobooks/x/c.mp3"),
        ];
        for (t, (n, d)) in tracks.iter_mut().zip([(1, 2), (2, 1), (1, 1)]) {
            t.album = "Momo".into();
            t.album_artist = "Michael Ende".into();
            t.track = n;
            t.disc = d;
        }
        let book = &books(&tracks)[0];
        assert_eq!(
            (book.title.as_str(), book.author.as_str()),
            ("Momo", "Michael Ende")
        );
        let names: Vec<_> = book
            .parts
            .iter()
            .map(|&i| tracks[i].filename.as_str())
            .collect();
        assert_eq!(names, ["c.mp3", "a.mp3", "b.mp3"]);
        // One part without a number: names decide, naturally.
        tracks[0].track = 0;
        tracks[0].disc = 0;
        tracks[1].disc = 0;
        tracks[2].disc = 0;
        let mut parts = vec![0, 1, 2];
        order(&tracks, &mut parts);
        assert_eq!(parts, [1, 0, 2]);
        // An unknown length leaves the book's length unknown.
        tracks[1].duration_ms = 0;
        assert_eq!(books(&tracks)[0].duration_ms, 0);
    }

    #[test]
    fn natural_order_reads_numbers_as_numbers() {
        assert_eq!(natural("Chapter 2", "chapter 10"), Ordering::Less);
        assert_eq!(natural("01", "1"), Ordering::Equal);
        assert_eq!(natural("b", "A"), Ordering::Greater);
        assert!(subdivision("CD1") && subdivision("Disc 02") && subdivision("Part 3"));
        assert!(subdivision("Side A") && subdivision("Teil 2"));
        assert!(!subdivision("Discovery") && !subdivision("Part of Me") && !subdivision("CD"));
    }
}
