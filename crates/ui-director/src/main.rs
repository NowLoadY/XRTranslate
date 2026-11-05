fn main() {
    std::process::exit(ui_director::cli::run(std::env::args().skip(1).collect()));
}
