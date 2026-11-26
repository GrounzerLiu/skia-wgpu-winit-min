//! Minimal window whose content Skia draws with Graphite on the application's wgpu device.
//!
//! Skia renders on the same wgpu device and queue as the window surface, so it draws
//! straight into the swapchain texture that wgpu hands over every frame.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use skia_safe::gpu::graphite;
use skia_safe::wgpu;
use skia_safe::{Canvas, Color, ColorType, Paint, Rect};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};
use winit::window::{Window, WindowAttributes, WindowId};

/// A window that owns itself, so that a wgpu surface created from it can be 'static.
struct OwnedWindow(Arc<dyn Window>);

impl HasWindowHandle for OwnedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for OwnedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

struct State {
    window: Arc<dyn Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    // Skia's recorder and context must be dropped before the device they render with.
    skia: Skia,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// The Graphite context and the recorder that collects Skia's draws.
struct Skia {
    context: graphite::Context,
    recorder: graphite::Recorder,
}

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window: Arc<dyn Window> = Arc::from(
            event_loop
                .create_window(WindowAttributes::default().with_title("Skia on wgpu"))
                .expect("creating the window failed"),
        );

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(OwnedWindow(window.clone()))
            .expect("creating the surface failed");
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("no wgpu adapter");
        let (device, queue) = block_on(adapter.request_device(&Default::default()))
            .expect("no wgpu device");

        let size = window.surface_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("the surface does not support the adapter");
        // Skia writes sRGB-encoded values; a non-sRGB surface keeps them from being encoded twice.
        let capabilities = surface.get_capabilities(&adapter);
        if let Some(format) = capabilities.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *format;
        }
        surface.configure(&device, &config);

        let skia = Skia::new(&device, &queue);
        window.request_redraw();
        self.state = Some(State { window, surface, config, skia, device, queue });
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::SurfaceResized(size) => {
                if size.width > 0 && size.height > 0 {
                    state.config.width = size.width;
                    state.config.height = size.height;
                    state.surface.configure(&state.device, &state.config);
                }
            }
            WindowEvent::RedrawRequested => {
                state.render();
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

impl Skia {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut context = graphite::wgpu::context_factory::make_wgpu(device, queue, None)
            .expect("no Graphite context for the wgpu device");
        let recorder = context.make_recorder(None).expect("no Graphite recorder");
        Self { context, recorder }
    }
}

impl Skia {
    /// Draws one frame into the swapchain texture and submits it to the shared queue.
    fn draw(&mut self, texture: &wgpu::Texture, format: wgpu::TextureFormat, width: u32, height: u32) {
        let color_type = match format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => ColorType::BGRA8888,
            _ => ColorType::RGBA8888,
        };
        let mut surface = graphite::wgpu::surfaces::wrap_texture(
            &mut self.recorder,
            texture,
            color_type,
            None,
            None,
        )
        .expect("no Skia surface for the swapchain texture");

        paint_frame(surface.canvas(), width as f32, height as f32);
        drop(surface);

        let mut recording = self.recorder.snap().expect("no recording");
        self.context
            .insert_recording(&graphite::InsertRecordingInfo::new(&mut recording));
        self.context.submit(None);
    }
}

impl State {
    fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };

        self.skia
            .draw(&frame.texture, self.config.format, self.config.width, self.config.height);
        self.queue.present(frame);
    }
}

/// Draws the content of one frame.
fn paint_frame(canvas: &Canvas, width: f32, height: f32) {
    canvas.clear(Color::from_rgb(18, 20, 28));

    let mut paint = Paint::default();
    paint.set_anti_alias(true);

    paint.set_color(Color::from_rgb(255, 90, 95));
    canvas.draw_circle((width * 0.35, height * 0.45), width.min(height) * 0.18, &paint);

    paint.set_color(Color::from_rgb(80, 200, 160));
    canvas.draw_circle((width * 0.55, height * 0.55), width.min(height) * 0.18, &paint);

    paint.set_color(Color::from_rgb(120, 140, 255));
    paint.set_style(skia_safe::paint::Style::Stroke);
    paint.set_stroke_width(3.0);
    canvas.draw_rect(Rect::from_xywh(40.0, 40.0, width - 80.0, height - 80.0), &paint);
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("creating the event loop failed");
    event_loop
        .run_app(App::default())
        .expect("running the event loop failed");
}
