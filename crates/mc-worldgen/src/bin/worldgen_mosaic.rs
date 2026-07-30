use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use mc_world::BlockRegistry;
use mc_worldgen::{
    MosaicConfig, TellusWorldgenSettings, TerrainGenerator, WorldgenMode, render_mosaic,
    write_mosaic,
};

const USAGE: &str = "usage: worldgen_mosaic --seed <i64> --center-x <i32> --center-z <i32> \\
    --extent <u32> --blocks-per-pixel <u32> --output-dir <path>";

#[derive(Debug)]
struct Args {
    seed: i64,
    center_x: i32,
    center_z: i32,
    extent: u32,
    blocks_per_pixel: u32,
    output_dir: PathBuf,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("worldgen_mosaic: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(env::args().skip(1))?;
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .map_err(|error| format!("failed to build production block registry: {error}"))?,
    );
    let generator = TerrainGenerator::with_worldgen_mode(
        args.seed,
        registry,
        WorldgenMode::TellusLike(TellusWorldgenSettings::default()),
    );
    let images = render_mosaic(
        &generator,
        &MosaicConfig {
            center_x: args.center_x,
            center_z: args.center_z,
            extent_blocks: args.extent,
            blocks_per_pixel: args.blocks_per_pixel,
        },
    )
    .map_err(|error| error.to_string())?;
    write_mosaic(&images, &args.output_dir).map_err(|error| error.to_string())?;
    println!(
        "wrote {}x{} Tellus mosaic covering {}x{} blocks to {}",
        images.width,
        images.height,
        args.extent,
        args.extent,
        args.output_dir.display()
    );
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut seed = None;
    let mut center_x = None;
    let mut center_z = None;
    let mut extent = None;
    let mut blocks_per_pixel = None;
    let mut output_dir = None;
    let mut arguments = arguments.into_iter();

    while let Some(flag) = arguments.next() {
        if flag == "--help" || flag == "-h" {
            return Err("help requested".to_owned());
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--seed" => seed = Some(parse_value(&flag, &value)?),
            "--center-x" => center_x = Some(parse_value(&flag, &value)?),
            "--center-z" => center_z = Some(parse_value(&flag, &value)?),
            "--extent" => extent = Some(parse_value(&flag, &value)?),
            "--blocks-per-pixel" => blocks_per_pixel = Some(parse_value(&flag, &value)?),
            "--output-dir" => output_dir = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }

    Ok(Args {
        seed: seed.ok_or("missing required --seed")?,
        center_x: center_x.ok_or("missing required --center-x")?,
        center_z: center_z.ok_or("missing required --center-z")?,
        extent: extent.ok_or("missing required --extent")?,
        blocks_per_pixel: blocks_per_pixel.ok_or("missing required --blocks-per-pixel")?,
        output_dir: output_dir.ok_or("missing required --output-dir")?,
    })
}

fn parse_value<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("invalid value for {flag}: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_arguments_are_explicit_and_required() {
        let error = parse_args(["--seed".to_owned(), "712816".to_owned()]).unwrap_err();
        assert_eq!(error, "missing required --center-x");
    }
}
