use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

use super::Animation;
use crate::color::{blend_rgb, ratatui_color_to_rgb};

const EXHALE: [&str; 8] = [
    r"",
    r"   /\___/\      _..--------.._",
    r"  /       \ .-''              ''-.",
    r" |  -   -  |'                     `.",
    r" |    ^    |                        \",
    r"  \  ~~~  /                          |",
    r"   `-----'   ,                      /",
    r"  (___(____)_/_______.------'___)___)",
];

const INHALE: [&str; 8] = [
    r"               _..--------.._",
    r"   /\___/\ .-''              ''-.",
    r"  /       \'                      `.",
    r" |  -   -  |                        \",
    r" |    ^    |                         |",
    r"  \  ~~~  /                          |",
    r"   `-----'   ,                      /",
    r"  (___(____)_/_______.------'___)___)",
];

const WALK_BODY: [&str; 6] = [
    r"   /\___/\                          ,",
    r"  /       \  _..--------------.._  //",
    r" |  o   o  |'                    `//",
    r" |    ^    |                      |",
    r"  \  ~~~  /                       |",
    r"   `-----'\                      /",
];
const WALK_LEGS: [[&str; 2]; 2] = [
    [
        r"      \ \  `-.____________..--'\ \",
        r"      /_/                       /_/",
    ],
    [
        r"      / /  `-.____________..--'/ /",
        r"     /_/                       /_/",
    ],
];

const BANNER_WORD: &str = "PURR";
const FACE_TILE: [&str; 3] = [r" /\_/\ ", r"( ^.^ )", r" > ^ < "];
const COMPACT_FACE_TILE: [&str; 1] = ["=^.^="];
const FACE_STYLE: BannerStyle = BannerStyle {
    tile: &FACE_TILE,
    letter_gap: 3,
};
const COMPACT_STYLE: BannerStyle = BannerStyle {
    tile: &COMPACT_FACE_TILE,
    letter_gap: 2,
};
const BANNER_TO_CAT_GAP: i32 = 3;

const BREATH_PERIOD: f64 = 3.2;
const PURR_PERIOD: f64 = 0.45;
const PURR_RIPPLES: i32 = 3;
const PURR_ROWS: [i32; 3] = [3, 4, 5];

const BANNER_WRITE_DURATION: f64 = 2.5;
const LETTER_FADE_DURATION: f64 = 0.6;
const WALK_START: f64 = BANNER_WRITE_DURATION + LETTER_FADE_DURATION;
const WALK_DURATION: f64 = 4.0;
const SLEEP_START: f64 = WALK_START + WALK_DURATION;
const STEP_PERIOD: f64 = 0.25;

const Z_SPAWN_INTERVAL: f64 = 1.5;
const Z_LIFETIME: f64 = 4.5;
const Z_RISE_SPEED: f64 = 1.1;
const Z_DRIFT_SPEED: f64 = 0.8;
const Z_GROW_AGE: f64 = 1.5;
const Z_START_COL: i32 = 2;

const CAT_COLOR: (u8, u8, u8) = (235, 165, 95);
const PURR_COLOR: (u8, u8, u8) = (240, 150, 180);
const Z_COLOR: (u8, u8, u8) = (170, 200, 255);

struct SleepZ {
    age: f64,
}

/// The scene plays once: the banner is written, the cat walks in, then sleeps for good.
enum ScenePhase {
    WritingBanner,
    CatWalking { progress: f64 },
    Asleep { for_secs: f64 },
}

/// How one lit pixel of a banner letter is drawn.
struct BannerStyle {
    tile: &'static [&'static str],
    letter_gap: usize,
}

/// Top-left corners of the banner and the cat, centered as one block.
struct Layout {
    banner: (i32, i32),
    cat: (i32, i32),
    banner_index: usize,
}

/// A "PURR" banner made of cat faces fades in, then a cat walks in from the
/// right, curls up and sleeps: breathing, purring in ripples, with Z's drifting up.
pub struct Purr {
    /// Banner variants from largest to smallest; the largest one that fits is drawn.
    banners: Vec<Vec<String>>,
    zs: Vec<SleepZ>,
    since_last_z: f64,
    time: f64,
    bg: (u8, u8, u8),
}

impl Purr {
    pub fn new(bg: Color) -> Self {
        Self {
            banners: build_banner_variants(BANNER_WORD),
            zs: Vec::new(),
            since_last_z: 0.0,
            time: 0.0,
            bg: ratatui_color_to_rgb(bg),
        }
    }

    fn phase(&self) -> ScenePhase {
        if self.time < WALK_START {
            ScenePhase::WritingBanner
        } else if self.time < SLEEP_START {
            ScenePhase::CatWalking {
                progress: (self.time - WALK_START) / WALK_DURATION,
            }
        } else {
            ScenePhase::Asleep {
                for_secs: self.time - SLEEP_START,
            }
        }
    }

    fn layout(&self, area: Rect) -> Layout {
        let banner_index = self.largest_fitting_banner(area);
        let banner = &self.banners[banner_index];
        let cat_width = text_width(&INHALE);
        let top = (area.height as i32 - scene_height(banner)) / 2;
        let cat_top = top + banner.len() as i32 + BANNER_TO_CAT_GAP;
        Layout {
            banner: ((area.width as i32 - text_width(banner)) / 2, top),
            cat: ((area.width as i32 - cat_width) / 2, cat_top),
            banner_index,
        }
    }

    fn largest_fitting_banner(&self, area: Rect) -> usize {
        let fits = |banner: &Vec<String>| {
            text_width(banner) <= area.width as i32 && scene_height(banner) <= area.height as i32
        };
        self.banners
            .iter()
            .position(fits)
            .unwrap_or(self.banners.len() - 1)
    }

    fn fade(&self, color: (u8, u8, u8), amount: f64) -> Color {
        let (r, g, b) = blend_rgb(color, self.bg, amount as f32);
        Color::Rgb(r, g, b)
    }

    fn fill_background(&self, area: Rect, buf: &mut Buffer) {
        let bg = Color::Rgb(self.bg.0, self.bg.1, self.bg.2);
        for row in 0..area.height {
            for col in 0..area.width {
                if let Some(cell) = buf.cell_mut(Position::new(area.x + col, area.y + row)) {
                    cell.set_symbol(" ");
                    cell.set_bg(bg);
                }
            }
        }
    }

    /// Each column fades in once the writing sweep reaches it, then stays put.
    fn render_banner(&self, banner: &[String], origin: (i32, i32), area: Rect, buf: &mut Buffer) {
        let width = text_width(banner).max(1) as f64;
        for (row, line) in banner.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                let written_at = BANNER_WRITE_DURATION * col as f64 / width;
                let visibility = (self.time - written_at) / LETTER_FADE_DURATION;
                if ch == ' ' || visibility <= 0.0 {
                    continue;
                }
                let fg = self.fade(PURR_COLOR, 1.0 - visibility.min(1.0));
                put(
                    area,
                    buf,
                    (origin.0 + col as i32, origin.1 + row as i32),
                    ch,
                    fg,
                );
            }
        }
    }

    fn render_walking_cat(&self, progress: f64, target: (i32, i32), area: Rect, buf: &mut Buffer) {
        let start_x = area.width as i32;
        let x = start_x - ((start_x - target.0) as f64 * progress) as i32;
        let step = ((self.time / STEP_PERIOD) as usize) % WALK_LEGS.len();
        let lines = WALK_BODY.iter().chain(WALK_LEGS[step].iter());
        self.render_sprite(lines, (x, target.1), area, buf);
    }

    fn render_sleeping_cat(&self, for_secs: f64, origin: (i32, i32), area: Rect, buf: &mut Buffer) {
        let is_inhaling = (for_secs % BREATH_PERIOD) < BREATH_PERIOD / 2.0;
        let sprite = if is_inhaling { &INHALE } else { &EXHALE };
        self.render_sprite(sprite.iter(), origin, area, buf);
    }

    fn render_sprite<'a>(
        &self,
        lines: impl Iterator<Item = &'a &'static str>,
        origin: (i32, i32),
        area: Rect,
        buf: &mut Buffer,
    ) {
        let fg = self.fade(CAT_COLOR, 0.0);
        for (row, line) in lines.enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch != ' ' {
                    put(
                        area,
                        buf,
                        (origin.0 + col as i32, origin.1 + row as i32),
                        ch,
                        fg,
                    );
                }
            }
        }
    }

    fn render_purr(&self, origin: (i32, i32), area: Rect, buf: &mut Buffer) {
        let wave = ((self.time / PURR_PERIOD) as i32) % PURR_RIPPLES;
        let fg = self.fade(PURR_COLOR, wave as f64 / PURR_RIPPLES as f64);
        let cat_width = text_width(&INHALE);
        for row in PURR_ROWS {
            let y = origin.1 + row;
            put(area, buf, (origin.0 - 2 - wave, y), '(', fg);
            put(area, buf, (origin.0 + cat_width + 1 + wave, y), ')', fg);
        }
    }

    fn render_zs(&self, origin: (i32, i32), area: Rect, buf: &mut Buffer) {
        for z in &self.zs {
            let x = origin.0 + Z_START_COL - (z.age * Z_DRIFT_SPEED) as i32;
            let y = origin.1 - (z.age * Z_RISE_SPEED) as i32;
            let symbol = if z.age < Z_GROW_AGE { 'z' } else { 'Z' };
            let fg = self.fade(Z_COLOR, z.age / Z_LIFETIME);
            put(area, buf, (x, y), symbol, fg);
        }
    }
}

/// Full-size faces in one row, then stacked in two rows, then the same with compact faces.
fn build_banner_variants(word: &str) -> Vec<Vec<String>> {
    let (first_half, second_half) = word.split_at(word.len() / 2);
    [FACE_STYLE, COMPACT_STYLE]
        .iter()
        .flat_map(|style| {
            [
                build_banner(&[word], style),
                build_banner(&[first_half, second_half], style),
            ]
        })
        .collect()
}

/// Spells each word on its own row in a 3x5 pixel font, drawing each lit pixel as a tile.
fn build_banner(rows: &[&str], style: &BannerStyle) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, word) in rows.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }
        lines.extend(build_banner_row(word, style));
    }
    lines
}

fn build_banner_row(word: &str, style: &BannerStyle) -> Vec<String> {
    let letters: Vec<[&str; 5]> = word.chars().map(letter_pixels).collect();
    let gap = " ".repeat(style.letter_gap);
    let blank_tile = " ".repeat(text_width(style.tile) as usize);
    let mut lines = Vec::new();
    for pixel_row in 0..5 {
        for tile_line in style.tile {
            let letter_lines: Vec<String> = letters
                .iter()
                .map(|letter| {
                    letter[pixel_row]
                        .chars()
                        .map(|pixel| {
                            if pixel == '#' {
                                *tile_line
                            } else {
                                blank_tile.as_str()
                            }
                        })
                        .collect()
                })
                .collect();
            lines.push(letter_lines.join(&gap));
        }
    }
    lines
}

fn letter_pixels(letter: char) -> [&'static str; 5] {
    match letter {
        'P' => ["###", "#.#", "###", "#..", "#.."],
        'U' => ["#.#", "#.#", "#.#", "#.#", "###"],
        'R' => ["###", "#.#", "##.", "#.#", "#.#"],
        _ => ["...", "...", "...", "...", "..."],
    }
}

fn scene_height(banner: &[String]) -> i32 {
    banner.len() as i32 + BANNER_TO_CAT_GAP + INHALE.len() as i32
}

fn text_width<S: AsRef<str>>(lines: &[S]) -> i32 {
    lines
        .iter()
        .map(|line| line.as_ref().chars().count())
        .max()
        .unwrap_or(0) as i32
}

fn put(area: Rect, buf: &mut Buffer, (x, y): (i32, i32), symbol: char, fg: Color) {
    let inside = x >= 0 && y >= 0 && x < area.width as i32 && y < area.height as i32;
    if !inside {
        return;
    }
    if let Some(cell) = buf.cell_mut(Position::new(area.x + x as u16, area.y + y as u16)) {
        cell.set_char(symbol);
        cell.set_fg(fg);
    }
}

impl Animation for Purr {
    fn tick(&mut self, dt: f64) {
        self.time += dt;

        let time_asleep = self.time - SLEEP_START;
        if time_asleep < 0.0 {
            return;
        }
        self.since_last_z += dt.min(time_asleep);
        while self.since_last_z >= Z_SPAWN_INTERVAL {
            self.since_last_z -= Z_SPAWN_INTERVAL;
            self.zs.push(SleepZ { age: 0.0 });
        }

        for z in &mut self.zs {
            z.age += dt;
        }
        self.zs.retain(|z| z.age < Z_LIFETIME);
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.fill_background(area, buf);
        let layout = self.layout(area);
        self.render_banner(&self.banners[layout.banner_index], layout.banner, area, buf);
        match self.phase() {
            ScenePhase::WritingBanner => {}
            ScenePhase::CatWalking { progress } => {
                self.render_walking_cat(progress, layout.cat, area, buf);
            }
            ScenePhase::Asleep { for_secs } => {
                self.render_purr(layout.cat, area, buf);
                self.render_sleeping_cat(for_secs, layout.cat, area, buf);
                self.render_zs(layout.cat, area, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: Color = Color::Rgb(10, 10, 35);
    const WIDE: Rect = Rect::new(0, 0, 120, 40);

    fn render_to_string(cat: &Purr, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        cat.render(area, &mut buf);
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    fn cat_at(time: f64) -> Purr {
        let mut cat = Purr::new(BG);
        cat.tick(time);
        cat
    }

    #[test]
    fn test_banner_starts_unwritten() {
        assert!(!render_to_string(&cat_at(0.0), WIDE).contains("^.^"));
    }

    #[test]
    fn test_banner_is_written_left_to_right() {
        let halfway = render_to_string(&cat_at(BANNER_WRITE_DURATION / 2.0), WIDE);
        let cells = halfway.chars().collect::<Vec<_>>();
        let row_with_faces = cells
            .chunks(WIDE.width as usize)
            .find(|row| row.iter().collect::<String>().contains("^.^"))
            .expect("some faces are written by halfway");
        let row: String = row_with_faces.iter().collect();
        assert!(row[..60].contains("^.^"));
        assert!(!row[60..].contains("^.^"));
    }

    #[test]
    fn test_banner_is_fully_drawn_with_cat_faces_once_written() {
        let rendered = render_to_string(&cat_at(WALK_START), WIDE);
        assert!(rendered.contains("( ^.^ )( ^.^ )( ^.^ )"));
    }

    #[test]
    fn test_no_cat_while_banner_is_written() {
        assert!(!render_to_string(&cat_at(0.0), WIDE).contains("~~~"));
    }

    #[test]
    fn test_cat_walks_in_awake_from_the_right() {
        let early = render_to_string(&cat_at(WALK_START + 1.0), WIDE);
        let later = render_to_string(&cat_at(WALK_START + 2.0), WIDE);
        assert!(early.contains("|  o   o  |"));
        assert!(early.find("o   o").unwrap() % 120 > later.find("o   o").unwrap() % 120);
    }

    #[test]
    fn test_cat_falls_asleep_after_walking() {
        let rendered = render_to_string(&cat_at(SLEEP_START), WIDE);
        assert!(rendered.contains("|  -   -  |"));
    }

    #[test]
    fn test_cat_stays_asleep() {
        let rendered = render_to_string(&cat_at(SLEEP_START + 600.0), WIDE);
        assert!(rendered.contains("|  -   -  |"));
    }

    #[test]
    fn test_no_zs_before_sleeping() {
        assert!(cat_at(SLEEP_START - 0.1).zs.is_empty());
    }

    #[test]
    fn test_first_sleeping_tick_spawns_no_backlog_of_zs() {
        assert!(cat_at(SLEEP_START).zs.is_empty());
    }

    #[test]
    fn test_banner_has_three_lines_per_pixel_row() {
        let banner = build_banner(&["PURR"], &FACE_STYLE);
        assert_eq!(banner.len(), 5 * FACE_TILE.len());
    }

    #[test]
    fn test_banner_width_fits_letters_and_gaps() {
        let banner = build_banner(&["PURR"], &FACE_STYLE);
        assert_eq!(text_width(&banner), 4 * 3 * 7 + 3 * 3);
    }

    #[test]
    fn test_banner_spells_p_top_row_as_three_faces() {
        let banner = build_banner(&["P"], &FACE_STYLE);
        assert_eq!(banner[1], "( ^.^ )( ^.^ )( ^.^ )");
    }

    #[test]
    fn test_stacked_banner_puts_second_half_below_a_blank_line() {
        let one_row = build_banner(&["PU"], &FACE_STYLE);
        let stacked = build_banner(&["PU", "RR"], &FACE_STYLE);
        assert_eq!(stacked.len(), one_row.len() * 2 + 1);
        assert_eq!(stacked[one_row.len()], "");
    }

    #[test]
    fn test_wide_terminal_uses_full_faces_in_one_row() {
        let cat = Purr::new(BG);
        assert_eq!(cat.layout(WIDE).banner_index, 0);
    }

    #[test]
    fn test_tall_narrow_terminal_stacks_full_faces() {
        let cat = Purr::new(BG);
        assert_eq!(cat.layout(Rect::new(0, 0, 80, 50)).banner_index, 1);
    }

    #[test]
    fn test_standard_terminal_uses_compact_faces_that_fit() {
        let area = Rect::new(0, 0, 80, 24);
        let cat = Purr::new(BG);
        let banner = &cat.banners[cat.layout(area).banner_index];
        assert!(banner.iter().any(|line| line.contains("=^.^=")));
        assert!(text_width(banner) <= 80 && scene_height(banner) <= 24);
    }

    #[test]
    fn test_nothing_fits_falls_back_to_smallest_banner() {
        let cat = Purr::new(BG);
        let index = cat.layout(Rect::new(0, 0, 5, 2)).banner_index;
        assert_eq!(index, cat.banners.len() - 1);
    }

    #[test]
    fn test_cat_breathes_between_two_frames() {
        let mut cat = cat_at(SLEEP_START);
        let inhale = render_to_string(&cat, WIDE);
        cat.tick(BREATH_PERIOD / 2.0 + 0.01);
        let exhale = render_to_string(&cat, WIDE);
        assert_ne!(inhale, exhale);
    }

    #[test]
    fn test_zs_spawn_and_expire() {
        let mut cat = cat_at(SLEEP_START);
        cat.tick(Z_SPAWN_INTERVAL);
        assert_eq!(cat.zs.len(), 1);
        for _ in 0..100 {
            cat.tick(Z_LIFETIME / 10.0);
        }
        assert!(cat.zs.iter().all(|z| z.age < Z_LIFETIME));
    }

    #[test]
    fn test_tiny_area_does_not_panic() {
        let area = Rect::new(0, 0, 5, 2);
        let mut cat = Purr::new(BG);
        cat.tick(SLEEP_START + 5.0);
        render_to_string(&cat, area);
    }

    #[test]
    fn test_background_uses_configured_color() {
        let mut buf = Buffer::empty(WIDE);
        Purr::new(BG).render(WIDE, &mut buf);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().bg, BG);
    }
}
