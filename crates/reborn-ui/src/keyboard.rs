//! The full-screen keyboard for Wi-Fi passwords, station search and library
//! search (Beta 5).
//!
//! Driven by buttons, not the wheel (`reborn_platform::input::ActionRouter`
//! in keyboard mode): Back, Play/Pause, Previous and Next move the focus up,
//! down, left and right; Select types the focused key; holding Select is
//! Done, holding Back cancels. Rows wrap, so Done is one press above the
//! letters.
use crate::{
    canvas::{Canvas, Icon},
    components,
    text::{self, Face},
    theme::{alpha, color, WIDTH},
};
use reborn_core::Action;

/// What the typed text is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    WifiPassword {
        ssid: String,
    },
    RadioSearch,
    LibrarySearch,
    /// A new playlist's name, or a playlist's new name.
    PlaylistNew,
    PlaylistRename,
    /// Last.fm fallback sign-in: the username, then the password.
    LastFmUser,
    LastFmPassword {
        username: String,
    },
}

impl Purpose {
    fn max_chars(&self) -> usize {
        match self {
            // WPA passphrases: 8 to 63 printable ASCII characters.
            Self::WifiPassword { .. } => 63,
            Self::RadioSearch | Self::LibrarySearch => 40,
            Self::PlaylistNew | Self::PlaylistRename => 60,
            Self::LastFmUser => 32,
            Self::LastFmPassword { .. } => 64,
        }
    }
    fn secret(&self) -> bool {
        matches!(
            self,
            Self::WifiPassword { .. } | Self::LastFmPassword { .. }
        )
    }
    pub fn done_label(&self) -> &'static str {
        match self {
            Self::WifiPassword { .. } => "Join",
            Self::RadioSearch | Self::LibrarySearch => "Search",
            Self::PlaylistNew => "Create",
            Self::PlaylistRename => "Rename",
            Self::LastFmUser => "Next",
            Self::LastFmPassword { .. } => "Sign In",
        }
    }
    pub(crate) fn title(&self) -> String {
        match self {
            // The prompt names the network; the title says what this is.
            Self::WifiPassword { .. } => "Wi-Fi Password".into(),
            Self::RadioSearch => "Radio Search".into(),
            Self::LibrarySearch => "Search".into(),
            Self::PlaylistNew => "New Playlist".into(),
            Self::PlaylistRename => "Rename Playlist".into(),
            Self::LastFmUser => "Last.fm Username".into(),
            Self::LastFmPassword { .. } => "Last.fm Password".into(),
        }
    }
    fn prompt(&self) -> String {
        match self {
            Self::WifiPassword { ssid } => format!("Password for “{ssid}”"),
            Self::RadioSearch => "Station name".into(),
            Self::LibrarySearch => "Artist, album or song".into(),
            Self::PlaylistNew | Self::PlaylistRename => "Playlist name".into(),
            Self::LastFmUser => "Username".into(),
            Self::LastFmPassword { username } => format!("Password for “{username}”"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Letters,
    Digits,
    Symbols,
    Accents,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Off,
    /// The next letter only.
    Once,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Shift,
    /// Switch to another page of keys.
    Page(Layer),
    Space,
    Delete,
    /// Show or hide the password.
    Reveal,
    Cancel,
    Done,
}

/// A key and its width in tenths of the row.
type Slot = (Key, f32);

const LETTERS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const DIGITS: [&str; 3] = ["1234567890", "-/:;()$&@\"", ".,?!'_%"];
const SYMBOLS: [&str; 3] = ["[]{}#^*+=`", "\\|~<>.,?!'", "_-/:;\"@"];
const ACCENTS: [&str; 3] = ["äöüßéèêàâá", "íìîóòôúùû", "çñøåæëï"];

#[derive(Clone, Debug)]
pub struct Keyboard {
    pub purpose: Purpose,
    pub text: String,
    pub layer: Layer,
    pub shift: Shift,
    /// Show the password instead of dots.
    pub reveal: bool,
    pub row: usize,
    pub col: usize,
    /// A short line under the text (the library search's live count).
    pub note: String,
    /// Library search: the results so far, shown above the keys as you type.
    pub results: Vec<crate::Item>,
    /// A result has the focus instead of a key (Back on the top row).
    pub result_focus: Option<usize>,
    pub result_scroll: usize,
    /// Where up/down aim, kept over several rows so Down then Up returns to
    /// the same key across the half-key offsets.
    aim: Option<f32>,
}

/// What a key press asks of the owner of the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing beyond redrawing.
    Stay,
    /// The text changed (the library search recounts).
    Edited,
    Done,
    Cancel,
    /// Select on the result at this position.
    Open(usize),
}

/// Results visible at once above the live keyboard.
pub const LIVE_ROWS: usize = 3;

impl Keyboard {
    /// The library search shows its results while typing.
    pub fn live(&self) -> bool {
        self.purpose == Purpose::LibrarySearch
    }

    /// New results after the text changed; the focus stays on the keys.
    pub fn set_results(&mut self, results: Vec<crate::Item>) {
        self.results = results;
        self.result_focus = None;
        self.result_scroll = 0;
    }

    fn focus_result(&mut self, index: usize) {
        self.result_focus = Some(index);
        self.result_scroll = self
            .result_scroll
            .min(index)
            .max((index + 1).saturating_sub(LIVE_ROWS));
    }

    /// Buttons while a result has the focus: up and down walk the list, Down
    /// past its end returns to the keys, Select opens the result.
    fn result_action(&mut self, focus: usize, action: Action) -> Outcome {
        match action {
            Action::NavigateUp | Action::Back => {
                if focus > 0 {
                    self.focus_result(focus - 1);
                }
                Outcome::Stay
            }
            Action::NavigateDown if focus + 1 < self.results.len() => {
                self.focus_result(focus + 1);
                Outcome::Stay
            }
            Action::NavigateDown => {
                self.result_focus = None;
                self.row = 0;
                Outcome::Stay
            }
            Action::Select => Outcome::Open(focus),
            Action::ContextMenu => Outcome::Done,
            Action::Home => Outcome::Cancel,
            _ => Outcome::Stay,
        }
    }

    pub fn new(purpose: Purpose, text: impl Into<String>) -> Self {
        Self {
            purpose,
            text: text.into(),
            layer: Layer::Letters,
            shift: Shift::Off,
            reveal: false,
            row: 0,
            col: 0,
            note: String::new(),
            results: vec![],
            result_focus: None,
            result_scroll: 0,
            aim: None,
        }
    }

    /// The rows of the current page, each 10 tenths wide.
    pub fn rows(&self) -> [Vec<Slot>; 4] {
        let chars = match self.layer {
            Layer::Letters => LETTERS,
            Layer::Digits => DIGITS,
            Layer::Symbols => SYMBOLS,
            Layer::Accents => ACCENTS,
        };
        let upper =
            self.shift != Shift::Off && matches!(self.layer, Layer::Letters | Layer::Accents);
        let keys = |s: &str| -> Vec<Slot> {
            s.chars()
                .map(|c| {
                    let c = if upper { upper_case(c) } else { c };
                    (Key::Char(c), 1.)
                })
                .collect()
        };
        let mut third = vec![match self.layer {
            Layer::Letters | Layer::Accents => (Key::Shift, 1.5),
            Layer::Digits => (Key::Page(Layer::Symbols), 1.5),
            Layer::Symbols => (Key::Page(Layer::Digits), 1.5),
        }];
        third.extend(keys(chars[2]));
        third.push((Key::Delete, 1.5));
        let page = match self.layer {
            Layer::Letters | Layer::Accents => Key::Page(Layer::Digits),
            Layer::Digits | Layer::Symbols => Key::Page(Layer::Letters),
        };
        // Passwords are ASCII: their extra key shows the password instead.
        let extra = if self.purpose.secret() {
            Key::Reveal
        } else if self.layer == Layer::Accents {
            Key::Page(Layer::Letters)
        } else {
            Key::Page(Layer::Accents)
        };
        [
            keys(chars[0]),
            keys(chars[1]),
            third,
            vec![
                (page, 1.5),
                (extra, 1.),
                (Key::Space, 3.5),
                (Key::Cancel, 1.5),
                (Key::Done, 2.5),
            ],
        ]
    }

    /// Each key's left edge and width in tenths; short rows are centred.
    pub fn spans(row: &[Slot]) -> Vec<(f32, f32)> {
        let total: f32 = row.iter().map(|(_, w)| w).sum();
        let mut x = (10. - total) / 2.;
        row.iter()
            .map(|&(_, w)| {
                let span = (x, w);
                x += w;
                span
            })
            .collect()
    }

    pub fn focused(&self) -> Key {
        let rows = self.rows();
        let row = &rows[self.row.min(3)];
        row[self.col.min(row.len() - 1)].0
    }

    /// Keep the focus on the key under the same spot after the page changed.
    fn keep_column(&mut self, from: f32) {
        let rows = self.rows();
        let spans = Self::spans(&rows[self.row]);
        self.col = spans
            .iter()
            .enumerate()
            // On a tie (half-key offsets) the right-hand key wins.
            .min_by(|(_, a), (_, b)| {
                let da = (a.0 + a.1 / 2. - from).abs();
                let db = (b.0 + b.1 / 2. - from).abs();
                da.total_cmp(&db).then(std::cmp::Ordering::Greater)
            })
            .map_or(0, |(i, _)| i);
    }

    fn centre(&self) -> f32 {
        let rows = self.rows();
        let spans = Self::spans(&rows[self.row]);
        let (x, w) = spans[self.col.min(spans.len() - 1)];
        x + w / 2.
    }

    /// One button press. Wheel turns never reach here (`ActionRouter`).
    pub fn action(&mut self, action: Action) -> Outcome {
        if let Some(focus) = self.result_focus {
            return self.result_action(focus, action);
        }
        // Up from the top row goes to the results just above the keys.
        if self.live()
            && self.row == 0
            && !self.results.is_empty()
            && matches!(action, Action::NavigateUp | Action::Back)
        {
            let last = (self.result_scroll + LIVE_ROWS).min(self.results.len()) - 1;
            self.focus_result(last);
            return Outcome::Stay;
        }
        if !matches!(
            action,
            Action::NavigateUp | Action::NavigateDown | Action::Back
        ) {
            self.aim = None;
        }
        match action {
            Action::NavigateLeft | Action::NavigateRight => {
                let len = self.rows()[self.row].len();
                self.col = if action == Action::NavigateLeft {
                    (self.col + len - 1) % len
                } else {
                    (self.col + 1) % len
                };
                Outcome::Stay
            }
            Action::NavigateUp | Action::NavigateDown | Action::Back => {
                let from = *self.aim.get_or_insert(self.centre());
                self.row = if action == Action::NavigateDown {
                    (self.row + 1) % 4
                } else {
                    (self.row + 3) % 4
                };
                self.keep_column(from);
                Outcome::Stay
            }
            Action::Select => self.press(),
            Action::ContextMenu => Outcome::Done,
            Action::Home => Outcome::Cancel,
            _ => Outcome::Stay,
        }
    }

    fn press(&mut self) -> Outcome {
        match self.focused() {
            Key::Char(c) => self.type_char(c),
            Key::Space => self.type_char(' '),
            Key::Delete => {
                if self.text.pop().is_some() {
                    Outcome::Edited
                } else {
                    Outcome::Stay
                }
            }
            Key::Shift => {
                self.shift = match self.shift {
                    Shift::Off => Shift::Once,
                    Shift::Once => Shift::Locked,
                    Shift::Locked => Shift::Off,
                };
                Outcome::Stay
            }
            Key::Page(layer) => {
                let from = self.centre();
                self.layer = layer;
                if layer == Layer::Digits || layer == Layer::Symbols {
                    self.shift = Shift::Off;
                }
                self.keep_column(from);
                Outcome::Stay
            }
            Key::Reveal => {
                self.reveal = !self.reveal;
                Outcome::Stay
            }
            Key::Cancel => Outcome::Cancel,
            Key::Done => Outcome::Done,
        }
    }

    fn type_char(&mut self, c: char) -> Outcome {
        if self.text.chars().count() >= self.purpose.max_chars() {
            return Outcome::Stay;
        }
        self.text.push(c);
        if self.shift == Shift::Once {
            self.shift = Shift::Off;
        }
        Outcome::Edited
    }
}

/// Upper case where that is one letter (ß stays ß).
fn upper_case(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

// ---------------------------------------------------------------- drawing

/// Where the field and the keys go: roomy for a password or a station,
/// compact under the live results for the library search.
struct Layout {
    field_y: f32,
    field_h: f32,
    key_top: f32,
    key_h: f32,
    row_step: f32,
}
const ROOMY: Layout = Layout {
    field_y: 70.,
    field_h: 42.,
    key_top: 124.,
    // No hint line under the keys (UI v4): Cancel and the done key say it.
    key_h: 48.,
    row_step: 54.,
};
const COMPACT: Layout = Layout {
    field_y: 42.,
    field_h: 36.,
    key_top: 200.,
    key_h: 34.,
    row_step: 39.,
};
const RESULTS_TOP: f32 = 82.;
const RESULT_H: f32 = 37.;
const UNIT: f32 = 46.;
const LEFT: f32 = (WIDTH - 10. * UNIT) / 2.;

pub fn draw(
    c: &mut Canvas,
    ui: &crate::Ui,
    m: &reborn_core::AppModel,
    tracks: &[reborn_core::Track],
    k: &Keyboard,
) {
    draw_with(c, ui, m, tracks, k, true);
}

/// The keyboard under a skin's own title bar (its `keyboard` layout).
pub(crate) fn draw_body(
    c: &mut Canvas,
    ui: &crate::Ui,
    m: &reborn_core::AppModel,
    tracks: &[reborn_core::Track],
    k: &Keyboard,
) {
    draw_with(c, ui, m, tracks, k, false);
}

fn draw_with(
    c: &mut Canvas,
    ui: &crate::Ui,
    m: &reborn_core::AppModel,
    tracks: &[reborn_core::Track],
    k: &Keyboard,
    header: bool,
) {
    let accent = ui.accent();
    let live = k.live();
    let at = if live { &COMPACT } else { &ROOMY };
    c.rect(0., 0., WIDTH, crate::theme::HEIGHT, color::BG);
    if header {
        components::status_bar(c, ui, m, &k.purpose.title(), false);
    }
    if !live {
        c.text(
            LEFT + 4.,
            58. + Face::R13.mid_offset(),
            &text::fit(Face::R13, &k.purpose.prompt(), 10. * UNIT - 8.),
            Face::R13,
            color::MUTED,
        );
    }
    // The text field: the end of the text stays in view.
    let mid = at.field_y + at.field_h / 2.;
    c.rrect(LEFT, at.field_y, 10. * UNIT, at.field_h, 12., color::GLASS);
    let shown = if k.purpose.secret() && !k.reveal {
        "•".repeat(k.text.chars().count())
    } else {
        k.text.clone()
    };
    let note_w = if k.note.is_empty() {
        0.
    } else {
        text::width(Face::R11, &k.note) + 14.
    };
    let room = 10. * UNIT - 34. - note_w;
    let shown = tail(Face::S20, &shown, room);
    let w = text::width(Face::S20, &shown);
    if shown.is_empty() && live {
        // The live search names what it finds where the text will go.
        c.text(
            LEFT + 22.,
            mid + Face::R15.mid_offset(),
            &k.purpose.prompt(),
            Face::R15,
            color::DIM,
        );
    }
    c.text(
        LEFT + 14.,
        mid + Face::S20.mid_offset(),
        &shown,
        Face::S20,
        color::TEXT,
    );
    let caret = (at.field_h - 20.).max(16.);
    c.rect(
        (LEFT + 14. + w + 2.).round(),
        (mid - caret / 2.).round(),
        2.,
        caret,
        if k.result_focus.is_some() {
            color::DIM
        } else {
            accent
        },
    );
    if !k.note.is_empty() {
        c.text_mid_right(
            LEFT + 10. * UNIT - 12.,
            mid,
            &k.note,
            Face::R11,
            color::MUTED,
        );
    }
    if live {
        draw_results(c, ui, tracks, k, accent);
    }
    let rows = k.rows();
    for (r, row) in rows.iter().enumerate() {
        let y = at.key_top + r as f32 * at.row_step;
        for (i, ((key, _), (x, w))) in row.iter().zip(Keyboard::spans(row)).enumerate() {
            let focused = k.result_focus.is_none() && r == k.row && i == k.col.min(row.len() - 1);
            let (x, w) = (LEFT + x * UNIT + 2., w * UNIT - 4.);
            draw_key(c, k, *key, x, y, w, at.key_h, focused, accent);
        }
    }
}

/// The live results: three rows above the keys, or a hint while empty.
fn draw_results(
    c: &mut Canvas,
    ui: &crate::Ui,
    tracks: &[reborn_core::Track],
    k: &Keyboard,
    accent: u32,
) {
    let area = LIVE_ROWS as f32 * RESULT_H;
    if k.results.is_empty() {
        let line = if k.text.trim().is_empty() {
            "Results appear as you type"
        } else {
            "No matches"
        };
        let mid = RESULTS_TOP + area / 2.;
        c.text_center(
            240.,
            mid + Face::S15.mid_offset(),
            line,
            Face::S15,
            color::MUTED,
        );
        return;
    }
    let bg_selected = components::FOCUS_OVER_BG;
    for (i, item) in k
        .results
        .iter()
        .enumerate()
        .skip(k.result_scroll)
        .take(LIVE_ROWS)
    {
        let y = RESULTS_TOP + (i - k.result_scroll) as f32 * RESULT_H;
        let selected = k.result_focus == Some(i);
        if selected {
            components::focus_row(c, LEFT, y + 1., 10. * UNIT, RESULT_H - 2.);
        }
        let art = item.art.and_then(|a| tracks.get(a).map(|t| (a, t)));
        let size = RESULT_H - 9.;
        components::cover(c, ui, art, LEFT + 6., y + 4.5, size);
        if item.key.starts_with("artist:") {
            c.round_off(
                LEFT + 6.,
                y + 4.5,
                size,
                size / 2.,
                if selected { bg_selected } else { color::BG },
            );
        }
        let x = LEFT + 16. + size;
        let room = 10. * UNIT - (x - LEFT) - 12.;
        let sub_w = text::width(Face::R11, &item.secondary).min(room * 0.45);
        c.text_mid(
            x,
            y + RESULT_H / 2.,
            room - sub_w - 12.,
            &item.label,
            if selected { Face::S15 } else { Face::R15 },
            color::TEXT,
        );
        c.text_mid_right(
            LEFT + 10. * UNIT - 12.,
            y + RESULT_H / 2.,
            &text::fit(Face::R11, &item.secondary, sub_w + 1.),
            Face::R11,
            color::MUTED,
        );
    }
    if k.results.len() > LIVE_ROWS {
        components::scroll_rail(
            c,
            RESULTS_TOP + 2.,
            area - 4.,
            k.result_scroll,
            k.results.len(),
            LIVE_ROWS,
            accent,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    c: &mut Canvas,
    k: &Keyboard,
    key: Key,
    x: f32,
    y: f32,
    w: f32,
    key_h: f32,
    focused: bool,
    accent: u32,
) {
    let function = !matches!(key, Key::Char(_) | Key::Space);
    let lit = match key {
        Key::Shift => k.shift != Shift::Off,
        Key::Reveal => k.reveal,
        Key::Done => true,
        _ => false,
    };
    let (fill, ink) = if focused {
        (0xF2F3F5FF, color::INK)
    } else if lit {
        (
            alpha(accent, if key == Key::Done { 120 } else { 90 }),
            color::TEXT,
        )
    } else if function {
        (color::GLASS_FAINT, color::MUTED)
    } else {
        (color::GLASS, color::TEXT)
    };
    let at = c.draw.len();
    if focused {
        // The focused key is lifted a little, as in the rest of Reborn.
        c.rrect(x - 3., y - 4., w + 6., key_h + 6., 11., fill);
        c.mark_focus(at);
    } else {
        c.rrect(x, y, w, key_h, 9., fill);
    }
    let (cx, cy) = (x + w / 2., y + key_h / 2. - if focused { 1. } else { 0. });
    let word = |c: &mut Canvas, s: &str| c.text_mid_center(cx, cy, s, Face::S13, ink);
    match key {
        Key::Char(ch) => {
            c.text_mid_center(cx, cy, &ch.to_string(), Face::S20, ink);
        }
        Key::Shift => c.icon(
            if k.shift == Shift::Off {
                Icon::Shift
            } else {
                Icon::ShiftOn
            },
            cx,
            cy,
            24.,
            ink,
        ),
        Key::Delete => c.icon(Icon::Backspace, cx, cy, 24., ink),
        Key::Reveal => c.icon(
            if k.reveal { Icon::EyeOff } else { Icon::Eye },
            cx,
            cy,
            24.,
            ink,
        ),
        Key::Page(Layer::Letters) => word(c, "abc"),
        Key::Page(Layer::Digits) => word(c, "123"),
        Key::Page(Layer::Symbols) => word(c, "#+="),
        Key::Page(Layer::Accents) => word(c, "àé"),
        Key::Space => word(c, "space"),
        Key::Cancel => word(c, "Cancel"),
        Key::Done => word(c, k.purpose.done_label()),
    }
    if key == Key::Shift && k.shift == Shift::Locked {
        c.rect(cx - 7., y + key_h - 8., 14., 2., ink);
    }
}

/// The end of `s` that fits `width`, with "…" in front when cut.
fn tail(face: Face, s: &str, width: f32) -> String {
    if text::width(face, s) <= width {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    for start in 1..chars.len() {
        let candidate: String = std::iter::once('…')
            .chain(chars[start..].iter().copied())
            .collect();
        if text::width(face, &candidate) <= width {
            return candidate;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(purpose: Purpose) -> Keyboard {
        Keyboard::new(purpose, "")
    }
    fn press(k: &mut Keyboard, actions: &[Action]) {
        for &a in actions {
            k.action(a);
        }
    }

    #[test]
    fn every_row_fills_the_width_and_keys_do_not_overlap() {
        for purpose in [
            Purpose::WifiPassword {
                ssid: "Home".into(),
            },
            Purpose::LibrarySearch,
        ] {
            let mut k = kb(purpose);
            for layer in [
                Layer::Letters,
                Layer::Digits,
                Layer::Symbols,
                Layer::Accents,
            ] {
                k.layer = layer;
                for row in k.rows() {
                    let spans = Keyboard::spans(&row);
                    let (x, w) = *spans.last().unwrap();
                    assert!(spans[0].0 >= 0. && x + w <= 10.001, "{layer:?}");
                    for pair in spans.windows(2) {
                        assert!((pair[0].0 + pair[0].1 - pair[1].0).abs() < 0.001);
                    }
                }
            }
        }
    }

    #[test]
    fn arrows_move_and_wrap_and_select_types() {
        let mut k = kb(Purpose::LibrarySearch);
        assert_eq!(k.focused(), Key::Char('q'));
        press(&mut k, &[Action::NavigateLeft]);
        assert_eq!(k.focused(), Key::Char('p'), "left wraps to the row's end");
        press(&mut k, &[Action::NavigateRight, Action::NavigateRight]);
        assert_eq!(k.focused(), Key::Char('w'));
        assert_eq!(k.action(Action::Select), Outcome::Edited);
        press(&mut k, &[Action::NavigateDown]);
        assert_eq!(k.focused(), Key::Char('s'), "down keeps the column");
        press(&mut k, &[Action::NavigateUp]);
        assert_eq!(k.focused(), Key::Char('w'), "and up comes back");
        press(&mut k, &[Action::NavigateDown]);
        // Up from the top row wraps to the bottom row: Done is one press away.
        press(&mut k, &[Action::NavigateUp, Action::NavigateUp]);
        assert_eq!(k.row, 3);
        k.col = 4;
        assert_eq!(k.focused(), Key::Done);
        assert_eq!(k.action(Action::Select), Outcome::Done);
        assert_eq!(k.text, "w");
    }

    #[test]
    fn shift_once_then_lock_and_pages_keep_the_spot() {
        let mut k = kb(Purpose::LibrarySearch);
        k.row = 2;
        k.col = 0;
        assert_eq!(k.focused(), Key::Shift);
        k.action(Action::Select);
        assert_eq!(k.shift, Shift::Once);
        k.action(Action::NavigateRight);
        k.action(Action::Select);
        k.action(Action::Select);
        assert_eq!(k.text, "Zz", "shift applies to one letter");
        k.col = 0;
        k.action(Action::Select);
        k.action(Action::Select);
        assert_eq!(k.shift, Shift::Locked);
        k.col = 1;
        k.action(Action::Select);
        k.action(Action::Select);
        assert_eq!(k.text, "ZzZZ");
        // 123 and back: digits, then the symbol page.
        k.row = 3;
        k.col = 0;
        k.action(Action::Select);
        assert_eq!(k.layer, Layer::Digits);
        assert_eq!(k.shift, Shift::Off);
        k.row = 0;
        k.col = 0;
        k.action(Action::Select);
        assert_eq!(k.text, "ZzZZ1");
        k.row = 2;
        k.action(Action::Select);
        assert_eq!(k.layer, Layer::Symbols);
        k.action(Action::Back);
        assert_eq!(k.row, 1, "Back is up");
    }

    #[test]
    fn passwords_reveal_instead_of_accents_and_stop_at_63() {
        let mut k = kb(Purpose::WifiPassword {
            ssid: "Home".into(),
        });
        k.row = 3;
        k.col = 1;
        assert_eq!(k.focused(), Key::Reveal);
        k.action(Action::Select);
        assert!(k.reveal);
        k.row = 0;
        k.col = 0;
        for _ in 0..70 {
            k.action(Action::Select);
        }
        assert_eq!(k.text.len(), 63);
        // The search keyboard has the accent page there.
        let mut s = kb(Purpose::RadioSearch);
        s.row = 3;
        s.col = 1;
        assert_eq!(s.focused(), Key::Page(Layer::Accents));
        s.action(Action::Select);
        s.row = 0;
        s.col = 0;
        s.action(Action::Select);
        assert_eq!(s.text, "ä");
    }

    #[test]
    fn delete_hold_select_and_hold_back() {
        let mut k = Keyboard::new(Purpose::RadioSearch, "jazz");
        k.row = 2;
        k.col = 8;
        assert_eq!(k.focused(), Key::Delete);
        assert_eq!(k.action(Action::Select), Outcome::Edited);
        assert_eq!(k.text, "jaz");
        assert_eq!(k.action(Action::ContextMenu), Outcome::Done);
        assert_eq!(k.action(Action::Home), Outcome::Cancel);
        k.row = 3;
        k.col = 3;
        assert_eq!(k.focused(), Key::Cancel);
        assert_eq!(k.action(Action::Select), Outcome::Cancel);
        // The wheel never types or moves.
        let before = (k.row, k.col, k.text.clone());
        k.action(Action::WheelClockwise(3));
        assert_eq!(before, (k.row, k.col, k.text.clone()));
    }

    #[test]
    fn live_results_are_reached_from_the_top_row_and_left_downwards() {
        let mut k = kb(Purpose::LibrarySearch);
        assert!(k.live());
        // No results: Up from the top row wraps to the bottom keys as before.
        k.action(Action::NavigateUp);
        assert_eq!((k.row, k.result_focus), (3, None));
        k.row = 0;
        k.set_results(
            (0..5)
                .map(|i| crate::Item::new(format!("r{i}"), format!("track:{i}")))
                .collect(),
        );
        // Up: the result just above the keys (the third visible one).
        k.action(Action::NavigateUp);
        assert_eq!(k.result_focus, Some(2));
        k.action(Action::Back);
        k.action(Action::Back);
        k.action(Action::Back);
        assert_eq!(k.result_focus, Some(0), "stops at the first result");
        for _ in 0..4 {
            k.action(Action::NavigateDown);
        }
        assert_eq!(
            (k.result_focus, k.result_scroll),
            (Some(4), 2),
            "scrolls with the focus"
        );
        assert_eq!(k.action(Action::Select), Outcome::Open(4));
        k.action(Action::NavigateDown);
        assert_eq!(
            (k.result_focus, k.row),
            (None, 0),
            "past the end: back to the keys"
        );
        // Typing replaces the results and keeps the focus on the keys.
        k.set_results(vec![]);
        assert_eq!(k.result_focus, None);
        // Passwords and station search never show results.
        assert!(!kb(Purpose::RadioSearch).live());
    }

    #[test]
    fn the_field_shows_the_end_of_long_text() {
        let long = "a very long station name that does not fit";
        let shown = tail(Face::S20, long, 120.);
        assert!(shown.starts_with('…') && shown.ends_with("fit"));
        assert!(text::width(Face::S20, &shown) <= 120.);
    }
}
