mod app;
mod backup;
mod config;
mod dialog;
mod diff;
mod document;
mod history;
mod i18n;
mod recent;
mod recovery;
mod replace;
mod row_object;
mod rowclip;
mod shortcuts;
mod sort;
mod textdoc;
mod xmldoc;

fn main() -> gtk::glib::ExitCode {
    app::run()
}
