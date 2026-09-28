//! GUI side of the shared single-line input (`yalda::line_input`).
//!
//! The model (text + caret + the one key-editing policy) lives in the lib
//! crate so the lib file browser shares it; this module holds what every
//! GUI query/filter/rename field renders with. A site owns a `LineInput`,
//! handles its own Enter/Esc/Up/Down, and routes every other key through
//! `LineInput::handle`, reacting to `LineEdit::Edited`.

pub(crate) use yalda::line_input::{LineEdit, LineInput};

/// The block caret every single-line input draws at its caret position.
pub(crate) const LINE_INPUT_CARET: char = '\u{2588}';
