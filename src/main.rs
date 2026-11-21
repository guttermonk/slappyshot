mod app;
mod cli;
mod config;
mod render;
mod screen;
mod state;
mod style;
mod tools;

use std::io::Read;

use clap::Parser;

use app::App;
use cli::Args;
use config::Config;
use state::LastUsed;

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
    let tool_from_cli = args.initial_tool.is_some();
    if let Some(v) = args.initial_tool {
        config.general.initial_tool = v;
    }
    if args.no_remember {
        config.general.remember_last_used = false;
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
    if args.no_zoom_to_fit {
        config.general.zoom_to_fit = false;
    }

    // The tool, size and colour from the last run. Loaded here with the rest
    // of the precedence so there is one place to read the whole order off:
    // the command line beats what was remembered, and what was remembered
    // beats the config file's initial_tool -- which is the first run's seed
    // rather than a standing instruction. --no-remember opts out of both
    // halves, reading nothing and writing nothing.
    let mut last_used = if config.general.remember_last_used {
        LastUsed::load()
    } else {
        LastUsed::default()
    };
    if tool_from_cli {
        last_used.tool = None;
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

    // THE size, and the only chance to get it right. Nothing afterwards can
    // change it -- measured on Hyprland 0.55.4, sixty frames of each with the
    // window never moving a point:
    //
    //   ViewportCommand::InnerSize     ignored
    //   ViewportCommand::MaxInnerSize  ignored
    //   ViewportCommand::MinInnerSize  ignored below the current size
    //
    // So the two facts the size depends on have to be in hand BEFORE the
    // window exists, and neither is available from it: the display's scale,
    // which converts the image's PIXELS into the POINTS inner_size is
    // measured in, and the usable area, which is the screen minus whatever a
    // bar has reserved along an edge. screen::probe asks the compositor for
    // both. See that module for how, and for why the obvious route -- the
    // bounds xdg_toplevel is supposed to suggest -- is not available.
    //
    // Without an answer, the fallback is the old behaviour: pixels used as
    // points, which is exactly right at scale 1.0 and scale-factor too large
    // everywhere else, clamped by eframe to the monitor's point size
    // (epi_integration.rs, clamp_size_to_monitor_size) and by App to a
    // fraction of it. A window that is somewhat too large is a far smaller
    // fault than a window that is not there yet, and the probe is the thing
    // that can fail.
    //
    // MIN_WINDOW_W_POINTS is wide enough to clear the toolbars' own measured
    // minimum, so the floor they impose later never disagrees with this.
    let screen = screen::probe();
    let scale = screen.as_ref().and_then(|s| s.scale).unwrap_or(1.0);

    let mut size = egui::vec2(
        (img_w / scale).max(app::MIN_WINDOW_W_POINTS),
        (img_h / scale).max(app::MIN_WINDOW_H_POINTS) + app::TOOLBAR_RESERVE_POINTS,
    );
    if let Some(screen) = &screen {
        // Clamped to the usable area and not to the screen, because the
        // difference between the two is precisely a window with its toolbars
        // tucked under the bar. The floors below win over this on a screen too
        // small to hold the toolbars at all; there is nothing better to do
        // with that case than overflow it.
        size = size.min(screen.work_area).max(egui::vec2(
            app::MIN_WINDOW_W_POINTS,
            app::MIN_WINDOW_H_POINTS,
        ));
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("slappyshot")
        .with_app_id("slappyshot")
        .with_inner_size(size)
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

    // Handed on so App knows whether the size above was measured or guessed.
    // Its own sizing pass is the correction for a guess and has nothing to add
    // to a measurement.
    let app = App::new(image, config, screen, last_used);

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
