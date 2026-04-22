mod app;
mod config;
mod render;
mod style;
mod tools;

use std::io::Read;

use clap::Parser;

use app::App;
use config::Config;

#[derive(Parser, Debug)]
#[command(name = "slappyshot", about = "Screenshot annotation tool")]
struct Args {
    /// Input image file ("-" for stdin)
    #[arg(long, short)]
    filename: Option<String>,

    /// Output filename template
    #[arg(long)]
    output_filename: Option<String>,

    /// Start fullscreen
    #[arg(long)]
    fullscreen: bool,

    /// Custom copy command
    #[arg(long)]
    copy_command: Option<String>,

    /// Config file path
    #[arg(long)]
    config: Option<String>,

    /// Initial tool (arrow, rect, etc.)
    #[arg(long)]
    initial_tool: Option<String>,

    /// Annotation size factor
    #[arg(long)]
    annotation_size_factor: Option<f32>,

    /// Exit after saving
    #[arg(long)]
    early_exit: bool,

    /// Auto-copy after annotation
    #[arg(long)]
    auto_copy: bool,
}

fn load_image(filename: Option<&str>) -> anyhow::Result<image::RgbaImage> {
    match filename {
        None | Some("-") => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            Ok(image::load_from_memory(&buf)?.to_rgba8())
        }
        Some(path) => Ok(image::open(path)?.to_rgba8()),
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut config = Config::load(args.config.as_deref());

    // CLI overrides
    if let Some(v) = args.output_filename {
        config.general.output_filename = Some(v);
    }
    if let Some(v) = args.copy_command {
        config.general.copy_command = Some(v);
    }
    if let Some(v) = args.annotation_size_factor {
        config.general.annotation_size_factor = v;
    }
    if let Some(v) = args.initial_tool {
        config.general.initial_tool = v;
    }
    if args.fullscreen {
        config.general.fullscreen = true;
    }
    if args.early_exit {
        config.general.early_exit = true;
    }
    if args.auto_copy {
        config.general.auto_copy = true;
    }

    let image = match load_image(args.filename.as_deref()) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("Failed to load image: {}", e);
            std::process::exit(1);
        }
    };

    let img_w = image.width() as f32;
    let img_h = image.height() as f32;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("slappyshot")
            .with_app_id("slappyshot")
            .with_inner_size([img_w.max(400.0), img_h.max(300.0) + 80.0])
            .with_min_inner_size([400.0, 300.0])
            .with_fullscreen(config.general.fullscreen)
            .with_decorations(!config.general.no_window_decoration),
        ..Default::default()
    };

    let app = App::new(image, config);

    eframe::run_native(
        "slappyshot",
        native_options,
        Box::new(|_cc| Ok(Box::new(app) as Box<dyn eframe::App>)),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(())
}
