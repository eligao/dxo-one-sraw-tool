use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use dxo_sraw_core::SuperRaw;

/// Split DxO ONE SuperRAW (.DXO) files into one DNG per frame.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write <name>_1.dng .. <name>_N.dng
    Split {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Output directory (default: next to the input)
        #[arg(short, long)]
        out_dir: Option<PathBuf>,
        /// Overwrite existing outputs
        #[arg(short, long)]
        force: bool,
    },
    /// Show the frame layout of SuperRAW files
    Info {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

fn info(src: &Path) -> anyhow::Result<()> {
    let buf = std::fs::read(src)?;
    let sr = SuperRaw::parse(&buf)?;
    println!(
        "{}: SuperRAW, {} frames, {}x{}, {}-bit",
        src.display(),
        sr.frames.len(),
        sr.width(),
        sr.height(),
        sr.bits_per_sample()
    );
    for f in &sr.frames {
        println!(
            "  frame {}: IFD @ {}, {} bytes @ {}",
            f.index,
            f.ifd.offset,
            f.nbytes(),
            f.data[0].0
        );
    }
    Ok(())
}

fn split(src: &Path, out_dir: Option<&Path>, force: bool) -> anyhow::Result<Vec<PathBuf>> {
    let buf = std::fs::read(src)?;
    let sr = SuperRaw::parse(&buf)?;
    let out_dir = out_dir.unwrap_or_else(|| src.parent().unwrap_or(Path::new(".")));
    let stem = src
        .file_stem()
        .context("input has no file name")?
        .to_string_lossy();
    let targets: Vec<PathBuf> = sr
        .frames
        .iter()
        .map(|f| out_dir.join(format!("{stem}_{}.dng", f.index)))
        .collect();
    if !force && let Some(p) = targets.iter().find(|p| p.exists()) {
        bail!("{} exists (use --force to overwrite)", p.display());
    }
    std::fs::create_dir_all(out_dir)?;
    for (frame, target) in sr.frames.iter().zip(&targets) {
        std::fs::write(target, sr.frame_dng(frame)?)
            .with_context(|| format!("writing {}", target.display()))?;
    }
    Ok(targets)
}

fn main() -> ExitCode {
    let mut ok = true;
    let mut report = |src: &Path, r: anyhow::Result<()>| {
        if let Err(e) = r {
            eprintln!("{}: {e:#}", src.display());
            ok = false;
        }
    };
    match Cli::parse().cmd {
        Cmd::Info { files } => files.iter().for_each(|f| report(f, info(f))),
        Cmd::Split {
            files,
            out_dir,
            force,
        } => {
            for f in &files {
                let r = split(f, out_dir.as_deref(), force).map(|outs| {
                    let outs: Vec<_> = outs.iter().map(|p| p.display().to_string()).collect();
                    println!("{} -> {}", f.display(), outs.join(", "));
                });
                report(f, r);
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
