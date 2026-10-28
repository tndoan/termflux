pub mod external;
pub mod fire;
pub mod matrix;
pub mod purr;
pub mod starfield;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

pub trait Animation: Send {
    fn tick(&mut self, dt: f64);
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn resize(&mut self, _rows: u16, _cols: u16) {}
}
