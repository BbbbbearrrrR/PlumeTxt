#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod document;
mod external;
mod imageview;
mod large;
mod markdown;
mod outline;
mod palette;
mod pdf;
mod reader;
mod scroll;
mod selection;
mod syntax;
mod terminal;
mod theme;
mod ui;

fn main() {
    ui::run();
}
