//! Performance readout, overlay and benchmark (`--fps`, `F3`, `--bench`; see
//! `docs/performance.md`).
//!
//! Every second the plugin gathers: the frame time (mean and worst), the main world's update
//! time, the render world's time split into extract-to-prepare (this includes waiting for the
//! swap chain, i.e. for the GPU) and the render graph (command encoding and submission), GPU
//! time per pass where the backend has timestamp queries (Vulkan, DX12; not Metal), the shadow
//! maps rendered, and churn counters that should stay at 0 once the level has loaded: render
//! pipelines created and shader, mesh, image and material assets changed. Growing counters point
//! at per-frame re-specialisation or re-uploads, which cost far more on Vulkan (pipeline creation
//! compiles SPIR-V to the driver's ISA) than on Metal.
//!
//! `F3` toggles an on-screen overlay with these numbers; `--fps` starts with it shown, prints
//! them to the log and turns on Bevy's render diagnostics for the GPU times.
//!
//! `--bench <seconds>` waits for the level to settle (no pipeline created for a second, at
//! least five seconds after start), measures for that many seconds, prints mean, median, 95th
//! and 99th percentile frame times and exits.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::{Render, RenderApp, RenderSystems};

pub struct PerfPlugin {
    /// Show the overlay from the start and print the readout to the log.
    pub readout: bool,
    /// Measure for this many seconds after warm-up, then exit.
    pub bench: Option<f32>,
}

/// Timings shared between the main and render worlds.
#[derive(Default)]
struct Shared {
    render_start: Option<Instant>,
    render_graph_start: Option<Instant>,
    /// Accumulated over the current window.
    prepare: Duration,
    graph: Duration,
    render_frames: u32,
    pipelines: usize,
    waiting_pipelines: usize,
}

#[derive(Resource, Clone, Default)]
struct PerfShared(Arc<Mutex<Shared>>);

#[derive(Resource)]
struct PerfState {
    readout: bool,
    bench: Option<f32>,
    started: Instant,
    window_start: Instant,
    frame_times: Vec<f32>,
    update_time: Duration,
    update_start: Option<Instant>,
    last_frame: Option<Instant>,
    pipelines_at_window: usize,
    /// Since when the pipeline count has been stable (bench warm-up).
    stable_since: Option<(Instant, usize)>,
    bench_start: Option<Instant>,
    bench_frames: Vec<f32>,
    /// Asset changes in the current window: shaders, meshes, images, materials.
    changed: [u32; 4],
    /// The last window's summary, shown by the overlay.
    text: String,
}

#[derive(Component)]
struct PerfOverlay;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        let shared = PerfShared::default();
        let now = Instant::now();
        if self.readout {
            app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
        }
        app.insert_resource(shared.clone())
            .insert_resource(PerfState {
                readout: self.readout,
                bench: self.bench,
                started: now,
                window_start: now,
                frame_times: Vec::new(),
                update_time: Duration::ZERO,
                update_start: None,
                last_frame: None,
                pipelines_at_window: 0,
                stable_since: None,
                bench_start: None,
                bench_frames: Vec::new(),
                changed: [0; 4],
                text: "measuring…".into(),
            })
            .add_systems(Startup, spawn_overlay)
            .add_systems(First, update_start)
            .add_systems(Update, toggle_overlay)
            .add_systems(Last, update_end);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(shared).add_systems(
                Render,
                (
                    render_start.before(RenderSystems::ExtractCommands),
                    render_graph_start
                        .after(RenderSystems::Prepare)
                        .before(RenderSystems::Render),
                    render_end.after(RenderSystems::Render),
                ),
            );
            if std::env::var_os("BG3_PERF_PASSES").is_some() {
                render_app.add_systems(Render, log_phase_sizes.in_set(RenderSystems::Render));
            }
        }
    }
}

fn spawn_overlay(mut commands: Commands, state: Res<PerfState>) {
    commands.spawn((
        PerfOverlay,
        Text::new(state.text.clone()),
        TextFont {
            font_size: bevy::text::FontSize::Px(12.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 1.0, 0.85)),
        Node {
            // Top left: clear of the turn bar, the combat log and the dialog panel.
            position_type: PositionType::Absolute,
            left: Val::Px(8.0),
            top: Val::Px(8.0),
            padding: UiRect::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
        GlobalZIndex(i32::MAX),
        if state.readout {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        },
    ));
}

/// `F3` shows or hides the overlay.
fn toggle_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    mut overlay: Query<&mut Visibility, With<PerfOverlay>>,
) {
    if !keys.just_pressed(KeyCode::F3) {
        return;
    }
    for mut v in &mut overlay {
        *v = match *v {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };
    }
}

fn update_start(mut state: ResMut<PerfState>) {
    state.update_start = Some(Instant::now());
}

fn render_start(shared: Res<PerfShared>) {
    shared.0.lock().unwrap().render_start = Some(Instant::now());
}

fn render_graph_start(shared: Res<PerfShared>) {
    let mut s = shared.0.lock().unwrap();
    let now = Instant::now();
    if let Some(start) = s.render_start {
        s.prepare += now - start;
    }
    s.render_graph_start = Some(now);
}

fn render_end(shared: Res<PerfShared>, pipelines: Res<PipelineCache>) {
    let mut s = shared.0.lock().unwrap();
    if let Some(start) = s.render_graph_start.take() {
        s.graph += start.elapsed();
    }
    s.render_frames += 1;
    s.pipelines = pipelines.pipelines().count();
    s.waiting_pipelines = pipelines.waiting_pipelines().count();
}

/// Asset events counted as churn: shaders, meshes, images and the three material types.
type AssetChanges<'w, 's> = (
    MessageReader<'w, 's, AssetEvent<Shader>>,
    MessageReader<'w, 's, AssetEvent<Mesh>>,
    MessageReader<'w, 's, AssetEvent<Image>>,
    MessageReader<'w, 's, AssetEvent<StandardMaterial>>,
    MessageReader<'w, 's, AssetEvent<crate::level_materials::LevelMaterial>>,
    MessageReader<'w, 's, AssetEvent<crate::char_materials::CharacterMaterial>>,
);

#[allow(clippy::too_many_arguments)]
fn update_end(
    mut state: ResMut<PerfState>,
    shared: Res<PerfShared>,
    diagnostics: Option<Res<DiagnosticsStore>>,
    directional: Query<(&DirectionalLight, Option<&crate::lights::BoxShadowCarrier>)>,
    points: Query<&PointLight>,
    spots: Query<&SpotLight>,
    mut overlay: Query<(&mut Text, &Visibility), With<PerfOverlay>>,
    mut events: AssetChanges,
    mut exit: MessageWriter<AppExit>,
) {
    let now = Instant::now();
    if let Some(start) = state.update_start.take() {
        state.update_time += now - start;
    }
    state.changed[0] += modified(&mut events.0);
    state.changed[1] += modified(&mut events.1);
    state.changed[2] += modified(&mut events.2);
    state.changed[3] += modified(&mut events.3) + modified(&mut events.4) + modified(&mut events.5);
    if let Some(last) = state.last_frame.replace(now) {
        let dt = (now - last).as_secs_f32() * 1000.0;
        state.frame_times.push(dt);
        if state.bench_start.is_some() {
            state.bench_frames.push(dt);
        }
    }
    let pipelines = shared.0.lock().unwrap().pipelines;
    if let Some(seconds) = state.bench {
        bench(&mut state, now, pipelines, seconds, &mut exit);
    }

    if now - state.window_start < Duration::from_secs(1) {
        return;
    }
    let frames = state.frame_times.len().max(1);
    let mean = state.frame_times.iter().sum::<f32>() / frames as f32;
    let worst = state.frame_times.iter().copied().fold(0.0, f32::max);
    let mut s = shared.0.lock().unwrap();
    let rf = s.render_frames.max(1) as f32;
    let ms = |d: Duration, n: f32| d.as_secs_f32() * 1000.0 / n;

    // Shadow maps: directional cascades (sun and box light carriers), point lights (6 faces)
    // and spot lights.
    let mut carriers = [0, 0];
    let mut cascades = 0;
    for (light, carrier) in &directional {
        if light.shadow_maps_enabled {
            if let Some(c) = carrier {
                carriers[0] += 1;
                carriers[1] += c.in_use() as usize;
            }
            cascades += 1;
        }
    }
    let point_shadows = points.iter().filter(|l| l.shadow_maps_enabled).count();
    let spot_shadows = spots.iter().filter(|l| l.shadow_maps_enabled).count();

    let mut text = String::new();
    let _ = writeln!(
        text,
        "{:.1} fps, {mean:.2} ms (worst {worst:.1})",
        1000.0 / mean
    );
    let _ = writeln!(
        text,
        "CPU main update {:.2} ms",
        ms(state.update_time, frames as f32)
    );
    let _ = writeln!(
        text,
        "render prepare {:.2} ms (incl. GPU wait)",
        ms(s.prepare, rf)
    );
    let _ = writeln!(text, "render graph {:.2} ms", ms(s.graph, rf));
    let _ = writeln!(text, "GPU {}", gpu_summary(diagnostics.as_deref()));
    let _ = writeln!(
        text,
        "shadows: {cascades} directional ({} box carriers,\n  {} in use), {point_shadows} point, {spot_shadows} spot",
        carriers[0], carriers[1],
    );
    let [shaders, meshes, images, materials] = state.changed;
    let _ = write!(
        text,
        "last 1 s: {} new pipelines ({} total,\n  {} compiling); changed: {shaders} shaders,\n  {meshes} meshes, {images} images, {materials} materials",
        s.pipelines.saturating_sub(state.pipelines_at_window),
        s.pipelines,
        s.waiting_pipelines,
    );
    s.prepare = Duration::ZERO;
    s.graph = Duration::ZERO;
    s.render_frames = 0;
    drop(s);

    if state.readout {
        eprintln!("fps: {}", text.replace(",\n  ", ", ").replace('\n', "; "));
    }
    for (mut t, visibility) in &mut overlay {
        if *visibility != Visibility::Hidden {
            t.0.clone_from(&text);
        }
    }
    state.text = text;
    state.pipelines_at_window = pipelines;
    state.window_start = now;
    state.frame_times.clear();
    state.update_time = Duration::ZERO;
    state.changed = [0; 4];
}

/// GPU time summed over Bevy's top-level render passes, and the three most expensive.
fn gpu_summary(diagnostics: Option<&DiagnosticsStore>) -> String {
    let Some(store) = diagnostics else {
        return "n/a (start with --fps)".into();
    };
    // BG3_PERF_PASSES=1: every render diagnostic (nested passes, CPU and GPU), sorted by path.
    if std::env::var_os("BG3_PERF_PASSES").is_some() {
        let mut all: Vec<(String, f64)> = store
            .iter()
            .filter(|d| d.path().as_str().starts_with("render/"))
            .map(|d| (d.path().as_str().to_string(), d.smoothed().unwrap_or(0.0)))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, t) in all {
            eprintln!("pass: {path} {t:.3}");
        }
    }
    let mut passes: Vec<(String, f64)> = store
        .iter()
        .filter_map(|d| {
            let path = d.path().as_str();
            let pass = path.strip_prefix("render/")?.strip_suffix("/elapsed_gpu")?;
            (!pass.contains('/')).then(|| (pass.to_string(), d.smoothed().unwrap_or(0.0)))
        })
        .collect();
    if passes.is_empty() {
        return "n/a (no timestamp queries)".into();
    }
    passes.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = passes.iter().map(|p| p.1).sum();
    let top: Vec<String> = passes
        .iter()
        .take(3)
        .map(|(n, t)| format!("{n} {t:.2}"))
        .collect();
    format!("{total:.2} ms over passes\n  {}", top.join("\n  "))
}

fn bench(
    state: &mut PerfState,
    now: Instant,
    pipelines: usize,
    seconds: f32,
    exit: &mut MessageWriter<AppExit>,
) {
    match state.bench_start {
        None => {
            let stable = match state.stable_since {
                Some((since, count)) if count == pipelines => now - since,
                _ => {
                    state.stable_since = Some((now, pipelines));
                    Duration::ZERO
                }
            };
            if now - state.started >= Duration::from_secs(5) && stable >= Duration::from_secs(1) {
                eprintln!(
                    "bench: warmed up after {:.1?}, measuring",
                    now - state.started
                );
                state.bench_start = Some(now);
            }
        }
        Some(start) if (now - start).as_secs_f32() >= seconds => {
            let mut f = std::mem::take(&mut state.bench_frames);
            f.sort_by(f32::total_cmp);
            if !f.is_empty() {
                let pct = |p: f32| f[((f.len() - 1) as f32 * p).round() as usize];
                let mean = f.iter().sum::<f32>() / f.len() as f32;
                eprintln!(
                    "bench: {} frames in {seconds} s: mean {mean:.2} ms ({:.1} fps), median {:.2}, p95 {:.2}, p99 {:.2}, max {:.2} ms",
                    f.len(),
                    1000.0 / mean,
                    pct(0.5),
                    pct(0.95),
                    pct(0.99),
                    f[f.len() - 1],
                );
            }
            exit.write(AppExit::Success);
        }
        Some(_) => {}
    }
}

fn modified<A: Asset>(events: &mut MessageReader<AssetEvent<A>>) -> u32 {
    events
        .read()
        .filter(|e| matches!(e, AssetEvent::Modified { .. }))
        .count() as u32
}

/// `BG3_PERF_PASSES=1`: per binned phase, the views and their batch sets, bins and entities
/// (every 120 render frames). Each multidrawable batch set costs buffer writes, a bind group
/// and a bin-unpacking dispatch per view.
fn log_phase_sizes(
    shadow: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::pbr::Shadow>>,
    opaque: Res<
        bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::core_3d::Opaque3d>,
    >,
    mask: Res<
        bevy::render::render_phase::ViewBinnedRenderPhases<
            bevy::core_pipeline::core_3d::AlphaMask3d,
        >,
    >,
    prepass: Res<
        bevy::render::render_phase::ViewBinnedRenderPhases<
            bevy::core_pipeline::prepass::Opaque3dPrepass,
        >,
    >,
    mut frame: Local<u32>,
) {
    *frame += 1;
    if !(*frame).is_multiple_of(120) {
        return;
    }
    fn sizes<P: bevy::render::render_phase::BinnedPhaseItem>(
        name: &str,
        phases: &bevy::render::render_phase::ViewBinnedRenderPhases<P>,
    ) {
        let mut line = format!("phase {name}: {} views;", phases.len());
        for phase in phases.values() {
            let multi = phase.multidrawable_meshes.len();
            let _ = write!(
                line,
                " [{multi} sets; {} batchable, {} unbatchable]",
                phase.batchable_meshes.len(),
                phase.unbatchable_meshes.len()
            );
        }
        eprintln!("{line}");
    }
    sizes("shadow", &shadow);
    sizes("opaque", &opaque);
    sizes("alpha mask", &mask);
    sizes("prepass", &prepass);
}
