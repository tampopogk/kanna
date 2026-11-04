use crate::complete;
use crate::args::Argv;
use kanna_client::Failure;

pub const HELP: &str = "usage: kanna completions zsh|bash\n\nPrints the tab-completion script for your shell. Load it from your shell's startup file:\n  zsh:  source <(kanna completions zsh)     in ~/.zshrc\n  bash: eval \"$(kanna completions bash)\"  in ~/.bashrc\n\nTab then completes commands, options, pane names and IDs, tabs, windows, instances,\nand files of the kind a command takes (markdown for open md, HTML for markup).";

pub fn run(args: &mut Argv) -> Result<(), Failure> {
    match args.positional().as_deref() {
        Some("zsh") => print!("{}", complete::ZSH),
        Some("bash") => print!("{}", complete::BASH),
        _ => println!("{HELP}"),
    }
    Ok(())
}
