//! What the compositor knows about the screen, asked before the window exists.
//!
//! Two numbers are wanted and neither is available from the window once it has
//! one: the display's scale factor, and the usable area -- the output minus
//! whatever a bar has reserved along an edge.
//!
//! They have to be had early because on Wayland the size a window is created
//! at is the only size it gets. Measured on Hyprland 0.55.4: a window asked to
//! resize itself afterwards does not move, for sixty consecutive frames,
//! whether it is asked through `ViewportCommand::InnerSize` (winit drops the
//! request unless the last configure was stateless) or through a minimum size
//! below the current one (the compositor is entitled to ignore a floor it
//! already satisfies, and does). So anything the window gets wrong at
//! creation, it wears.
//!
//! The toplevel protocol will not answer either question. `xdg_toplevel` has a
//! `configure_bounds` event meant for exactly this, and Hyprland never sends
//! it (checked with WAYLAND_DEBUG=1); what it does send is the monitor's full
//! size, bar included. So this asks a different way, through the layer shell
//! that bars themselves are built on: a layer surface anchored to all four
//! edges with an exclusive zone of zero is positioned in what is left after
//! every other surface's exclusive zone, and the compositor states that size
//! in its configure. The surface never gets a buffer attached, so nothing is
//! ever drawn and nothing appears on screen -- the configure arrives before a
//! layer surface maps, which is the whole point of the initial commit.
//!
//! wlr-layer-shell is a wlroots protocol, which is to say Hyprland, Sway,
//! river, Niri, KDE -- but not GNOME, and not X11. On anything that does not
//! offer it this returns None and the caller falls back to its guess.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputInfo, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};

/// How many round trips to spend waiting for the configure.
///
/// A round trip is a request to the compositor and its reply over a local
/// socket; the configure is normally in the first one. This is a budget
/// against a compositor that advertises the layer shell and then says nothing,
/// because every trip here is time the user spends looking at no window at
/// all.
const CONFIGURE_ROUND_TRIPS: usize = 4;

/// The display, as the compositor describes it.
pub struct Screen {
    /// The usable area in POINTS: the output minus the bars' exclusive zones.
    pub work_area: egui::Vec2,
    /// Physical pixels per point, if the output could be pinned down.
    ///
    /// None rather than 1.0, because a scale of 1.0 is a real answer and
    /// "could not tell" is not: the caller treats them differently.
    pub scale: Option<f32>,
}

/// Ask the compositor for the usable area and the scale, or return None.
///
/// Every failure is the same failure -- no Wayland, no layer shell, no
/// configure, a nonsense size -- and they all mean the caller should go back
/// to guessing. None of them are worth a message on the terminal: a screenshot
/// tool that prints a complaint about protocols on a system where it is merely
/// running under X11 is noise.
pub fn probe() -> Option<Screen> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<Probe>(&conn).ok()?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).ok()?;
    let layer_shell = LayerShell::bind(&globals, &qh).ok()?;

    let mut probe = Probe {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        work_area: None,
        entered: None,
    };

    // The outputs are bound but empty until they have been dispatched once.
    queue.roundtrip(&mut probe).ok()?;

    // Output None: let the compositor pick, which it does the same way it will
    // pick for the window -- the focused one. Asking for a particular output
    // would mean choosing one here, and a client has no way to know which
    // output is focused.
    //
    // Layer::Background is the bottom of the stack, as far from anything the
    // user is looking at as the protocol allows. It never becomes visible
    // regardless, having no buffer.
    let layer = layer_shell.create_layer_surface(
        &qh,
        compositor.create_surface(&qh),
        Layer::Background,
        Some("slappyshot-probe"),
        None,
    );
    // All four edges, and a size of zero on both axes: anchored opposite edges
    // with no size asked for is what makes the compositor state the size
    // instead, and what it states is the area left over.
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, 0);
    layer.set_exclusive_zone(0);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();

    for _ in 0..CONFIGURE_ROUND_TRIPS {
        queue.roundtrip(&mut probe).ok()?;
        if probe.work_area.is_some() {
            break;
        }
    }

    let (w, h) = probe.work_area?;
    // A zero on either axis is the compositor declining to decide, which it
    // should not do for a surface anchored to opposite edges. Nothing useful
    // can be built from it.
    if w == 0 || h == 0 {
        return None;
    }

    let scale = probe.scale();
    // The layer surface and the connection both go here, before eframe opens
    // its own.
    drop(layer);

    Some(Screen {
        work_area: egui::vec2(w as f32, h as f32),
        scale,
    })
}

struct Probe {
    registry_state: RegistryState,
    output_state: OutputState,
    /// The configured size of the layer surface, in points: the answer.
    work_area: Option<(u32, u32)>,
    /// The output the probe surface was placed on, if the compositor said.
    entered: Option<wl_output::WlOutput>,
}

impl Probe {
    /// Physical pixels per point on the output the window will open on.
    ///
    /// The output has to be identified before it can be measured, and there is
    /// no event that simply names it. Three ways to pin it down, in descending
    /// order of how much they are worth:
    ///
    ///   - the compositor said so, with a `wl_surface.enter` on the probe
    ///   - there is only one output, so there is nothing to choose between
    ///   - an output whose logical size could contain this work area, which is
    ///     weak but a match on width is a good deal better than a coin toss
    ///
    /// Running out of ways is not a failure: the work area is the larger half
    /// of what this function is for, and it is already known by here.
    fn scale(&self) -> Option<f32> {
        let mut infos: Vec<OutputInfo> = self
            .output_state
            .outputs()
            .filter_map(|output| self.output_state.info(&output))
            .collect();

        let info = if let Some(entered) = &self.entered {
            self.output_state.info(entered)?
        } else if infos.len() == 1 {
            infos.remove(0)
        } else {
            let (work_w, work_h) = self.work_area?;
            infos.into_iter().find(|info| {
                info.logical_size
                    .is_some_and(|(w, h)| w == work_w as i32 && h >= work_h as i32)
            })?
        };
        scale_of(&info)
    }
}

/// The scale factor of an output, fractions included.
///
/// Taken as the ratio of the mode to the logical size rather than from
/// `wl_output.scale`, which is an integer and so reports 2 for the 1.25 a
/// fractional-scale display is actually running at. xdg-output's logical size
/// carries the fraction: 2880 pixels over 1440 points is 2.0, 1920 over 1536
/// is 1.25.
fn scale_of(info: &OutputInfo) -> Option<f32> {
    let (logical_w, _) = info.logical_size?;
    let mode = info.modes.iter().find(|mode| mode.current)?;
    // A rotated output reports its mode in the panel's own axes and its
    // logical size in the screen's, so on a quarter turn the width to compare
    // against is the mode's height.
    let pixels = match info.transform {
        wl_output::Transform::_90
        | wl_output::Transform::_270
        | wl_output::Transform::Flipped90
        | wl_output::Transform::Flipped270 => mode.dimensions.1,
        _ => mode.dimensions.0,
    };
    if logical_w <= 0 || pixels <= 0 {
        return None;
    }
    let scale = pixels as f32 / logical_w as f32;
    scale.is_finite().then_some(scale).filter(|s| *s > 0.0)
}

impl LayerShellHandler for Probe {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.work_area = Some(configure.new_size);
    }
}

impl CompositorHandler for Probe {
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        self.entered = Some(output.clone());
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
}

impl OutputHandler for Probe {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for Probe {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_compositor!(Probe);
delegate_output!(Probe);
delegate_layer!(Probe);
delegate_registry!(Probe);
