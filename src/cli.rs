// The command line, in a module of its own so that build.rs can see it.
//
// This lives apart from main.rs for one reason: build.rs generates the
// manpage from these very definitions, and a build script cannot import
// anything from the binary crate it is building. It `include!`s this file
// instead, which is why everything here is `pub` and why the file depends on
// clap and nothing else -- an `use` of any other crate would have to be a
// build-dependency too.
//
// The payoff is that the manpage cannot drift from the flags: add an option
// below and it documents itself on the next build.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "slappyshot",
    about = "Screenshot annotation tool",
    version,
    long_about = "Annotate a screenshot: crop it, draw on it, blur what should \
                  not be shared, then save it or put it on the clipboard.\n\n\
                  Reads an image from a file or, given \"-\" or no filename at \
                  all, from standard input -- which is how it is usually \
                  reached, with a region grabber piping into it:\n\n    \
                  grim -g \"$(slurp)\" - | slappyshot -f -"
)]
pub struct Args {
    /// Input image file ("-" for stdin)
    #[arg(long, short)]
    pub filename: Option<String>,

    /// Output filename template
    #[arg(long)]
    pub output_filename: Option<String>,

    /// Start fullscreen
    #[arg(long)]
    pub fullscreen: bool,

    /// Custom copy command
    #[arg(long)]
    pub copy_command: Option<String>,

    /// Config file path
    #[arg(long)]
    pub config: Option<String>,

    /// Initial tool (arrow, rect, etc.)
    #[arg(long)]
    pub initial_tool: Option<String>,

    /// Annotation size factor
    #[arg(long)]
    pub annotation_size_factor: Option<f32>,

    /// Exit after saving
    #[arg(long)]
    pub early_exit: bool,

    /// Auto-copy after annotation
    #[arg(long)]
    pub auto_copy: bool,
}
