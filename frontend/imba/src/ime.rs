pub trait ImeClient {
    fn has_marked_text(&self) -> bool;

    fn marked_range(&self) -> Option<(u32, u32)>;

    fn selected_range(&self) -> Option<(u32, u32)>;

    fn set_selected_range(&mut self, start: u32, len: u32);

    fn substring_utf16(&self, start: u32, len: u32) -> Option<String>;

    fn document_length(&self) -> u32;

    fn first_rect(&self, start: u32, len: u32) -> Option<(f32, f32, f32, f32)>;

    fn selection_rects(&self, start: u32, len: u32) -> Vec<(f32, f32, f32, f32)>;

    fn reveal_selection(&mut self);

    fn char_index_at(&self, x: f32, y: f32) -> Option<u32>;

    fn insert_text(&mut self, text: &str, replacement: Option<(u32, u32)>);

    fn set_marked_text(
        &mut self,
        text: &str,
        selected: (u32, u32),
        replacement: Option<(u32, u32)>,
    );

    fn unmark_text(&mut self);
}
