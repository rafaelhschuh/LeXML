mod app;
mod xmldoc;
mod document;
mod row_object;

fn main() -> gtk::glib::ExitCode {
    app::run()
}
