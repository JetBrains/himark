// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba, TypeRole};
use serde::Deserialize;
use skia_safe::Color;

impl ToolbarChrome {
    pub const KEY: Key<ToolbarChrome> = Key::new("toolbar");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ToolbarChrome {
    pub height: f32,
    pub background: Rgba,

    pub rule: Rgba,

    /// The active button's backdrop.
    pub pressed_fill: Rgba,

    pub title_size: f32,
    pub title_color: Rgba,

    pub button_size: f32,
    pub button_gap: f32,
    pub button_inset: f32,
    pub button_radius: f32,
    pub glyph_color: Rgba,

    /// Every column header puts its title's baseline at this y from
    /// the band's top — one line across the window, whatever each
    /// header's largest word (docs/Design.md: the headers share ONE
    /// baseline).
    pub title_baseline: f32,
    /// The header's subtitle ("in himark-jb", the file's directory):
    /// the subtitle role, this far after the title.
    pub subtitle_gap: f32,
    /// The gap between the trailing text buttons of a header.
    pub button_text_gap: f32,
    /// The trailing inset the rightmost text button keeps off the
    /// column's edge.
    pub trailing_inset: f32,
    /// The tabs' gap in the side column's header (History · Files …).
    pub tab_gap: f32,
    /// A title standing AFTER tabs keeps this much off the last one —
    /// the tabs read as a group, the title as the column's name.
    pub tabs_title_gap: f32,
    /// A header text button's color — secondary ink; the display
    /// role's button (Sessions) draws in primary ink.
    pub button_text_color: Rgba,
    /// The dimmed tab / quiet button color.
    pub dim_text_color: Rgba,
    /// The LEFTMOST header's title inset — past the platform
    /// semaphore (the traffic lights), short of the column's text
    /// edge so the title leads the prose by half a gutter.
    pub lead_inset: f32,
}

impl Default for ToolbarChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            height: 104.0,
            background: Rgba(Color::new(0xff0d0d0f)),
            rule: Rgba(Color::new(0xff222226)),
            pressed_fill: Rgba(Color::new(0xff17171b)),
            title_size: 34.0,
            title_color: Rgba(Color::new(0xffeceae4)),
            button_size: 44.0,
            button_gap: 10.0,
            button_inset: 0.0,
            button_radius: 0.0,
            glyph_color: Rgba(Color::new(0xff8a8781)),
            title_baseline: 66.0,
            subtitle_gap: 16.0,
            button_text_gap: 32.0,
            trailing_inset: 48.0,
            tab_gap: 24.0,
            tabs_title_gap: 48.0,
            button_text_color: Rgba(Color::new(0xffb5b2ab)),
            dim_text_color: Rgba(Color::new(0xff8a8781)),
            lead_inset: 144.0,
        }
    }
}

impl ChatChrome {
    pub const KEY: Key<ChatChrome> = Key::new("chat");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ChatChrome {
    pub pad: f32,

    pub gap: f32,
    pub radius: f32,

    pub min_cell_height: f32,

    pub text_color: Rgba,
    pub title_size: f32,

    pub loader_color: Rgba,
    pub loader_height: f32,

    pub added_color: Rgba,
    pub removed_color: Rgba,

    pub input_height: f32,
    pub input_fill: Rgba,

    pub input_max_height: f32,

    pub accent: Rgba,
    pub on_accent: Rgba,

    pub stop_color: Rgba,

    // ── the typographic transcript (docs/Design.md: "size carries
    // rank"). The transcript is a two-column grid — the VOICE gutter
    // (`window.chat_text_edge` wide) and the text column.
    /// The gutter's air between its right-aligned label and the text
    /// column.
    pub gutter_pad: f32,
    /// The gutter label's line box — "YOU" sits centered in it, the
    /// row's top.
    pub gutter_line: f32,
    /// The text column's right inset.
    pub text_trailing: f32,
    /// The air between two transcript rows.
    pub row_gap: f32,

    /// A tool call's own row.
    pub tool_row_height: f32,
    /// The tool's name ("Bash"), bold in ink 2.
    pub tool_name: TypeRole,
    /// The invocation after the name, in the mono face.
    pub tool_detail_size: f32,
    /// The air between the name and the invocation.
    pub tool_gap: f32,
    /// The disclosure chevron: its glyph box, how far it hangs into
    /// the gutter, its stroke and ink.
    pub chevron_size: f32,
    pub chevron_hang: f32,
    pub chevron_stroke: f32,
    pub chevron_color: Rgba,
    /// A FAILED call's error line under its row: mono, its own line
    /// box, this far below the row.
    pub tool_error_size: f32,
    pub tool_error_line: f32,
    pub tool_error_gap: f32,

    /// "Turn cancelled." — the turn's quiet end.
    pub notice_size: f32,
    /// "8 in · 2,757 out" — what the turn spent, in its line box.
    pub usage_size: f32,
    pub usage_line: f32,

    /// The composer FOOTER: the air above the input (under the
    /// hairline) and below the options row.
    pub footer_top: f32,
    pub footer_bottom: f32,
    /// The input's lines at rest — the box stands this many prompt
    /// lines tall before it grows.
    pub input_lines: f32,
    /// The air between the text and the rule under it.
    pub input_rule_gap: f32,
    pub placeholder_color: Rgba,
    /// The options row under the input (model · effort · mode): the
    /// air above it, its line box and the gap between its words.
    pub options_top: f32,
    pub options_line: f32,
    pub options_gap: f32,
    /// SEND: the word, and the key hint a gap after it.
    pub send: TypeRole,
    pub send_key_size: f32,
    pub send_key_gap: f32,
}

impl Default for ChatChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            pad: 56.0,
            gap: 36.0,
            radius: 0.0,
            min_cell_height: 48.0,
            text_color: Rgba(Color::new(0xffd9d6d0)),
            title_size: 30.0,
            loader_color: Rgba(Color::new(0xff8a8781)),
            loader_height: 48.0,
            added_color: Rgba(Color::new(0xff7fd08a)),
            removed_color: Rgba(Color::new(0xfff07a6b)),
            input_height: 84.0,
            input_fill: Rgba(Color::new(0x00000000)),
            input_max_height: 360.0,
            accent: Rgba(Color::new(0xfff2b544)),
            on_accent: Rgba(Color::new(0xff0d0d0f)),
            stop_color: Rgba(Color::new(0xfff07a6b)),
            gutter_pad: 32.0,
            gutter_line: 48.0,
            text_trailing: 56.0,
            row_gap: 36.0,
            tool_row_height: 48.0,
            tool_name: TypeRole { size: 26.0, weight: 600.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            tool_detail_size: 26.0,
            tool_gap: 16.0,
            chevron_size: 24.0,
            chevron_hang: 44.0,
            chevron_stroke: 2.5,
            chevron_color: Rgba(Color::new(0xff6e6b66)),
            tool_error_size: 22.0,
            tool_error_line: 36.0,
            tool_error_gap: 4.0,
            notice_size: 30.0,
            usage_size: 26.0,
            usage_line: 48.0,
            footer_top: 36.0,
            footer_bottom: 28.0,
            input_lines: 3.0,
            input_rule_gap: 12.0,
            placeholder_color: Rgba(Color::new(0xff6e6b66)),
            options_top: 20.0,
            options_line: 32.0,
            options_gap: 16.0,
            send: TypeRole { size: 30.0, weight: 680.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            send_key_size: 22.0,
            send_key_gap: 8.0,
        }
    }
}

impl ComboChrome {
    pub const KEY: Key<ComboChrome> = Key::new("combo");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ComboChrome {
    pub pad: f32,

    pub gap: f32,
    pub label_size: f32,
    pub label_color: Rgba,
    pub value_size: f32,
    pub value_color: Rgba,

    pub chevron: f32,
    pub chevron_color: Rgba,
    /// The capsule behind a toolbar pill (the composer's combos).
    pub pill_fill: Rgba,
    pub menu_fill: Rgba,
    pub menu_border: Rgba,
    pub menu_row_height: f32,
    pub menu_row_size: f32,
    pub menu_text: Rgba,

    pub menu_trail: Rgba,

    pub menu_pad: f32,
    pub menu_min_width: f32,
}

impl Default for ComboChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            pad: 0.0,
            gap: 8.0,
            label_size: 22.0,
            label_color: Rgba(Color::new(0xff8a8781)),
            value_size: 26.0,
            value_color: Rgba(Color::new(0xffb5b2ab)),
            chevron: 0.0,
            chevron_color: Rgba(Color::new(0xff8a8781)),
            pill_fill: Rgba(Color::new(0x00000000)),
            menu_fill: Rgba(Color::new(0xff151518)),
            menu_border: Rgba(Color::new(0xff2a2a2f)),
            menu_row_height: 68.0,
            menu_row_size: 30.0,
            menu_text: Rgba(Color::new(0xffeceae4)),
            menu_trail: Rgba(Color::new(0xff8a8781)),
            menu_pad: 24.0,
            menu_min_width: 320.0,
        }
    }
}

impl TreeChrome {
    pub const KEY: Key<TreeChrome> = Key::new("tree");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TreeChrome {
    pub font_size: f32,

    pub indent: f32,

    pub text_x: f32,

    pub highlight: Rgba,

    /// Directory labels — LIGHT, the declaration-name register, not
    /// the darker keyword blue.
    pub directory: Rgba,
    /// File labels.
    pub file: Rgba,
}

impl Default for TreeChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            font_size: 30.0,
            indent: 36.0,
            text_x: 36.0,
            highlight: Rgba(Color::new(0xff1a1a1f)),
            directory: Rgba(Color::new(0xffd9d6d0)),
            file: Rgba(Color::new(0xffb5b2ab)),
        }
    }
}

impl DiffChrome {
    pub const KEY: Key<DiffChrome> = Key::new("diff");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DiffChrome {
    pub spacer_fill: Rgba,

    pub fold_height: f32,
    pub fold_background: Rgba,

    pub fold_rule: Rgba,

    pub fold_text: Rgba,
    pub fold_text_size: f32,

    pub fold_button: Rgba,
    pub fold_button_size: f32,

    /// The canvas PLAQUES (docs/himark-islands-canvas-many-files.png):
    /// each file's header + diff rides its own rounded card over the
    /// canvas page. The page fill behind and between the cards, the
    /// body fill, the header band, the inset from the pane edges, and
    /// the corner radius. Dark: cards lighter than the page; light:
    /// white cards on the gray page.
    pub plaque_page: Rgba,
    pub plaque: Rgba,
    pub plaque_header: Rgba,
    pub plaque_margin: f32,
    pub plaque_radius: f32,

    /// The COMMIT BLOCK heading a commit canvas (the typographic
    /// redesign, Main.dc.html): the byline, then the message in the
    /// HEADLINE rung, on the ground. Its left inset is the code text
    /// edge (`window.code_text_edge`); these are the other three, the
    /// gap between the byline's words, the air above the headline
    /// (and above a message body), and the headline's measure.
    pub commit_pad_top: f32,
    pub commit_pad_right: f32,
    pub commit_pad_bottom: f32,
    pub byline_gap: f32,
    pub headline_gap: f32,
    pub headline_max_width: f32,

    /// The FILE BAND: one hairline-topped row of words per file over
    /// the ground — no card. Its insets (left is the code text edge),
    /// the AIR under every file's diff before the next band's hairline
    /// (the files group by whitespace, Multi-breaks.dc.html — it rides
    /// the DIFF row, never the band: the band is sticky), the gap
    /// between its words, the tighter step from the directory into
    /// the name, the name's own rung, and the ordinal's size.
    pub band_pad_top: f32,
    pub band_pad_right: f32,
    pub band_pad_bottom: f32,
    pub band_air: f32,
    pub band_gap: f32,
    pub band_name_gap: f32,
    pub file_name: TypeRole,
    pub ordinal_size: f32,

    /// The canvas SUMMARY line (`+A −R · N files`, the toggle-all
    /// press): its inset from the pane's top-right and the gap
    /// between its words.
    pub summary_pad: f32,
    pub summary_gap: f32,

    /// The FOLD BAND's two EDGE rows (fold_height is their sum): each
    /// edge's height, the chevron glyph's side, its hit box and
    /// stroke, the gutter's right inset, the gap between an edge's
    /// line number and its chevron, the gap before "Show all", and
    /// the chevron's hover wash.
    pub fold_edge_height: f32,
    pub fold_chevron_size: f32,
    pub fold_chevron_hit: f32,
    pub fold_chevron_stroke: f32,
    pub fold_gutter_inset: f32,
    pub fold_number_gap: f32,
    pub fold_summary_gap: f32,
    pub fold_hover: Rgba,
}

impl Default for DiffChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            spacer_fill: Rgba(Color::new(0x10000000)),
            fold_height: 88.0,
            fold_background: Rgba(Color::new(0xff111115)),
            fold_rule: Rgba(Color::new(0xff222226)),
            fold_text: Rgba(Color::new(0xff8a8781)),
            fold_text_size: 26.0,
            fold_button: Rgba(Color::new(0xffb5b2ab)),
            fold_button_size: 44.0,
            plaque_page: Rgba(Color::new(0xff0d0d0f)),
            plaque: Rgba(Color::new(0xff0d0d0f)),
            plaque_header: Rgba(Color::new(0xff0d0d0f)),
            plaque_margin: 0.0,
            plaque_radius: 0.0,
            commit_pad_top: 52.0,
            commit_pad_right: 64.0,
            commit_pad_bottom: 44.0,
            byline_gap: 16.0,
            headline_gap: 16.0,
            headline_max_width: 1152.0,
            band_pad_top: 24.0,
            band_pad_right: 48.0,
            band_pad_bottom: 20.0,
            band_air: 96.0,
            band_gap: 20.0,
            band_name_gap: 8.0,
            file_name: TypeRole { size: 44.0, weight: 680.0, width: 100.0, tracking: -0.44, line_height: 1.0 },
            ordinal_size: 26.0,
            summary_pad: 20.0,
            summary_gap: 16.0,
            fold_edge_height: 44.0,
            fold_chevron_size: 28.0,
            fold_chevron_hit: 44.0,
            fold_chevron_stroke: 2.5,
            fold_gutter_inset: 24.0,
            fold_number_gap: 8.0,
            fold_summary_gap: 36.0,
            fold_hover: Rgba(Color::new(0xff1e1e23)),
        }
    }
}

impl PeekerChrome {
    pub const KEY: Key<PeekerChrome> = Key::new("peeker");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PeekerChrome {
    pub background: Rgba,
    pub well: Rgba,
    pub well_radius: f32,
    pub text: Rgba,
    pub dim_text: Rgba,
    pub rule: Rgba,

    pub rule_inset: f32,
    pub highlight: Rgba,
    pub highlight_radius: f32,
    pub accent: Rgba,
    pub accent_width: f32,
    pub accent_inset: f32,
    pub dir_text: Rgba,
    pub row_height: f32,
    pub input_height: f32,
    pub margin: f32,
    pub title_baseline: f32,
    pub hint_bottom: f32,
    pub input_inset_x: f32,
    pub input_inset_y: f32,

    pub input_pad: f32,

    pub input_shrink: f32,
    pub title_size: f32,
    pub row_size: f32,
    pub hint_size: f32,
    pub no_preview_offset: f32,

    pub list_ratio: f32,
    pub list_min: f32,
    pub list_max: f32,

    pub list_preview_gap: f32,

    pub preview_margin: f32,

    pub list_top_gap: f32,

    pub row_text_x: f32,
    pub input_min_width: f32,

    /// The command palette's sheet (docs/Design.md: the one sheet the
    /// design keeps — a query, a count, ranked rows, a key footer).
    #[serde(default)]
    pub palette: PaletteSheetChrome,

    /// The jump-to-file sheet: the matched files at the left, the
    /// selected file's preview at the right.
    #[serde(default)]
    pub jump: JumpSheetChrome,
}

impl Default for PeekerChrome {
    fn default() -> Self {
        Self {
            background: Rgba(Color::new(0xff0d0d0f)),
            well: Rgba(Color::new(0x00000000)),
            well_radius: 0.0,
            text: Rgba(Color::new(0xffeceae4)),
            dim_text: Rgba(Color::new(0xff8a8781)),
            rule: Rgba(Color::new(0xff222226)),
            rule_inset: 0.0,
            highlight: Rgba(Color::new(0xff1a1a1f)),
            highlight_radius: 0.0,
            accent: Rgba(Color::new(0xfff2b544)),
            accent_width: 0.0,
            accent_inset: 0.0,
            dir_text: Rgba(Color::new(0xff8a8781)),
            row_height: 68.0,
            input_height: 168.0,
            margin: 56.0,
            title_baseline: 66.0,
            hint_bottom: 24.0,
            input_inset_x: 0.0,
            input_inset_y: 8.0,
            input_pad: 0.0,
            input_shrink: 0.0,
            title_size: 88.0,
            row_size: 30.0,
            hint_size: 26.0,
            no_preview_offset: 48.0,
            list_ratio: 0.344,
            list_min: 880.0,
            list_max: 880.0,
            list_preview_gap: 0.0,
            preview_margin: 0.0,
            list_top_gap: 16.0,
            row_text_x: 56.0,
            input_min_width: 240.0,
            palette: PaletteSheetChrome::default(),
            jump: JumpSheetChrome::default(),
        }
    }
}

/// A centered modal SHEET's surface and placement: the only boxed
/// surface the typographic design allows — a flat fill, a hairline
/// border, a soft radius and a soft drop shadow; the page behind
/// dims nothing.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SheetChrome {
    pub fill: Rgba,
    pub border: Rgba,
    pub radius: f32,
    /// The drop shadow's color, its downward offset and its blur
    /// (the CSS blur radius; a zero blur draws no shadow).
    pub shadow: Rgba,
    pub shadow_dy: f32,
    pub shadow_blur: f32,
    /// The sheet's widest width; narrower windows keep `gutter` on
    /// each side.
    pub width: f32,
    pub gutter: f32,
    /// The sheet's top edge from the window's top.
    pub top: f32,
    /// The room kept under a sheet that fills the window's height
    /// (or caps one that grows with its content).
    pub bottom: f32,
}

impl Default for SheetChrome {
    fn default() -> Self {
        Self {
            fill: Rgba(Color::new(0xff151518)),
            border: Rgba(Color::new(0xff2a2a2f)),
            radius: 20.0,
            shadow: Rgba(Color::new(0x8c000000)),
            shadow_dy: 60.0,
            shadow_blur: 160.0,
            width: 1520.0,
            gutter: 32.0,
            top: 144.0,
            bottom: 144.0,
        }
    }
}

/// The command palette (Palette.dc.html, CSS px doubled).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PaletteSheetChrome {
    pub sheet: SheetChrome,
    /// The hairlines between the query band, the rows and the footer.
    pub rule: Rgba,
    /// The query band's insets: top, sides, bottom.
    pub pad_top: f32,
    pub pad_x: f32,
    pub pad_bottom: f32,
    /// The caps label → query, and query → count line gaps.
    pub label_gap: f32,
    pub count_gap: f32,
    /// The rows block's own air above and below.
    pub list_pad_top: f32,
    pub list_pad_bottom: f32,
    /// The rows' cap before the list scrolls.
    pub list_max_height: f32,
    /// Every row's height (the selected row only grows its words).
    pub row_height: f32,
    /// The gap between a command's name and its shortcut.
    pub shortcut_gap: f32,
    /// A row's name, and the SELECTED row's larger, heavier name.
    pub row: TypeRole,
    pub selected_row: TypeRole,
    /// The selected row's flat wash.
    pub selected_fill: Rgba,
    /// The shortcut's mono size on a row and on the selected row.
    pub shortcut_size: f32,
    pub selected_shortcut_size: f32,
    /// The typed letters: their weight, and the LIVE underline's
    /// thickness and drop below the baseline.
    pub mark_weight: f32,
    pub mark_thickness: f32,
    pub mark_offset: f32,
    /// The footer's vertical insets, its gap between key hints and
    /// its text size (the keys ride the mono face at the same size).
    pub footer_pad_y: f32,
    pub footer_gap: f32,
    pub footer_size: f32,
}

impl Default for PaletteSheetChrome {
    fn default() -> Self {
        Self {
            sheet: SheetChrome::default(),
            rule: Rgba(Color::new(0xff26262a)),
            pad_top: 52.0,
            pad_x: 64.0,
            pad_bottom: 36.0,
            label_gap: 12.0,
            count_gap: 12.0,
            list_pad_top: 20.0,
            list_pad_bottom: 12.0,
            list_max_height: 880.0,
            row_height: 88.0,
            shortcut_gap: 32.0,
            row: TypeRole {
                size: 34.0,
                weight: 500.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.41,
            },
            selected_row: TypeRole {
                size: 44.0,
                weight: 600.0,
                width: 100.0,
                tracking: -0.66,
                line_height: 1.27,
            },
            selected_fill: Rgba(Color::new(0xff1f1f24)),
            shortcut_size: 22.0,
            selected_shortcut_size: 22.0,
            mark_weight: 700.0,
            mark_thickness: 4.0,
            mark_offset: 8.0,
            footer_pad_y: 24.0,
            footer_gap: 44.0,
            footer_size: 26.0,
        }
    }
}

/// The jump-to-file sheet (Jump.dc.html, CSS px doubled).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct JumpSheetChrome {
    pub sheet: SheetChrome,
    /// The matches column's width; the preview takes the rest.
    pub list_width: f32,
    /// The query band's insets: top, sides, bottom.
    pub pad_top: f32,
    pub pad_x: f32,
    pub pad_bottom: f32,
    /// The caps label → query, and query → count line gaps.
    pub label_gap: f32,
    pub count_gap: f32,
    /// The rows block's air above and below.
    pub list_pad_y: f32,
    /// A row's height, and the SELECTED row's taller one.
    pub row_height: f32,
    pub selected_row_height: f32,
    /// The least gap between a file's name and its directory.
    pub dir_gap: f32,
    /// A row's name, and the selected row's larger, heavier name.
    pub name: TypeRole,
    pub name_color: Rgba,
    pub selected_name: TypeRole,
    /// The selected row's flat wash.
    pub selected_fill: Rgba,
    /// The typed letters: weight, LIVE underline thickness and drop.
    pub mark_weight: f32,
    pub mark_thickness: f32,
    pub mark_offset: f32,
    /// The footer's vertical insets, key-hint gap and text size.
    pub footer_pad_y: f32,
    pub footer_gap: f32,
    pub footer_size: f32,
    /// The preview's header: top, right, bottom and left insets (the
    /// left one lines the name up with the code past the gutter).
    pub preview_pad_top: f32,
    pub preview_pad_right: f32,
    pub preview_pad_bottom: f32,
    pub preview_pad_left: f32,
    /// The previewed file's name, the gap between the header's
    /// words, the full path's mono size and the line-count size.
    pub preview_title: TypeRole,
    pub preview_gap: f32,
    pub path_size: f32,
    pub meta_size: f32,
    /// The air between the header's hairline and the first line.
    pub preview_top_gap: f32,
}

impl Default for JumpSheetChrome {
    fn default() -> Self {
        Self {
            sheet: SheetChrome {
                fill: Rgba(Color::new(0xff121215)),
                border: Rgba(Color::new(0xff26262b)),
                radius: 20.0,
                shadow: Rgba(Color::new(0x99000000)),
                shadow_dy: 60.0,
                shadow_blur: 160.0,
                width: 2560.0,
                gutter: 48.0,
                top: 80.0,
                bottom: 80.0,
            },
            list_width: 880.0,
            pad_top: 48.0,
            pad_x: 56.0,
            pad_bottom: 28.0,
            label_gap: 8.0,
            count_gap: 8.0,
            list_pad_y: 16.0,
            row_height: 68.0,
            selected_row_height: 88.0,
            dir_gap: 32.0,
            name: TypeRole {
                size: 30.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            name_color: Rgba(Color::new(0xffd9d6d0)),
            selected_name: TypeRole {
                size: 30.0,
                weight: 640.0,
                width: 100.0,
                tracking: -0.3,
                line_height: 1.0,
            },
            selected_fill: Rgba(Color::new(0xff1c1c21)),
            mark_weight: 700.0,
            mark_thickness: 4.0,
            mark_offset: 8.0,
            footer_pad_y: 24.0,
            footer_gap: 36.0,
            footer_size: 26.0,
            preview_pad_top: 48.0,
            preview_pad_right: 64.0,
            preview_pad_bottom: 28.0,
            preview_pad_left: 160.0,
            preview_title: TypeRole {
                size: 44.0,
                weight: 680.0,
                width: 100.0,
                tracking: -0.44,
                line_height: 1.0,
            },
            preview_gap: 24.0,
            path_size: 26.0,
            meta_size: 26.0,
            preview_top_gap: 16.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ComboChrome;

    /// The redesign draws a combo as a bare WORD: same geometry in
    /// both themes, inks from the light theme's own ramp.
    #[test]
    fn the_light_combo_matches_dark_geometry_and_wears_light_ink() {
        let light = ::editor::theme::light();
        let dark = ::editor::theme::embedded();
        let light_combo = light.part(ComboChrome::KEY);
        let dark_combo = dark.part(ComboChrome::KEY);
        assert_eq!(light_combo.pad, dark_combo.pad, "combo geometry matches");
        assert_eq!(
            light_combo.menu_row_height, dark_combo.menu_row_height,
            "menu geometry matches"
        );
        let ink = light.part(::editor::theme::InkChrome::KEY);
        assert_eq!(
            light_combo.value_color.0,
            ink.ink_2.0,
            "closed values use the secondary ink"
        );
        assert_eq!(
            light_combo.menu_fill.0,
            ink.sheet.0,
            "expanded menus use the sheet surface"
        );
        assert_eq!(
            light_combo.menu_border.0,
            ink.sheet_border.0,
            "expanded menus have the sheet's edge"
        );
        assert_eq!(
            light_combo.menu_text.0,
            ink.ink.0,
            "expanded menu labels use primary ink"
        );
    }
}
