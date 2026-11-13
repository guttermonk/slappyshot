mod app;
mod cli;
mod config;
mod render;
mod style;
mod tools;

use std::io::Read;

use clap::Parser;

use app::App;
use cli::Args;
use config::Config;

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

    // THE size, not a provisional one. What is asked for here is very nearly
    // what the window will be for the rest of its life, because nothing
    // afterwards can change it:
    //
    //   ViewportCommand::InnerSize     ignored (measured: 60 sends, no movement)
    //   ViewportCommand::MaxInnerSize  ignored
    //   ViewportCommand::MinInnerSize  honoured, and the only one that is
    //
    // And a floor imposed later that disagrees with this number is not merely
    // ineffective, it is visible: the compositor keeps proposing the size the
    // window was created at, the floor pushes back, and the two alternate on
    // every reconfigure -- a flicker under the cursor for as long as the
    // window is being dragged.
    //
    // So this asks for the image's own dimensions and App corrects nothing
    // downward. That is knowingly a little large on a fractional-scale
    // display: these are PIXELS where inner_size wants POINTS, so the window
    // comes out scale-factor bigger than the image strictly needs. It is the
    // lesser fault by a distance -- exactly right at scale 1.0, and the
    // alternative was correct-but-flickering.
    //
    // eframe clamps this to the monitor's point size before the window is
    // built (epi_integration.rs, clamp_size_to_monitor_size) using winit's
    // real scale factor, so a capture bigger than the screen does not ask for
    // a window bigger than the screen.
    //
    // MIN_WINDOW_W_POINTS is wide enough to clear the toolbars' own measured
    // minimum, so the floor they impose later never disagrees with this.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("slappyshot")
        .with_app_id("slappyshot")
        .with_inner_size([
            img_w.max(app::MIN_WINDOW_W_POINTS),
            img_h.max(app::MIN_WINDOW_H_POINTS) + app::TOOLBAR_RESERVE_POINTS,
        ])
        .with_min_inner_size([app::MIN_WINDOW_W_POINTS, app::MIN_WINDOW_H_POINTS])
        .with_fullscreen(config.general.fullscreen)
        .with_decorations(!config.general.no_window_decoration);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    // `vsync: false` used to be set here, and the field is gone in eframe
    // 0.37 — but it would not be wanted even if it were still available.
    //
    // It was a workaround: on Wayland an occluded surface (on another
    // workspace, say) stops receiving frame callbacks, so a vsync'd
    // eglSwapBuffers blocked forever inside the swap. The app thread never
    // returned to dispatch the Wayland queue, sctk never auto-ponged
    // xdg_wm_base.ping, and Hyprland's ANR detection put up an "Application
    // Not Responding" dialog. Turning vsync off stopped the swap blocking.
    //
    // eframe 0.37 fixes the cause -- "Keep running on Wayland when frame
    // callbacks stop", emilk/egui#8631 -- so the workaround goes with it.
    // The wake thread below stays: it is cheap, and it is the belt to this
    // particular braces.
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app = App::new(image, config);

    eframe::run_native(
        "slappyshot",
        native_options,
        Box::new(|cc| {
            // The toolbar icons are Nerd Font glyphs, so that font is added to
            // both families as a fallback.
            //
            // add_font rather than mutating FontDefinitions::default(): the
            // old code rebuilt the whole definition and called set_fonts,
            // which in egui 0.37 also discards the font PROVIDERS eframe
            // installs (system fonts, via fontique). add_font keeps them and
            // adds to them.
            //
            // Lowest, not Highest: these icons live in the private use area,
            // where nothing else competes, so there is nothing to win a fight
            // against -- and Highest would put a symbols-only font ahead of
            // the real text fonts for every lookup.
            cc.egui_ctx.add_font(egui::FontInsert::new(
                "nerd_symbols",
                egui::FontData::from_static(include_bytes!(
                    "../assets/SymbolsNerdFont-Regular.ttf"
                )),
                vec![
                    egui::InsertFontFamily {
                        family: egui::FontFamily::Proportional,
                        priority: egui::FontPriority::Lowest,
                    },
                    egui::InsertFontFamily {
                        family: egui::FontFamily::Monospace,
                        priority: egui::FontPriority::Lowest,
                    },
                ],
            ));

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
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    wake_ctx.request_repaint();
                }
            });

            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(())
}
