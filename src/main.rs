#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod document;
mod external;
mod imageview;
mod large;
mod markdown;
mod outline;
mod paged;
mod palette;
mod pdf;
mod pdftext;
mod printing;
mod reader;
mod render;
mod scroll;
mod search;
mod selection;
mod syntax;
mod terminal;
mod theme;
mod ui;
mod workspace;

fn main() {
    ui::run();
}
