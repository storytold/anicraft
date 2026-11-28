//! `anicraft-cli`: AniCraft without a window. See the library crate for the commands.

fn main() {
    // `args_os`, not `args`: a non-Unicode argument must reach clap, not panic before anything is printed.
    let code = anicraft_cli::run(std::env::args_os().skip(1), &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code);
}
