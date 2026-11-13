//! Generates the manpage from the command line definition.
//!
//! src/cli.rs is `include!`d rather than imported, because a build script is
//! its own crate and cannot reach into the binary it is building. That is the
//! whole reason the CLI lives in a file of its own -- see the note at the top
//! of it.
//!
//! The page goes to OUT_DIR, which is where cargo says generated files belong
//! and the only place guaranteed writable (a source tree need not be, and is
//! not under nix). Packagers therefore have to go and find it:
//!
//!   make install           does this, via the `man` target
//!   flake.nix postInstall  does this, with a find over target/
//!
//! It is a single page named after the binary, because that is the one
//! artefact worth shipping. This build script once produced six shell
//! completion files as well, for a binary named satty, by way of a crate that
//! no longer exists here -- it had been reduced to `fn main() {}` since April
//! and nothing noticed, which is a fair measure of how much the completions
//! were wanted.

use std::fs;
use std::io;
use std::path::PathBuf;

use clap::CommandFactory;
use clap_mangen::Man;

include!("src/cli.rs");

fn main() -> io::Result<()> {
    // Only the CLI definition affects the page; without this cargo re-runs the
    // script on every change to anything.
    println!("cargo:rerun-if-changed=src/cli.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or(io::Error::from(io::ErrorKind::NotFound))?);

    let mut buffer: Vec<u8> = Vec::new();
    Man::new(Args::command()).render(&mut buffer)?;
    fs::write(out_dir.join("slappyshot.1"), buffer)?;

    Ok(())
}
