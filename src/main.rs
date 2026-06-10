mod app;
mod config;
mod i18n;
mod xmldoc;
mod document;
mod textdoc;
mod row_object;

fn main() -> gtk::glib::ExitCode {
    app::run()
}
