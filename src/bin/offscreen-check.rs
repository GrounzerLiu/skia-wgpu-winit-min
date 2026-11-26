//! Renders with Skia into an offscreen wgpu texture, reads the pixels back with wgpu, and
//! checks them. Unlike the window example this needs no display, so CI can run it.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use skia_safe::gpu::graphite;
use skia_safe::wgpu;
use skia_safe::{Color, ColorType, Paint, Rect};

const SIZE: u32 = 64;

fn main() {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("no wgpu adapter in this environment");
    println!("adapter: {:?}", adapter.get_info());
    let (device, queue) = block_on(adapter.request_device(&Default::default()))
        .expect("no wgpu device");

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skia target"),
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let mut context = graphite::wgpu::context_factory::make_wgpu(&device, &queue, None)
        .expect("no Graphite context for the wgpu device");
    let mut recorder = context.make_recorder(None).expect("no Graphite recorder");
    let mut surface = graphite::wgpu::surfaces::wrap_texture(
        &mut recorder,
        &texture,
        ColorType::RGBA8888,
        None,
        None,
    )
    .expect("no Skia surface for the wgpu texture");

    {
        let canvas = surface.canvas();
        canvas.clear(Color::WHITE);
        let mut paint = Paint::default();
        paint.set_anti_alias(false);
        paint.set_color(Color::from_rgb(255, 0, 0));
        canvas.draw_rect(Rect::from_xywh(0.0, 0.0, 16.0, 16.0), &paint);
        paint.set_color(Color::from_rgb(0, 0, 255));
        canvas.draw_rect(Rect::from_xywh(32.0, 32.0, 16.0, 16.0), &paint);
    }
    drop(surface);

    let mut recording = recorder.snap().expect("no recording");
    context.insert_recording(&graphite::InsertRecordingInfo::new(&mut recording));
    assert!(context.submit_and_wait(), "submitting to Graphite failed");

    let pixels = read_texture(&device, &queue, &texture);
    let red = pixel(&pixels, 8, 8);
    let blue = pixel(&pixels, 40, 40);
    let white = pixel(&pixels, 56, 8);
    assert_eq!(red, [255, 0, 0, 255], "the red rect is not red: {red:?}");
    assert_eq!(blue, [0, 0, 255, 255], "the blue rect is not blue: {blue:?}");
    assert_eq!(white, [255, 255, 255, 255], "the background is not white: {white:?}");
    println!("offscreen check OK: red {red:?}, blue {blue:?}, background {white:?}");
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bytes_per_row = SIZE * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row * SIZE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| {
        result.expect("mapping the readback buffer failed")
    });
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("polling the device failed");
    buffer
        .get_mapped_range(..)
        .expect("reading the mapped buffer failed")
        .to_vec()
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
