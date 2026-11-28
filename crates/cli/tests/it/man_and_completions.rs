//! The clap definition is consistent, and every shell's completions and every man page
//! (`examples/man-and-completions.rs`) can be generated from it.

use clap::ValueEnum;
use clap_complete::Shell;

// The example itself, so the test writes the files exactly as the packages get them.
#[allow(dead_code)]
#[path = "../../examples/man-and-completions.rs"]
mod example;

#[test]
fn command_is_consistent() {
    anicraft_cli::command().debug_assert();
}

#[test]
fn completions_generate_for_every_shell() {
    for &shell in Shell::value_variants() {
        let mut out = Vec::new();
        clap_complete::generate(shell, &mut anicraft_cli::command(), "anicraft-cli", &mut out);
        assert!(!out.is_empty(), "{shell}");
    }
}

/// The example writes the five completion files under the names `packaging/linux/package.sh`
/// installs, `anicraft-cli.1` and a page per subcommand, and no pages for the `help` subcommand.
#[test]
fn the_example_writes_the_files_the_packages_install() {
    let dir = std::env::temp_dir().join(format!("anicraft-cli-{}-man-and-completions", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    example::generate(&dir).unwrap();

    let mut written: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    written.sort();
    let completions = ["anicraft-cli.bash", "_anicraft-cli", "anicraft-cli.fish", "anicraft-cli.elv", "_anicraft-cli.ps1"];
    let mut expected: Vec<String> = completions.map(String::from).into();
    expected.push("anicraft-cli.1".into());
    let cmd = anicraft_cli::command();
    expected.extend(cmd.get_subcommands().filter(|s| s.get_name() != "help").flat_map(|s| {
        // `guide search` is a page of its own.
        let mut pages = vec![format!("anicraft-cli-{}.1", s.get_name())];
        pages.extend(s.get_subcommands().filter(|n| n.get_name() != "help").map(|n| format!("anicraft-cli-{}-{}.1", s.get_name(), n.get_name())));
        pages
    }));
    expected.sort();
    assert_eq!(written, expected);
    assert!(written.iter().any(|w| w == "anicraft-cli-render-seq.1") && written.iter().any(|w| w == "anicraft-cli-guide-search.1"), "{written:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
