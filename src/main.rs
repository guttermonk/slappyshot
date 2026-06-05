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

fn load_icon() -> Option<std::sync::Arc<egui::IconData>> {
    let bytes = include_bytes!("../assets/slappyshot.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(std::sync::Arc::new(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }))
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

    let icon = load_icon();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("slappyshot")
        .with_app_id("slappyshot")
        .with_inner_size([img_w.max(400.0), img_h.max(300.0) + 80.0])
        .with_min_inner_size([400.0, 300.0])
        .with_fullscreen(config.general.fullscreen)
        .with_decorations(!config.general.no_window_decoration);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        // Disable vsync so eglSwapBuffers doesn't block waiting for a frame
        // callback. On Wayland, an occluded surface (e.g. on another workspace)
        // never receives frame callbacks, so a vsync'd swap wedges the app
        // thread inside swap_buffers — which means the Wayland event queue
        // never gets dispatched, sctk's auto-pong for xdg_wm_base.ping never
        // runs, and compositors with ANR detection (Hyprland) raise an
        // "Application Not Responding" dialog. Slappyshot is a static
        // annotation UI; vsync brings nothing and tearing is not a concern.
        vsync: false,
        ..Default::default()
    };

    let app = App::new(image, config);

    eframe::run_native(
        "slappyshot",
        native_options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "nerd_symbols".to_owned(),
                egui::FontData::from_static(include_bytes!(
                    "../assets/SymbolsNerdFont-Regular.ttf"
                )),
            );
            fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
                .unwrap()
                .push("nerd_symbols".to_owned());
            fonts
                .families
                .get_mut(&egui::FontFamily::Monospace)
                .unwrap()
                .push("nerd_symbols".to_owned());
            cc.egui_ctx.set_fonts(fonts);

            // Wake the event loop from outside so winit keeps servicing the
            // Wayland queue (and replying to xdg_wm_base pings) even when the
            // surface is occluded — e.g. on another workspace. An in-update
            // request_repaint_after() is not sufficient on Wayland because a
            // hidden surface stops receiving frame callbacks, so the loop
            // never wakes to fire the timer. ctx.request_repaint() from
            // another thread goes through winit's EventLoopProxy (a calloop
            // pipe), which is independent of surface visibility. 250ms is
            // well under typical ANR ping deadlines and costs ~nothing.
            let wake_ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                wake_ctx.request_repaint();
            });

            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(())
}
