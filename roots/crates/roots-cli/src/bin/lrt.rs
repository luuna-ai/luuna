fn main() {
    std::process::exit(roots_cli::run(std::env::args().skip(1).collect()));
}
